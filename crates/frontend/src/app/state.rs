use crate::features::jobs::Job;
use crossbeam_channel::Receiver;
use mediaa_platform::screen::{MonitorInfo, WindowInfo};
use std::path::PathBuf;
use std::time::Instant;

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
}

pub(crate) struct DesktopEvent {
    pub action: DesktopAction,
    pub result: Result<Option<PathBuf>, String>,
    pub notify_errors: bool,
}
