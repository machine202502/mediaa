//! Audio track recording: samples from capture callbacks are written to a WAV file on a
//! background thread (never doing file I/O inside real-time audio callbacks).
//! The WAV files are muxed into the final video by the media crate.
//!
//! Every chunk is placed on the recording timeline by its arrival time: gaps (a loopback
//! device that delivers nothing during silence, a microphone that starts late, a hiccup)
//! are filled with silence, so all tracks stay in sync with each other and with the video.

use crossbeam_channel::{Receiver, Sender};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

/// Active (pause-excluded) recording time, shared with the video capture.
pub type Clock = Arc<dyn Fn() -> Duration + Send + Sync>;

/// Gaps shorter than this are treated as jitter and not padded.
const GAP_TOLERANCE: f64 = 0.06;

/// Samples with their arrival time on the recording timeline.
type Chunk = (Duration, Vec<f32>);

/// Cheap, cloneable handle given to audio callbacks.
#[derive(Clone)]
pub struct AudioSink {
    tx: Sender<Chunk>,
    paused: Arc<AtomicBool>,
    samples: Arc<AtomicU64>,
    clock: Clock,
    pub sample_rate: u32,
    pub channels: u16,
}

impl AudioSink {
    /// Queues interleaved `f32` samples that just arrived. Dropped while paused.
    pub fn push(&self, samples: &[f32]) {
        if self.paused.load(Ordering::Relaxed) || samples.is_empty() {
            return;
        }
        self.samples.fetch_add(samples.len() as u64, Ordering::Relaxed);
        let _ = self.tx.send(((self.clock)(), samples.to_vec()));
    }
}

/// Frames of silence to insert before a chunk of `chunk_frames` that ended at `arrived`
/// (seconds on the timeline), when `written` frames are already in the file.
pub fn gap_frames(written: u64, chunk_frames: u64, arrived: f64, rate: u32) -> u64 {
    let start = arrived * f64::from(rate) - chunk_frames as f64;
    let gap = start - written as f64;
    if gap > GAP_TOLERANCE * f64::from(rate) {
        gap.round() as u64
    } else {
        0
    }
}

/// Owns the writer thread of one WAV file.
pub struct AudioTrack {
    path: PathBuf,
    sink: Option<AudioSink>,
    join: Option<JoinHandle<anyhow::Result<()>>>,
    samples: Arc<AtomicU64>,
}

impl AudioTrack {
    /// Creates `path` (32-bit float WAV). `paused` and `clock` are shared with the session.
    pub fn create(
        path: &Path,
        sample_rate: u32,
        channels: u16,
        paused: Arc<AtomicBool>,
        clock: Clock,
    ) -> anyhow::Result<AudioTrack> {
        anyhow::ensure!(sample_rate > 0 && channels > 0, "invalid audio format");
        let spec = hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(path, spec)?;
        let (tx, rx): (Sender<Chunk>, Receiver<Chunk>) = crossbeam_channel::unbounded();
        let ch = u64::from(channels);
        let join = std::thread::Builder::new()
            .name("audio-writer".into())
            .spawn(move || {
                let mut written = 0u64; // frames
                for (arrived, chunk) in rx {
                    let frames = chunk.len() as u64 / ch;
                    let pad = gap_frames(written, frames, arrived.as_secs_f64(), sample_rate);
                    for _ in 0..pad * ch {
                        writer.write_sample(0.0f32)?;
                    }
                    for s in chunk {
                        writer.write_sample(s)?;
                    }
                    written += pad + frames;
                }
                writer.finalize()?;
                Ok(())
            })?;
        let samples = Arc::new(AtomicU64::new(0));
        Ok(AudioTrack {
            path: path.to_path_buf(),
            sink: Some(AudioSink {
                tx,
                paused,
                samples: samples.clone(),
                clock,
                sample_rate,
                channels,
            }),
            join: Some(join),
            samples,
        })
    }

