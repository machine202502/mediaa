pub mod backend;
pub mod state;

use mediaa_core::geometry::Rect;
use mediaa_core::settings::{HotkeyAction, RecordProfile};
use mediaa_core::{OutputKind, Settings};
use mediaa_media::recorder::StopAction;
use mediaa_platform::screen::{MonitorInfo, WindowInfo};
use serde::{Deserialize, Serialize};

pub const SETTINGS: &str = "settings";
pub const RECORDING: &str = "recording";
pub const LIBRARY: &str = "library";
pub const RECOVERY: &str = "recovery";
pub const NOTICES: &str = "notices";
pub const SET_SETTINGS: &str = "set_settings";
pub const RECOVERY_EVENT: &str = "recovery";
pub const TRAY_EVENT: &str = "tray";
pub const HOTKEYS_EVENT: &str = "hotkeys";
pub const HOTKEYS_PAUSED: &str = "hotkeys_paused";
pub const TRAY_STATE: &str = "tray_state";
pub const TRAY_SHOW_ACK: &str = "tray_show_ack";
pub const TRAY_HIDE_ACK: &str = "tray_hide_ack";
pub const QUIT: &str = "quit";
pub const PICKER_SHOWN: &str = "picker_shown";
pub const PICKER_STARTING: &str = "picker_starting";
pub const SHOW_WINDOW: &str = "show_window";
pub const RECORDING_START: &str = "recording_start";
pub const RECORDING_STOP: &str = "recording_stop";
pub const RECORDING_TOGGLE: &str = "recording_toggle";
pub const RECORDING_CANCEL_START: &str = "recording_cancel_start";
pub const RECORDING_CANCEL_FINALIZE: &str = "recording_cancel_finalize";
pub const RECORDING_EVENT: &str = "recording";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RecordTargetWire {
    Audio,
    Screens(Vec<MonitorInfo>),
    Region(MonitorInfo, Rect),
    Windows(Vec<WindowInfo>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StopSourceWire {
    Ui,
    Tray,
    Hotkey,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RecordingCommandWire {
    Start {
        target: RecordTargetWire,
        profile: RecordProfile,
        hide: Option<bool>,
    },
    Stop {
        action: StopAction,
        source: StopSourceWire,
    },
    TogglePause,
    CancelStart,
    CancelFinalize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RecordingEventWire {
    ShowWindow,
    ClearRegion,
    FilesChanged(Vec<String>),
    Finished { paths: Vec<String>, sound: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingPageWire {
    Record,
    RecordAudio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingInfoWire {
    pub output: OutputKind,
    pub paused: bool,
    pub elapsed_ms: u64,
    pub has_audio: bool,
    pub sources: Vec<String>,
    pub frames: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RecordingViewWire {
    Idle,
    Starting {
        duration_ms: u64,
    },
    Recording(RecordingInfoWire),
    Saving {
        progress: Option<f32>,
        elapsed_ms: u64,
        recorded_ms: u64,
        discarding: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecordingStateWire {
    pub view: RecordingViewWire,
    pub page: RecordingPageWire,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrayCommandWire {
    PrimaryClick,
    ShowWindow,
    RegionScreenshot,
    FullscreenScreenshot,
    StartRecording,
    StartAudio,
    PauseResume,
    StopAndSave,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrayStateWire {
    Idle,
    Starting,
    Recording,
    Paused,
    Saving,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrayUpdateWire {
    pub state: TrayStateWire,
    pub progress: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoticeLevelWire {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NoticeWire {
    pub id: u64,
    pub level: NoticeLevelWire,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryStateWire {
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryReportWire {
    pub files: Vec<String>,
    pub failures: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryChangeWire {
    pub paths: Vec<String>,
    pub rescan: bool,
}

pub fn encode_tray_command(command: TrayCommandWire) -> Vec<u8> {
    serde_json::to_vec(&command).expect("tray command is serializable")
}

pub fn decode_tray_command(payload: &[u8]) -> Result<TrayCommandWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_hotkeys(actions: &[HotkeyAction]) -> Vec<u8> {
    serde_json::to_vec(actions).expect("hotkeys are serializable")
}

pub fn decode_hotkeys(payload: &[u8]) -> Result<Vec<HotkeyAction>, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_tray_update(update: &TrayUpdateWire) -> Vec<u8> {
    serde_json::to_vec(update).expect("tray update is serializable")
}

pub fn decode_tray_update(payload: &[u8]) -> Result<TrayUpdateWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_notices(notices: &[NoticeWire]) -> Vec<u8> {
    serde_json::to_vec(notices).expect("notices are serializable")
}

pub fn decode_notices(payload: &[u8]) -> Result<Vec<NoticeWire>, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_recovery_state(state: &RecoveryStateWire) -> Vec<u8> {
    serde_json::to_vec(state).expect("recovery state is serializable")
}

pub fn decode_recovery_state(payload: &[u8]) -> Result<RecoveryStateWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_recovery_report(report: &RecoveryReportWire) -> Vec<u8> {
    serde_json::to_vec(report).expect("recovery report is serializable")
}

pub fn decode_recovery_report(payload: &[u8]) -> Result<RecoveryReportWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_library(change: &LibraryChangeWire) -> Vec<u8> {
    serde_json::to_vec(change).expect("library change is serializable")
}

pub fn decode_library(payload: &[u8]) -> Result<LibraryChangeWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_settings(settings: &Settings) -> Vec<u8> {
    serde_json::to_vec(settings).expect("settings are serializable")
}

pub fn decode_settings(payload: &[u8]) -> Result<Settings, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_recording_command(command: &RecordingCommandWire) -> Vec<u8> {
    serde_json::to_vec(command).expect("recording command is serializable")
}

pub fn decode_recording_command(payload: &[u8]) -> Result<RecordingCommandWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_recording_event(event: &RecordingEventWire) -> Vec<u8> {
    serde_json::to_vec(event).expect("recording event is serializable")
}

pub fn decode_recording_event(payload: &[u8]) -> Result<RecordingEventWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_recording(state: &RecordingStateWire) -> Vec<u8> {
    serde_json::to_vec(state).expect("recording state is serializable")
}

pub fn decode_recording(payload: &[u8]) -> Result<RecordingStateWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_roundtrip_through_the_contract() {
        let settings = Settings::default();
        let decoded = decode_settings(&encode_settings(&settings)).expect("decode");
        assert_eq!(decoded, settings);
    }

    #[test]
    fn tray_and_hotkeys_roundtrip_through_the_contract() {
        let command = TrayCommandWire::StartAudio;
        assert_eq!(
            decode_tray_command(&encode_tray_command(command)).expect("decode"),
            command
        );
        let update = TrayUpdateWire {
            state: TrayStateWire::Recording,
            progress: true,
        };
        assert_eq!(
            decode_tray_update(&encode_tray_update(&update)).expect("decode"),
            update
        );
        assert_eq!(
            decode_hotkeys(&encode_hotkeys(&[HotkeyAction::RegionScreenshot])).expect("decode"),
            vec![HotkeyAction::RegionScreenshot]
        );
    }

    #[test]
    fn notices_and_recovery_roundtrip_through_the_contract() {
        let notices = vec![NoticeWire {
            id: 3,
            level: NoticeLevelWire::Warning,
            summary: "s".into(),
            detail: "d".into(),
        }];
        assert_eq!(
            decode_notices(&encode_notices(&notices)).expect("decode"),
            notices
        );
        let state = RecoveryStateWire { running: true };
        assert_eq!(
            decode_recovery_state(&encode_recovery_state(&state)).expect("decode"),
            state
        );
        let report = RecoveryReportWire {
            files: vec!["a".into()],
            failures: vec!["b".into()],
        };
        assert_eq!(
            decode_recovery_report(&encode_recovery_report(&report)).expect("decode"),
            report
        );
    }

    #[test]
    fn library_changes_roundtrip_through_the_contract() {
        let change = LibraryChangeWire {
            paths: vec!["a.mkv".into()],
            rescan: true,
        };
        assert_eq!(
            decode_library(&encode_library(&change)).expect("decode"),
            change
        );
    }

    #[test]
    fn recording_commands_roundtrip_through_the_contract() {
        let monitor = MonitorInfo {
            id: mediaa_platform::screen::MonitorId::new(1),
            device: "display".into(),
            index: 0,
            name: "monitor".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: true,
        };
        let command = RecordingCommandWire::Start {
            target: RecordTargetWire::Region(monitor, Rect::new(0, 0, 100, 100)),
            profile: RecordProfile::Region,
            hide: Some(false),
        };
        let decoded =
            decode_recording_command(&encode_recording_command(&command)).expect("decode");
        assert_eq!(decoded, command);
    }

    #[test]
    fn recording_events_roundtrip_through_the_contract() {
        let event = RecordingEventWire::Finished {
            paths: vec!["a.mkv".into()],
            sound: true,
        };
        let decoded = decode_recording_event(&encode_recording_event(&event)).expect("decode");
        assert_eq!(decoded, event);
    }

    #[test]
    fn recording_state_roundtrip_through_the_contract() {
        let state = RecordingStateWire {
            view: RecordingViewWire::Saving {
                progress: Some(0.5),
                elapsed_ms: 1200,
                recorded_ms: 6000,
                discarding: false,
            },
            page: RecordingPageWire::Record,
        };
        let decoded = decode_recording(&encode_recording(&state)).expect("decode");
        assert_eq!(decoded, state);
    }

    #[test]
    fn recording_recording_view_roundtrip_through_the_contract() {
        let state = RecordingStateWire {
            view: RecordingViewWire::Recording(RecordingInfoWire {
                output: OutputKind::Video(mediaa_core::VideoContainer::Mp4),
                paused: false,
                elapsed_ms: 5000,
                has_audio: true,
                sources: vec!["Screen".into()],
                frames: 150,
            }),
            page: RecordingPageWire::RecordAudio,
        };
        let decoded = decode_recording(&encode_recording(&state)).expect("decode");
        assert_eq!(decoded, state);
    }
}
