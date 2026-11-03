use crate::command::{ClientCommand, CommandError, CommandOk, PreviewKind};
use crate::state::State;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Launcher,
    Ui,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum UiStartup {
    Show,
    Tray,
    Picker { session: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewReady {
    pub path: PathBuf,
    pub kind: PreviewKind,
    pub file: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiRequest {
    ShowWindow,
    HideWindow,
    OpenPicker { session: u64 },
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiEvent {
    Ready,
    WindowShown,
    WindowHidden,
    Closing,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RejectReason {
    ProtocolVersion { client: u32 },
    UiAlreadyOpen,
    BadHandshake,
}

#[allow(
    clippy::large_enum_variant,
    reason = "protocol messages are short-lived and boxing would complicate matching"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientMessage {
    Hello {
        protocol: u32,
        role: Role,
        startup: Option<UiStartup>,
    },
    Command {
        id: u64,
        command: ClientCommand,
    },
    Ui(UiEvent),
}

#[allow(
    clippy::large_enum_variant,
    reason = "protocol messages are short-lived and boxing would complicate matching"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ServerMessage {
    State(State),
    Result {
        id: u64,
        result: Result<CommandOk, CommandError>,
    },
    UiRequest(UiRequest),
    Preview(PreviewReady),
    Reject {
        reason: RejectReason,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ErrorCode, MediaInfo, PickerCommand, RecordingCommand};
    use crate::state::{
        ConversionState, Notice, NoticeAction, PickerState, RecordingState, RecoveryState,
    };
    use crate::PROTOCOL;
    use mediaa2_core::annotate::{Shape, Style, Tool};
    use mediaa2_core::formats::{ImageFormat, MediaKind, OutputKind, VideoContainer};
    use mediaa2_core::geometry::Rect;
    use mediaa2_core::library::{Details, LibraryEntry};
    use mediaa2_core::notices::NoticeLevel;
    use mediaa2_core::query::LibraryPage;
    use mediaa2_core::recording::RecordTarget;
    use mediaa2_core::screen::{MonitorId, MonitorInfo};
    use mediaa2_core::settings::{AfterCapture, RecordProfile, Settings};
    use std::time::SystemTime;

    fn roundtrip<T>(value: &T) -> T
    where
        T: Serialize + serde::de::DeserializeOwned,
    {
        let text = serde_json::to_string(value).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(1),
            device: r"\\.\DISPLAY1".into(),
            index: 0,
            name: "DELL U2720Q".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            scale: 1.5,
            primary: true,
        }
    }

    fn state() -> State {
        State {
            settings: Settings::default(),
            recording: RecordingState::Active {
                job: 7,
                target: RecordTarget::Region {
                    monitor: monitor(),
                    region: Rect::new(10, 20, 300, 200),
                },
                started_ms: 1_000,
                paused: false,
                elapsed_ms: 4_500,
                sources: vec!["DELL U2720Q".into()],
                has_audio: true,
                output: OutputKind::Video(VideoContainer::Mkv),
            },
            conversion: ConversionState::Running {
                job: 3,
                done: 2,
                total: 5,
                progress: Some(0.4),
                current: Some("clip.mp4".into()),
            },
            recovery: RecoveryState { running: true },
            notices: vec![Notice {
                id: 9,
                level: NoticeLevel::Warning,
                summary: "Recording stopped".into(),
                detail: "The output folder is not writable".into(),
                action: Some(NoticeAction::Reveal("C:/media".into())),
            }],
            picker: Some(PickerState {
                session: 4,
                monitor: monitor(),
                image: "frame.png".into(),
                scale: 1.25,
            }),
            library_revision: 12,
        }
    }

    fn entry(name: &str, kind: MediaKind) -> LibraryEntry {
        LibraryEntry {
            path: PathBuf::from(name),
            name: name.into(),
            title: "Clip".into(),
            kind,
            size: 4096,
            modified: SystemTime::UNIX_EPOCH,
            taken: Default::default(),
            details: Some(Details {
                width: 1920,
                height: 1080,
                duration_secs: Some(12.5),
                audio: true,
            }),
        }
    }

    fn page() -> LibraryPage {
        LibraryPage {
            revision: 12,
            total: 2,
            entries: vec![
                entry("clip.mp4", MediaKind::Video),
                entry("shot.png", MediaKind::Image),
            ],
            next_offset: Some(200),
        }
    }

    #[test]
    fn hello_survives_json() {
        let message = ClientMessage::Hello {
            protocol: PROTOCOL,
            role: Role::Ui,
            startup: Some(UiStartup::Picker { session: 7 }),
        };
        assert_eq!(roundtrip(&message), message);
    }

    #[test]
    fn state_survives_json() {
        let message = ServerMessage::State(state());
        assert_eq!(roundtrip(&message), message);
    }

    #[test]
    fn library_page_survives_json() {
        let result = CommandOk::LibraryPage(page());
        assert_eq!(roundtrip(&result), result);
    }

    #[test]
    fn recording_start_survives_json() {
        let message = ClientMessage::Command {
            id: 11,
            command: ClientCommand::Recording(RecordingCommand::Start {
                target: RecordTarget::Audio,
                profile: RecordProfile::Hotkey,
                hide: Some(true),
            }),
        };
        assert_eq!(roundtrip(&message), message);
    }

    #[test]
    fn picker_commit_survives_json() {
        let mut shape = Shape::begin(Tool::Arrow, (10.0, 20.0), Style::new([255, 0, 0, 255], 4.0));
        shape.drag_to((50.0, 60.0));
        let message = ClientMessage::Command {
            id: 12,
            command: ClientCommand::Picker(PickerCommand::Commit {
                session: 5,
                selection: Rect::new(10, 20, 200, 100),
                shapes: vec![shape],
                format: ImageFormat::Png,
                action: AfterCapture::CopyAndSave,
                output: None,
            }),
        };
        assert_eq!(roundtrip(&message), message);
    }

    #[test]
    fn rejected_result_survives_json() {
        let message = ServerMessage::Result {
            id: 13,
            result: Err(CommandError {
                code: ErrorCode::Busy,
                message: "recording is active".into(),
            }),
        };
        assert_eq!(roundtrip(&message), message);
    }

    #[test]
    fn rejection_survives_json() {
        for reason in [
            RejectReason::ProtocolVersion { client: 2 },
            RejectReason::UiAlreadyOpen,
            RejectReason::BadHandshake,
        ] {
            let message = ServerMessage::Reject { reason };
            assert_eq!(roundtrip(&message), message);
        }
    }

    #[test]
    fn probe_result_survives_json() {
        let result = CommandOk::Probe(MediaInfo {
            kind: MediaKind::Video,
            width: 1920,
            height: 1080,
            duration_secs: Some(12.5),
            audio: true,
        });
        assert_eq!(roundtrip(&result), result);
    }
}
