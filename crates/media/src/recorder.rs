use crate::convert::{self, ConvertSettings, Target};
use crate::filters;
use crossbeam_channel::{Receiver, Sender};
use ez_ffmpeg::{FfmpegContext, FramePushSource, Output};
use mediaa_core::audio::{AudioTrack, Clock};
use mediaa_core::formats::OutputKind;
use mediaa_core::naming;
use mediaa_core::pacer::{SessionClock, SlotTracker};
use mediaa_core::settings::AnimSettings;
use mediaa_core::source::{AudioSource, SourceFactory};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

pub struct RecordingRequest {
    pub sources: Vec<SourceFactory>,
    pub audio: Vec<Box<dyn AudioSource>>,
    pub output: OutputKind,
    pub fps: u32,
    pub video_quality: u8,
    pub max_height: Option<u32>,
    pub audio_in_every_file: bool,
    pub audio_kbps: u32,
    pub anim: AnimSettings,
    pub out_dir: PathBuf,
    pub work_dir: PathBuf,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Warning(String),
    CaptureFailed(String),
    Finalizing(f32),
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(String),
}

#[derive(Debug, Default)]
struct ControlState {
    paused: bool,
    stopping: bool,
}

struct Control {
    state: Mutex<ControlState>,
    cv: Condvar,
    clock: Mutex<SessionClock>,
    started: Instant,
    audio_paused: Arc<AtomicBool>,
}

impl Control {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn active_time(&self) -> Duration {
        let now = self.now();
        self.clock.lock().expect("clock lock").active_time(now)
    }

    fn set_paused(&self, paused: bool) {
        let now = self.now();
        {
            let mut clock = self.clock.lock().expect("clock lock");
            if paused {
                clock.pause(now);
            } else {
                clock.resume(now);
            }
        }
        self.audio_paused.store(paused, Ordering::Relaxed);
        self.state.lock().expect("state lock").paused = paused;
        self.cv.notify_all();
    }

    fn stop(&self) {
        self.state.lock().expect("state lock").stopping = true;
        self.cv.notify_all();
    }

    fn wait(&self, timeout: Duration) -> bool {
        let guard = self.state.lock().expect("state lock");
        let (guard, _) = self
            .cv
            .wait_timeout_while(guard, timeout, |s| !s.stopping)
            .expect("state lock");
        let guard = self
            .cv
            .wait_while(guard, |s| s.paused && !s.stopping)
            .expect("state lock");
        !guard.stopping
    }
}

struct Capture {
    name: String,
    scratch: PathBuf,
    frames: Arc<AtomicU64>,
    join: JoinHandle<anyhow::Result<()>>,
}

pub struct RecordingSession {
    control: Arc<Control>,
    captures: Vec<Capture>,
    audio: Vec<Box<dyn AudioSource>>,
    tracks: Vec<AudioTrack>,
    output: OutputKind,
    video_quality: u8,
    audio_in_every_file: bool,
    audio_kbps: u32,
    anim: AnimSettings,
    out_dir: PathBuf,
    work_dir: PathBuf,
    title: String,
    started: chrono::DateTime<chrono::Local>,
    events_tx: Sender<SessionEvent>,
    events_rx: Receiver<SessionEvent>,
}

fn scratch_output(path: &Path, kind: OutputKind, quality: u8, max_height: Option<u32>, size: (u32, u32), fps: u32) -> (String, Output) {
    let out = Output::from(path.to_string_lossy().into_owned())
        .set_format("matroska")
        .add_stream_map("[v]");
    match kind {
        OutputKind::Video(_) => (
            filters::scratch_graph("yuv420p", max_height),
            out.set_video_codec("libx264")
                .set_video_codec_opt("preset", {
                    let (w, h) = filters::scaled_size(size.0, size.1, max_height);
                    filters::live_preset(w, h, fps)
                })
                .set_video_codec_opt("crf", filters::crf_for_quality(quality).to_string()),
        ),
        OutputKind::Audio(_) => unreachable!("audio recordings have no picture to encode"),
        OutputKind::Anim(_) => (
            filters::scratch_graph("bgr0", max_height),
            out.set_video_codec("libx264rgb")
                .set_video_codec_opt("preset", "ultrafast")
                .set_video_codec_opt("qp", "0"),
        ),
    }
}

