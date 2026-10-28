use crate::features::recording::RecordTarget;
use crate::jobs::Job;
use crate::overlay;
use crossbeam_channel::Receiver;
use mediaa_core::settings::RecordProfile;
use mediaa_platform::screen::{MonitorInfo, WindowId, WindowInfo};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Screenshot,
    Record,
    RecordAudio,
    Library,
    Convert,
    Catalogs,
    Providers,
    Models,
    Queue,
    Budget,
    Settings,
    About,
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
    pub job: Job<()>,
    pub started: Instant,
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
    pub notify_errors: bool,
}
