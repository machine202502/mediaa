use mediaa2_core::formats::OutputKind;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::recording::RecordTarget;
use mediaa2_core::screen::MonitorInfo;
use mediaa2_core::settings::Settings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub settings: Settings,
    pub recording: RecordingState,
    pub conversion: ConversionState,
    pub recovery: RecoveryState,
    pub notices: Vec<Notice>,
    pub picker: Option<PickerState>,
    pub library_revision: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum RecordingState {
    Idle,
    Starting {
        started_ms: u64,
        target: RecordTarget,
    },
    Active {
        job: u64,
        target: RecordTarget,
        started_ms: u64,
        paused: bool,
        elapsed_ms: u64,
        sources: Vec<String>,
        has_audio: bool,
        output: OutputKind,
    },
    Finalizing {
        job: u64,
        recorded_ms: u64,
        progress: Option<f32>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum ConversionState {
    Idle,
    Running {
        job: u64,
        done: u32,
        total: u32,
        progress: Option<f32>,
        current: Option<String>,
    },
    Cancelling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryState {
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PickerState {
    pub session: u64,
    pub monitor: MonitorInfo,
    pub image: PathBuf,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub id: u64,
    pub level: NoticeLevel,
    pub summary: String,
    pub detail: String,
    pub action: Option<NoticeAction>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum NoticeAction {
    Reveal(PathBuf),
    Open(PathBuf),
    Copy(String),
}
