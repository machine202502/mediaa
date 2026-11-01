use crate::logic::command::AppCommand;
use crate::logic::conversion::ConversionUiAction;
use crate::logic::notices::NoticeLevel;
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
        purpose: crate::logic::region_editor::Purpose,
    },
    ReapplyHotkeys,
    SuspendHotkeys,
    StartRecording,
    StartAudioRecording {
        profile: RecordProfile,
        hide: Option<bool>,
    },
    ConversionUiAction(ConversionUiAction),
    Command(AppCommand),
}

#[derive(Debug, Clone, PartialEq)]
pub enum CaptureIntent {
    Region,
    Fullscreen(Option<MonitorInfo>),
    Window(WindowInfo),
}