    pub fn sink(&self) -> AudioSink {
        self.sink.clone().expect("sink taken only by finish")
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Interleaved samples received so far (not counting inserted silence).
    pub fn samples_written(&self) -> u64 {
        self.samples.load(Ordering::Relaxed)
    }

    /// Flushes and closes the WAV file. All [`AudioSink`] clones must be dropped first
    /// (i.e. the capture stream stopped), otherwise this blocks until they are.
    pub fn finish(mut self) -> anyhow::Result<PathBuf> {
        drop(self.sink.take());
        if let Some(j) = self.join.take() {
            j.join()
                .map_err(|_| anyhow::anyhow!("audio writer thread panicked"))??;
        }
        Ok(self.path.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// A clock the test moves by hand.
    fn manual_clock() -> (Clock, Arc<Mutex<Duration>>) {
        let now = Arc::new(Mutex::new(Duration::ZERO));
        let n = now.clone();
        (Arc::new(move || *n.lock().unwrap()), now)
    }

    fn read(path: &Path) -> Vec<f32> {
        hound::WavReader::open(path).unwrap().samples::<f32>().map(Result::unwrap).collect()
    }

    #[test]
    fn writes_wav_and_respects_pause() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.wav");
        let paused = Arc::new(AtomicBool::new(false));
        let (clock, now) = manual_clock();
        let track = AudioTrack::create(&path, 48_000, 2, paused.clone(), clock).unwrap();
        let sink = track.sink();
        *now.lock().unwrap() = Duration::from_millis(10); // 480 frames arrive after 10 ms
        sink.push(&[0.5; 960]);
        paused.store(true, Ordering::Relaxed);
        sink.push(&[0.9; 480]); // dropped
        paused.store(false, Ordering::Relaxed);
        sink.push(&[0.5; 4]);
        drop(sink);
        assert_eq!(track.samples_written(), 964);
        let samples = read(&track.finish().unwrap());
        assert_eq!(samples.len(), 964, "no padding for contiguous chunks");
        assert_eq!(samples[0], 0.5);
        assert_eq!(samples[963], 0.5);
    }

    #[test]
    fn silence_gaps_are_filled_on_the_timeline() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("loopback.wav");
        let (clock, now) = manual_clock();
        let track = AudioTrack::create(&path, 1000, 1, Arc::new(AtomicBool::new(false)), clock).unwrap();
        let sink = track.sink();
        // Device starts late: first 100 frames arrive at t = 0.5 s.
        *now.lock().unwrap() = Duration::from_millis(500);
        sink.push(&[1.0; 100]);
        // Then nothing for a second (loopback during silence), next chunk at t = 1.6 s.
        *now.lock().unwrap() = Duration::from_millis(1600);
        sink.push(&[1.0; 100]);
        drop(sink);
        let s = read(&track.finish().unwrap());
        assert_eq!(s.len(), 1600, "track length follows the timeline");
        assert_eq!(s[0], 0.0, "leading silence");
        assert_eq!(s[450], 1.0, "first chunk at 0.4–0.5 s");
        assert_eq!(s[1000], 0.0, "gap filled with silence");
        assert_eq!(s[1550], 1.0, "second chunk at 1.5–1.6 s");
    }

    #[test]
    fn small_jitter_is_not_padded() {
        assert_eq!(gap_frames(1000, 100, 1.13, 1000), 0, "30 ms early/late is jitter");
        assert_eq!(gap_frames(1000, 100, 1.30, 1000), 200);
        assert_eq!(gap_frames(5000, 100, 1.0, 1000), 0, "running ahead never pads");
    }

    #[test]
    fn rejects_invalid_format() {
        let dir = tempfile::tempdir().unwrap();
        let (clock, _) = manual_clock();
        let paused = Arc::new(AtomicBool::new(false));
        assert!(AudioTrack::create(&dir.path().join("x.wav"), 0, 2, paused, clock).is_err());
    }
}
