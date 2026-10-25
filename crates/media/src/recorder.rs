use crate::animation_backends::{self, AnimationEffort, AnimationEncodeRequest, AnimationOrigin};
use crate::convert;
use crate::filters;
use crate::live_webp::{LiveResult, LiveWebp};
use crossbeam_channel::{Receiver, Sender};
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use mediaa_core::audio::{AudioTrack, Clock};
use mediaa_core::formats::{AnimFormat, OutputKind};
use mediaa_core::naming;
use mediaa_core::pacer::{SessionClock, SlotTracker};
use mediaa_core::settings::AnimSettings;
use mediaa_core::source::{AudioSource, SourceFactory};
use mediaa_webp_pipeline::{PipelineTuning, DEFAULT_MAX_WORKERS};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

static NEXT_WEBP_TRACE: AtomicU64 = AtomicU64::new(1);

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
    pub segmented_webp: bool,
    pub parallel_webp: bool,
    pub out_dir: PathBuf,
    pub work_dir: PathBuf,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionFailure {
    pub error: String,
    pub recovery_files: Vec<PathBuf>,
    pub log_ref: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Warning(String),
    Notice(SessionNotice),
    CaptureFailed(String),
    Finalizing(Option<f32>),
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(SessionFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionNotice {
    WebpFallback { trace_id: String, detail: String },
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

fn animation_path(output: OutputKind, segmented_webp: bool, parallel_webp: bool) -> AnimationPath {
    if !matches!(output, OutputKind::Anim(AnimFormat::Webp)) || !segmented_webp {
        AnimationPath::Linear
    } else if parallel_webp {
        AnimationPath::Live
    } else {
        AnimationPath::Segmented
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
}

impl From<anyhow::Error> for FinalizeFailure {
    fn from(error: anyhow::Error) -> Self {
        Self {
            error,
            recovery_files: Vec::new(),
        }
    }
}

pub struct RecordingSession {
    control: Arc<Control>,
    captures: Vec<Capture>,
    audio: Vec<Box<dyn AudioSource>>,
    tracks: Vec<AudioTrack>,
    output: OutputKind,
    audio_in_every_file: bool,
    audio_kbps: u32,
    anim: AnimSettings,
    segmented_webp: bool,
    parallel_webp: bool,
    live_webp: Option<LiveWebp>,
    webp_trace: Option<String>,
    out_dir: PathBuf,
    work_dir: PathBuf,
    title: String,
    started: chrono::DateTime<chrono::Local>,
    events_tx: Sender<SessionEvent>,
    events_rx: Receiver<SessionEvent>,
}

fn scratch_output(
    path: &Path,
    live: Option<&LivePaths>,
    kind: OutputKind,
    quality: u8,
    max_height: Option<u32>,
    size: (u32, u32),
    fps: u32,
) -> (String, Output) {
    let out = match live {
        Some(live) => Output::from(live.template.to_string_lossy().into_owned())
            .set_format("segment")
            .set_format_opt("segment_format", "matroska")
            .set_format_opt("segment_time", "1")
            .set_format_opt("segment_list", live.list.to_string_lossy().into_owned())
            .set_format_opt("segment_list_type", "flat")
            .set_format_opt("reset_timestamps", "1")
            .add_stream_map("[v]"),
        None => Output::from(path.to_string_lossy().into_owned())
            .set_format("matroska")
            .add_stream_map("[v]"),
    };
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
        OutputKind::Anim(_) => {
            let out = out
                .set_video_codec("libx264rgb")
                .set_video_codec_opt("preset", "ultrafast")
                .set_video_codec_opt("qp", "0");
            let out = if live.is_some() {
                out.set_video_codec_opt("g", fps.to_string())
                    .set_video_codec_opt("keyint_min", fps.to_string())
                    .set_video_codec_opt("sc_threshold", "0")
            } else {
                out
            };
            (filters::scratch_graph("bgr0", max_height), out)
        }
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
    live: Option<LivePaths>,
    scratch: PathBuf,
    frames: Arc<AtomicU64>,
    ready: Sender<anyhow::Result<(String, (u32, u32))>>,
) -> anyhow::Result<Vec<PathBuf>> {
    let setup = (|| -> anyhow::Result<_> {
        if let Some(live) = &live {
            std::fs::create_dir_all(&live.dir)?;
        }
        let source = factory()?;
        let (w, h) = source.size();
        let (push_source, handle) = FramePushSource::builder(w, h)
            .pixel_format("rgba")
            .time_base(1, fps as i32)
            .build()?;
        let (graph, output) = scratch_output(
            &scratch,
            live.as_ref(),
            kind,
            quality,
            max_height,
            (w, h),
            fps,
        );
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
            let size = v.0.size();
            let _ = ready.send(Ok((v.0.name(), size)));
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
    scheduler.wait().map_err(anyhow::Error::from)?;
    result?;
    match live {
        Some(live) => chunk_files(&live),
        None => Ok(vec![scratch]),
    }
}

fn chunk_files(live: &LivePaths) -> anyhow::Result<Vec<PathBuf>> {
    if let Ok(text) = std::fs::read_to_string(&live.list) {
        let files: Vec<_> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let path = PathBuf::from(line.trim());
                if path.is_absolute() {
                    path
                } else {
                    live.dir.join(path)
                }
            })
            .collect();
        if !files.is_empty() {
            return Ok(files);
        }
    }
    let mut files = std::fs::read_dir(&live.dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "mkv"))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

impl RecordingSession {
    pub fn start(req: RecordingRequest) -> anyhow::Result<RecordingSession> {
        match req.output {
            OutputKind::Audio(_) => anyhow::ensure!(
                req.sources.is_empty() && !req.audio.is_empty(),
                "audio recording needs sound sources only"
            ),
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

        let animation_path = animation_path(req.output, req.segmented_webp, req.parallel_webp);
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
        for (i, factory) in req.sources.into_iter().enumerate() {
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
                req.fps.clamp(1, 60),
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
            match ready_rx.recv() {
                Ok(Ok((name, size))) => {
                    live_sizes.push(size);
                    captures.push(Capture {
                        index: i,
                        name,
                        frames,
                        join,
                    });
                }
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

        let live_webp = if live_enabled {
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
            audio_in_every_file: req.audio_in_every_file,
            audio_kbps: req.audio_kbps,
            anim: req.anim,
            segmented_webp: req.segmented_webp,
            parallel_webp: req.parallel_webp,
            live_webp,
            webp_trace,
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
        let webp_trace = self.webp_trace.clone();
        std::thread::Builder::new()
            .name("finalize".into())
            .spawn(move || {
                let events = self.events_tx.clone();
                let work_dir = self.work_dir.clone();
                let event = if save {
                    match self.finalize(&cancel) {
                        Ok(paths) => SessionEvent::Finished(paths),
                        Err(failure) if crate::job::is_cancelled(&failure.error) => SessionEvent::Cancelled,
                        Err(failure) => SessionEvent::Failed(SessionFailure {
                            error: format!("{:#}", failure.error),
                            recovery_files: failure.recovery_files,
                            log_ref: webp_trace,
                        }),
                    }
                } else {
                    self.discard();
                    SessionEvent::Cancelled
                };
                let keep_work_dir = matches!(&event, SessionEvent::Failed(failure) if failure.recovery_files.iter().any(|path| path.starts_with(&work_dir)));
                if !keep_work_dir {
                    let _ = std::fs::remove_dir_all(&work_dir);
                }
                let _ = events.send(event);
            })
            .expect("spawn finalize thread")
    }

    fn shutdown(
        &mut self,
        finish_live: bool,
        finalize_cancel: Option<Arc<AtomicBool>>,
    ) -> (Vec<RecordedVideo>, Option<LiveResult>) {
        self.control.stop();
        let mut done = Vec::new();
        for c in self.captures.drain(..) {
            let ok = c.join.join().map_err(|_| anyhow::anyhow!("panicked"));
            let frames = c.frames.load(Ordering::Relaxed);
            match ok {
                Ok(Ok(files)) if frames > 0 && !files.is_empty() => done.push(RecordedVideo {
                    index: c.index,
                    name: c.name,
                    files,
                }),
                _ => log::warn!("capture \"{}\" produced no usable output", c.name),
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
        (done, live)
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

    fn discard(mut self) {
        self.shutdown(false, None);
        let _ = self.finish_tracks();
        let _ = std::fs::remove_dir_all(&self.work_dir);
    }

    fn finalize(mut self, cancel: &Arc<AtomicBool>) -> Result<Vec<PathBuf>, FinalizeFailure> {
        let (mut videos, live) = self.shutdown(true, Some(cancel.clone()));
        let wavs = self.finish_tracks();
        if cancel.load(Ordering::Relaxed) {
            return Err(anyhow::Error::from(crate::job::Cancelled).into());
        }
        if let OutputKind::Audio(format) = self.output {
            if wavs.is_empty() {
                return Err(anyhow::anyhow!("no sound was recorded").into());
            }
            let out =
                naming::media_path(&self.out_dir, &self.title, self.started, format.extension());
            let events = self.events_tx.clone();
            let mut progress = |p: f32| {
                let _ = events.send(SessionEvent::Finalizing(Some(p)));
            };
            convert::encode_audio_recording(
                &wavs,
                &out,
                format,
                self.audio_kbps,
                &mut progress,
                cancel,
            )?;
            return Ok(vec![out]);
        }
        if videos.is_empty() {
            return Err(anyhow::anyhow!("no frames were recorded").into());
        }
        let requested_path = animation_path(self.output, self.segmented_webp, self.parallel_webp);
        let trace = self.webp_trace.as_deref();
        if let Some(trace) = trace {
            log::info!("[{trace}] stage=finalize path={requested_path:?}");
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
                let events = self.events_tx.clone();
                let _ = events.send(SessionEvent::Finalizing(None));
                if let Err(error) = mediaa_webp_pipeline::merge_parts(parts, &out) {
                    let _ = std::fs::remove_file(&out);
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
                return Ok(live_outputs);
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
                    });
                }
                video.files = vec![combined];
            }
        }
        let total = videos.len() as f32;
        let mut outputs = Vec::new();
        for (i, video) in videos.iter().enumerate() {
            let scratch = &video.files[0];
            let with_audio = !wavs.is_empty() && (i == 0 || self.audio_in_every_file);
            let mark = with_audio && !self.audio_in_every_file;
            let title = linked_title(&self.title, i, videos.len(), &video.name, mark);
            let out =
                naming::media_path(&self.out_dir, &title, self.started, self.output.extension());
            let events = self.events_tx.clone();
            let mut progress = |p: f32| {
                let _ = events.send(SessionEvent::Finalizing(Some((i as f32 + p) / total)));
            };
            let encoded: anyhow::Result<()> = match self.output {
                OutputKind::Video(container) => {
                    let tracks: &[PathBuf] = if with_audio { &wavs } else { &[] };
                    convert::mux_recording(scratch, tracks, &out, container, &mut progress, cancel)
                }
                OutputKind::Audio(_) => unreachable!("handled above"),
                OutputKind::Anim(format) => {
                    let events = self.events_tx.clone();
                    let mut animation_progress = |p: Option<f32>| {
                        let _ = events.send(SessionEvent::Finalizing(p));
                    };
                    let scheduling = PipelineTuning {
                        max_workers: if self.parallel_webp {
                            DEFAULT_MAX_WORKERS
                        } else {
                            1
                        },
                        ..PipelineTuning::default()
                    };
                    let request = |segmented_webp| AnimationEncodeRequest {
                        input: scratch,
                        output: &out,
                        window: None,
                        format,
                        settings: &self.anim,
                        effort: AnimationEffort::Balanced,
                        origin: AnimationOrigin::Recording,
                        segmented_webp,
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
            };
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
        Ok(outputs)
    }
}

fn concat_chunks(files: &[PathBuf], output: &Path, cancel: &AtomicBool) -> anyhow::Result<()> {
    anyhow::ensure!(files.len() > 1, "at least two MKV chunks are required");
    let list = output.with_extension("concat.txt");
    let mut entries = String::new();
    for file in files {
        anyhow::ensure!(
            file.is_file() && std::fs::metadata(file).is_ok_and(|metadata| metadata.len() > 0),
            "recorded chunk is missing or empty: {}",
            file.display()
        );
        entries.push_str(&format!("file '{}'\n", concat_path(file)));
    }
    std::fs::write(&list, entries)
        .map_err(|error| anyhow::anyhow!("write concat list {}: {error}", list.display()))?;
    let input = Input::from(path_arg(&list))
        .set_format("concat")
        .set_format_opt("safe", "0");
    let result = FfmpegContext::builder()
        .input(input)
        .output(
            Output::from(path_arg(output))
                .set_format("matroska")
                .add_stream_map_with_copy("0:v"),
        )
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|context| crate::job::run(context, None, &mut |_| {}, cancel));
    let _ = std::fs::remove_file(&list);
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

fn concat_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''")
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn preserve_recordings(
    videos: &[RecordedVideo],
    out_dir: &Path,
    title: &str,
    started: chrono::DateTime<chrono::Local>,
) -> Vec<PathBuf> {
    let mut kept = Vec::new();
    for (index, video) in videos.iter().enumerate() {
        let sources: Vec<_> = video.files.iter().filter(|path| path.is_file()).collect();
        for (part, source) in sources.iter().enumerate() {
            let title = linked_title(title, index, videos.len(), &video.name, false);
            let suffix = if sources.len() == 1 {
                " (source)".to_string()
            } else {
                format!(" (source part {} of {})", part + 1, sources.len())
            };
            let target = naming::media_path(out_dir, &format!("{title}{suffix}"), started, "mkv");
            if let Some(path) = preserve_file(source, &target) {
                kept.push(path);
            }
        }
    }
    kept
}

fn preserve_file(source: &Path, target: &Path) -> Option<PathBuf> {
    if std::fs::rename(source, target).is_ok() {
        return Some(target.to_path_buf());
    }
    match std::fs::copy(source, target) {
        Ok(_) => {
            let _ = std::fs::remove_file(source);
            Some(target.to_path_buf())
        }
        Err(error) => {
            let _ = std::fs::remove_file(target);
            log::warn!("keep source recording {}: {error:#}", source.display());
            Some(source.to_path_buf())
        }
    }
}

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
mod tests {
    use super::{
        animation_path, concat_chunks, encode_with_linear_fallback, linked_title,
        preserve_recordings, AnimationPath,
    };
    use chrono::{Local, TimeZone};
    use ez_ffmpeg::{FfmpegContext, Input, Output};
    use mediaa_core::formats::{AnimFormat, OutputKind, VideoContainer};
    use std::path::PathBuf;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn linked_names() {
        assert_eq!(linked_title("Rec", 0, 1, "Display 1", true), "Rec");
        assert_eq!(
            linked_title("Rec", 0, 2, "Display 1", true),
            "Rec [1 of 2] - Display 1 (audio)"
        );
        assert_eq!(
            linked_title("Rec", 1, 2, "Code: main.rs", false),
            "Rec [2 of 2] - Code main.rs"
        );
    }

    #[test]
    fn live_webp_is_the_only_chunked_recording_path() {
        let webp = OutputKind::Anim(AnimFormat::Webp);
        assert_eq!(animation_path(webp, true, true), AnimationPath::Live);
        assert_eq!(animation_path(webp, true, false), AnimationPath::Segmented);
        assert_eq!(animation_path(webp, false, true), AnimationPath::Linear);
        assert_eq!(
            animation_path(OutputKind::Anim(AnimFormat::Gif), true, true),
            AnimationPath::Linear
        );
        assert_eq!(
            animation_path(OutputKind::Video(VideoContainer::Mkv), true, true),
            AnimationPath::Linear
        );
    }

    #[test]
    fn segmented_failure_retries_only_the_linear_encoder() {
        let mut calls = Vec::new();
        let mut warnings = Vec::new();
        encode_with_linear_fallback(
            AnimationPath::Segmented,
            &mut |path| {
                calls.push(path);
                if path == AnimationPath::Segmented {
                    anyhow::bail!("out of memory")
                }
                Ok(())
            },
            &mut |warning| warnings.push(warning),
        )
        .unwrap();
        assert_eq!(calls, [AnimationPath::Segmented, AnimationPath::Linear]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("out of memory"));
    }

    #[test]
    fn cancel_and_linear_paths_do_not_retry() {
        let mut cancelled_calls = Vec::new();
        let error = encode_with_linear_fallback(
            AnimationPath::Live,
            &mut |path| {
                cancelled_calls.push(path);
                Err(crate::job::Cancelled.into())
            },
            &mut |_| panic!("cancelled encoding must not warn"),
        )
        .unwrap_err();
        assert!(crate::job::is_cancelled(&error));
        assert_eq!(cancelled_calls, [AnimationPath::Segmented]);

        let mut linear_calls = Vec::new();
        encode_with_linear_fallback(
            AnimationPath::Linear,
            &mut |path| {
                linear_calls.push(path);
                Ok(())
            },
            &mut |_| panic!("linear encoding has no fallback"),
        )
        .unwrap();
        assert_eq!(linear_calls, [AnimationPath::Linear]);
    }

    #[test]
    fn failed_finalize_keeps_source_recordings_in_the_library() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let out = dir.path().join("out");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let source = work.join("video-0.mkv");
        std::fs::write(&source, b"recorded frames").unwrap();
        let started = Local.with_ymd_and_hms(2026, 10, 25, 6, 0, 0).unwrap();
        let kept = preserve_recordings(
            &[super::RecordedVideo {
                index: 0,
                name: "Display 1".into(),
                files: vec![source.clone()],
            }],
            &out,
            "Recording",
            started,
        );
        assert_eq!(kept.len(), 1);
        assert!(!source.exists());
        assert_eq!(
            kept[0].extension().and_then(|ext| ext.to_str()),
            Some("mkv")
        );
        assert_eq!(std::fs::read(&kept[0]).unwrap(), b"recorded frames");
        assert!(kept[0].starts_with(&out));
        let expected: PathBuf = out.join("Recording (source)_2026-10-25_06-00-00.mkv");
        assert_eq!(kept[0], expected);
    }

    #[test]
    fn failed_chunk_conversion_keeps_every_source_part() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let out = dir.path().join("out");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        let first = work.join("chunk-000000.mkv");
        let second = work.join("chunk-000001.mkv");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let started = Local.with_ymd_and_hms(2026, 10, 25, 6, 0, 0).unwrap();
        let kept = preserve_recordings(
            &[super::RecordedVideo {
                index: 0,
                name: "Display 1".into(),
                files: vec![first, second],
            }],
            &out,
            "Recording",
            started,
        );
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().all(|path| path.starts_with(&out)));
        assert!(kept[0].to_string_lossy().contains("part 1 of 2"));
        assert!(kept[1].to_string_lossy().contains("part 2 of 2"));
        assert_eq!(std::fs::read(&kept[0]).unwrap(), b"first");
        assert_eq!(std::fs::read(&kept[1]).unwrap(), b"second");
    }

    #[test]
    fn chunk_concat_keeps_both_parts_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut chunks = Vec::new();
        for (index, color) in ["red", "blue"].into_iter().enumerate() {
            let path = dir.path().join(format!("part-{index}.mkv"));
            let context = FfmpegContext::builder()
                .input(
                    Input::from(format!("color=c={color}:s=64x48:r=10:d=0.4")).set_format("lavfi"),
                )
                .filter_desc("[0:v]format=bgr0[v]")
                .output(
                    Output::from(path.to_string_lossy().into_owned())
                        .set_format("matroska")
                        .set_video_codec("libx264rgb")
                        .set_video_codec_opt("preset", "ultrafast")
                        .set_video_codec_opt("qp", "0")
                        .add_stream_map("[v]"),
                )
                .build()
                .unwrap();
            crate::job::run(context, None, &mut |_| {}, &AtomicBool::new(false)).unwrap();
            chunks.push(path);
        }
        let combined = dir.path().join("combined.mkv");
        concat_chunks(&chunks, &combined, &AtomicBool::new(false)).unwrap();
        let info = crate::info::probe(&combined).unwrap();
        assert_eq!((info.width, info.height), (64, 48));
        assert!(
            info.duration_secs()
                .is_some_and(|duration| (0.7..0.9).contains(&duration)),
            "{info:?}"
        );
    }
}
