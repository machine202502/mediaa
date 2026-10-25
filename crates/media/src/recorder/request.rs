use crate::error::MediaErrorKind;
use mediaa_core::formats::{AnimFormat, OutputKind};
use mediaa_core::paths::{MediaDir, WorkDir};
use mediaa_core::settings::AnimSettings;
use mediaa_core::source::{AudioSource, SourceFactory};
use std::path::PathBuf;
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingStartCancelled;

impl std::fmt::Display for RecordingStartCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("recording start was cancelled")
    }
}

impl std::error::Error for RecordingStartCancelled {}

pub struct RecordingRequest {
    pub(super) sources: Vec<SourceFactory>,
    pub(super) audio: Vec<Box<dyn AudioSource>>,
    pub(super) output: OutputKind,
    pub(super) fps: u32,
    pub(super) video_quality: u8,
    pub(super) max_height: Option<u32>,
    pub(super) audio_layout: AudioLayout,
    pub(super) audio_kbps: u32,
    pub(super) anim: AnimSettings,
    pub(super) webp_mode: WebpMode,
    pub(super) out_dir: MediaDir,
    pub(super) work_dir: WorkDir,
    pub(super) title: String,
    pub(super) limits: RecordingLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordingLimits {
    pub source_ready_timeout: Duration,
    pub ready_poll_interval: Duration,
}

impl Default for RecordingLimits {
    fn default() -> Self {
        Self {
            source_ready_timeout: Duration::from_secs(65),
            ready_poll_interval: Duration::from_millis(100),
        }
    }
}

pub struct RecordingRequestBuilder {
    request: RecordingRequest,
}

impl RecordingRequest {
    pub fn builder(
        output: OutputKind,
        sources: Vec<SourceFactory>,
        audio: Vec<Box<dyn AudioSource>>,
        out_dir: MediaDir,
        work_dir: WorkDir,
        title: String,
    ) -> RecordingRequestBuilder {
        RecordingRequestBuilder {
            request: RecordingRequest {
                sources,
                audio,
                output,
                fps: 30,
                video_quality: 80,
                max_height: None,
                audio_layout: AudioLayout::FirstFileOnly,
                audio_kbps: 192,
                anim: AnimSettings::default(),
                webp_mode: WebpMode::Direct,
                out_dir,
                work_dir,
                title,
                limits: RecordingLimits::default(),
            },
        }
    }

    pub fn output(&self) -> OutputKind {
        self.output
    }

    pub fn set_max_height(&mut self, max_height: Option<u32>) {
        self.max_height = max_height;
    }
}

impl RecordingRequestBuilder {
    pub fn video(mut self, fps: u32, quality: u8, max_height: Option<u32>) -> Self {
        self.request.fps = fps;
        self.request.video_quality = quality;
        self.request.max_height = max_height;
        self
    }

    pub fn audio_layout(mut self, layout: AudioLayout) -> Self {
        self.request.audio_layout = layout;
        self
    }

    pub fn audio_kbps(mut self, kbps: u32) -> Self {
        self.request.audio_kbps = kbps;
        self
    }

    pub fn animation(mut self, settings: AnimSettings) -> Self {
        self.request.anim = settings;
        self
    }

    pub fn webp_mode(mut self, mode: WebpMode) -> Self {
        self.request.webp_mode = mode;
        self
    }

    pub fn limits(mut self, limits: RecordingLimits) -> Self {
        self.request.limits = limits;
        self
    }

    pub fn build(mut self) -> anyhow::Result<RecordingRequest> {
        anyhow::ensure!(
            (1..=60).contains(&self.request.fps),
            "recording fps must be between 1 and 60"
        );
        anyhow::ensure!(
            (1..=100).contains(&self.request.video_quality),
            "recording quality must be between 1 and 100"
        );
        anyhow::ensure!(
            self.request.audio.is_empty() || (32..=320).contains(&self.request.audio_kbps),
            "audio bitrate must be between 32 and 320 kbps"
        );
        match self.request.output {
            OutputKind::Audio(_) => anyhow::ensure!(
                self.request.sources.is_empty() && !self.request.audio.is_empty(),
                "audio recording needs sound sources only"
            ),
            _ => anyhow::ensure!(!self.request.sources.is_empty(), "nothing to record"),
        }
        anyhow::ensure!(
            !self.request.out_dir.as_os_str().is_empty()
                && !self.request.work_dir.as_os_str().is_empty(),
            "recording directories must not be empty"
        );
        anyhow::ensure!(
            !self.request.limits.source_ready_timeout.is_zero()
                && !self.request.limits.ready_poll_interval.is_zero()
                && self.request.limits.ready_poll_interval
                    <= self.request.limits.source_ready_timeout,
            "recording source limits are invalid"
        );
        if !matches!(self.request.output, OutputKind::Anim(AnimFormat::Webp)) {
            self.request.webp_mode = WebpMode::Direct;
        }
        if matches!(self.request.output, OutputKind::Audio(_)) {
            self.request.audio_layout = AudioLayout::EveryFile;
        }
        Ok(self.request)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioLayout {
    FirstFileOnly,
    EveryFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebpMode {
    Direct,
    Segmented,
    SegmentedParallel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopAction {
    Save,
    Discard,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SessionFailure {
    pub error: String,
    pub kind: MediaErrorKind,
    pub recovery_files: Vec<PathBuf>,
    pub log_ref: Option<String>,
}

#[derive(Debug, Clone)]
pub struct RecordingProgress(Arc<AtomicI32>);

impl Default for RecordingProgress {
    fn default() -> Self {
        Self(Arc::new(AtomicI32::new(-1)))
    }
}

impl RecordingProgress {
    pub fn fraction(&self) -> Option<f32> {
        let value = self.0.load(Ordering::Relaxed);
        (value >= 0).then(|| value as f32 / 1000.0)
    }

    pub(super) fn set(&self, progress: Option<f32>) {
        self.0.store(
            progress.map_or(-1, |value| (value.clamp(0.0, 1.0) * 1000.0) as i32),
            Ordering::Relaxed,
        );
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Warning(String),
    Notice(SessionNotice),
    CaptureFailed(String),
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(SessionFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionNotice {
    WebpFallback { trace_id: String, detail: String },
}