#[allow(clippy::too_many_arguments, reason = "plain data handed to a thread")]
fn capture_loop(
    factory: SourceFactory,
    control: Arc<Control>,
    fps: u32,
    kind: OutputKind,
    quality: u8,
    max_height: Option<u32>,
    scratch: PathBuf,
    frames: Arc<AtomicU64>,
    ready: Sender<anyhow::Result<String>>,
) -> anyhow::Result<()> {
    let setup = (|| -> anyhow::Result<_> {
        let source = factory()?;
        let (w, h) = source.size();
        let (push_source, handle) = FramePushSource::builder(w, h)
            .pixel_format("rgba")
            .time_base(1, fps as i32)
            .build()?;
        let (graph, output) = scratch_output(&scratch, kind, quality, max_height, (w, h), fps);
        let scheduler = FfmpegContext::builder()
            .input(push_source)
            .filter_desc(graph)
            .output(output)
            .build()?
            .start()?;
        Ok((source, handle, scheduler))
    })();
    let (mut source, mut handle, scheduler) = match setup {
        Ok(v) => {
            let _ = ready.send(Ok(v.0.name()));
            v
        }
        Err(e) => {
            let _ = ready.send(Err(anyhow::anyhow!("{e:#}")));
            return Err(e);
        }
    };

    let mut slots = SlotTracker::new(fps);
    let mut result = Ok(());
    loop {
        let active = control.active_time();
        if let Some(slot) = slots.slot(active) {
            match source.grab() {
                Ok(frame) => {
                    if let Err(e) = handle.push_owned(frame.data, slot as i64, Some(1)) {
                        result = Err(anyhow::anyhow!("encoder stopped: {e}"));
                        break;
                    }
                    frames.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    result = Err(e.context(format!("capturing \"{}\"", source.name())));
                    break;
                }
            }
        }
        let wait = slots.next_deadline().saturating_sub(control.active_time());
        if !control.wait(wait.max(Duration::from_millis(1))) {
            break;
        }
    }
    handle.finish();
    let encoded = scheduler.wait().map_err(anyhow::Error::from);
    result.and(encoded)
}

