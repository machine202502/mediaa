use crate::features::command::RecordingCommand;
use crate::features::navigation::Page;
use crate::features::recording::{RecordTarget, RecordingInfo, RecordingView, StopSource};
use mediaa_protocol::backend::LibraryEntryWire;
use mediaa_protocol::{
    RecordTargetWire, RecordingCommandWire, RecordingPageWire, RecordingStateWire,
    RecordingViewWire, StopSourceWire,
};
use std::time::Duration;

pub(crate) fn recording_from_state(state: &RecordingStateWire) -> (RecordingView, Page) {
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

fn target_to_wire(target: RecordTarget) -> RecordTargetWire {
    match target {
        RecordTarget::Audio => RecordTargetWire::Audio,
        RecordTarget::Screens(screens) => RecordTargetWire::Screens(screens),
        RecordTarget::Region(monitor, rect) => RecordTargetWire::Region(monitor, rect),
        RecordTarget::Windows(windows) => RecordTargetWire::Windows(windows),
    }
}

fn source_to_wire(source: StopSource) -> StopSourceWire {
    match source {
        StopSource::Ui => StopSourceWire::Ui,
        StopSource::Tray => StopSourceWire::Tray,
        StopSource::Hotkey => StopSourceWire::Hotkey,
    }
}

#[allow(dead_code, reason = "used by the frontend shell binary until F4")]
pub(crate) fn recording_command_to_wire(command: RecordingCommand) -> RecordingCommandWire {
    match command {
        RecordingCommand::Start {
            target,
            profile,
            hide,
        } => RecordingCommandWire::Start {
            target: target_to_wire(target),
            profile,
            hide,
        },
        RecordingCommand::Stop { action, source } => RecordingCommandWire::Stop {
            action,
            source: source_to_wire(source),
        },
        RecordingCommand::TogglePause => RecordingCommandWire::TogglePause,
        RecordingCommand::CancelStart => RecordingCommandWire::CancelStart,
        RecordingCommand::CancelFinalize => RecordingCommandWire::CancelFinalize,
    }
}

pub(crate) fn library_entry_from_wire(
    entry: LibraryEntryWire,
) -> Option<mediaa_core::library::LibraryEntry> {
    let modified = std::time::UNIX_EPOCH + std::time::Duration::from_millis(entry.modified_ms);
    let taken = entry
        .taken
        .and_then(|taken| chrono::NaiveDateTime::parse_from_str(&taken, "%Y-%m-%d %H:%M:%S").ok())
        .unwrap_or_default();
    Some(mediaa_core::library::LibraryEntry {
        path: entry.path.into(),
        name: entry.name,
        title: entry.title,
        kind: entry.kind,
        size: entry.size,
        modified,
        taken,
        details: entry.details.map(|details| mediaa_core::library::Details {
            width: details.width,
            height: details.height,
            duration_secs: details.duration_secs,
            audio: details.audio,
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_saving_state_maps_both_ways() {
        let state = RecordingStateWire {
            view: RecordingViewWire::Saving {
                progress: Some(0.25),
                elapsed_ms: 1000,
                recorded_ms: 4000,
                discarding: false,
            },
            page: RecordingPageWire::Record,
        };
        let (view, page) = recording_from_state(&state);
        assert!(matches!(view, RecordingView::Saving { .. }));
        assert_eq!(page, Page::Record);
    }
}
