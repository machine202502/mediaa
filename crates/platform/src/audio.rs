use anyhow::{anyhow, Context};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossbeam_channel::{bounded, Sender};
use mediaa_core::audio::AudioSink;
use mediaa_core::source::AudioSource;
use parking_lot::Mutex;
use std::collections::HashSet;
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub fn system_audio_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos"))
}

pub fn default_input_device() -> Option<String> {
    cpal::default_host()
        .default_input_device()
        .map(|d| d.to_string())
}

pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    let mut seen = HashSet::new();
    names.retain(|name| seen.insert(name.clone()));
    names
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CpalTarget {
    Input(String),
    System,
}

pub struct CpalSource {
    target: CpalTarget,
    stop_tx: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl CpalSource {
    pub fn new(target: CpalTarget) -> Self {
        Self {
            target,
            stop_tx: None,
            thread: None,
        }
    }
}

fn open_device(target: &CpalTarget) -> anyhow::Result<(cpal::Device, cpal::SupportedStreamConfig)> {
    let host = cpal::default_host();
    match target {
        CpalTarget::Input(name) => {
            let device = host
                .input_devices()?
                .find(|d| &d.to_string() == name)
                .ok_or_else(|| anyhow!("audio device \"{name}\" not found"))?;
            let config = device.default_input_config()?;
            Ok((device, config))
        }
        CpalTarget::System => {
            let device = host
                .default_output_device()
                .ok_or_else(|| anyhow!("no audio output device"))?;
            let config = device.default_output_config()?;
            Ok((device, config))
        }
    }
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    sink: AudioSink,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample,
    f32: FromSample<T>,
{
    let stream = device.build_input_stream::<T, _, _>(
        *config,
        move |data: &[T], _| {
            let samples: Vec<f32> = data.iter().map(|s| s.to_sample::<f32>()).collect();
            sink.push(&samples);
        },
        |err| log::warn!("audio stream error: {err}"),
        None,
    )?;
    Ok(stream)
}

impl AudioSource for CpalSource {
    fn name(&self) -> String {
        match &self.target {
            CpalTarget::Input(n) => n.clone(),
            CpalTarget::System => "System audio".into(),
        }
    }

    fn start(
        &mut self,
        open_sink: &mut dyn FnMut(u32, u16) -> anyhow::Result<AudioSink>,
    ) -> anyhow::Result<()> {
        let (device, supported) = open_device(&self.target)?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let sink = open_sink(config.sample_rate, config.channels)?;
        let (ready_tx, ready_rx) = bounded::<anyhow::Result<()>>(1);
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let name = self.name();
        let thread = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || {
                let stream = match format {
                    cpal::SampleFormat::F32 => build_stream::<f32>(&device, &config, sink),
                    cpal::SampleFormat::I16 => build_stream::<i16>(&device, &config, sink),
                    cpal::SampleFormat::I32 => build_stream::<i32>(&device, &config, sink),
                    cpal::SampleFormat::U16 => build_stream::<u16>(&device, &config, sink),
                    other => Err(anyhow!("unsupported sample format {other}")),
                };
                let stream = match stream.and_then(|s| s.play().map(|_| s).map_err(Into::into)) {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        return;
                    }
                };
                let _ = ready_tx.send(Ok(()));
                let _ = stop_rx.recv();
                drop(stream);
                log::info!("audio capture \"{name}\" stopped");
            })?;
        let ready = match ready_rx.recv_timeout(Duration::from_secs(15)) {
            Ok(result) => result,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                anyhow::bail!("audio start timed out")
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("audio thread exited")
            }
        };
        ready.with_context(|| format!("cannot record from \"{}\"", self.name()))?;
        self.stop_tx = Some(stop_tx);
        self.thread = Some(thread);
        Ok(())
    }

    fn stop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for CpalSource {
    fn drop(&mut self) {
        self.stop();
    }
}

pub struct AudioOut {
    shared: Arc<OutShared>,
    rate: u32,
    stop_tx: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Default)]
struct OutShared {
    queue: Mutex<std::collections::VecDeque<f32>>,
    paused: AtomicBool,
    volume: AtomicU32,
}

