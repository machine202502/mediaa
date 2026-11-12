#![forbid(unsafe_code)]

pub mod command;
pub mod message;
pub mod state;
pub mod transcription;
pub mod wire;

pub use command::{
    ChatModelInfo, ChatView, CleanupReport, ClientCommand, CommandError, CommandOk,
    ConversionCommand, ErrorCode, LibraryCommand, MediaInfo, PickerCommand, PreviewKind,
    PreviewState, RecordingCommand, ScreenshotCommand, ShotResult, TopicChapter, TopicItem,
    TopicSection, TranscriptView, TranscriptionCommand,
};
pub use message::{
    ClientMessage, PreviewReady, RejectReason, Role, ServerMessage, UiEvent, UiRequest, UiStartup,
};
pub use state::{
    ConversionState, Notice, NoticeAction, PickerState, RecordingState, RecoveryState, State,
};
pub use transcription::{
    TranscriptionChatError, TranscriptionEvent, TranscriptionOk, TranscriptionState,
};

pub const PROTOCOL: u32 = 3;
