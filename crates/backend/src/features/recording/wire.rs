use crate::features::recording::{RecordTarget, RecordingInfo, RecordingView, StopSource};
use mediaa_core::navigation::Page;
use mediaa_protocol::{
    RecordTargetWire, RecordingInfoWire, RecordingPageWire, RecordingStateWire, RecordingViewWire,
    StopSourceWire,
};
use std::time::Duration;

pub fn recording_state(view: &RecordingView, page: Page) -> RecordingStateWire {
    let view = match view {
        RecordingView::Idle => RecordingViewWire::Idle,
        RecordingView::Starting(duration) => RecordingViewWire::Starting {
            duration_ms: duration.as_millis() as u64,
        },
        RecordingView::Recording(info) => RecordingViewWire::Recording(RecordingInfoWire {
            output: info.output,
            paused: info.paused,
            elapsed_ms: info.elapsed.as_millis() as u64,
            has_audio: info.has_audio,
            sources: info.sources.clone(),
            frames: info.frames,
        }),
        RecordingView::Saving {
            progress,
            elapsed,
            recorded,
            discarding,
        } => RecordingViewWire::Saving {
            progress: *progress,
            elapsed_ms: elapsed.as_millis() as u64,
            recorded_ms: recorded.as_millis() as u64,
            discarding: *discarding,
        },
    };
    let page = match page {
        Page::RecordAudio => RecordingPageWire::RecordAudio,
        _ => RecordingPageWire::Record,
    };
    RecordingStateWire { view, page }
}

pub fn recording_from_state(state: &RecordingStateWire) -> (RecordingView, Page) {
    let view = match &state.view {
        RecordingViewWire::Idle => RecordingView::Idle,
        RecordingViewWire::Starting { duration_ms } => {
            RecordingView::Starting(Duration::from_millis(*duration_ms))
        }
        RecordingViewWire::Recording(info) => RecordingView::Recording(RecordingInfo {
            output: info.output,
            paused: info.paused,
            elapsed: Duration::from_millis(info.elapsed_ms),
            has_audio: info.has_audio,
            sources: info.sources.clone(),
            frames: info.frames,
        }),
        RecordingViewWire::Saving {
            progress,
            elapsed_ms,
            recorded_ms,
            discarding,
        } => RecordingView::Saving {
            progress: *progress,
            elapsed: Duration::from_millis(*elapsed_ms),
            recorded: Duration::from_millis(*recorded_ms),
            discarding: *discarding,
        },
    };
    let page = match state.page {
        RecordingPageWire::RecordAudio => Page::RecordAudio,
        RecordingPageWire::Record => Page::Record,
    };
    (view, page)
}

pub fn target_from_wire(target: RecordTargetWire) -> RecordTarget {
    match target {
        RecordTargetWire::Audio => RecordTarget::Audio,
        RecordTargetWire::Screens(screens) => RecordTarget::Screens(screens),
        RecordTargetWire::Region(monitor, rect) => RecordTarget::Region(monitor, rect),
        RecordTargetWire::Windows(windows) => RecordTarget::Windows(windows),
    }
}

pub fn source_from_wire(source: StopSourceWire) -> StopSource {
    match source {
        StopSourceWire::Ui => StopSource::Ui,
        StopSourceWire::Tray => StopSource::Tray,
        StopSourceWire::Hotkey => StopSource::Hotkey,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_protocol::RecordingStateWire;

    #[test]
    fn backend_recording_state_roundtrips() {
        let state = RecordingStateWire {
            view: RecordingViewWire::Saving {
                progress: Some(0.5),
                elapsed_ms: 1000,
                recorded_ms: 4000,
                discarding: false,
            },
            page: RecordingPageWire::RecordAudio,
        };
        let (view, page) = recording_from_state(&state);
        assert_eq!(recording_state(&view, page), state);
    }
}
