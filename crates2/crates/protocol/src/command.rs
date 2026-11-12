use mediaa2_core::annotate::Shape;
use mediaa2_core::convert::ConversionOptions;
use mediaa2_core::formats::{ImageFormat, MediaKind};
use mediaa2_core::geometry::Rect;
use mediaa2_core::navigation::Page;
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
    ShowWindow { page: Option<Page> },
    Quit,
    SetSettings(Settings),
    HotkeysPaused(bool),
    Recording(RecordingCommand),
    Conversion(ConversionCommand),
    Screenshot(ScreenshotCommand),
    Picker(PickerCommand),
    Transcription(TranscriptionCommand),
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
    Fullscreen {
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
    Transcript(TranscriptView),
    Topics(Vec<TopicChapter>),
    ChatView(ChatView),
    /// the chat a question was queued into, with its pinned model
    Chat {
        chat: i64,
        model: Option<ChatModelInfo>,
    },
    /// connection id -> model ids, from `TranscriptionCommand::ModelsCheck`
    Models(std::collections::BTreeMap<String, Vec<String>>),
    /// number of found pages, from `TranscriptionCommand::WebCheck`
    Web(u32),
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

#[allow(
    clippy::large_enum_variant,
    reason = "protocol commands are short-lived and boxing would complicate matching"
)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum TranscriptionCommand {
    /// run the transcription pipeline over a recording
    Start {
        path: String,
        options: mediaa2_core::agent::TranscriptionOptions,
        from: Option<String>,
    },
    RestartStage {
        task: i64,
        stage: String,
    },
    CancelTask {
        task: i64,
    },
    LoadTranscript {
        path: String,
    },
    LoadTopics {
        path: String,
    },
    LoadChat {
        store: String,
        chat: i64,
    },
    Pause,
    Resume,
    Send {
        store: String,
        chat: Option<i64>,
        question: String,
        locale: String,
        path: Option<String>,
        files: Vec<String>,
        duration: Option<f64>,
        position: Option<f64>,
        stored: bool,
        /// the user confirmed switching the chat to the current agent model
        switch_model: bool,
    },
    ChatOpen {
        store: String,
        chat: Option<i64>,
    },
    ChatClose {
        store: String,
        chat: i64,
    },
    RenameSpeaker {
        store: String,
        speaker: u32,
        name: String,
    },
    /// the user declined the "sounds like X" hint for this speaker
    DismissSuggestion {
        store: String,
        speaker: u32,
    },
    Search {
        query: String,
    },
    ProviderSave {
        connection: mediaa2_core::agent::Connection,
        key: Option<String>,
    },
    ProviderDelete {
        id: String,
    },
    ProviderCheck {
        connection: mediaa2_core::agent::Connection,
        key: Option<String>,
    },
    ModelsSet {
        settings: mediaa2_core::agent::AgentSettings,
    },
    ModelsCheck,
    OllamaPull {
        model: String,
    },
    OllamaServe,
    WebCheck,
    WebSearchSet {
        enabled: bool,
        key: Option<String>,
    },
}

/// A loaded chat: its pinned model (if any) and the message timeline.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ChatView {
    pub chat: i64,
    /// the model pinned to the chat; `None` for a fresh chat
    pub model: Option<ChatModelInfo>,
    pub timeline: mediaa2_core::chat::Timeline,
}

/// The model a chat is pinned to, enough to display and compare.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatModelInfo {
    pub connection: String,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptReply {
    pub start: f64,
    pub end: f64,
    pub speaker: Option<u32>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptSpeaker {
    pub index: u32,
    pub name: String,
    pub source: String,
    /// a name the voice match suggests but is not sure enough to apply
    #[serde(default)]
    pub suggested: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptView {
    pub file_id: String,
    pub path: String,
    pub replies: Vec<TranscriptReply>,
    pub speakers: Vec<TranscriptSpeaker>,
    /// the embedding index of the current model covers the whole transcript
    #[serde(default)]
    pub index_ready: bool,
    /// an index task for this recording is running right now
    #[serde(default)]
    pub index_building: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TopicSection {
    pub title: String,
    pub kind: String,
    pub items: Vec<TopicItem>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TopicItem {
    pub text: String,
    pub time: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TopicChapter {
    pub start: f64,
    pub end: f64,
    pub title: String,
    pub summary: String,
    pub decisions: Vec<String>,
    pub actions: Vec<String>,
    pub sections: Vec<TopicSection>,
}
