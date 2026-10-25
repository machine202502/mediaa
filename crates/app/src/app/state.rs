use super::tasks::TaskId;
use crate::overlay;
use crossbeam_channel::Receiver;
use mediaa_core::geometry::Rect;
use mediaa_core::settings::RecordProfile;
use mediaa_media::error::MediaErrorKind;
use mediaa_media::recorder::{RecordingProgress, RecordingSession, SessionEvent};
use mediaa_platform::screen::{MonitorInfo, WindowInfo};
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
    WindowScreenshot(u32),
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
