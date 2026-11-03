#![forbid(unsafe_code)]

pub mod command;
pub mod message;
pub mod state;
pub mod wire;

pub use command::{
    CleanupReport, ClientCommand, CommandError, CommandOk, ConversionCommand, ErrorCode,
    LibraryCommand, MediaInfo, PickerCommand, PreviewKind, PreviewState, RecordingCommand,
    ScreenshotCommand, ShotResult,
};
pub use message::{
    ClientMessage, PreviewReady, RejectReason, Role, ServerMessage, UiEvent, UiRequest, UiStartup,
};
pub use state::{
    ConversionState, Notice, NoticeAction, PickerState, RecordingState, RecoveryState, State,
};

pub const PROTOCOL: u32 = 1;
