use crate::animation_backends::{self, AnimationEncodeRequest};
use crate::convert;
use crate::filters;
use crate::filters::Effort;
use crossbeam_channel::{Receiver, Sender};
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use mediaa_core::atomic_file::AtomicFile;
use mediaa_core::audio::{AudioTrack, Clock};
use mediaa_core::formats::{AnimFormat, OutputKind};
use mediaa_core::limits::FPS as FPS_LIMIT;
use mediaa_core::naming;
use mediaa_core::pacer::{SessionClock, SlotTracker};
use mediaa_core::paths::{MediaDir, WorkDir};
use mediaa_core::settings::{AnimSettings, WebpMode};
use mediaa_core::source::{AudioSource, SourceFactory};
use parking_lot::{Condvar, Mutex};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

type SourceReadyResult = (String, (u32, u32));
type SourceReadyReceiver = Receiver<Result<SourceReadyResult, anyhow::Error>>;

mod request;
pub use request::*;

#[derive(Debug, Default)]
struct ControlState {
    paused: bool,
    stopping: bool,
    stopped_at: Option<Duration>,
}

#[derive(Default)]
struct StartGate {
    open: Mutex<bool>,
    cv: Condvar,
}

impl StartGate {
    fn wait(&self) {
        let mut open = self.open.lock();
        while !*open {
            self.cv.wait(&mut open);
        }
    }

    fn open(&self) {
        *self.open.lock() = true;
        self.cv.notify_all();
    }
}

struct Control {
    state: Mutex<ControlState>,
    cv: Condvar,
    clock: Mutex<SessionClock>,
    started: Instant,
    capture_origin: Mutex<Option<Duration>>,
    audio_paused: Arc<AtomicBool>,
}

impl Control {
    fn now(&self) -> Duration {
        self.started.elapsed()
    }

    fn begin(&self) {
        *self.capture_origin.lock() = Some(self.active_time());
    }

    fn capture_time(&self) -> Duration {
        let origin = *self.capture_origin.lock();
        self.active_time()
            .saturating_sub(origin.unwrap_or_default())
    }

    fn active_time(&self) -> Duration {
        let now = self.now();
        self.clock.lock().active_time(now)
    }

    fn set_paused(&self, paused: bool) {
        let now = self.now();
        {
            let mut clock = self.clock.lock();
            if paused {
                clock.pause(now);
            } else {
                clock.resume(now);
            }
        }
        self.audio_paused.store(paused, Ordering::Relaxed);
        self.state.lock().paused = paused;
        self.cv.notify_all();
    }

    fn stop(&self) {
        let at = self.capture_time();
        let mut state = self.state.lock();
        state.stopping = true;
        state.stopped_at = Some(at);
        drop(state);
        self.cv.notify_all();
    }

    fn stopped_at(&self) -> Option<Duration> {
        self.state.lock().stopped_at
    }

    fn wait(&self, timeout: Duration) -> bool {
        let mut guard = self.state.lock();
        self.cv.wait_for(&mut guard, timeout);
        self.cv.wait_while(&mut guard, |s| s.paused && !s.stopping);
        !guard.stopping
    }
}

struct Capture {
    index: usize,
    name: String,
    frames: Arc<AtomicU64>,
    join: JoinHandle<anyhow::Result<Vec<PathBuf>>>,
}

struct RecordedVideo {
    index: usize,
    name: String,
    files: Vec<PathBuf>,
}

