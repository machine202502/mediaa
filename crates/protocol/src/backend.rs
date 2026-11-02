use mediaa_core::formats::MediaKind;
use serde::{Deserialize, Serialize};

pub const LIBRARY_QUERY: &str = "library_query";
pub const LIBRARY_EDIT: &str = "library_edit";
pub const CAPTURE_REQUEST: &str = "capture_request";
pub const SCREENSHOT_COMMIT: &str = "screenshot_commit";
pub const CLIPBOARD_COPY: &str = "clipboard_copy";
pub const CONVERSION_START: &str = "conversion_start";
pub const CONVERSION_CANCEL: &str = "conversion_cancel";
pub const CONVERSION_EVENT: &str = "conversion";
pub const LIBRARY_ADD: &str = "library_add";
pub const LIBRARY_REMOVE: &str = "library_remove";
pub const LIBRARY_RENAME: &str = "library_rename";
pub const PREVIEW_ENSURE: &str = "preview_ensure";
pub const PREVIEW_EVENT: &str = "preview";
pub const AGENT_EVENT: &str = "agent";
pub const AGENT_SEND: &str = "agent_send";
pub const AGENT_STOP: &str = "agent_stop";
pub const AGENT_RESUME: &str = "agent_resume";
pub const AGENT_STATE: &str = "agent_state";
pub const AGENT_SNAPSHOT: &str = "agent_snapshot";
pub const AGENT_TASK: &str = "agent_task";
pub const AGENT_OP: &str = "agent_op";
pub const AGENT_STEER: &str = "agent_steer";
pub const AGENT_ANALYSE: &str = "agent_analyse";
pub const AGENT_NAMES: &str = "agent_names";
pub const AGENT_REQUEST: &str = "agent_request";
pub const AGENT_REQUEST_EVENT: &str = "agent_request_event";
pub const LIBRARY_CHANGED: &str = "library_changed";

