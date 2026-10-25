use crate::animation_backends::{self, AnimationEncodeRequest, AnimationOrigin, WebpSegmentation};
use crate::convert;
use crate::filters;
use crate::filters::Effort;
use crate::live_webp::{LiveResult, LiveWebp};
use crossbeam_channel::{Receiver, Sender};
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use mediaa_core::atomic_file::AtomicFile;
use mediaa_core::audio::{AudioTrack, Clock};
use mediaa_core::formats::{AnimFormat, OutputKind};
use mediaa_core::limits::FPS as FPS_LIMIT;
use mediaa_core::naming;
use mediaa_core::pacer::{SessionClock, SlotTracker};
use mediaa_core::paths::{MediaDir, WorkDir};
use mediaa_core::settings::AnimSettings;
use mediaa_core::source::{AudioSource, SourceFactory};
use mediaa_webp_pipeline::{PipelineTuning, DEFAULT_MAX_WORKERS};
use parking_lot::{Condvar, Mutex};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static NEXT_WEBP_TRACE: AtomicU64 = AtomicU64::new(1);
type SourceReadyResult = (String, (u32, u32));
type SourceReadyReceiver = Receiver<Result<SourceReadyResult, anyhow::Error>>;

mod request;
pub use request::*;

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
        self.state.lock().stopping = true;
        self.cv.notify_all();
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

#[derive(Debug, Clone)]
struct LivePaths {
    dir: PathBuf,
    template: PathBuf,
    list: PathBuf,
}

struct RecordedVideo {
    index: usize,
    name: String,
    files: Vec<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnimationPath {
    Live,
    Segmented,
    Linear,
}

fn animation_path(output: OutputKind, mode: WebpMode) -> AnimationPath {
    if !matches!(output, OutputKind::Anim(AnimFormat::Webp)) {
        return AnimationPath::Linear;
    }
    match mode {
        WebpMode::Direct => AnimationPath::Linear,
        WebpMode::Segmented => AnimationPath::Segmented,
        WebpMode::SegmentedParallel => AnimationPath::Live,
    }
}

fn new_webp_trace_id() -> String {
    format!(
        "webp-{}-{:04}",
        std::process::id(),
        NEXT_WEBP_TRACE.fetch_add(1, Ordering::Relaxed)
    )
}

fn encode_with_linear_fallback(
    requested: AnimationPath,
    encode: &mut dyn FnMut(AnimationPath) -> anyhow::Result<()>,
    warn: &mut dyn FnMut(String),
) -> anyhow::Result<()> {
    match requested {
        AnimationPath::Live | AnimationPath::Segmented => match encode(AnimationPath::Segmented) {
            Err(error) if !crate::job::is_cancelled(&error) => {
                let warning =
                    format!("segmented WebP failed; falling back to linear encoding: {error:#}");
                warn(warning);
                encode(AnimationPath::Linear)
            }
            result => result,
        },
        AnimationPath::Linear => encode(AnimationPath::Linear),
    }
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
    Unavailable,
}

fn join_with_timeout<T: Send + 'static>(handle: JoinHandle<T>, timeout: Duration) -> TimedJoin<T> {
    let (sender, receiver) = crossbeam_channel::bounded(1);
    let spawned = std::thread::Builder::new()
        .name("join-watchdog".into())
        .spawn(move || {
            let _ = sender.send(handle.join().ok());
        });
    if spawned.is_err() {
        return TimedJoin::Unavailable;
    }
    match receiver.recv_timeout(timeout) {
        Ok(Some(value)) => TimedJoin::Finished(value),
        Ok(None) | Err(crossbeam_channel::RecvTimeoutError::Disconnected) => TimedJoin::Panicked,
        Err(crossbeam_channel::RecvTimeoutError::Timeout) => TimedJoin::TimedOut,
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
        TimedJoin::Unavailable => Err((true, "capture thread could not be joined".into())),
    }
}

