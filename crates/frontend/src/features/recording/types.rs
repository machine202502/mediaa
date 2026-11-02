use crate::features::navigation::Page;
use mediaa_core::geometry::Rect;
use mediaa_platform::screen::{CaptureWindowId, MonitorId, MonitorInfo};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum RecordTarget {
    Audio,
    Screens(Vec<MonitorInfo>),
    Region(MonitorInfo, Rect),
    Windows(Vec<mediaa_platform::screen::WindowInfo>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum TargetKind {
    #[default]
    Screen,
    Region,
    Windows,
}

#[derive(Default)]
pub(crate) struct RecordUi {
    pub target: TargetKind,
    pub monitors: BTreeSet<MonitorId>,
    pub region: Option<(MonitorInfo, Rect)>,
    pub windows: BTreeSet<CaptureWindowId>,
}

impl RecordUi {
    pub(crate) fn region(&self) -> Option<&(MonitorInfo, Rect)> {
        self.region.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn set_region_for_target(&mut self, target: &RecordTarget) {
        self.region = match target {
            RecordTarget::Region(monitor, rect) => Some((monitor.clone(), *rect)),
            RecordTarget::Audio | RecordTarget::Screens(_) | RecordTarget::Windows(_) => None,
        };
    }

    pub(crate) fn clear_region(&mut self) {
        self.region = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum StopSource {
    Ui,
    Tray,
    Hotkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) enum RecordingPhase {
    Idle,
    Starting,
    Recording { paused: bool },
    Saving,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct RecordingInfo {
    pub output: mediaa_core::OutputKind,
    pub paused: bool,
    pub elapsed: std::time::Duration,
    pub has_audio: bool,
    pub sources: Vec<String>,
    pub frames: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) enum RecordingView {
    Idle,
    Starting(std::time::Duration),
    Recording(RecordingInfo),
    Saving {
        progress: Option<f32>,
        elapsed: std::time::Duration,
        recorded: std::time::Duration,
        discarding: bool,
    },
}

pub(crate) struct ViewState {
    pub ui: RecordUi,
    view: RecordingView,
    phase: RecordingPhase,
    page: Page,
    cancel_requested: bool,
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            ui: RecordUi::default(),
            view: RecordingView::Idle,
            phase: RecordingPhase::Idle,
            page: Page::Record,
            cancel_requested: false,
        }
    }
}

fn phase_of(view: &RecordingView) -> RecordingPhase {
    match view {
        RecordingView::Idle => RecordingPhase::Idle,
        RecordingView::Starting(_) => RecordingPhase::Starting,
        RecordingView::Recording(info) => RecordingPhase::Recording {
            paused: info.paused,
        },
        RecordingView::Saving { .. } => RecordingPhase::Saving,
    }
}

impl ViewState {
    pub(crate) fn is_recording(&self) -> bool {
        matches!(self.phase, RecordingPhase::Recording { .. })
    }

    pub(crate) fn is_paused(&self) -> bool {
        matches!(self.phase, RecordingPhase::Recording { paused: true })
    }

    pub(crate) fn phase(&self) -> RecordingPhase {
        self.phase
    }

    pub(crate) fn apply(&mut self, view: RecordingView, page: Page) {
        self.phase = phase_of(&view);
        self.view = view;
        self.page = page;
    }

    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.phase, RecordingPhase::Idle)
    }

    pub(crate) fn is_starting(&self) -> bool {
        matches!(self.phase, RecordingPhase::Starting)
    }

    pub(crate) fn view(&self) -> RecordingView {
        self.view.clone()
    }

    pub(crate) fn region(&self) -> Option<&(MonitorInfo, Rect)> {
        self.ui.region()
    }

    pub(crate) fn page(&self) -> Page {
        self.page
    }

    pub(crate) fn cancel_finalize(&mut self) {
        self.cancel_requested = true;
    }

    pub(crate) fn take_cancel_requested(&mut self) -> bool {
        std::mem::take(&mut self.cancel_requested)
    }
}