fn equalize_outputs(
    paths: &[PathBuf],
    output: OutputKind,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if paths.len() < 2 {
        return Ok(());
    }
    let mut durations = Vec::with_capacity(paths.len());
    for path in paths {
        durations.push(
            crate::info::probe(path)?
                .duration_us
                .filter(|duration| *duration > 0),
        );
    }
    let Some(min) = durations.iter().flatten().copied().min() else {
        return Ok(());
    };
    for (path, duration) in paths.iter().zip(durations) {
        match duration {
            Some(duration) if duration > min => {
                match output {
                    OutputKind::Anim(AnimFormat::Webp) => {
                        mediaa_webp::trim_part(path, min)?;
                    }
                    OutputKind::Anim(AnimFormat::Gif) => trim_gif(path, min, cancel)?,
                    OutputKind::Video(container) => trim_video(path, min, container, cancel)?,
                    OutputKind::Audio(_) => continue,
                }
                log::info!(
                    "trimmed output {} by {} us to a shared duration",
                    path.display(),
                    duration - min
                );
            }
            None if output == OutputKind::Anim(AnimFormat::Webp) => {
                mediaa_webp::normalize_part(path, min)?;
                log::info!("normalized still WebP {} to {min} us", path.display());
            }
            _ => {}
        }
    }
    Ok(())
}

fn trim_gif(path: &Path, max_duration_us: i64, cancel: &AtomicBool) -> anyhow::Result<()> {
    let pending = AtomicFile::new(path)?;
    let seconds = max_duration_us as f64 / 1_000_000.0;
    let output = Output::from(pending.path().to_string_lossy().into_owned())
        .set_format("gif")
        .set_format_opt("t", format!("{seconds:.6}"))
        .add_stream_map("0:v");
    let context = FfmpegContext::builder()
        .input(Input::from(path.to_string_lossy().into_owned()))
        .output(output)
        .build()?;
    crate::job::run(context, None, &mut |_| {}, cancel)?;
    pending.commit_replace()?;
    Ok(())
}

fn trim_video(
    path: &Path,
    max_duration_us: i64,
    container: mediaa_core::formats::VideoContainer,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let pending = AtomicFile::new(path)?;
    let seconds = max_duration_us as f64 / 1_000_000.0;
    let output = Output::from(pending.path().to_string_lossy().into_owned())
        .set_format(container.muxer())
        .add_stream_map_with_copy("0")
        .set_format_opt("t", format!("{seconds:.6}"));
    let context = FfmpegContext::builder()
        .input(Input::from(path.to_string_lossy().into_owned()))
        .output(output)
        .build()?;
    crate::job::run(context, None, &mut |_| {}, cancel)?;
    pending.commit_replace()?;
    Ok(())
}

struct FinalizeFailure {
    error: anyhow::Error,
    recovery_files: Vec<PathBuf>,
    keep_work_dir: bool,
}

struct FinalizeOutput {
    paths: Vec<PathBuf>,
    keep_work_dir: bool,
}

impl From<anyhow::Error> for FinalizeFailure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            error,
            recovery_files: Vec::new(),
            keep_work_dir: false,
        }
    }
}

enum TimedJoin<T> {
    Finished(T),
    Panicked,
    TimedOut,
}

fn join_with_timeout<T: Send + 'static>(handle: JoinHandle<T>, timeout: Duration) -> TimedJoin<T> {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return TimedJoin::TimedOut;
        }
        std::thread::sleep(remaining.min(Duration::from_millis(10)));
    }
    match handle.join() {
        Ok(value) => TimedJoin::Finished(value),
        Err(_) => TimedJoin::Panicked,
    }
}

fn reap_capture(
    handle: JoinHandle<anyhow::Result<Vec<PathBuf>>>,
    timeout: Duration,
) -> Result<(), (bool, String)> {
    match join_with_timeout(handle, timeout) {
        TimedJoin::Finished(Ok(_)) => Ok(()),
        TimedJoin::Finished(Err(error)) => Err((false, format!("{error:#}"))),
        TimedJoin::Panicked => Err((false, "capture thread panicked".into())),
        TimedJoin::TimedOut => Err((true, "capture thread did not stop".into())),
    }
}

struct CaptureShutdown {
    videos: Vec<RecordedVideo>,
    failures: Vec<String>,
}