struct CaptureShutdown {
    videos: Vec<RecordedVideo>,
    live: Option<LiveResult>,
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
    live_webp: Option<LiveWebp>,
    webp_trace: Option<String>,
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

        let animation_path = animation_path(req.output, req.webp_mode);
        let live_enabled = animation_path == AnimationPath::Live;
        let webp_trace =
            matches!(req.output, OutputKind::Anim(AnimFormat::Webp)).then(new_webp_trace_id);
        if let Some(trace) = &webp_trace {
            log::info!("[{trace}] stage=requested path={animation_path:?}");
        }
        let mut live_lists = Vec::new();
        let mut live_sizes = Vec::new();
        let mut captures = Vec::new();
        let mut errors = Vec::new();
        let mut fatal_error = None;
        let mut preserve_work_dir = false;
        for (i, factory) in req.sources.into_iter().enumerate() {
            if start_cancel.load(Ordering::Relaxed) {
                fatal_error = Some(StartFailure::Cancelled);
                break;
            }
            let scratch = req.work_dir.join(format!("video-{i}.mkv"));
            let live = (live_enabled).then(|| {
                let dir = req.work_dir.join(format!("video-{i}-chunks"));
                LivePaths {
                    template: dir.join("chunk-%06d.mkv"),
                    list: dir.join("chunks.txt"),
                    dir,
                }
            });
            if let Some(live) = &live {
                live_lists.push(live.list.clone());
            }
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
            let live2 = live.clone();
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
                        live2,
                        s,
                        f,
                        ready_tx,
                    );
                    if let Err(e) = &r {
                        log::error!("capture {i} failed: {e:#}");
                        let _ = events.send(SessionEvent::CaptureFailed(format!("{e:#}")));
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
                    live_sizes.push(size);
                    captures.push(Capture {
                        index: i,
                        name,
                        frames,
                        join,
                    });
                }
                SourceReady::Failed(error) => {
                    errors.push(error);
                    if let Err((preserve, error)) = reap_capture(join, limits.capture_stop_timeout)
                    {
                        preserve_work_dir |= preserve;
                        errors.push(error);
                    }
                }
                SourceReady::Exited => {
                    errors.push("capture thread exited".into());
                    if let Err((preserve, error)) = reap_capture(join, limits.capture_stop_timeout)
                    {
                        preserve_work_dir |= preserve;
                        errors.push(error);
                    }
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

        let live_webp =
            if live_enabled && fatal_error.is_none() && !start_cancel.load(Ordering::Relaxed) {
                let root = req.work_dir.join("live-webp");
                match LiveWebp::start(
                    live_lists,
                    root,
                    req.anim.clone(),
                    PipelineTuning {
                        max_workers: DEFAULT_MAX_WORKERS,
                        ..PipelineTuning::default()
                    },
                    webp_trace.clone().expect("live WebP trace"),
                    live_sizes
                        .into_iter()
                        .fold((1, 1), |(width, height), (w, h)| {
                            (width.max(w), height.max(h))
                        }),
                ) {
                    Ok(live) => Some(live),
                    Err(error) => {
                        let trace = webp_trace.as_deref().unwrap_or("webp");
                        log::warn!("[{trace}] stage=live unavailable: {error:#}");
                        None
                    }
                }
            } else {
                None
            };

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
            live_webp,
            webp_trace,
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
        let webp_trace = self.webp_trace.clone();
        let session = Arc::new(Mutex::new(Some(self)));
        let worker_session = session.clone();
        let worker_cancel = cancel.clone();
        let worker_trace = webp_trace.clone();
        let spawn = std::thread::Builder::new()
            .name("finalize".into())
            .spawn(move || {
                let session = worker_session.lock().take();
                if let Some(session) = session {
                    finish_session(session, action, worker_cancel, worker_trace);
                }
            });
        match spawn {
            Ok(handle) => Ok(handle),
            Err(error) => {
                let session = session.lock().take();
                if let Some(session) = session {
                    finish_session(session, action, cancel, webp_trace);
                }
                Err(error.into())
            }
        }
    }