impl AudioOut {
    pub fn open() -> anyhow::Result<AudioOut> {
        let device = cpal::default_host()
            .default_output_device()
            .ok_or_else(|| anyhow!("no audio output device"))?;
        let supported = device.default_output_config()?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let shared = Arc::new(OutShared {
            volume: AtomicU32::new(1.0f32.to_bits()),
            ..Default::default()
        });
        let (ready_tx, ready_rx) = bounded::<anyhow::Result<()>>(1);
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("audio-out".into())
            .spawn(move || {
                let stream = match format {
                    cpal::SampleFormat::F32 => build_out::<f32>(&device, &config, s),
                    cpal::SampleFormat::I16 => build_out::<i16>(&device, &config, s),
                    cpal::SampleFormat::I32 => build_out::<i32>(&device, &config, s),
                    cpal::SampleFormat::U16 => build_out::<u16>(&device, &config, s),
                    other => Err(anyhow!("unsupported sample format {other}")),
                };
                match stream.and_then(|st| st.play().map(|_| st).map_err(Into::into)) {
                    Ok(st) => {
                        let _ = ready_tx.send(Ok(()));
                        let _ = stop_rx.recv();
                        drop(st);
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })?;
        ready_rx.recv().context("audio output thread exited")??;
        Ok(AudioOut {
            shared,
            rate: config.sample_rate,
            stop_tx: Some(stop_tx),
            thread: Some(thread),
        })
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&self, samples: &[f32]) {
        self.shared.queue.lock().extend(samples);
    }

    pub fn queued_secs(&self) -> f64 {
        self.shared.queue.lock().len() as f64 / 2.0 / f64::from(self.rate)
    }

    pub fn clear(&self) {
        self.shared.queue.lock().clear();
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared
            .volume
            .store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        if let Some(tx) = self.stop_tx.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn build_out<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    shared: Arc<OutShared>,
) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let stream = device.build_output_stream::<T, _, _>(
        *config,
        move |out: &mut [T], _| {
            let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
            let paused = shared.paused.load(Ordering::Relaxed);
            let mut queue = shared.queue.lock();
            for frame in out.chunks_mut(channels) {
                let (l, r) = if paused {
                    (0.0, 0.0)
                } else {
                    (
                        queue.pop_front().unwrap_or(0.0),
                        queue.pop_front().unwrap_or(0.0),
                    )
                };
                for (i, s) in frame.iter_mut().enumerate() {
                    let v = match (channels, i) {
                        (1, _) => (l + r) / 2.0,
                        (_, 0) => l,
                        (_, 1) => r,
                        _ => 0.0,
                    };
                    *s = T::from_sample(v * volume);
                }
            }
        },
        |err| log::warn!("audio output error: {err}"),
        None,
    )?;
    Ok(stream)
}

const CONFIRMATION_WAV: &[u8] = include_bytes!("../assets/recording-saved.wav");

fn confirmation_samples(target_rate: u32) -> anyhow::Result<Vec<f32>> {
    let mut reader = hound::WavReader::new(Cursor::new(CONFIRMATION_WAV))?;
    let spec = reader.spec();
    anyhow::ensure!(
        spec.channels == 1
            && spec.sample_format == hound::SampleFormat::Int
            && spec.bits_per_sample == 16,
        "confirmation sound must be mono 16-bit PCM"
    );
    let source: Vec<f32> = reader
        .samples::<i16>()
        .map(|sample| sample.map(|v| f32::from(v) / 32_768.0))
        .collect::<Result<_, _>>()?;
    anyhow::ensure!(!source.is_empty(), "confirmation sound is empty");
    let count = (source.len() as u64 * u64::from(target_rate) / u64::from(spec.sample_rate)).max(1)
        as usize;
    let mut output = Vec::with_capacity(count * 2);
    for i in 0..count {
        let position = i as f64 * f64::from(spec.sample_rate) / f64::from(target_rate);
        let index = position.floor() as usize;
        let fraction = (position - position.floor()) as f32;
        let first = source[index.min(source.len() - 1)];
        let second = source[(index + 1).min(source.len() - 1)];
        let sample = first + (second - first) * fraction;
        output.extend([sample, sample]);
    }
    Ok(output)
}

pub fn confirmation_tone() {
    let out = match AudioOut::open() {
        Ok(out) => out,
        Err(e) => {
            log::warn!("confirmation sound unavailable: {e:#}");
            return;
        }
    };
    let samples = match confirmation_samples(out.sample_rate()) {
        Ok(samples) => samples,
        Err(e) => {
            log::warn!("confirmation sound is invalid: {e:#}");
            return;
        }
    };
    let duration = samples.len() as f32 / 2.0 / out.sample_rate() as f32;
    out.push(&samples);
    std::thread::sleep(Duration::from_secs_f32(duration + 0.04));
}

#[cfg(test)]
mod tests {
    use super::{confirmation_samples, CONFIRMATION_WAV};

    #[test]
    fn confirmation_tone_comes_from_the_embedded_wav_file() {
        assert_eq!(&CONFIRMATION_WAV[..4], b"RIFF");
        let samples = confirmation_samples(48_000).unwrap();
        assert_eq!(samples.len() % 2, 0);
        assert!(samples.len() > 35_000);
        assert!(samples
            .iter()
            .all(|sample| sample.is_finite() && sample.abs() <= 0.2));
    }
}