#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct DetailsWire {
    pub width: u32,
    pub height: u32,
    pub duration_secs: Option<f64>,
    pub audio: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryEntryWire {
    pub path: String,
    pub name: String,
    pub title: String,
    pub kind: MediaKind,
    pub size: u64,
    pub modified_ms: u64,
    pub taken: Option<String>,
    pub details: Option<DetailsWire>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LibrarySortWire {
    #[default]
    Newest,
    Oldest,
    Title,
    Largest,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryQueryWire {
    pub search: String,
    pub kind: Option<MediaKind>,
    pub sort: LibrarySortWire,
    pub limit: u32,
    pub cursor: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LibraryChangedWire {
    pub version: u64,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSendWire {
    pub store: String,
    pub chat: Option<i64>,
    pub question: String,
    pub locale: String,
    pub path: Option<String>,
    pub files: Vec<String>,
    pub duration: Option<f64>,
    pub position: Option<f64>,
    #[serde(default)]
    pub stored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSendResultWire {
    pub chat: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentTaskWire {
    pub id: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentStateWire {
    pub running: bool,
    pub task: Option<String>,
    pub status: Option<String>,
    pub progress: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryPageWire {
    pub entries: Vec<LibraryEntryWire>,
    pub next_cursor: Option<u64>,
    pub version: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LibraryEditWire {
    Rename { path: String, title: String },
    Delete { path: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PreviewKindWire {
    Thumbnail,
    Hover,
    Player,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewRequestWire {
    pub path: String,
    pub kind: PreviewKindWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PreviewStateWire {
    Ready { path: String },
    Pending,
    Failed { error: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreviewChangedWire {
    pub path: String,
    pub kind: PreviewKindWire,
    pub state: PreviewStateWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEventWire {
    pub task: i64,
    pub event: mediaa_core::assistant::Event,
}

pub fn encode_library_query(query: &LibraryQueryWire) -> Vec<u8> {
    serde_json::to_vec(query).expect("library query is serializable")
}

pub fn decode_library_query(payload: &[u8]) -> Result<LibraryQueryWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_library_changed(changed: &LibraryChangedWire) -> Vec<u8> {
    serde_json::to_vec(changed).expect("library changed is serializable")
}

pub fn decode_library_changed(payload: &[u8]) -> Result<LibraryChangedWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_state(state: &AgentStateWire) -> Vec<u8> {
    serde_json::to_vec(state).expect("agent state is serializable")
}

pub fn decode_agent_state(payload: &[u8]) -> Result<AgentStateWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_library_page(page: &LibraryPageWire) -> Vec<u8> {
    serde_json::to_vec(page).expect("library page is serializable")
}

pub fn decode_library_page(payload: &[u8]) -> Result<LibraryPageWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryRemoveWire {
    pub paths: Vec<String>,
    pub permanent: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryRenameWire {
    pub from: String,
    pub to: String,
    pub title: String,
}

pub fn encode_library_rename(rename: &LibraryRenameWire) -> Vec<u8> {
    serde_json::to_vec(rename).expect("library rename is serializable")
}

pub fn decode_library_rename(payload: &[u8]) -> Result<LibraryRenameWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_library_remove(remove: &LibraryRemoveWire) -> Vec<u8> {
    serde_json::to_vec(remove).expect("library remove is serializable")
}

pub fn decode_library_remove(payload: &[u8]) -> Result<LibraryRemoveWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_library_add(paths: &[String]) -> Vec<u8> {
    serde_json::to_vec(paths).expect("library paths are serializable")
}

pub fn decode_library_add(payload: &[u8]) -> Result<Vec<String>, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_library_edit(edit: &LibraryEditWire) -> Vec<u8> {
    serde_json::to_vec(edit).expect("library edit is serializable")
}

pub fn decode_library_edit(payload: &[u8]) -> Result<LibraryEditWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_preview_request(request: &PreviewRequestWire) -> Vec<u8> {
    serde_json::to_vec(request).expect("preview request is serializable")
}

pub fn decode_preview_request(payload: &[u8]) -> Result<PreviewRequestWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_preview_changed(changed: &PreviewChangedWire) -> Vec<u8> {
    serde_json::to_vec(changed).expect("preview changed is serializable")
}

pub fn decode_preview_changed(payload: &[u8]) -> Result<PreviewChangedWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_event(event: &AgentEventWire) -> Vec<u8> {
    serde_json::to_vec(event).expect("agent event is serializable")
}

pub fn decode_agent_event(payload: &[u8]) -> Result<AgentEventWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_and_preview_contracts_roundtrip() {
        let entry = LibraryEntryWire {
            path: "C:/media/a.mp4".into(),
            name: "a.mp4".into(),
            title: "a".into(),
            kind: MediaKind::Video,
            size: 42,
            modified_ms: 1,
            taken: Some("2026-11-02 10:00:00".into()),
            details: Some(DetailsWire {
                width: 1920,
                height: 1080,
                duration_secs: Some(3.5),
                audio: true,
            }),
        };
        let page = LibraryPageWire {
            entries: vec![entry],
            next_cursor: None,
            version: 7,
        };
        assert_eq!(
            decode_library_page(&encode_library_page(&page)).expect("page"),
            page
        );
        let edit = LibraryEditWire::Rename {
            path: "C:/media/a.mp4".into(),
            title: "b".into(),
        };
        assert_eq!(
            decode_library_edit(&encode_library_edit(&edit)).expect("edit"),
            edit
        );
        let remove = LibraryRemoveWire {
            paths: vec!["C:/media/a.mp4".into()],
            permanent: false,
        };
        assert_eq!(
            decode_library_remove(&encode_library_remove(&remove)).expect("remove"),
            remove
        );
        let rename = LibraryRenameWire {
            from: "C:/media/a.mp4".into(),
            to: "C:/media/b.mp4".into(),
            title: "b".into(),
        };
        assert_eq!(
            decode_library_rename(&encode_library_rename(&rename)).expect("rename"),
            rename
        );
        let changed = LibraryChangedWire {
            version: 8,
            removed: vec!["C:/media/old.mp4".into()],
        };
        assert_eq!(
            decode_library_changed(&encode_library_changed(&changed)).expect("changed"),
            changed
        );
        let state = AgentStateWire {
            running: true,
            task: Some("t".into()),
            status: Some("working".into()),
            progress: Some(50),
        };
        assert_eq!(
            decode_agent_state(&encode_agent_state(&state)).expect("state"),
            state
        );
        let request = PreviewRequestWire {
            path: "C:/media/a.mp4".into(),
            kind: PreviewKindWire::Thumbnail,
        };
        assert_eq!(
            decode_preview_request(&encode_preview_request(&request)).expect("request"),
            request
        );
        let changed = PreviewChangedWire {
            path: request.path,
            kind: request.kind,
            state: PreviewStateWire::Ready {
                path: "C:/cache/a.webp".into(),
            },
        };
        assert_eq!(
            decode_preview_changed(&encode_preview_changed(&changed)).expect("changed"),
            changed
        );
    }

    #[test]
    fn agent_events_roundtrip() {
        let events = vec![
            AgentEventWire {
                task: 1,
                event: mediaa_core::assistant::Event::AnswerText("hello".into()),
            },
            AgentEventWire {
                task: 1,
                event: mediaa_core::assistant::Event::StepStarted {
                    title: "search".into(),
                    tool: "search".into(),
                },
            },
            AgentEventWire {
                task: 1,
                event: mediaa_core::assistant::Event::Metrics(mediaa_core::assistant::Metrics {
                    context_tokens: 10,
                    context: 20,
                    speed: Some(1.5),
                }),
            },
            AgentEventWire {
                task: 1,
                event: mediaa_core::assistant::Event::Done,
            },
            AgentEventWire {
                task: 1,
                event: mediaa_core::assistant::Event::Failed("bad".into()),
            },
        ];
        for event in events {
            assert_eq!(
                decode_agent_event(&encode_agent_event(&event)).expect("event"),
                event
            );
        }
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipboardWire {
    pub text: Option<String>,
    pub files: Vec<String>,
    pub image: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversionStartWire {
    pub inputs: Vec<String>,
    pub target: mediaa_media::convert::Target,
    pub folder: Option<String>,
    pub settings: mediaa_media::convert::ConvertSettings,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConversionStateWire {
    pub running: bool,
    pub progress: Option<f32>,
    pub done: usize,
    pub total: usize,
    pub error: Option<String>,
    pub finished: Option<String>,
}
pub fn encode_conversion_start(start: &ConversionStartWire) -> Vec<u8> {
    serde_json::to_vec(start).expect("conversion start is serializable")
}

pub fn decode_conversion_start(payload: &[u8]) -> Result<ConversionStartWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_conversion_state(state: &ConversionStateWire) -> Vec<u8> {
    serde_json::to_vec(state).expect("conversion state is serializable")
}

pub fn decode_conversion_state(payload: &[u8]) -> Result<ConversionStateWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}
pub fn encode_agent_task(task: &AgentTaskWire) -> Vec<u8> {
    serde_json::to_vec(task).expect("agent task is serializable")
}

pub fn decode_agent_task(payload: &[u8]) -> Result<AgentTaskWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}
pub fn encode_agent_send(send: &AgentSendWire) -> Vec<u8> {
    serde_json::to_vec(send).expect("agent send is serializable")
}

pub fn decode_agent_send(payload: &[u8]) -> Result<AgentSendWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}
pub fn encode_agent_send_result(result: &AgentSendResultWire) -> Vec<u8> {
    serde_json::to_vec(result).expect("agent send result is serializable")
}

pub fn decode_agent_send_result(payload: &[u8]) -> Result<AgentSendResultWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}
pub fn encode_clipboard(copy: &ClipboardWire) -> Vec<u8> {
    serde_json::to_vec(copy).expect("clipboard is serializable")
}

pub fn decode_clipboard(payload: &[u8]) -> Result<ClipboardWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CaptureTargetWire {
    Monitor { index: usize },
    Window { id: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CaptureRequestWire {
    pub target: CaptureTargetWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CaptureSourceWire {
    pub path: String,
    pub work: String,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScreenshotCommitWire {
    pub source: String,
    pub output: Option<String>,
    pub format: String,
    pub copy_image: bool,
    pub copy_file: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShotResultWire {
    pub path: Option<String>,
    pub size: String,
    pub copied: bool,
}

pub fn encode_capture_request(request: &CaptureRequestWire) -> Vec<u8> {
    serde_json::to_vec(request).expect("capture request is serializable")
}
pub fn decode_capture_request(payload: &[u8]) -> Result<CaptureRequestWire, String> {
    serde_json::from_slice(payload).map_err(|e| e.to_string())
}
pub fn encode_capture_source(source: &CaptureSourceWire) -> Vec<u8> {
    serde_json::to_vec(source).expect("capture source is serializable")
}
pub fn encode_capture_target(target: &CaptureTargetWire) -> Vec<u8> {
    serde_json::to_vec(target).expect("capture target is serializable")
}
pub fn encode_screenshot_commit(commit: &ScreenshotCommitWire) -> Vec<u8> {
    serde_json::to_vec(commit).expect("screenshot commit is serializable")
}
pub fn decode_screenshot_commit(payload: &[u8]) -> Result<ScreenshotCommitWire, String> {
    serde_json::from_slice(payload).map_err(|e| e.to_string())
}
pub fn encode_shot_result(result: &ShotResultWire) -> Vec<u8> {
    serde_json::to_vec(result).expect("shot result is serializable")
}
pub fn decode_shot_result(payload: &[u8]) -> Result<ShotResultWire, String> {
    serde_json::from_slice(payload).map_err(|e| e.to_string())
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTaskViewWire {
    pub id: i64,
    pub parent: Option<i64>,
    pub kind: String,
    pub title: String,
    pub scope: Option<String>,
    pub status: AgentTaskStatusWire,
    pub progress: f32,
    pub waiting: Option<AgentWaitReasonWire>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub elapsed_ms: i64,
    pub estimate_ms: Option<i64>,
    pub error: Option<String>,
    pub note: Option<AgentNoteWire>,
    pub stats: AgentStatsWire,
    pub units: f64,
    pub resource: Option<AgentResourceWire>,
    pub model: Option<String>,
    pub role: String,
    pub inline: bool,
    pub attempts: i64,
    pub input: serde_json::Value,
    pub cursor: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentSnapshotWire {
    pub version: u64,
    pub now_ms: i64,
    pub tasks: Vec<AgentTaskViewWire>,
    pub kind_stats: Vec<AgentKindStatWire>,
    pub scopes: Vec<AgentScopeWire>,
    pub slots: mediaa_core::slots::Status,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentTaskStatusWire {
    Queued,
    Running,
    Stopping,
    Paused,
    Waiting,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentResourceWire {
    Speech,
    Embed,
    Llm,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentWaitReasonWire {
    Dependency,
    Resource(AgentResourceWire),
    Memory,
    Paused,
    Turn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AgentNoteWire {
    PausedBy {
        title: String,
    },
    FailedBy {
        title: String,
    },
    Hang {
        resource: AgentResourceWire,
        idle_s: u64,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentStatsWire {
    pub model_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub model_ms: u64,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentKindStatWire {
    pub kind: String,
    pub runs: i64,
    pub ms: i64,
    pub units: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentScopeWire {
    pub key: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentTaskDetailsWire {
    pub task: AgentTaskViewWire,
    pub result: serde_json::Value,
    pub detail: serde_json::Value,
    pub partial: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentOpWire {
    Pause,
    Resume,
    Cancel,
    Restart,
    Remove,
    ClearDone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOpRequestWire {
    pub task: Option<i64>,
    pub op: AgentOpWire,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSteerWire {
    pub task: i64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentAnalyseWire {
    pub path: String,
    pub from: Option<String>,
    pub options: mediaa_core::agent::AnalysisOptions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentNamesWire {
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentRequestWire {
    pub request: u64,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentResponseWire {
    pub request: u64,
    pub state: AgentResponseStateWire,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum AgentResponseStateWire {
    Started,
    Progress { done: u64, total: u64 },
    Done { payload: serde_json::Value },
    Failed { error: String },
}

pub fn encode_agent_request(request: &AgentRequestWire) -> Vec<u8> {
    serde_json::to_vec(request).expect("agent request is serializable")
}

pub fn decode_agent_request(payload: &[u8]) -> Result<AgentRequestWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_response(response: &AgentResponseWire) -> Vec<u8> {
    serde_json::to_vec(response).expect("agent response is serializable")
}

pub fn encode_agent_op(request: &AgentOpRequestWire) -> Vec<u8> {
    serde_json::to_vec(request).expect("agent op is serializable")
}

pub fn decode_agent_op(payload: &[u8]) -> Result<AgentOpRequestWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_steer(steer: &AgentSteerWire) -> Vec<u8> {
    serde_json::to_vec(steer).expect("agent steer is serializable")
}

pub fn decode_agent_steer(payload: &[u8]) -> Result<AgentSteerWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_analyse(analyse: &AgentAnalyseWire) -> Vec<u8> {
    serde_json::to_vec(analyse).expect("agent analyse is serializable")
}

pub fn decode_agent_analyse(payload: &[u8]) -> Result<AgentAnalyseWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_names(names: &AgentNamesWire) -> Vec<u8> {
    serde_json::to_vec(names).expect("agent names are serializable")
}

pub fn decode_agent_names(payload: &[u8]) -> Result<AgentNamesWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_task_details(details: &AgentTaskDetailsWire) -> Vec<u8> {
    serde_json::to_vec(details).expect("agent task details are serializable")
}

pub fn decode_agent_task_details(payload: &[u8]) -> Result<AgentTaskDetailsWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}

pub fn encode_agent_snapshot(snapshot: &AgentSnapshotWire) -> Vec<u8> {
    serde_json::to_vec(snapshot).expect("agent snapshot is serializable")
}
pub fn decode_agent_snapshot(payload: &[u8]) -> Result<AgentSnapshotWire, String> {
    serde_json::from_slice(payload).map_err(|error| error.to_string())
}