    fn shutdown(
        &mut self,
        finish_live: bool,
        finalize_cancel: Option<Arc<AtomicBool>>,
    ) -> CaptureShutdown {
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
                TimedJoin::Unavailable => {
                    failures.push(format!("capture \"{}\" could not be joined", c.name));
                }
            }
        }
        for a in &mut self.audio {
            a.stop();
        }
        self.audio.clear();
        let live = self.live_webp.take().map(|live| {
            if finish_live {
                live.finish(finalize_cancel.expect("finalization cancellation flag"))
            } else {
                live.cancel()
            }
        });
        done.sort_by_key(|video| video.index);
        CaptureShutdown {
            videos: done,
            live,
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
        self.shutdown(false, None);
        let _ = self.finish_tracks();
        if remove_work_dir {
            let _ = std::fs::remove_dir_all(&self.work_dir);
        }
    }

    fn finalize(mut self, cancel: &Arc<AtomicBool>) -> Result<FinalizeOutput, FinalizeFailure> {
        let CaptureShutdown {
            mut videos,
            live,
            failures,
        } = self.shutdown(true, Some(cancel.clone()));
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
        let requested_path = animation_path(self.output, self.webp_mode);
        let trace = self.webp_trace.as_deref();
        if let Some(trace) = trace {
            log::info!("[{trace}] stage=finalize path={requested_path:?}");
        }
        if live.as_ref().is_some_and(|result| result.timed_out) {
            let recovery_files =
                preserve_recordings(&videos, &self.out_dir, &self.title, self.started);
            return Err(FinalizeFailure {
                error: crate::error::timed_out("live WebP did not stop in time"),
                recovery_files,
                keep_work_dir: true,
            });
        }
        let live_requested = requested_path == AnimationPath::Live;
        let mut webp_fallback = false;
        let mut webp_fallback_detail = Vec::new();
        let mut live_success = live.as_ref().is_some_and(|result| {
            result.error.is_none()
                && videos.iter().all(|video| {
                    result
                        .parts
                        .get(video.index)
                        .is_some_and(|parts| !parts.is_empty())
                })
        });
        if let Some(error) = live.as_ref().and_then(|result| result.error.as_deref()) {
            let trace = trace.unwrap_or("webp");
            log::warn!("[{trace}] stage=live stopped; converting the MKV after recording: {error}");
            webp_fallback_detail.push(format!("live: {error}"));
            webp_fallback = true;
        } else if live_requested && live.is_none() {
            let trace = trace.unwrap_or("webp");
            log::warn!("[{trace}] stage=live not-started; converting the MKV after recording");
            webp_fallback_detail.push("live: not started".into());
            webp_fallback = true;
        } else if live_requested && !live_success {
            let trace = trace.unwrap_or("webp");
            log::warn!("[{trace}] stage=live incomplete; converting the MKV after recording");
            webp_fallback_detail.push("live: incomplete".into());
            webp_fallback = true;
        }

        if live_success {
            let live = live.as_ref().expect("checked above");
            let mut live_outputs = Vec::new();
            for (i, video) in videos.iter().enumerate() {
                let source = if video.files.len() > 1 {
                    let combined = self
                        .work_dir
                        .join(format!("video-{}-preview.mkv", video.index));
                    match concat_chunks(&video.files, &combined, cancel) {
                        Ok(()) => Some(combined),
                        Err(error) => {
                            log::warn!("preview source for {}: {error:#}", video.name);
                            None
                        }
                    }
                } else {
                    Some(video.files[0].clone())
                };
                let title = linked_title(&self.title, i, videos.len(), &video.name, false);
                let out = naming::media_path(&self.out_dir, &title, self.started, "webp");
                let parts = &live.parts[video.index];
                self.progress.set(None);
                let pending = AtomicFile::new(&out)?;
                if let Err(error) = mediaa_webp_pipeline::merge_parts(parts, pending.path())
                    .and_then(|()| pending.commit().map(|_| ()))
                {
                    webp_fallback_detail.push(format!("live merge: {error:#}"));
                    live_success = false;
                    break;
                }
                if let Some(source) = source {
                    if let Err(error) =
                        crate::preview_cache::ensure_proxy_for(&source, &out, cancel)
                    {
                        log::warn!("preview proxy {}: {error:#}", source.display());
                    }
                }
                live_outputs.push(out);
            }
            if live_success {
                if let Some(trace) = trace {
                    log::info!("[{trace}] stage=live complete; merged=webp");
                }
                return Ok(FinalizeOutput {
                    paths: live_outputs,
                    keep_work_dir,
                });
            }
            for output in live_outputs {
                let _ = std::fs::remove_file(output);
            }
            let trace = trace.unwrap_or("webp");
            log::warn!("[{trace}] stage=live merge-failed; falling back to MKV encoding");
            webp_fallback = true;
        }

        if let Some(trace) = trace {
            log::info!("[{trace}] stage=offline-mkv");
        }
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
        let mut outputs = Vec::new();
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
                    let scheduling = PipelineTuning {
                        max_workers: if self.webp_mode == WebpMode::SegmentedParallel {
                            DEFAULT_MAX_WORKERS
                        } else {
                            1
                        },
                        ..PipelineTuning::default()
                    };
                    let request = |_segmented_webp| AnimationEncodeRequest {
                        input: scratch,
                        output: pending.path(),
                        window: None,
                        format,
                        settings: &self.anim,
                        effort: Effort::Balanced,
                        origin: AnimationOrigin::Recording,
                        segmentation: if self.webp_mode == WebpMode::Direct {
                            WebpSegmentation::Direct
                        } else {
                            WebpSegmentation::Segmented
                        },
                        scheduling: scheduling.clone(),
                        threads: None,
                        trace_id: trace,
                    };
                    let mut encode = |path: AnimationPath| {
                        if let Some(trace) = trace {
                            log::info!("[{trace}] stage={path:?}");
                        }
                        animation_backends::encode(
                            &request(path == AnimationPath::Segmented),
                            &mut animation_progress,
                            cancel,
                        )
                    };
                    let mut warn = |warning: String| {
                        let trace = trace.unwrap_or("webp");
                        log::warn!(
                            "[{trace}] stage=offline-segment failed; linear fallback: {warning}"
                        );
                        webp_fallback_detail.push(warning);
                        webp_fallback = true;
                    };
                    encode_with_linear_fallback(requested_path, &mut encode, &mut warn)
                }
            }
            .and_then(|()| pending.commit().map(|_| ()));
            if let Err(error) = encoded {
                if let Some(trace) = trace {
                    log::error!("[{trace}] stage=failed: {error:#}");
                }
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
            if matches!(self.output, OutputKind::Anim(_)) {
                if let Err(error) = crate::preview_cache::ensure_proxy_for(scratch, &out, cancel) {
                    log::warn!("preview proxy {}: {error:#}", scratch.display());
                }
            }
            outputs.push(out);
        }
        if webp_fallback {
            let trace_id = self
                .webp_trace
                .clone()
                .unwrap_or_else(|| "webp".to_string());
            log::info!("[{trace_id}] stage=complete fallback=true");
            let _ = self
                .events_tx
                .send(SessionEvent::Notice(SessionNotice::WebpFallback {
                    trace_id,
                    detail: webp_fallback_detail.join("\n"),
                }));
        } else if let Some(trace) = trace {
            log::info!("[{trace}] stage=complete fallback=false");
        }
        Ok(FinalizeOutput {
            paths: outputs,
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
