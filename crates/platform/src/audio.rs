use anyhow::{anyhow, Context};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SizedSample};
use crossbeam_channel::{bounded, Sender};
use mediaa_core::audio::AudioSink;
use mediaa_core::source::AudioSource;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub fn system_audio_supported() -> bool {
    cfg!(any(target_os = "windows", target_os = "macos"))
}

pub fn default_input_device() -> Option<String> {
    cpal::default_host().default_input_device().map(|d| d.to_string())
}

pub fn input_devices() -> Vec<String> {
    let host = cpal::default_host();
    let mut names: Vec<String> = host
        .input_devices()
        .map(|it| it.map(|d| d.to_string()).collect())
        .unwrap_or_default();
    names.dedup();
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
        ready_rx
            .recv()
            .context("audio thread exited")?
            .with_context(|| format!("cannot record from \"{}\"", self.name()))?;
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
        let device = cpal::default_host().default_output_device().ok_or_else(|| anyhow!("no audio output device"))?;
        let supported = device.default_output_config()?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.config();
        let shared = Arc::new(OutShared { volume: AtomicU32::new(1.0f32.to_bits()), ..Default::default() });
        let (ready_tx, ready_rx) = bounded::<anyhow::Result<()>>(1);
        let (stop_tx, stop_rx) = bounded::<()>(1);
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("audio-out".into()).spawn(move || {
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
        Ok(AudioOut { shared, rate: config.sample_rate, stop_tx: Some(stop_tx), thread: Some(thread) })
    }

    pub fn sample_rate(&self) -> u32 {
        self.rate
    }

    pub fn push(&self, samples: &[f32]) {
        self.shared.queue.lock().expect("audio queue").extend(samples);
    }

    pub fn queued_secs(&self) -> f64 {
        self.shared.queue.lock().expect("audio queue").len() as f64 / 2.0 / f64::from(self.rate)
    }

    pub fn clear(&self) {
        self.shared.queue.lock().expect("audio queue").clear();
    }

    pub fn set_paused(&self, paused: bool) {
        self.shared.paused.store(paused, Ordering::Relaxed);
    }

    pub fn set_volume(&self, volume: f32) {
        self.shared.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
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

fn build_out<T>(device: &cpal::Device, config: &cpal::StreamConfig, shared: Arc<OutShared>) -> anyhow::Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = usize::from(config.channels.max(1));
    let stream = device.build_output_stream::<T, _, _>(
        *config,
        move |out: &mut [T], _| {
            let volume = f32::from_bits(shared.volume.load(Ordering::Relaxed));
            let paused = shared.paused.load(Ordering::Relaxed);
            let mut queue = shared.queue.lock().expect("audio queue");
            for frame in out.chunks_mut(channels) {
                let (l, r) = if paused { (0.0, 0.0) } else { (queue.pop_front().unwrap_or(0.0), queue.pop_front().unwrap_or(0.0)) };
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