impl RecordingSession {
    pub fn start(req: RecordingRequest) -> anyhow::Result<RecordingSession> {
        match req.output {
            OutputKind::Audio(_) => anyhow::ensure!(req.sources.is_empty() && !req.audio.is_empty(), "audio recording needs sound sources only"),
            _ => anyhow::ensure!(!req.sources.is_empty(), "nothing to record"),
        }
        std::fs::create_dir_all(&req.work_dir)?;
        std::fs::create_dir_all(&req.out_dir)?;
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let control = Arc::new(Control {
            state: Mutex::new(ControlState::default()),
            cv: Condvar::new(),
            clock: Mutex::new(SessionClock::new()),
            started: Instant::now(),
            audio_paused: Arc::new(AtomicBool::new(false)),
        });

        let mut audio = Vec::new();
        let mut tracks = Vec::new();
        if req.output.supports_audio() {
            for (i, mut src) in req.audio.into_iter().enumerate() {
                let path = req.work_dir.join(format!("audio-{i}.wav"));
                let paused = control.audio_paused.clone();
                let ctl = control.clone();
                let clock: Clock = Arc::new(move || ctl.active_time());
                let mut opened: Option<AudioTrack> = None;
                let res = src.start(&mut |rate, channels| {
                    let track = AudioTrack::create(&path, rate, channels, paused.clone(), clock.clone())?;
                    let sink = track.sink();
                    opened = Some(track);
                    Ok(sink)
                });
                match (res, opened) {
                    (Ok(()), Some(track)) => {
                        tracks.push(track);
                        audio.push(src);
                    }
                    (res, _) => {
                        let msg = match res {
                            Err(e) => format!("audio \"{}\" disabled: {e:#}", src.name()),
                            Ok(()) => format!("audio \"{}\" produced no track", src.name()),
                        };
                        log::warn!("{msg}");
                        let _ = events_tx.send(SessionEvent::Warning(msg));
                    }
                }
            }
        }

        let mut captures = Vec::new();
        let mut errors = Vec::new();
        for (i, factory) in req.sources.into_iter().enumerate() {
            let scratch = req.work_dir.join(format!("video-{i}.mkv"));
            let frames = Arc::new(AtomicU64::new(0));
            let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
            let ctl = control.clone();
            let (kind, fps, quality, max_h) = (req.output, req.fps.clamp(1, 60), req.video_quality, req.max_height);
            let (s, f) = (scratch.clone(), frames.clone());
            let events = events_tx.clone();
            let join = std::thread::Builder::new()
                .name(format!("capture-{i}"))
                .spawn(move || {
                    let r = capture_loop(factory, ctl.clone(), fps, kind, quality, max_h, s, f, ready_tx);
                    if let Err(e) = &r {
                        log::error!("capture {i} failed: {e:#}");
                        let _ = events.send(SessionEvent::CaptureFailed(format!("{e:#}")));
                    }
                    r
                })?;
            match ready_rx.recv() {
                Ok(Ok(name)) => captures.push(Capture {
                    name,
                    scratch,
                    frames,
                    join,
                }),
                Ok(Err(e)) => {
                    errors.push(format!("{e:#}"));
                    let _ = join.join();
                }
                Err(_) => {
                    errors.push("capture thread exited".into());
                    let _ = join.join();
                }
            }
        }

        let session = RecordingSession {
            control,
            captures,
            audio,
            tracks,
            output: req.output,
            video_quality: req.video_quality,
            audio_in_every_file: req.audio_in_every_file,
            audio_kbps: req.audio_kbps,
            anim: req.anim,
            out_dir: req.out_dir,
            work_dir: req.work_dir,
            title: req.title,
            started: chrono::Local::now(),
            events_tx,
            events_rx,
        };
        if let OutputKind::Audio(_) = session.output {
            if session.tracks.is_empty() {
                session.discard();
                anyhow::bail!("could not start recording: no sound source could be opened");
            }
        } else if session.captures.is_empty() {
            let msg = errors.join("; ");
            session.discard();
            anyhow::bail!("could not start recording: {msg}");
        }
        for e in errors {
            let _ = session.events_tx.send(SessionEvent::Warning(e));
        }
        Ok(session)
    }

    pub fn output(&self) -> OutputKind {
        self.output
    }

    pub fn events(&self) -> Receiver<SessionEvent> {
        self.events_rx.clone()
    }

    pub fn pause(&self) {
        self.control.set_paused(true);
    }

    pub fn resume(&self) {
        self.control.set_paused(false);
    }

    pub fn is_paused(&self) -> bool {
        self.control.state.lock().expect("state lock").paused
    }

    pub fn elapsed(&self) -> Duration {
        self.control.active_time()
    }

    pub fn source_names(&self) -> Vec<String> {
        self.captures.iter().map(|c| c.name.clone()).collect()
    }

    pub fn frames_captured(&self) -> u64 {
        self.captures
            .iter()
            .map(|c| c.frames.load(Ordering::Relaxed))
            .sum()
    }

    pub fn has_audio(&self) -> bool {
        !self.tracks.is_empty()
    }

    pub fn stop(self, save: bool, cancel: Arc<AtomicBool>) -> JoinHandle<()> {
        std::thread::Builder::new()
            .name("finalize".into())
            .spawn(move || {
                let events = self.events_tx.clone();
                let work_dir = self.work_dir.clone();
                let event = if save {
                    match self.finalize(&cancel) {
                        Ok(paths) => SessionEvent::Finished(paths),
                        Err(e) if crate::job::is_cancelled(&e) => SessionEvent::Cancelled,
                        Err(e) => SessionEvent::Failed(format!("{e:#}")),
                    }
                } else {
                    self.discard();
                    SessionEvent::Cancelled
                };
                let _ = std::fs::remove_dir_all(&work_dir);
                let _ = events.send(event);
            })
            .expect("spawn finalize thread")
    }

