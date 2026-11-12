use mediaa2_core::assistant::Event;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionTaskStatus {
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
pub enum TranscriptionResource {
    Speech,
    Embed,
    Llm,
    Provider,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionWaitReason {
    Dependency,
    Resource(TranscriptionResource),
    Memory,
    Paused,
    Turn,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptionNote {
    PausedBy {
        title: String,
    },
    FailedBy {
        title: String,
    },
    Hang {
        resource: TranscriptionResource,
        idle_s: u64,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionStats {
    pub model_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub model_ms: u64,
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionTaskView {
    pub id: i64,
    pub parent: Option<i64>,
    pub kind: String,
    pub title: String,
    pub scope: Option<String>,
    pub status: TranscriptionTaskStatus,
    pub progress: f32,
    pub waiting: Option<TranscriptionWaitReason>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub elapsed_ms: i64,
    pub estimate_ms: Option<i64>,
    pub error: Option<String>,
    pub note: Option<TranscriptionNote>,
    pub stats: TranscriptionStats,
    pub units: f64,
    pub resource: Option<TranscriptionResource>,
    pub model: Option<String>,
    pub role: String,
    pub inline: bool,
    pub attempts: i64,
    pub input: serde_json::Value,
    pub cursor: serde_json::Value,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionGroupLoad {
    pub name: String,
    pub busy: u32,
    pub limit: u32,
    pub waiting: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionToolLoad {
    pub busy: u32,
    pub limit: u32,
    pub waiting: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionSlots {
    pub groups: Vec<TranscriptionGroupLoad>,
    pub web: TranscriptionToolLoad,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionKindStat {
    pub kind: String,
    pub runs: i64,
    pub ms: i64,
    pub units: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionScope {
    pub name: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionSnapshot {
    pub version: u64,
    pub now_ms: i64,
    pub tasks: Vec<TranscriptionTaskView>,
    pub kind_stats: Vec<TranscriptionKindStat>,
    pub scopes: Vec<TranscriptionScope>,
    pub slots: TranscriptionSlots,
}

pub struct TranscriptionEvent {
    pub task: i64,
    pub event: Event,
}

/// A failed model call, explained: `kind` is a stable tag from
/// `model_errors::model_error_kind`, `detail` keeps the raw provider text.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptionChatError {
    pub kind: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TranscriptionState {
    pub snapshot: Option<TranscriptionSnapshot>,
    pub running: bool,
    pub task: Option<String>,
    pub status: Option<String>,
    pub progress: Option<u32>,
    pub streaming_text: Option<String>,
    pub streaming_task: Option<i64>,
    pub chat_error: Option<TranscriptionChatError>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TranscriptionOk {
    Started {
        task: i64,
    },
    Chat {
        chat: i64,
    },
    Models {
        models: BTreeMap<String, Vec<String>>,
    },
    Done,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TranscriptionCommand;
    use mediaa2_core::agent::TranscriptionOptions;

    #[test]
    fn snapshot_survives_json() {
        let state = TranscriptionState {
            snapshot: Some(TranscriptionSnapshot {
                version: 7,
                now_ms: 1000,
                tasks: vec![TranscriptionTaskView {
                    id: 3,
                    parent: None,
                    kind: "transcribe".into(),
                    title: "clip.mp4".into(),
                    scope: Some("scope".into()),
                    status: TranscriptionTaskStatus::Running,
                    progress: 0.4,
                    waiting: Some(TranscriptionWaitReason::Resource(
                        TranscriptionResource::Speech,
                    )),
                    created_ms: 1,
                    updated_ms: 2,
                    started_ms: Some(1),
                    finished_ms: None,
                    elapsed_ms: 10,
                    estimate_ms: Some(20),
                    error: None,
                    note: Some(TranscriptionNote::Hang {
                        resource: TranscriptionResource::Llm,
                        idle_s: 3,
                    }),
                    stats: TranscriptionStats::default(),
                    units: 1.5,
                    resource: Some(TranscriptionResource::Speech),
                    model: Some("gigaam".into()),
                    role: "speech_recognition".into(),
                    inline: false,
                    attempts: 1,
                    input: serde_json::json!({}),
                    cursor: serde_json::json!(null),
                }],
                kind_stats: vec![TranscriptionKindStat {
                    kind: "transcribe".into(),
                    runs: 2,
                    ms: 30,
                    units: 4.0,
                }],
                scopes: vec![TranscriptionScope {
                    name: "clip…".into(),
                    key: "scope".into(),
                }],
                slots: TranscriptionSlots::default(),
            }),
            running: true,
            task: Some("clip.mp4".into()),
            status: Some("transcribe".into()),
            progress: Some(40),
            streaming_text: None,
            streaming_task: None,
            chat_error: Some(TranscriptionChatError {
                kind: "rate_limited".into(),
                detail: "too many requests".into(),
            }),
        };
        let text = serde_json::to_string(&state).unwrap();
        assert_eq!(
            serde_json::from_str::<TranscriptionState>(&text).unwrap(),
            state
        );
    }

    #[test]
    fn commands_survive_json() {
        for command in [
            TranscriptionCommand::Start {
                path: "clip.mp4".into(),
                options: TranscriptionOptions::default(),
                from: None,
            },
            TranscriptionCommand::Send {
                store: "id".into(),
                chat: Some(2),
                question: "?".into(),
                locale: "ru".into(),
                path: None,
                files: vec![],
                duration: None,
                position: None,
                stored: false,
                switch_model: false,
            },
            TranscriptionCommand::Pause,
        ] {
            let message = crate::ClientMessage::Command {
                id: 5,
                command: crate::ClientCommand::Transcription(command),
            };
            let text = serde_json::to_string(&message).unwrap();
            assert_eq!(
                serde_json::from_str::<crate::ClientMessage>(&text).unwrap(),
                message
            );
        }
    }
}