pub struct RecordingSession {
    control: Arc<Control>,
    captures: Vec<Capture>,
    audio: Vec<Box<dyn AudioSource>>,
    tracks: Vec<AudioTrack>,
    output: OutputKind,
    audio_layout: AudioLayout,
    audio_kbps: u32,
    anim: AnimSettings,
    webp_mode: WebpMode,
    progress: RecordingProgress,
    out_dir: MediaDir,
    work_dir: WorkDir,
    title: String,
    started: chrono::DateTime<chrono::Local>,
    capture_stop_timeout: Duration,
    events_tx: Sender<SessionEvent>,
    events_rx: Receiver<SessionEvent>,
}

mod capture;
use capture::{capture_loop, wait_for_source, SourceReady, StartFailure};

impl RecordingSession {
    pub fn start(req: RecordingRequest) -> anyhow::Result<RecordingSession> {
        Self::start_with_cancel(req, &AtomicBool::new(false))
    }

    pub fn start_with_cancel(
        req: RecordingRequest,
        start_cancel: &AtomicBool,
    ) -> anyhow::Result<RecordingSession> {
        if start_cancel.load(Ordering::Relaxed) {
            return Err(RecordingStartCancelled.into());
        }
        match req.output {
            OutputKind::Audio(_) => anyhow::ensure!(
                req.sources.is_empty() && !req.audio.is_empty(),
                "audio recording needs sound sources only"
            ),
            _ => anyhow::ensure!(!req.sources.is_empty(), "nothing to record"),
        }
        std::fs::create_dir_all(&req.work_dir)?;
        std::fs::create_dir_all(&req.out_dir)?;
        let (events_tx, events_rx) = crossbeam_channel::bounded(64);
        let limits = req.limits;
        let control = Arc::new(Control {
            state: Mutex::new(ControlState::default()),
            cv: Condvar::new(),
            clock: Mutex::new(SessionClock::new()),
            started: Instant::now(),
            capture_origin: Mutex::new(None),
            audio_paused: Arc::new(AtomicBool::new(false)),
        });

        let mut audio = Vec::new();
        let mut tracks = Vec::new();
        if req.output.supports_audio() {
            for (i, mut src) in req.audio.into_iter().enumerate() {
                if start_cancel.load(Ordering::Relaxed) {
                    break;
                }
                let path = req.work_dir.join(format!("audio-{i}.wav"));
                let paused = control.audio_paused.clone();
                let ctl = control.clone();
                let clock: Clock = Arc::new(move || ctl.active_time());
                let mut opened: Option<AudioTrack> = None;
                let res = src.start(&mut |rate, channels| {
                    let track =
                        AudioTrack::create(&path, rate, channels, paused.clone(), clock.clone())?;
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
        let gate = Arc::new(StartGate::default());
        let mut errors = Vec::new();
        let mut fatal_error = None;
        let mut preserve_work_dir = false;
        for (i, factory) in req.sources.into_iter().enumerate() {
            if start_cancel.load(Ordering::Relaxed) {
                fatal_error = Some(StartFailure::Cancelled);
                break;
            }
            let scratch = req.work_dir.join(format!("video-{i}.mkv"));
            let frames = Arc::new(AtomicU64::new(0));
            let (ready_tx, ready_rx) = crossbeam_channel::bounded(1);
            let ctl = control.clone();
            let (kind, fps, quality, max_h) = (
                req.output,
                FPS_LIMIT.clamp(req.fps),
                req.video_quality,
                req.max_height,
            );
            let (s, f) = (scratch.clone(), frames.clone());
            let events = events_tx.clone();
            let gate2 = gate.clone();
            let join = std::thread::Builder::new()
                .name(format!("capture-{i}"))
                .spawn(move || {
                    let r = capture_loop(
                        factory,
                        ctl.clone(),
                        fps,
                        kind,
                        quality,
                        max_h,
                        s,
                        f,
                        ready_tx,
                        gate2,
                        events.clone(),
                        req.show_no_signal,
                    );
                    if let Err(e) = &r {
                        log::error!("capture {i} failed: {e:#}");
                        let _ = events.send(SessionEvent::CaptureFailed(format!("{e:#}")));
                        ctl.stop();
                    }
                    r
                })?;
            match wait_for_source(
                &ready_rx,
                start_cancel,
                limits.source_ready_timeout,
                limits.ready_poll_interval,
            ) {
                SourceReady::Ready(name, size) => {
                    let _ = size;
                    captures.push(Capture {
                        index: i,
                        name,
                        frames,
                        join,
                    });
                }
                SourceReady::Failed(error) => {
                    fatal_error = Some(StartFailure::Message(error.clone()));
                    errors.push(error);
                    if let Err((preserve, error)) = reap_capture(join, limits.capture_stop_timeout)
                    {
                        preserve_work_dir |= preserve;
                        errors.push(error);
                    }
                    break;
                }
                SourceReady::Exited => {
                    let error = "capture thread exited".to_string();
                    fatal_error = Some(StartFailure::Message(error.clone()));
                    errors.push(error);
                    if let Err((preserve, error)) = reap_capture(join, limits.capture_stop_timeout)
                    {
                        preserve_work_dir |= preserve;
                        errors.push(error);
                    }
                    break;
                }
                SourceReady::TimedOut => {
                    let error = "capture source did not become ready";
                    errors.push(error.into());
                    control.stop();
                    if let Err((preserve, stop_error)) =
                        reap_capture(join, limits.capture_stop_timeout)
                    {
                        preserve_work_dir |= preserve;
                        errors.push(stop_error);
                    }
                    fatal_error = Some(StartFailure::Message(error.into()));
                    break;
                }
                SourceReady::Cancelled => {
                    control.stop();
                    if let Err((preserve, error)) = reap_capture(join, limits.capture_stop_timeout)
                    {
                        preserve_work_dir |= preserve;
                        errors.push(error);
                    }
                    fatal_error = Some(StartFailure::Cancelled);
                    break;
                }
            }
        }

        if fatal_error.is_none() && !start_cancel.load(Ordering::Relaxed) {
            control.begin();
        }
        gate.open();

        let session = RecordingSession {
            control,
            captures,
            audio,
            tracks,
            output: req.output,
            audio_layout: req.audio_layout,
            audio_kbps: req.audio_kbps,
            anim: req.anim,
            webp_mode: req.webp_mode,
            progress: RecordingProgress::default(),
            out_dir: req.out_dir,
            work_dir: req.work_dir,
            title: req.title,
            started: chrono::Local::now(),
            capture_stop_timeout: limits.capture_stop_timeout,
            events_tx,
            events_rx,
        };
        if let Some(failure) = fatal_error {
            session.discard_with_work_dir(!preserve_work_dir);
            return Err(failure.into());
        } else if start_cancel.load(Ordering::Relaxed) {
            session.discard_with_work_dir(!preserve_work_dir);
            return Err(RecordingStartCancelled.into());
        } else if let OutputKind::Audio(_) = session.output {
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

    pub fn progress(&self) -> RecordingProgress {
        self.progress.clone()
    }

    pub fn pause(&self) {
        self.control.set_paused(true);
    }

    pub fn resume(&self) {
        self.control.set_paused(false);
    }

    pub fn is_paused(&self) -> bool {
        self.control.state.lock().paused
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

    pub fn stop(
        self,
        action: StopAction,
        cancel: Arc<AtomicBool>,
    ) -> anyhow::Result<JoinHandle<()>> {
        let session = Arc::new(Mutex::new(Some(self)));
        let worker_session = session.clone();
        let worker_cancel = cancel.clone();
        let spawn = std::thread::Builder::new()
            .name("finalize".into())
            .spawn(move || {
                let session = worker_session.lock().take();
                if let Some(session) = session {
                    finish_session(session, action, worker_cancel);
                }
            });
        match spawn {
            Ok(handle) => Ok(handle),
            Err(error) => {
                let session = session.lock().take();
                if let Some(session) = session {
                    finish_session(session, action, cancel);
                }
                Err(error.into())
            }
        }
    }

    fn shutdown(&mut self) -> CaptureShutdown {
        self.control.stop();
        let mut done = Vec::new();
        let mut failures = Vec::new();
        let deadline = Instant::now() + self.capture_stop_timeout;
        for c in self.captures.drain(..) {
            let frames = c.frames.load(Ordering::Relaxed);
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                failures.push(format!(
                    "capture \"{}\" did not stop within {:?}",
                    c.name, self.capture_stop_timeout
                ));
                continue;
            }
            match join_with_timeout(c.join, remaining) {
                TimedJoin::Finished(Ok(files)) if frames > 0 && !files.is_empty() => {
                    done.push(RecordedVideo {
                        index: c.index,
                        name: c.name,
                        files,
                    });
                }
                TimedJoin::Finished(Ok(_)) => {
                    failures.push(format!("capture \"{}\" produced no usable output", c.name));
                }
                TimedJoin::Finished(Err(error)) => {
                    failures.push(format!("capture \"{}\" failed: {error:#}", c.name));
                }
                TimedJoin::Panicked => {
                    failures.push(format!("capture \"{}\" panicked", c.name));
                }
                TimedJoin::TimedOut => failures.push(format!(
                    "capture \"{}\" did not stop within {:?}",
                    c.name, self.capture_stop_timeout
                )),
            }
        }
        for a in &mut self.audio {
            a.stop();
        }
        self.audio.clear();
        done.sort_by_key(|video| video.index);
        CaptureShutdown {
            videos: done,
            failures,
        }
    }

    fn finish_tracks(&mut self) -> Vec<PathBuf> {
        self.tracks
            .drain(..)
            .filter(|t| t.samples_written() > 0)
            .filter_map(|t| {
                t.finish()
                    .map_err(|e| log::warn!("audio track: {e:#}"))
                    .ok()
            })
            .collect()
    }

    fn discard(self) {
        self.discard_with_work_dir(true);
    }

    fn discard_with_work_dir(mut self, remove_work_dir: bool) {
        self.shutdown();
        let _ = self.finish_tracks();
        if remove_work_dir {
            let _ = std::fs::remove_dir_all(&self.work_dir);
        }
    }

    fn finalize(mut self, cancel: &Arc<AtomicBool>) -> Result<FinalizeOutput, FinalizeFailure> {
        let CaptureShutdown {
            mut videos,
            failures,
        } = self.shutdown();
        let keep_work_dir = !failures.is_empty();
        for failure in &failures {
            let _ = self.events_tx.send(SessionEvent::Warning(failure.clone()));
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(FinalizeFailure {
                error: anyhow::Error::from(crate::job::Cancelled),
                recovery_files: Vec::new(),
                keep_work_dir: true,
            });
        }
        let wavs = self.finish_tracks();
        if let OutputKind::Audio(format) = self.output {
            if wavs.is_empty() {
                return Err(anyhow::anyhow!("no sound was recorded").into());
            }
            let out =
                naming::media_path(&self.out_dir, &self.title, self.started, format.extension());
            let progress_state = self.progress.clone();
            let mut progress = |p: f32| {
                progress_state.set(Some(p));
            };
            let pending = AtomicFile::new(&out)?;
            convert::encode_audio_recording(
                &wavs,
                pending.path(),
                format,
                self.audio_kbps,
                &mut progress,
                cancel,
            )?;
            pending.commit()?;
            return Ok(FinalizeOutput {
                paths: vec![out],
                keep_work_dir,
            });
        }
        if videos.is_empty() {
            let error = if failures.is_empty() {
                anyhow::anyhow!("no frames were recorded")
            } else {
                anyhow::anyhow!(
                    "recording captured no usable sources: {}",
                    failures.join("; ")
                )
            };
            return Err(FinalizeFailure {
                error,
                recovery_files: Vec::new(),
                keep_work_dir,
            });
        }
        log::info!("stage=finalize mode={:?}", self.webp_mode);
        log::info!("stage=offline-mkv");
        for video in &mut videos {
            if video.files.len() > 1 {
                let combined = self
                    .work_dir
                    .join(format!("video-{}-combined.mkv", video.index));
                if let Err(error) = concat_chunks(&video.files, &combined, cancel) {
                    if crate::job::is_cancelled(&error) {
                        return Err(error.into());
                    }
                    let recovery_files =
                        preserve_recordings(&videos, &self.out_dir, &self.title, self.started);
                    return Err(FinalizeFailure {
                        error,
                        recovery_files,
                        keep_work_dir,
                    });
                }
                video.files = vec![combined];
            }
        }
        let total = videos.len() as f32;
        let mut outputs: Vec<Option<PathBuf>> = vec![None; videos.len()];
        for (i, video) in videos.iter().enumerate() {
            let scratch = &video.files[0];
            let with_audio = !wavs.is_empty()
                && match self.audio_layout {
                    AudioLayout::FirstFileOnly => i == 0,
                    AudioLayout::EveryFile => true,
                };
            let mark = with_audio && self.audio_layout == AudioLayout::FirstFileOnly;
            let title = linked_title(&self.title, i, videos.len(), &video.name, mark);
            let out =
                naming::media_path(&self.out_dir, &title, self.started, self.output.extension());
            let pending = AtomicFile::new(&out)?;
            let progress_state = self.progress.clone();
            let mut progress = |p: f32| {
                progress_state.set(Some((i as f32 + p) / total));
            };
            let encoded: anyhow::Result<()> = match self.output {
                OutputKind::Video(container) => {
                    let tracks: &[PathBuf] = if with_audio { &wavs } else { &[] };
                    convert::mux_recording(
                        scratch,
                        tracks,
                        pending.path(),
                        container,
                        self.audio_kbps,
                        &mut progress,
                        cancel,
                    )
                }
                OutputKind::Audio(_) => unreachable!("handled above"),
                OutputKind::Anim(format) => {
                    let animation_progress_state = self.progress.clone();
                    let mut animation_progress = |p: Option<f32>| {
                        animation_progress_state.set(p);
                    };
                    let effort = match self.webp_mode {
                        WebpMode::Quality => Effort::Balanced,
                        WebpMode::Fast => Effort::Fastest,
                    };
                    let request = AnimationEncodeRequest {
                        input: scratch,
                        output: pending.path(),
                        window: None,
                        format,
                        settings: &self.anim,
                        effort,
                        threads: None,
                    };
                    animation_backends::encode(&request, &mut animation_progress, cancel)
                }
            }
            .and_then(|()| pending.commit().map(|_| ()));
            if let Err(error) = encoded {
                log::error!("stage=failed: {error:#}");
                let recovery_files = if crate::job::is_cancelled(&error) {
                    Vec::new()
                } else {
                    preserve_recordings(&videos, &self.out_dir, &self.title, self.started)
                };
                return Err(FinalizeFailure {
                    error,
                    recovery_files,
                    keep_work_dir,
                });
            }
            if matches!(self.output, OutputKind::Anim(_))
                && crate::preview_cache::full_previews_enabled()
            {
                if let Err(error) = crate::preview_cache::ensure_proxy_for(&out, &out, cancel) {
                    log::warn!("preview proxy {}: {error:#}", out.display());
                }
            }
            outputs[i] = Some(out);
        }
        log::info!("stage=complete");
        let paths: Vec<_> = outputs.into_iter().flatten().collect();
        equalize_outputs(&paths, self.output, cancel)?;
        Ok(FinalizeOutput {
            paths,
            keep_work_dir,
        })
    }
}

mod finalize;
use finalize::{concat_chunks, finish_session, preserve_recordings};

mod recovery;
use recovery::move_recovery_file;
pub use recovery::{recover_incomplete_sessions, RecoveryReport};

pub fn linked_title(title: &str, i: usize, n: usize, source: &str, with_audio: bool) -> String {
    if n <= 1 {
        return title.to_string();
    }
    let audio = if with_audio { " (audio)" } else { "" };
    format!(
        "{title} [{} of {n}] - {}{audio}",
        i + 1,
        naming::sanitize(source, 60)
    )
}

#[cfg(test)]
mod tests;