    fn shutdown(&mut self) -> Vec<(String, PathBuf)> {
        self.control.stop();
        let mut done = Vec::new();
        for c in self.captures.drain(..) {
            let ok = c.join.join().map_err(|_| anyhow::anyhow!("panicked"));
            let frames = c.frames.load(Ordering::Relaxed);
            match ok {
                Ok(_) if frames > 0 && c.scratch.is_file() => done.push((c.name, c.scratch)),
                _ => log::warn!("capture \"{}\" produced no usable output", c.name),
            }
        }
        for a in &mut self.audio {
            a.stop();
        }
        self.audio.clear();
        done
    }

    fn finish_tracks(&mut self) -> Vec<PathBuf> {
        self.tracks
            .drain(..)
            .filter(|t| t.samples_written() > 0)
            .filter_map(|t| t.finish().map_err(|e| log::warn!("audio track: {e:#}")).ok())
            .collect()
    }

    fn discard(mut self) {
        self.shutdown();
        let _ = self.finish_tracks();
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }

    fn finalize(mut self, cancel: &AtomicBool) -> anyhow::Result<Vec<PathBuf>> {
        let videos = self.shutdown();
        let wavs = self.finish_tracks();
        if let OutputKind::Audio(format) = self.output {
            anyhow::ensure!(!wavs.is_empty(), "no sound was recorded");
            let out = naming::media_path(&self.out_dir, &self.title, self.started, format.extension());
            let events = self.events_tx.clone();
            let mut progress = |p: f32| {
                let _ = events.send(SessionEvent::Finalizing(p));
            };
            convert::encode_audio_recording(&wavs, &out, format, self.audio_kbps, &mut progress, cancel)?;
            return Ok(vec![out]);
        }
        anyhow::ensure!(!videos.is_empty(), "no frames were recorded");
        let total = videos.len() as f32;
        let mut outputs = Vec::new();
        for (i, (name, scratch)) in videos.iter().enumerate() {
            let with_audio = !wavs.is_empty() && (i == 0 || self.audio_in_every_file);
            let mark = with_audio && !self.audio_in_every_file;
            let title = linked_title(&self.title, i, videos.len(), name, mark);
            let out = naming::media_path(&self.out_dir, &title, self.started, self.output.extension());
            let events = self.events_tx.clone();
            let mut progress = |p: f32| {
                let _ = events.send(SessionEvent::Finalizing((i as f32 + p) / total));
            };
            match self.output {
                OutputKind::Video(container) => {
                    let tracks: &[PathBuf] = if with_audio { &wavs } else { &[] };
                    convert::mux_recording(scratch, tracks, &out, container, &mut progress, cancel)?
                }
                OutputKind::Audio(_) => unreachable!("handled above"),
                OutputKind::Anim(format) => convert::convert(
                    scratch,
                    &out,
                    Target::Anim(format),
                    &ConvertSettings { anim: self.anim.clone(), quality: self.video_quality, ..ConvertSettings::default() },
                    &mut progress,
                    cancel,
                )?,
            }
            outputs.push(out);
        }
        Ok(outputs)
    }
}

pub fn linked_title(title: &str, i: usize, n: usize, source: &str, with_audio: bool) -> String {
    if n <= 1 {
        return title.to_string();
    }
    let audio = if with_audio { " (audio)" } else { "" };
    format!("{title} [{} of {n}] - {}{audio}", i + 1, naming::sanitize(source, 60))
}

#[cfg(test)]
mod tests {
    use super::linked_title;

    #[test]
    fn linked_names() {
        assert_eq!(linked_title("Rec", 0, 1, "Display 1", true), "Rec");
        assert_eq!(linked_title("Rec", 0, 2, "Display 1", true), "Rec [1 of 2] - Display 1 (audio)");
        assert_eq!(linked_title("Rec", 1, 2, "Code: main.rs", false), "Rec [2 of 2] - Code main.rs");
    }
}
