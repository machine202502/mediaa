pub use mediaa_core::navigation::Page;

use crate::features::recording::RecordTarget;
use mediaa_core::settings::RecordProfile;
use mediaa_platform::screen::{CaptureWindowId, MonitorInfo};

#[derive(Debug, Clone)]
pub enum Pending {
    RegionScreenshot {
        purpose: crate::features::region_editor::Purpose,
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
