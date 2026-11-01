use crate::logic::recording::RecordTarget;
use crate::logic::region_editor;
use mediaa_core::settings::RecordProfile;
use mediaa_platform::screen::{CaptureWindowId, MonitorInfo};

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
        purpose: region_editor::Purpose,
        after_completion: bool,
        restore: bool,
    },
    FullscreenScreenshot {
        monitor: MonitorInfo,
        after_completion: bool,
    },
    WindowScreenshot(CaptureWindowId),
    BeginRecording(RecordTarget, RecordProfile),
}
