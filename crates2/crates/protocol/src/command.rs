use mediaa2_core::annotate::Shape;
use mediaa2_core::convert::ConversionOptions;
use mediaa2_core::formats::{ImageFormat, MediaKind};
use mediaa2_core::geometry::Rect;
use mediaa2_core::query::{LibraryPage, LibraryQuery};
use mediaa2_core::recording::{RecordTarget, StopAction, StopSource};
use mediaa2_core::screen::{MonitorInfo, WindowInfo};
use mediaa2_core::settings::{AfterCapture, RecordProfile, Settings};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[allow(
    clippy::large_enum_variant,
    reason = "protocol commands are short-lived and boxing would complicate matching"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientCommand {
    ShowWindow,
    Quit,
    SetSettings(Settings),
    HotkeysPaused(bool),
    Recording(RecordingCommand),
    Conversion(ConversionCommand),
    Screenshot(ScreenshotCommand),
    Picker(PickerCommand),
    Library(LibraryCommand),
    Preview { path: PathBuf, kind: PreviewKind },
    Probe { path: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum RecordingCommand {
    Start {
        target: RecordTarget,
        profile: RecordProfile,
        hide: Option<bool>,
    },
    Stop {
        action: StopAction,
        source: StopSource,
    },
    TogglePause,
    CancelStart,
    CancelFinalize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ConversionCommand {
    Start {
        inputs: Vec<PathBuf>,
        target: ConversionOptions,
        output_dir: PathBuf,
    },
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum ScreenshotCommand {
    Monitor {
        monitor: MonitorInfo,
        after: AfterCapture,
    },
    Window {
        window: WindowInfo,
        after: AfterCapture,
    },
    StartPicker,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum PickerCommand {
    Commit {
        session: u64,
        selection: Rect,
        shapes: Vec<Shape>,
        format: ImageFormat,
        action: AfterCapture,
        output: Option<PathBuf>,
    },
    Cancel {
        session: u64,
    },
    Monitor {
        session: u64,
        index: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum LibraryCommand {
    Query(LibraryQuery),
    Refresh,
    Rename {
        from: PathBuf,
        to: PathBuf,
        title: String,
    },
    Delete {
        paths: Vec<PathBuf>,
        permanent: bool,
    },
    Cleanup {
        older_days: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReport {
    pub removed: u32,
    pub failed: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewKind {
    Thumbnail,
    Hover,
    Player,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", content = "value", rename_all = "snake_case")]
pub enum PreviewState {
    Ready(PathBuf),
    Pending,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShotResult {
    pub path: Option<PathBuf>,
    pub size: String,
    pub copied: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandOk {
    Unit,
    Accepted,
    Shot(ShotResult),
    PickerStarted {
        session: u64,
    },
    PickerFrame {
        path: PathBuf,
        width: u32,
        height: u32,
    },
    LibraryPage(LibraryPage),
    Cleanup(CleanupReport),
    Preview(PreviewState),
    Probe(MediaInfo),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandError {
    pub code: ErrorCode,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    Busy,
    Invalid,
    NotFound,
    Unavailable,
    Cancelled,
    Internal,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaInfo {
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub duration_secs: Option<f64>,
    pub audio: bool,
}
