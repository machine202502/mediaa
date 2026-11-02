use crate::features::command::AppCommand;
use crate::features::conversion::ConversionUiAction;
use crate::features::notices::NoticeLevel;
use mediaa_core::settings::RecordProfile;
use mediaa_platform::screen::{MonitorInfo, WindowInfo};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    SettingsEdited,
    RefreshCatalogs,
    OpenLogs,
    OpenUrl(String),
    Capture(CaptureIntent),
    Notice {
        level: NoticeLevel,
        text: String,
    },
    OpenPath(PathBuf),
    Reveal(PathBuf),
    RenameMedia {
        path: PathBuf,
        title: String,
    },
    OpenInConvert(Vec<PathBuf>),
    PreviewOptionsChanged {
        animated: bool,
        full: bool,
    },
    RegionEditor {
        purpose: crate::features::region_editor::Purpose,
    },
    ReapplyHotkeys,
    SuspendHotkeys,
    StartRecording,
    StartAudioRecording {
        profile: RecordProfile,
        hide: Option<bool>,
    },
    ConversionUiAction(ConversionUiAction),
    AgentOp(mediaa_protocol::backend::AgentOpRequestWire),
    AgentRequest(mediaa_protocol::backend::AgentRequestWire),
    AgentAnalyse(mediaa_protocol::backend::AgentAnalyseWire),
    AgentNames(mediaa_protocol::backend::AgentNamesWire),
    LibraryRemoved(Vec<PathBuf>),
    #[cfg_attr(test, allow(dead_code, reason = "UI tests do not start conversions"))]
    ConversionStart(mediaa_protocol::backend::ConversionStartWire),
    Command(AppCommand),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureIntent {
    Region,
    Fullscreen(Option<MonitorInfo>),
    Window(WindowInfo),
}
