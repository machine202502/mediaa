use mediaa_core::slots;
use mediaa_protocol::backend::{
    AgentNoteWire, AgentResourceWire, AgentSnapshotWire, AgentStatsWire, AgentTaskStatusWire,
    AgentTaskViewWire, AgentWaitReasonWire,
};
use rust_i18n::t;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

pub type TaskId = i64;

pub const ANSWER_KIND: &str = "chat-answer";

pub fn running_answer<'a>(tasks: &'a [Task], store: &str, chat: i64) -> Option<&'a Task> {
    tasks
        .iter()
        .filter(|task| {
            task.kind == ANSWER_KIND
                && task.scope.as_deref() == Some(store)
                && task.input.get("chat").and_then(Value::as_i64) == Some(chat)
                && !task.status.is_finished()
        })
        .max_by_key(|task| task.id)
}

pub fn answer_busy(task: &Task) -> bool {
    matches!(task.status, Status::Running | Status::Stopping)
}

pub fn phrases(tasks: &[Task], answer: i64) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for task in tasks {
        if task.kind != "step-title" || task.status != Status::Done {
            continue;
        }
        if task.input.get("answer").and_then(Value::as_i64) != Some(answer) {
            continue;
        }
        let Some(phrase) = task
            .result
            .as_ref()
            .and_then(|result| result.get("phrase"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(index) = task.input.get("index").and_then(Value::as_u64) else {
            continue;
        };
        out.push((index as usize, phrase.to_string()));
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindGroup {
    #[default]
    All,
    Analysis,
    Chat,
}

impl KindGroup {
    pub fn matches(self, kind: &str) -> bool {
        match self {
            KindGroup::All => true,
            KindGroup::Analysis => ANALYSIS_KINDS.contains(&kind),
            KindGroup::Chat => CHAT_KINDS.contains(&kind),
        }
    }
}

const ANALYSIS_KINDS: &[&str] = &[
    "analysis",
    "transcribe",
    "diarize",
    "index",
    "topics",
    "topics-plan",
    "topic-digest",
    "topics-merge",
    "details",
    "topic-detail",
];
const CHAT_KINDS: &[&str] = &[
    "chat",
    "chat-answer",
    "chat-title",
    "step-title",
    "answer",
    "model-step",
    "tool",
];

#[derive(Debug, Clone)]
pub struct Filter {
    pub scope: Option<String>,
    pub group: KindGroup,
}

impl Default for Filter {
    fn default() -> Filter {
        Filter {
            scope: None,
            group: KindGroup::All,
        }
    }
}

impl Filter {
    pub fn matches(&self, task: &Task) -> bool {
        self.scope
            .as_ref()
            .is_none_or(|scope| task.scope.as_deref() == Some(scope.as_str()))
            && self.group.matches(&task.kind)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Stopping,
    Paused,
    Waiting,
    Done,
    Failed,
    Cancelled,
}

impl Status {
    pub fn is_finished(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }

    #[allow(dead_code, reason = "used by the queue tests")]
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Stopping => "stopping",
            Status::Paused => "paused",
            Status::Waiting => "waiting",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Speech,
    Embed,
    Llm,
    Provider,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitReason {
    Dependency,
    Resource(Resource),
    Memory,
    Paused,
    Turn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Note {
    PausedBy { title: String },
    FailedBy { title: String },
    Hang { resource: Resource, idle_s: u64 },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stats {
    pub model_calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub model_ms: u64,
    pub models: Vec<String>,
}

impl Stats {
    pub fn add(&mut self, delta: &Stats) {
        self.model_calls += delta.model_calls;
        self.prompt_tokens += delta.prompt_tokens;
        self.completion_tokens += delta.completion_tokens;
        self.model_ms += delta.model_ms;
        for model in &delta.models {
            if !self.models.iter().any(|item| item == model) {
                self.models.push(model.clone());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KindStat {
    pub kind: String,
    pub runs: i64,
    pub ms: i64,
    pub units: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: TaskId,
    pub parent: Option<TaskId>,
    pub kind: String,
    pub title: String,
    pub scope: Option<String>,
    pub status: Status,
    pub progress: f32,
    pub waiting: Option<WaitReason>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub elapsed_ms: i64,
    pub estimate_ms: Option<i64>,
    pub error: Option<String>,
    pub note: Option<Note>,
    pub stats: Stats,
    pub units: f64,
    pub resource: Option<Resource>,
    pub model: Option<String>,
    pub role: String,
    pub inline: bool,
    pub attempts: i64,
    pub input: Value,
    pub cursor: Value,
    pub result: Option<Value>,
    pub detail: Value,
}

impl Task {
    pub fn from_wire(wire: &AgentTaskViewWire) -> Self {
        Self {
            id: wire.id,
            parent: wire.parent,
            kind: wire.kind.clone(),
            title: wire.title.clone(),
            scope: wire.scope.clone(),
            status: status(wire.status),
            progress: wire.progress,
            waiting: wire.waiting.as_ref().map(wait),
            created_ms: wire.created_ms,
            updated_ms: wire.updated_ms,
            started_ms: wire.started_ms,
            finished_ms: wire.finished_ms,
            elapsed_ms: wire.elapsed_ms,
            estimate_ms: wire.estimate_ms,
            error: wire.error.clone(),
            note: wire.note.as_ref().map(note),
            stats: stats(&wire.stats),
            units: wire.units,
            resource: wire.resource.map(resource_from_wire),
            model: wire.model.clone(),
            role: wire.role.clone(),
            inline: wire.inline,
            attempts: wire.attempts,
            input: wire.input.clone(),
            cursor: wire.cursor.clone(),
            result: None,
            detail: Value::Null,
        }
    }
}

pub fn tasks(snapshot: &AgentSnapshotWire) -> Vec<Task> {
    snapshot.tasks.iter().map(Task::from_wire).collect()
}

pub fn kind_stats(snapshot: &AgentSnapshotWire) -> Vec<KindStat> {
    snapshot
        .kind_stats
        .iter()
        .map(|stat| KindStat {
            kind: stat.kind.clone(),
            runs: stat.runs,
            ms: stat.ms,
            units: stat.units,
        })
        .collect()
}

pub fn slots(snapshot: &AgentSnapshotWire) -> slots::Status {
    snapshot.slots.clone()
}

pub fn elapsed(task: &Task, now_ms: i64) -> Duration {
    let started = task.started_ms.unwrap_or(now_ms);
    Duration::from_millis((now_ms - started).max(0) as u64)
}

pub fn estimate_tree(
    task: &Task,
    _tasks: &[Task],
    _now_ms: i64,
    _stats: &[KindStat],
) -> Option<Duration> {
    task.estimate_ms
        .map(|ms| Duration::from_millis(ms.max(0) as u64))
}

#[allow(dead_code, reason = "used by the queue tests")]
pub fn pause_note(title: &str) -> Note {
    Note::PausedBy {
        title: title.to_string(),
    }
}

#[allow(dead_code, reason = "used by the queue tests")]
pub fn failure_note(title: &str) -> Note {
    Note::FailedBy {
        title: title.to_string(),
    }
}

#[allow(dead_code, reason = "used by the queue tests")]
pub fn hang_note(resource: Resource, idle: Duration) -> Note {
    Note::Hang {
        resource,
        idle_s: idle.as_secs(),
    }
}

pub fn note_text(note: Option<&Note>, locale: &str) -> String {
    match note {
        Some(Note::PausedBy { title }) => t!(
            "queue.note_paused_by",
            title = title.as_str(),
            locale = locale
        )
        .to_string(),
        Some(Note::FailedBy { title }) => t!(
            "queue.note_failed_by",
            title = title.as_str(),
            locale = locale
        )
        .to_string(),
        Some(Note::Hang { resource, idle_s }) => hang_text(*resource, *idle_s, locale),
        None => String::new(),
    }
}

pub fn hang_text(resource: Resource, idle_s: u64, locale: &str) -> String {
    let what = match resource {
        Resource::Speech => t!("queue.hang_speech", locale = locale).to_string(),
        Resource::Embed => t!("queue.hang_embed", locale = locale).to_string(),
        Resource::Llm => t!("queue.hang_llm", locale = locale).to_string(),
        Resource::Provider => t!("queue.hang_provider", locale = locale).to_string(),
    };
    let idle = if idle_s >= 120 {
        t!("queue.hang_minutes", n = idle_s / 60, locale = locale).to_string()
    } else {
        t!("queue.hang_seconds", n = idle_s, locale = locale).to_string()
    };
    t!(
        "queue.hang_note",
        what = what.as_str(),
        idle = idle.as_str(),
        locale = locale
    )
    .to_string()
}

pub fn tool_text(detail: &Value) -> Option<String> {
    let tool = detail.get("tool")?.as_str()?;
    let args = detail
        .get("args")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut text = if args.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} {args}")
    };
    if let Some(chars) = detail.get("result_chars").and_then(Value::as_u64) {
        text = format!("{text} · {}", t!("queue.tool_result", n = chars));
    }
    Some(text)
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchChunk {
    pub start: f64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub id: i64,
    pub chunk: SearchChunk,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchFileHit {
    pub path: std::path::PathBuf,
    pub hit: SearchHit,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SearchTrace {
    pub files: usize,
    pub not_indexed: usize,
}

pub fn search_found(payload: &Value) -> Result<(Vec<SearchFileHit>, SearchTrace), String> {
    let hits = payload
        .get("hits")
        .and_then(Value::as_array)
        .ok_or_else(|| "search payload has no hits".to_string())?
        .iter()
        .map(|hit| {
            let chunk = &hit["hit"]["chunk"];
            Ok(SearchFileHit {
                path: std::path::PathBuf::from(
                    hit.get("path")
                        .and_then(Value::as_str)
                        .ok_or_else(|| "search hit has no path".to_string())?,
                ),
                hit: SearchHit {
                    id: hit["hit"]["id"].as_i64().unwrap_or_default(),
                    chunk: SearchChunk {
                        start: chunk["start"].as_f64().unwrap_or_default(),
                        text: chunk["text"].as_str().unwrap_or_default().to_string(),
                    },
                },
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let trace = SearchTrace {
        files: payload["trace"]["files"].as_u64().unwrap_or(0) as usize,
        not_indexed: payload["trace"]["not_indexed"].as_u64().unwrap_or(0) as usize,
    };
    Ok((hits, trace))
}

pub fn quote_token(label: &str, start: &str, end: &str, text: &str) -> String {
    let body =
        serde_json::json!({ "label": label, "start": start, "end": end, "text": text }).to_string();
    format!("\u{E000}{body}\u{E001}")
}

pub fn parse_time(text: &str) -> Option<f64> {
    let t = text
        .trim()
        .trim_matches(|c| matches!(c, '[' | ']' | '@' | ' '))
        .replace(',', ".");
    if t.is_empty() {
        return None;
    }
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let v: f64 = part
            .trim()
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        if i > 0 && v >= 60.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

#[allow(dead_code, reason = "used by the composer")]
pub fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0).floor() as u64;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn status(status: AgentTaskStatusWire) -> Status {
    match status {
        AgentTaskStatusWire::Queued => Status::Queued,
        AgentTaskStatusWire::Running => Status::Running,
        AgentTaskStatusWire::Stopping => Status::Stopping,
        AgentTaskStatusWire::Paused => Status::Paused,
        AgentTaskStatusWire::Waiting => Status::Waiting,
        AgentTaskStatusWire::Done => Status::Done,
        AgentTaskStatusWire::Failed => Status::Failed,
        AgentTaskStatusWire::Cancelled => Status::Cancelled,
    }
}

fn resource_from_wire(resource: AgentResourceWire) -> Resource {
    match resource {
        AgentResourceWire::Speech => Resource::Speech,
        AgentResourceWire::Embed => Resource::Embed,
        AgentResourceWire::Llm => Resource::Llm,
        AgentResourceWire::Provider => Resource::Provider,
    }
}

fn wait(wait: &AgentWaitReasonWire) -> WaitReason {
    match wait {
        AgentWaitReasonWire::Dependency => WaitReason::Dependency,
        AgentWaitReasonWire::Resource(resource) => {
            WaitReason::Resource(resource_from_wire(*resource))
        }
        AgentWaitReasonWire::Memory => WaitReason::Memory,
        AgentWaitReasonWire::Paused => WaitReason::Paused,
        AgentWaitReasonWire::Turn => WaitReason::Turn,
    }
}

fn note(note: &AgentNoteWire) -> Note {
    match note {
        AgentNoteWire::PausedBy { title } => Note::PausedBy {
            title: title.clone(),
        },
        AgentNoteWire::FailedBy { title } => Note::FailedBy {
            title: title.clone(),
        },
        AgentNoteWire::Hang { resource, idle_s } => Note::Hang {
            resource: resource_from_wire(*resource),
            idle_s: *idle_s,
        },
    }
}

fn stats(stats: &AgentStatsWire) -> Stats {
    Stats {
        model_calls: stats.model_calls,
        prompt_tokens: stats.prompt_tokens,
        completion_tokens: stats.completion_tokens,
        model_ms: stats.model_ms,
        models: stats.models.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_filter_separates_analysis_from_chat() {
        assert!(KindGroup::Analysis.matches("transcribe"));
        assert!(KindGroup::Analysis.matches("topic-detail"));
        assert!(!KindGroup::Analysis.matches("chat-answer"));
        assert!(KindGroup::Chat.matches("chat-answer"));
        assert!(!KindGroup::Chat.matches("analysis"));
        assert!(KindGroup::All.matches("anything"));
    }

    #[test]
    fn timecodes_roundtrip_without_losing_the_seconds() {
        let one = format_time(100.0);
        assert_eq!(parse_time(&one), Some(100.0));
        let two = format_time(3725.0);
        assert_eq!(parse_time(&two), Some(3725.0));
        assert_eq!(parse_time("no time"), None);
    }

    #[test]
    fn a_search_payload_becomes_file_hits_and_a_trace() {
        let payload = serde_json::json!({
            "hits": [{
                "path": "C:/media/a.mp4",
                "hit": {
                    "id": 1,
                    "chunk": { "start": 12.5, "text": "hello" }
                }
            }],
            "trace": { "files": 1, "not_indexed": 0 }
        });
        let (files, trace) = search_found(&payload).expect("search payload");
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].hit.chunk.text, "hello");
        assert_eq!(files[0].hit.chunk.start, 12.5);
        assert_eq!(trace.files, 1);
        assert_eq!(trace.not_indexed, 0);
    }
}

#[cfg(test)]
mod wire_tests {
    use super::*;
    use mediaa_protocol::backend::{
        AgentNoteWire, AgentResourceWire, AgentStatsWire, AgentTaskStatusWire, AgentTaskViewWire,
        AgentWaitReasonWire,
    };

    fn task_wire() -> AgentTaskViewWire {
        AgentTaskViewWire {
            id: 7,
            parent: Some(3),
            kind: "analysis".into(),
            title: "Тема".into(),
            scope: Some("store".into()),
            status: AgentTaskStatusWire::Running,
            progress: 0.5,
            waiting: Some(AgentWaitReasonWire::Resource(AgentResourceWire::Speech)),
            created_ms: 10,
            updated_ms: 20,
            started_ms: Some(11),
            finished_ms: None,
            elapsed_ms: 9,
            estimate_ms: Some(30),
            error: None,
            note: Some(AgentNoteWire::PausedBy {
                title: "Другая задача".into(),
            }),
            stats: AgentStatsWire {
                model_calls: 2,
                prompt_tokens: 40,
                completion_tokens: 8,
                model_ms: 70,
                models: vec!["m".into()],
            },
            units: 1.5,
            resource: Some(AgentResourceWire::Speech),
            model: Some("m".into()),
            role: "worker".into(),
            inline: false,
            attempts: 1,
            input: serde_json::json!({ "store": "store" }),
            cursor: serde_json::json!({}),
        }
    }

    #[test]
    fn a_task_wire_becomes_a_view_task() {
        let task = Task::from_wire(&task_wire());
        assert_eq!(task.id, 7);
        assert_eq!(task.parent, Some(3));
        assert_eq!(task.status, Status::Running);
        assert_eq!(task.resource, Some(Resource::Speech));
        assert_eq!(task.waiting, Some(WaitReason::Resource(Resource::Speech)));
        assert_eq!(
            task.note,
            Some(Note::PausedBy {
                title: "Другая задача".into()
            })
        );
        assert_eq!(task.stats.model_calls, 2);
        assert_eq!(task.stats.models, vec!["m".to_string()]);
        assert_eq!(elapsed(&task, 25), Duration::from_millis(14));
    }

    #[test]
    fn the_filter_checks_scope_and_kind_group() {
        let task = Task::from_wire(&task_wire());
        let mut filter = Filter::default();
        assert!(filter.matches(&task));
        filter.scope = Some("store".into());
        assert!(filter.matches(&task));
        filter.scope = Some("other".into());
        assert!(!filter.matches(&task));
        filter.scope = None;
        filter.group = KindGroup::Chat;
        assert!(!filter.matches(&task));
    }
}
