use super::tasks::TaskId;
use crate::overlay;
use crossbeam_channel::Receiver;
use mediaa_core::geometry::Rect;
use mediaa_core::settings::RecordProfile;
use mediaa_core::Frame;
use mediaa_media::error::MediaErrorKind;
use mediaa_media::recorder::{RecordingProgress, RecordingSession, SessionEvent};
use mediaa_platform::screen::{MonitorInfo, WindowId, WindowInfo};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Screenshot,
    Record,
    RecordAudio,
    Library,
    Convert,
    Settings,
    About,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordTarget {
    Audio,
    Screens(Vec<MonitorInfo>),
    Region(MonitorInfo, Rect),
    Windows(Vec<WindowInfo>),
}

#[derive(Debug, Clone)]
pub(crate) enum Pending {
    RegionScreenshot {
        purpose: overlay::Purpose,
        after_completion: bool,
    },
    FullscreenScreenshot {
        monitor: MonitorInfo,
        after_completion: bool,
    },
    WindowScreenshot(WindowId),
    BeginRecording(RecordTarget, RecordProfile),
}

pub enum RecState {
    Idle,
    Starting {
        rx: Receiver<Result<RecordingSession, RecordingStartFailure>>,
        task: TaskId,
        started: Instant,
        page: Page,
        cancel: Arc<AtomicBool>,
    },
    Recording {
        session: Box<RecordingSession>,
        events: Receiver<SessionEvent>,
    },
    Saving {
        events: Receiver<SessionEvent>,
        progress_state: RecordingProgress,
        progress: Option<f32>,
        started: Instant,
        cancel: Arc<AtomicBool>,
        sound_on_completion: bool,
        task: Option<TaskId>,
    },
}

pub enum RecordingStartFailure {
    Cancelled,
    Failed {
        kind: MediaErrorKind,
        message: String,
    },
}

impl RecState {
    pub fn is_idle(&self) -> bool {
        matches!(self, RecState::Idle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopSource {
    Ui,
    Tray,
    Hotkey,
}

pub(crate) struct ScreenshotJob {
    pub rx: Receiver<ScreenshotDone>,
    pub task: TaskId,
    pub after_completion: bool,
    pub copies_file: bool,
}

pub(crate) struct ScreenshotDone {
    pub copied: bool,
    pub saved: Option<(PathBuf, String)>,
    pub copy_error: Option<String>,
    pub save_error: Option<String>,
    pub copy_file_error: Option<String>,
    pub size: String,
}

pub(crate) struct CopyImageJob {
    pub rx: Receiver<Result<String, String>>,
    pub task: TaskId,
}

pub(crate) enum CatalogEvent {
    Monitors(Result<Vec<MonitorInfo>, String>),
    Windows(Result<Vec<WindowInfo>, String>),
    Microphones {
        devices: Vec<String>,
        default: Option<String>,
    },
}

pub(crate) struct CatalogJob {
    pub rx: Receiver<CatalogEvent>,
    pub task: TaskId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DesktopAction {
    Open(PathBuf),
    Reveal(PathBuf),
    Rename { path: PathBuf, title: String },
}

pub(crate) struct DesktopEvent {
    pub action: DesktopAction,
    pub result: Result<Option<PathBuf>, String>,
}

#[derive(Clone, Copy)]
pub(crate) enum CaptureKind {
    Region {
        purpose: overlay::Purpose,
        after_completion: bool,
        start: usize,
    },
    Fullscreen {
        after_completion: bool,
    },
    Window,
    OverlayMonitor {
        index: usize,
    },
}

pub(crate) struct CaptureJob {
    pub rx: Receiver<Result<Frame, String>>,
    pub task: TaskId,
    pub kind: CaptureKind,
}
