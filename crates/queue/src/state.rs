//! Statuses, the transition table, parent status (rule 13.7) and dependencies.
//! Every store applies an [`Op`] to a [`World`] and writes the result back in one transaction.

use crate::events::{Event, EventKind};
use crate::{Error, Result, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Status {
    Queued,
    Running,
    /// Stop was asked. The run has not returned, so the task cannot be taken again.
    Stopping,
    Paused,
    /// A composite no child has started. Never claimed.
    Waiting,
    Done,
    Failed,
    Cancelled,
}

impl Status {
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

    pub fn parse(s: &str) -> Result<Status> {
        Ok(match s {
            "queued" => Status::Queued,
            "running" => Status::Running,
            "stopping" => Status::Stopping,
            "paused" => Status::Paused,
            "waiting" => Status::Waiting,
            "done" => Status::Done,
            "failed" => Status::Failed,
            "cancelled" => Status::Cancelled,
            other => return Err(Error::Bad(format!("unknown status {other}"))),
        })
    }

    pub fn is_finished(self) -> bool {
        matches!(self, Status::Done | Status::Failed | Status::Cancelled)
    }
}

/// Why a run was asked to stop. The stored status stays `stopping` until the run returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopIntent {
    Pause,
    Cancel,
}

impl StopIntent {
    pub fn as_str(self) -> &'static str {
        match self {
            StopIntent::Pause => "pause",
            StopIntent::Cancel => "cancel",
        }
    }

    pub fn parse(s: &str) -> Result<StopIntent> {
        match s {
            "pause" => Ok(StopIntent::Pause),
            "cancel" => Ok(StopIntent::Cancel),
            other => Err(Error::Bad(format!("unknown stop {other}"))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resource {
    Speech,
    Embed,
    Llm,
    Provider,
}

impl Resource {
    pub fn as_str(self) -> &'static str {
        match self {
            Resource::Speech => "speech",
            Resource::Embed => "embed",
            Resource::Llm => "llm",
            Resource::Provider => "provider",
        }
    }

    pub fn parse(s: &str) -> Result<Resource> {
        Ok(match s {
            "speech" => Resource::Speech,
            "embed" => Resource::Embed,
            "llm" => Resource::Llm,
            "provider" => Resource::Provider,
            other => return Err(Error::Bad(format!("unknown resource {other}"))),
        })
    }
}

/// Who a task shares a turn with. A chat answer is one participant; a recording's analysis is one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Role {
    Chat(String),
    Analysis(String),
    Work,
}

impl Role {
    pub fn code(&self) -> String {
        match self {
            Role::Chat(k) => format!("c:{k}"),
            Role::Analysis(k) => format!("a:{k}"),
            Role::Work => String::new(),
        }
    }

    pub fn parse(s: &str) -> Role {
        if let Some(k) = s.strip_prefix("c:") {
            Role::Chat(k.to_string())
        } else if let Some(k) = s.strip_prefix("a:") {
            Role::Analysis(k.to_string())
        } else {
            Role::Work
        }
    }

    pub fn is_chat(&self) -> bool {
        matches!(self, Role::Chat(_))
    }
}

/// The round-robin key. Chats are per answer; analysis is per recording.
pub fn participant(task: &Task) -> String {
    match &task.role {
        Role::Chat(k) => format!("c:{k}"),
        Role::Analysis(k) => format!("a:{k}"),
        Role::Work => format!("w:{}", task.id),
    }
}

/// Status the rules use. A stopping run already counts as paused or cancelled.
pub fn effective_status(task: &Task) -> Status {
    match (task.status, task.stop_intent) {
        (Status::Stopping, Some(StopIntent::Cancel)) => Status::Cancelled,
        (Status::Stopping, _) => Status::Paused,
        _ => task.status,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    Going,
    Paused,
    Waiting,
    Done,
    Failed,
    Cancelled,
}

impl ChildState {
    pub fn of(status: Status, blocked: bool) -> ChildState {
        match status {
            Status::Running => ChildState::Going,
            Status::Queued if !blocked => ChildState::Going,
            Status::Queued | Status::Waiting | Status::Stopping => ChildState::Waiting,
            Status::Paused => ChildState::Paused,
            Status::Done => ChildState::Done,
            Status::Failed => ChildState::Failed,
            Status::Cancelled => ChildState::Cancelled,
        }
    }
}

/// Going, else paused, else waiting, else done when every child is done, else failed, else cancelled.
pub fn composite_status(children: &[ChildState]) -> Option<Status> {
    if children.is_empty() {
        return None;
    }
    if children.contains(&ChildState::Going) {
        return Some(Status::Running);
    }
    if children.contains(&ChildState::Paused) {
        return Some(Status::Paused);
    }
    if children.contains(&ChildState::Waiting) {
        return Some(Status::Waiting);
    }
    if children.iter().all(|state| *state == ChildState::Done) {
        return Some(Status::Done);
    }
    if children.contains(&ChildState::Failed) {
        return Some(Status::Failed);
    }
    Some(Status::Cancelled)
}

pub fn composite_error(children: &[ChildState], errors: &[Option<&str>]) -> Option<String> {
    match composite_status(children)? {
        Status::Failed => Some(
            children
                .iter()
                .zip(errors)
                .find(|(state, _)| **state == ChildState::Failed)
                .and_then(|(_, error)| *error)
                .filter(|error| !error.is_empty())
                .unwrap_or("failed")
                .to_string(),
        ),
        _ => None,
    }
}

/// The note a cancelled composite shows: that of the first child cancelled because another task
/// failed. A composite failed on its own carries the error, not a note.
fn composite_note(children: &[ChildState], notes: &[Option<&Note>]) -> Option<Note> {
    if composite_status(children)? != Status::Cancelled {
        return None;
    }
    children
        .iter()
        .zip(notes)
        .find(|(state, note)| **state == ChildState::Cancelled && is_failure_note(**note))
        .and_then(|(_, note)| (*note).cloned())
}

/// Why a task waits or what interrupted it, kept as data: the app turns it into words in the
/// user's language. The queue itself never writes a sentence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Note {
    /// A dependency of this task is paused.
    PausedBy { title: String },
    /// This task was cancelled because that task failed.
    FailedBy { title: String },
    /// A live run stopped answering: the task went back to the queue at its cursor.
    Hang { resource: Resource, idle_s: u64 },
}

pub fn failure_note(title: &str) -> Note {
    Note::FailedBy {
        title: title.to_string(),
    }
}

pub fn pause_note(title: &str) -> Note {
    Note::PausedBy {
        title: title.to_string(),
    }
}

pub fn hang_note(resource: Resource, idle: Duration) -> Note {
    Note::Hang {
        resource,
        idle_s: idle.as_secs(),
    }
}

fn is_failure_note(note: Option<&Note>) -> bool {
    matches!(note, Some(Note::FailedBy { .. }))
}

fn is_pause_note(note: Option<&Note>) -> bool {
    matches!(note, Some(Note::PausedBy { .. }))
}

/// Why a reaped run stopped (`Task::detail["hang"]`), kept until the task finishes.
fn set_hang(task: &mut Task, note: Option<Note>) {
    let Some(note) = note else { return };
    if !task.detail.is_object() {
        task.detail = Value::Object(serde_json::Map::new());
    }
    if let Some(map) = task.detail.as_object_mut() {
        map.insert(
            "hang".to_string(),
            serde_json::to_value(note).unwrap_or_default(),
        );
    }
}

fn clear_hang(task: &mut Task) {
    if let Some(map) = task.detail.as_object_mut() {
        map.remove("hang");
        if map.is_empty() {
            task.detail = Value::Null;
        }
    }
}

/// The run is gone (it returned, or the process died). A failure note stays; a plain cancel does not.
fn settle_stop(task: &mut Task, cancel: bool, now: i64) {
    let failure = task.note.clone().filter(|note| is_failure_note(Some(note)));
    task.status = if cancel {
        Status::Cancelled
    } else {
        Status::Paused
    };
    task.note = if cancel { failure } else { None };
    task.error = None;
    task.finished_ms = if cancel { Some(now) } else { None };
    task.stop_intent = None;
    task.heartbeat_ms = None;
    task.gen += 1;
    task.updated_ms = now;
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    #[serde(default)]
    pub model_calls: u64,
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub model_ms: u64,
    #[serde(default)]
    pub models: Vec<String>,
}

impl Stats {
    pub fn add(&mut self, delta: &Stats) {
        self.model_calls += delta.model_calls;
        self.prompt_tokens += delta.prompt_tokens;
        self.completion_tokens += delta.completion_tokens;
        self.model_ms += delta.model_ms;
        for model in &delta.models {
            if !self.models.iter().any(|m| m == model) {
                self.models.push(model.clone());
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindStat {
    pub kind: String,
    pub runs: i64,
    pub ms: i64,
    pub units: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: TaskId,
    pub kind: String,
    pub title: String,
    pub input: Value,
    pub priority: i64,
    pub status: Status,
    pub cursor: Value,
    pub partial: Value,
    pub result: Option<Value>,
    pub progress: f32,
    pub error: Option<String>,
    /// Why it waits or why it was cancelled. Separate from `error`, which is a real failure.
    pub note: Option<Note>,
    pub attempts: i64,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub parent: Option<TaskId>,
    pub scope: Option<String>,
    pub inline: bool,
    pub started_ms: Option<i64>,
    pub finished_ms: Option<i64>,
    pub stats: Stats,
    pub detail: Value,
    pub units: f64,
    pub resource: Option<Resource>,
    pub memory: u64,
    pub model: Option<String>,
    pub role: Role,
    /// Bumped when a run is invalidated (crash, stale heartbeat). Writes carry the run's gen.
    pub gen: i64,
    /// The next claim continues the yielded chunk and does not count a new attempt.
    pub yield_cont: bool,
    pub heartbeat_ms: Option<i64>,
    pub stop_intent: Option<StopIntent>,
}

impl Task {
    pub fn shown_name(&self) -> String {
        let title = self.title.trim();
        if title.is_empty() {
            self.kind.clone()
        } else {
            title.to_string()
        }
    }
}

#[derive(Debug, Clone)]
pub struct NewTask {
    pub kind: String,
    pub title: String,
    pub input: Value,
    pub priority: i64,
    pub parent: Option<TaskId>,
    pub scope: Option<String>,
    pub needs: Vec<TaskId>,
    pub units: f64,
    pub inline: bool,
    pub composite: bool,
    pub cursor: Option<Value>,
    pub resource: Option<Resource>,
    pub memory: u64,
    pub model: Option<String>,
    pub role: Role,
}

impl NewTask {
    pub fn new(kind: &str, title: &str, input: &Value) -> NewTask {
        NewTask {
            kind: kind.to_string(),
            title: title.to_string(),
            input: input.clone(),
            priority: 0,
            parent: None,
            scope: None,
            needs: Vec::new(),
            units: 0.0,
            inline: false,
            composite: false,
            cursor: None,
            resource: None,
            memory: 0,
            model: None,
            role: Role::Work,
        }
    }

    pub fn priority(mut self, priority: i64) -> NewTask {
        self.priority = priority;
        self
    }
    pub fn parent(mut self, parent: TaskId) -> NewTask {
        self.parent = Some(parent);
        self
    }
    pub fn scope(mut self, scope: &str) -> NewTask {
        self.scope = Some(scope.to_string());
        self
    }
    pub fn needs(mut self, needs: &[TaskId]) -> NewTask {
        self.needs = needs.to_vec();
        self
    }
    pub fn units(mut self, units: f64) -> NewTask {
        self.units = units;
        self
    }
    pub fn inline(mut self) -> NewTask {
        self.inline = true;
        self
    }
    pub fn composite(mut self) -> NewTask {
        self.composite = true;
        self.resource = None;
        self
    }
    pub fn cursor(mut self, cursor: &Value) -> NewTask {
        self.cursor = Some(cursor.clone());
        self
    }
    pub fn resource(mut self, resource: Resource) -> NewTask {
        self.resource = Some(resource);
        self
    }
    pub fn memory(mut self, memory: u64) -> NewTask {
        self.memory = memory;
        self
    }
    pub fn model(mut self, model: &str) -> NewTask {
        self.model = Some(model.to_string());
        self
    }
    pub fn chat(mut self, key: &str) -> NewTask {
        self.role = Role::Chat(key.to_string());
        self
    }
    pub fn analysis(mut self, key: &str) -> NewTask {
        self.role = Role::Analysis(key.to_string());
        self
    }
}

#[derive(Debug, Clone)]
pub struct Child {
    pub task: NewTask,
    pub after: Vec<usize>,
    pub children: Vec<Child>,
}

impl Child {
    pub fn new(task: NewTask) -> Child {
        Child {
            task,
            after: Vec::new(),
            children: Vec::new(),
        }
    }
    pub fn after(mut self, siblings: &[usize]) -> Child {
        self.after = siblings.to_vec();
        self
    }
    pub fn with_children(mut self, children: Vec<Child>) -> Child {
        self.children = children;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindGroup {
    #[default]
    All,
    Analysis,
    Chat,
}

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
    pub fn scope(scope: &str) -> Filter {
        Filter {
            scope: Some(scope.to_string()),
            group: KindGroup::All,
        }
    }
    pub fn group(group: KindGroup) -> Filter {
        Filter { scope: None, group }
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
pub struct World {
    pub tasks: Vec<Task>,
    pub deps: Vec<(TaskId, TaskId)>,
    pub kind_stats: Vec<KindStat>,
    pub next_id: i64,
}

impl Default for World {
    fn default() -> World {
        World {
            tasks: Vec::new(),
            deps: Vec::new(),
            kind_stats: Vec::new(),
            next_id: 1,
        }
    }
}

impl World {
    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }
    pub fn task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }
    pub fn child_ids(&self, id: TaskId) -> Vec<TaskId> {
        self.tasks
            .iter()
            .filter(|t| t.parent == Some(id))
            .map(|t| t.id)
            .collect()
    }
    pub fn has_children(&self, id: TaskId) -> bool {
        self.tasks.iter().any(|t| t.parent == Some(id))
    }

    /// Children that are tasks of their own. An inline step of an answer does not count:
    /// the answer stays one claimed run.
    pub fn has_owned_children(&self, id: TaskId) -> bool {
        self.tasks.iter().any(|t| t.parent == Some(id) && !t.inline)
    }
    fn needs_of(&self, id: TaskId) -> Vec<TaskId> {
        self.deps
            .iter()
            .filter(|(t, _)| *t == id)
            .map(|(_, n)| *n)
            .collect()
    }
}

/// One store operation. The store loads a world, applies this, and writes it back.
#[derive(Debug, Clone)]
pub enum Op {
    Enqueue(NewTask),
    EnqueueTree {
        root: NewTask,
        children: Vec<Child>,
    },
    EnqueueOnce(NewTask),
    Claim(TaskId),
    Checkpoint {
        id: TaskId,
        gen: i64,
        cursor: Value,
        partial: Value,
        progress: f32,
    },
    Progress {
        id: TaskId,
        gen: i64,
        progress: f32,
    },
    Complete {
        id: TaskId,
        gen: i64,
        result: Value,
        children: Vec<Child>,
    },
    Fail {
        id: TaskId,
        gen: i64,
        error: String,
    },
    Yield {
        id: TaskId,
        gen: i64,
    },
    Pause(TaskId),
    Resume(TaskId),
    Cancel(TaskId),
    CancelScope(String),
    CancelAnalysis(String),
    Restart(TaskId),
    FinishStop {
        id: TaskId,
        gen: i64,
    },
    Heartbeat {
        id: TaskId,
        gen: i64,
    },
    Recover,
    Reap {
        older_than_ms: i64,
    },
    /// A live run showed no sign of life for too long: the task goes back to the queue at its
    /// cursor, and `note` (when given) tells the user why in the task's details.
    ReapRun {
        id: TaskId,
        gen: i64,
        note: Option<Note>,
    },
    BeginInline {
        kind: String,
        title: String,
        input: Value,
        parent: Option<TaskId>,
        scope: Option<String>,
    },
    AddStats {
        id: TaskId,
        delta: Stats,
    },
    SetDetail {
        id: TaskId,
        detail: Value,
    },
    SetPriority {
        id: TaskId,
        priority: i64,
    },
    Remove(TaskId),
    ClearDone,
    Purge {
        cutoff_ms: i64,
    },
}

#[derive(Debug, Clone)]
pub struct Outcome {
    pub events: Vec<Event>,
    pub ids: Vec<TaskId>,
    pub changed: usize,
    pub accepted: bool,
    pub status: Option<Status>,
}

impl Default for Outcome {
    fn default() -> Outcome {
        Outcome {
            events: Vec::new(),
            ids: Vec::new(),
            changed: 0,
            accepted: true,
            status: None,
        }
    }
}

/// Every change to an existing task stamps `updated_ms` with `now`; [`Op::Heartbeat`] stamps
/// `heartbeat_ms` instead. The SQLite store relies on that to find the rows an operation changed.
pub fn apply(mut world: World, op: Op, now: i64) -> Result<(World, Outcome)> {
    let before: HashMap<TaskId, Status> = world.tasks.iter().map(|t| (t.id, t.status)).collect();
    let mut out = Outcome::default();
    let mut chunk: Option<TaskId> = None;
    match op {
        Op::Enqueue(task) => {
            out.ids.push(insert_new(&mut world, task, now)?);
        }
        Op::EnqueueTree { root, children } => {
            let scope = root.scope.clone();
            let inherited = root.needs.clone();
            let id = insert_new(&mut world, root, now)?;
            let mut all = Vec::new();
            let ids = insert_children(
                &mut world,
                Some(id),
                scope.as_deref(),
                &children,
                &inherited,
                &mut all,
                now,
            )?;
            out.ids = std::iter::once(id).chain(all).collect();
            out.changed = ids.len();
        }
        Op::EnqueueOnce(task) => {
            if let Some(found) = world.tasks.iter().find(|found| {
                found.kind == task.kind
                    && found.input == task.input
                    && matches!(
                        found.status,
                        Status::Queued
                            | Status::Running
                            | Status::Waiting
                            | Status::Paused
                            | Status::Stopping
                    )
            }) {
                out.ids.push(found.id);
            } else {
                out.ids.push(insert_new(&mut world, task, now)?);
            }
        }
        Op::Claim(id) => {
            if !is_claimable(&world, id) {
                out.accepted = false;
            } else if let Some(task) = world.task_mut(id) {
                if task.yield_cont {
                    task.yield_cont = false;
                } else {
                    task.attempts += 1;
                }
                // A new gen so a runner from the previous claim cannot write into this one.
                task.gen += 1;
                task.status = Status::Running;
                task.error = None;
                task.note = None;
                task.finished_ms = None;
                task.stop_intent = None;
                if task.started_ms.is_none() {
                    task.started_ms = Some(now);
                }
                task.heartbeat_ms = Some(now);
                task.updated_ms = now;
                out.ids.push(id);
            }
        }
        Op::Checkpoint {
            id,
            gen,
            cursor,
            partial,
            progress,
        } => {
            let ok = world
                .task(id)
                .is_some_and(|t| t.gen == gen && t.status == Status::Running);
            if !ok {
                out.accepted = false;
            } else if let Some(task) = world.task_mut(id) {
                task.cursor = cursor;
                task.partial = partial;
                task.progress = progress.clamp(0.0, 1.0);
                task.heartbeat_ms = Some(now);
                task.updated_ms = now;
                chunk = Some(id);
            }
        }
        Op::Progress { id, gen, progress } => {
            let ok = world.task(id).is_some_and(|t| {
                t.gen == gen && matches!(t.status, Status::Running | Status::Stopping)
            });
            out.accepted = ok;
            if ok {
                if let Some(task) = world.task_mut(id) {
                    task.progress = progress.clamp(0.0, 1.0);
                    task.updated_ms = now;
                }
            }
        }
        Op::Complete {
            id,
            gen,
            result,
            children,
        } => {
            let Some(task) = world.task(id).cloned() else {
                return Err(Error::Bad(format!("task {id} is not in the queue")));
            };
            let live = task.gen == gen
                && matches!(
                    task.status,
                    Status::Queued | Status::Running | Status::Stopping
                );
            if !live {
                out.accepted = false;
            } else {
                let leaf = !world.has_owned_children(id);
                if let Some(task) = world.task_mut(id) {
                    task.status = Status::Done;
                    task.result = Some(result);
                    task.progress = 1.0;
                    task.error = None;
                    task.note = None;
                    task.finished_ms = Some(now);
                    task.stop_intent = None;
                    task.heartbeat_ms = None;
                    task.updated_ms = now;
                    clear_hang(task);
                }
                if leaf && task.units > 0.0 {
                    add_kind_run(&mut world, &task, now);
                }
                let default_parent = task.parent.or(Some(task.id));
                let mut all = Vec::new();
                out.ids = insert_children(
                    &mut world,
                    default_parent,
                    task.scope.as_deref(),
                    &children,
                    &[],
                    &mut all,
                    now,
                )?;
            }
        }
        Op::Fail { id, gen, error } => {
            let ok = world.task(id).is_some_and(|t| {
                (gen == 0 || t.gen == gen)
                    && matches!(
                        t.status,
                        Status::Queued
                            | Status::Running
                            | Status::Paused
                            | Status::Waiting
                            | Status::Stopping
                    )
            });
            out.accepted = ok;
            if ok {
                if let Some(task) = world.task_mut(id) {
                    task.status = Status::Failed;
                    task.error = Some(error);
                    task.note = None;
                    task.finished_ms = Some(now);
                    task.stop_intent = None;
                    task.heartbeat_ms = None;
                    task.updated_ms = now;
                }
            }
        }
        Op::Yield { id, gen } => {
            let ok = world
                .task(id)
                .is_some_and(|t| t.gen == gen && t.status == Status::Running);
            out.accepted = ok;
            if ok {
                if let Some(task) = world.task_mut(id) {
                    task.status = Status::Queued;
                    task.yield_cont = true;
                    task.heartbeat_ms = None;
                    task.updated_ms = now;
                }
            }
        }
        Op::Pause(id) => {
            // A composite is never claimed, so the pause has to land on its leaves or it is undone
            // when the parent status is derived. Leaves that are mid-run go to `stopping`.
            if world.task(id).is_none() {
                out.accepted = false;
            } else {
                let ids = subtree_ids(&world, id);
                for sid in ids {
                    let (status, leaf) = match world.task(sid) {
                        Some(task) => (task.status, !world.has_owned_children(sid)),
                        None => continue,
                    };
                    if !leaf {
                        continue;
                    }
                    match status {
                        Status::Queued | Status::Waiting => {
                            if let Some(task) = world.task_mut(sid) {
                                task.status = Status::Paused;
                                task.updated_ms = now;
                            }
                            out.changed += 1;
                        }
                        Status::Running => {
                            if let Some(task) = world.task_mut(sid) {
                                // Inline work is never claimed, so nothing would return to finish the stop.
                                if task.inline {
                                    settle_stop(task, false, now);
                                } else {
                                    task.status = Status::Stopping;
                                    task.stop_intent = Some(StopIntent::Pause);
                                    task.updated_ms = now;
                                }
                            }
                            out.changed += 1;
                        }
                        _ => {}
                    }
                }
            }
        }
        Op::Resume(id) => {
            if subtree_stopping(&world, id) {
                out.accepted = false;
            } else {
                let ids = subtree_ids(&world, id);
                let parents: HashSet<TaskId> = ids
                    .iter()
                    .copied()
                    .filter(|sid| world.has_owned_children(*sid))
                    .collect();
                for sid in ids {
                    let paused = world.task(sid).is_some_and(|t| t.status == Status::Paused);
                    if !paused {
                        continue;
                    }
                    if let Some(task) = world.task_mut(sid) {
                        task.status = if parents.contains(&sid) {
                            Status::Waiting
                        } else {
                            Status::Queued
                        };
                        task.error = None;
                        task.note = None;
                        task.finished_ms = None;
                        task.updated_ms = now;
                        out.changed += 1;
                    }
                }
            }
        }
        Op::Cancel(id) => {
            let ids = subtree_ids(&world, id);
            out.changed = cancel_ids(&mut world, &ids, now);
        }
        Op::CancelScope(scope) => {
            let ids: Vec<TaskId> = world
                .tasks
                .iter()
                .filter(|t| t.scope.as_deref() == Some(scope.as_str()))
                .map(|t| t.id)
                .collect();
            out.changed = cancel_ids(&mut world, &ids, now);
        }
        Op::CancelAnalysis(scope) => {
            let roots: Vec<TaskId> = world
                .tasks
                .iter()
                .filter(|t| {
                    t.scope.as_deref() == Some(scope.as_str())
                        && t.kind == "analysis"
                        && t.parent.is_none()
                })
                .map(|t| t.id)
                .collect();
            let mut ids = Vec::new();
            for root in roots {
                ids.extend(subtree_ids(&world, root));
            }
            ids.sort_unstable();
            ids.dedup();
            out.changed = cancel_ids(&mut world, &ids, now);
        }
        Op::Restart(id) => restart(&mut world, id, now),
        Op::FinishStop { id, gen } => {
            let ok = world
                .task(id)
                .is_some_and(|t| t.gen == gen && t.status == Status::Stopping);
            out.accepted = ok;
            if ok {
                let cancel = world.task(id).and_then(|t| t.stop_intent) == Some(StopIntent::Cancel);
                if let Some(task) = world.task_mut(id) {
                    let failure = task.note.clone().filter(|note| is_failure_note(Some(note)));
                    task.status = if cancel {
                        Status::Cancelled
                    } else {
                        Status::Paused
                    };
                    task.note = if cancel { failure } else { None };
                    task.error = None;
                    task.finished_ms = if cancel { Some(now) } else { None };
                    task.stop_intent = None;
                    task.heartbeat_ms = None;
                    task.updated_ms = now;
                }
            }
        }
        Op::Heartbeat { id, gen } => {
            let ok = world.task(id).is_some_and(|t| {
                t.gen == gen && matches!(t.status, Status::Running | Status::Stopping)
            });
            out.accepted = ok;
            if ok {
                if let Some(task) = world.task_mut(id) {
                    task.heartbeat_ms = Some(now);
                }
            }
        }
        Op::Recover => out.changed = recover(&mut world, now),
        Op::Reap { older_than_ms } => out.changed = reap(&mut world, older_than_ms, now),
        Op::ReapRun { id, gen, note } => {
            let live = world.task(id).is_some_and(|t| {
                t.gen == gen && matches!(t.status, Status::Running | Status::Stopping)
            });
            out.accepted = live;
            if live {
                let cancel = world.task(id).and_then(|t| t.stop_intent) == Some(StopIntent::Cancel);
                if let Some(task) = world.task_mut(id) {
                    if task.status == Status::Stopping {
                        // The user asked to stop: the hang settles it instead of leaving it stuck.
                        settle_stop(task, cancel, now);
                    } else {
                        task.status = Status::Queued;
                        task.yield_cont = false;
                        task.heartbeat_ms = None;
                        task.gen += 1;
                        task.updated_ms = now;
                        set_hang(task, note);
                    }
                }
            }
        }
        Op::BeginInline {
            kind,
            title,
            input,
            parent,
            scope,
        } => {
            let id = world.next_id;
            world.next_id += 1;
            world.tasks.push(Task {
                id,
                kind,
                title,
                input,
                priority: 0,
                status: Status::Running,
                cursor: Value::Null,
                partial: Value::Null,
                result: None,
                progress: 0.0,
                error: None,
                note: None,
                attempts: 1,
                created_ms: now,
                updated_ms: now,
                parent,
                scope,
                inline: true,
                started_ms: Some(now),
                finished_ms: None,
                stats: Stats::default(),
                detail: Value::Null,
                units: 0.0,
                resource: None,
                memory: 0,
                model: None,
                role: Role::Chat(id.to_string()),
                gen: 1,
                yield_cont: false,
                heartbeat_ms: Some(now),
                stop_intent: None,
            });
            out.ids.push(id);
        }
        Op::AddStats { id, delta } => {
            if let Some(task) = world.task_mut(id) {
                task.stats.add(&delta);
                task.updated_ms = now;
            }
        }
        Op::SetDetail { id, detail } => {
            if let Some(task) = world.task_mut(id) {
                task.detail = detail;
                task.updated_ms = now;
            }
        }
        Op::SetPriority { id, priority } => {
            if let Some(task) = world.task_mut(id) {
                task.priority = priority;
                task.updated_ms = now;
            }
        }
        Op::Remove(id) => {
            let ids: HashSet<TaskId> = subtree_ids(&world, id).into_iter().collect();
            world.tasks.retain(|t| !ids.contains(&t.id));
            world
                .deps
                .retain(|(t, n)| !ids.contains(t) && !ids.contains(n));
            out.changed = ids.len();
        }
        Op::ClearDone => {
            let mut doomed = HashSet::new();
            let mut stack: Vec<TaskId> = world
                .tasks
                .iter()
                .filter(|t| t.parent.is_none() && t.status == Status::Done)
                .map(|t| t.id)
                .collect();
            while let Some(id) = stack.pop() {
                if doomed.insert(id) {
                    stack.extend(world.child_ids(id));
                }
            }
            out.changed = doomed.len();
            world.tasks.retain(|t| !doomed.contains(&t.id));
            world.deps.retain(|(t, n)| {
                world.tasks.iter().any(|x| x.id == *t) && world.tasks.iter().any(|x| x.id == *n)
            });
        }
        Op::Purge { cutoff_ms } => {
            let mut doomed = HashSet::new();
            let mut stack: Vec<TaskId> = world
                .tasks
                .iter()
                .filter(|t| {
                    t.parent.is_none()
                        && t.status.is_finished()
                        && t.finished_ms.unwrap_or(t.updated_ms) < cutoff_ms
                })
                .map(|t| t.id)
                .collect();
            while let Some(id) = stack.pop() {
                if doomed.insert(id) {
                    stack.extend(world.child_ids(id));
                }
            }
            out.changed = doomed.len();
            world.tasks.retain(|t| !doomed.contains(&t.id));
            world.deps.retain(|(t, n)| {
                world.tasks.iter().any(|x| x.id == *t) && world.tasks.iter().any(|x| x.id == *n)
            });
        }
    }
    reconcile(&mut world, now);
    out.events = diff_events(&before, &world);
    if let Some(id) = chunk {
        if let Some(task) = world.task(id) {
            out.events.push(Event {
                task: id,
                scope: task.scope.clone(),
                kind: EventKind::Chunk,
            });
        }
    }
    if let Some(id) = out.ids.first().copied() {
        out.status = world.task(id).map(|t| t.status);
    }
    Ok((world, out))
}

/// A leaf the scheduler may claim: queued, not inline, no children, deps done, nothing paused upstream.
pub fn is_claimable(world: &World, id: TaskId) -> bool {
    let Some(task) = world.task(id) else {
        return false;
    };
    if task.status != Status::Queued || task.inline || world.has_owned_children(id) {
        return false;
    }
    dep_hold(world, id).is_none()
}

/// `Paused` when a dependency is paused (even behind a done task), else `None` when every dep is done.
pub fn dep_hold(world: &World, id: TaskId) -> Option<DepHold> {
    let mut queue = VecDeque::from(world.needs_of(id));
    let mut seen = HashSet::new();
    let mut unmet = false;
    let mut paused: Option<String> = None;
    while let Some(dep) = queue.pop_front() {
        if !seen.insert(dep) {
            continue;
        }
        let Some(found) = world.task(dep) else {
            continue;
        };
        if effective_status(found) == Status::Paused && paused.is_none() {
            paused = Some(found.shown_name());
        }
        if found.status != Status::Done && effective_status(found) != Status::Done {
            unmet = true;
        }
        queue.extend(world.needs_of(dep));
    }
    if let Some(title) = paused {
        Some(DepHold::Paused(title))
    } else if unmet {
        Some(DepHold::Dependency)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DepHold {
    Paused(String),
    Dependency,
}

fn subtree_stopping(world: &World, id: TaskId) -> bool {
    subtree_ids(world, id).into_iter().any(|sid| {
        world
            .task(sid)
            .is_some_and(|t| t.status == Status::Stopping)
    })
}

pub fn subtree_ids(world: &World, id: TaskId) -> Vec<TaskId> {
    let mut out = Vec::new();
    let mut stack = vec![id];
    let mut seen = HashSet::new();
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur) {
            continue;
        }
        out.push(cur);
        stack.extend(world.child_ids(cur));
    }
    out
}

fn cancel_ids(world: &mut World, ids: &[TaskId], now: i64) -> usize {
    let mut n = 0;
    for id in ids {
        let Some(task) = world.task(*id) else {
            continue;
        };
        if !matches!(
            task.status,
            Status::Queued | Status::Running | Status::Waiting | Status::Paused | Status::Stopping
        ) {
            continue;
        }
        n += 1;
        let running = matches!(task.status, Status::Running | Status::Stopping);
        let inline = task.inline;
        if let Some(task) = world.task_mut(*id) {
            if running && !inline {
                task.status = Status::Stopping;
                task.stop_intent = Some(StopIntent::Cancel);
                task.updated_ms = now;
            } else if running {
                settle_stop(task, true, now);
            } else {
                task.status = Status::Cancelled;
                task.error = None;
                task.note = None;
                task.finished_ms = Some(now);
                task.stop_intent = None;
                task.updated_ms = now;
            }
        }
    }
    n
}

fn recover(world: &mut World, now: i64) -> usize {
    let ids: Vec<TaskId> = world.tasks.iter().map(|t| t.id).collect();
    let mut n = 0;
    for id in ids {
        let (status, inline, owned) = {
            let Some(task) = world.task(id) else { continue };
            (task.status, task.inline, world.has_owned_children(id))
        };
        // A composite's status comes from its tasks. An answer's inline steps do not.
        if owned && !inline {
            continue;
        }
        match status {
            Status::Running if inline => {
                if let Some(task) = world.task_mut(id) {
                    task.status = Status::Cancelled;
                    task.finished_ms = Some(now);
                    task.heartbeat_ms = None;
                    task.updated_ms = now;
                    task.gen += 1;
                }
                n += 1;
            }
            Status::Running => {
                if let Some(task) = world.task_mut(id) {
                    task.status = Status::Queued;
                    task.yield_cont = false;
                    task.heartbeat_ms = None;
                    task.updated_ms = now;
                    task.gen += 1;
                }
                n += 1;
            }
            Status::Stopping => {
                let cancel = world.task(id).and_then(|t| t.stop_intent) == Some(StopIntent::Cancel);
                if let Some(task) = world.task_mut(id) {
                    settle_stop(task, cancel, now);
                }
                n += 1;
            }
            _ => {}
        }
    }
    n
}

fn reap(world: &mut World, older_than_ms: i64, now: i64) -> usize {
    let ids: Vec<TaskId> = world.tasks.iter().map(|t| t.id).collect();
    let mut n = 0;
    for id in ids {
        let Some(task) = world.task(id) else { continue };
        if task.inline || world.has_owned_children(id) {
            continue;
        }
        if !matches!(task.status, Status::Running | Status::Stopping) {
            continue;
        }
        let beat = task.heartbeat_ms.unwrap_or(0);
        if beat >= older_than_ms {
            continue;
        }
        let stopping = task.status == Status::Stopping;
        let cancel = task.stop_intent == Some(StopIntent::Cancel);
        if let Some(task) = world.task_mut(id) {
            if stopping {
                settle_stop(task, cancel, now);
            } else {
                task.status = Status::Queued;
                task.yield_cont = false;
                task.heartbeat_ms = None;
                task.gen += 1;
                task.updated_ms = now;
            }
        }
        n += 1;
    }
    n
}

fn restart(world: &mut World, id: TaskId, now: i64) {
    let Some(task) = world.task(id).cloned() else {
        return;
    };
    let children = child_map(world);
    let needs = needs_map(world);
    let mut ids: HashSet<TaskId> = HashSet::new();
    if !matches!(task.status, Status::Running | Status::Stopping) {
        ids.insert(id);
    }
    let mut stack = children.get(&id).cloned().unwrap_or_default();
    let mut seen = HashSet::from([id]);
    while let Some(child) = stack.pop() {
        if !seen.insert(child) {
            continue;
        }
        if let Some(found) = world.task(child) {
            if matches!(found.status, Status::Failed | Status::Cancelled) {
                ids.insert(child);
            }
        }
        if let Some(more) = children.get(&child) {
            stack.extend(more.iter().copied());
        }
    }
    if matches!(task.status, Status::Failed | Status::Cancelled) {
        let mut roots = ids.clone();
        roots.insert(id);
        loop {
            let extra: Vec<TaskId> = world
                .tasks
                .iter()
                .filter(|item| item.status == Status::Cancelled && !ids.contains(&item.id))
                .filter(|item| {
                    roots.iter().any(|root| depends_on(item.id, *root, &needs))
                        || has_ancestor(item, &roots, world)
                })
                .map(|item| item.id)
                .collect();
            if extra.is_empty() {
                break;
            }
            for extra_id in extra {
                ids.insert(extra_id);
                roots.insert(extra_id);
            }
        }
    }
    for rid in ids {
        if world
            .task(rid)
            .is_some_and(|t| matches!(t.status, Status::Running | Status::Stopping))
        {
            continue;
        }
        let waiting = world.has_owned_children(rid);
        if let Some(task) = world.task_mut(rid) {
            task.status = if waiting {
                Status::Waiting
            } else {
                Status::Queued
            };
            task.cursor = Value::Null;
            task.partial = Value::Null;
            task.result = None;
            task.progress = 0.0;
            task.error = None;
            task.note = None;
            task.started_ms = None;
            task.finished_ms = None;
            task.yield_cont = false;
            task.stop_intent = None;
            task.updated_ms = now;
        }
    }
}

fn has_ancestor(task: &Task, roots: &HashSet<TaskId>, world: &World) -> bool {
    let mut parent = task.parent;
    let mut guard = 0;
    while let Some(id) = parent {
        if roots.contains(&id) {
            return true;
        }
        guard += 1;
        if guard > 64 {
            break;
        }
        parent = world.task(id).and_then(|t| t.parent);
    }
    false
}

fn depends_on(task: TaskId, target: TaskId, needs: &HashMap<TaskId, Vec<TaskId>>) -> bool {
    let mut stack = needs.get(&task).cloned().unwrap_or_default();
    let mut seen = HashSet::new();
    while let Some(id) = stack.pop() {
        if id == target {
            return true;
        }
        if seen.insert(id) {
            if let Some(more) = needs.get(&id) {
                stack.extend(more.iter().copied());
            }
        }
    }
    false
}

fn child_map(world: &World) -> HashMap<TaskId, Vec<TaskId>> {
    let mut map: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for task in &world.tasks {
        if let Some(parent) = task.parent {
            map.entry(parent).or_default().push(task.id);
        }
    }
    map
}

fn needs_map(world: &World) -> HashMap<TaskId, Vec<TaskId>> {
    let mut map: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for (task, need) in &world.deps {
        map.entry(*task).or_default().push(*need);
    }
    map
}

fn require_resource(task: &NewTask) -> Result<()> {
    if task.composite || task.inline || task.resource.is_some() {
        Ok(())
    } else {
        Err(Error::Bad(format!(
            "leaf task '{}' has no resource",
            task.kind
        )))
    }
}

fn insert_new(world: &mut World, task: NewTask, now: i64) -> Result<TaskId> {
    require_resource(&task)?;
    let status = if task.composite {
        Status::Waiting
    } else {
        Status::Queued
    };
    let needs = task.needs.clone();
    let id = push_task(world, task, status, now);
    for need in needs {
        if !world.deps.contains(&(id, need)) {
            world.deps.push((id, need));
        }
    }
    Ok(id)
}

fn push_task(world: &mut World, task: NewTask, status: Status, now: i64) -> TaskId {
    let id = world.next_id;
    world.next_id += 1;
    world.tasks.push(Task {
        id,
        kind: task.kind,
        title: task.title,
        input: task.input,
        priority: task.priority,
        status,
        cursor: task.cursor.unwrap_or(Value::Null),
        partial: Value::Null,
        result: None,
        progress: 0.0,
        error: None,
        note: None,
        attempts: 0,
        created_ms: now,
        updated_ms: now,
        parent: task.parent,
        scope: task.scope,
        inline: task.inline,
        started_ms: None,
        finished_ms: None,
        stats: Stats::default(),
        detail: Value::Null,
        units: task.units,
        resource: task.resource,
        memory: task.memory,
        model: task.model,
        role: task.role,
        gen: 0,
        yield_cont: false,
        heartbeat_ms: None,
        stop_intent: None,
    });
    id
}

fn insert_children(
    world: &mut World,
    parent: Option<TaskId>,
    scope: Option<&str>,
    children: &[Child],
    inherited: &[TaskId],
    all: &mut Vec<TaskId>,
    now: i64,
) -> Result<Vec<TaskId>> {
    let mut ids = Vec::with_capacity(children.len());
    for child in children {
        let mut insert = child.task.clone();
        if insert.parent.is_none() {
            insert.parent = parent;
        }
        if insert.scope.is_none() {
            insert.scope = scope.map(str::to_string);
        }
        require_resource(&insert)?;
        let status = if insert.composite {
            Status::Waiting
        } else {
            Status::Queued
        };
        ids.push(push_task(world, insert, status, now));
    }
    let mut deps: Vec<Vec<TaskId>> = Vec::with_capacity(children.len());
    for (i, child) in children.iter().enumerate() {
        let mut needs = inherited.to_vec();
        needs.extend(child.task.needs.iter().copied());
        for &sibling in &child.after {
            let Some(need) = ids.get(sibling) else {
                return Err(Error::Bad(format!(
                    "child {i} waits for sibling {sibling}, which is not in the list"
                )));
            };
            needs.push(*need);
        }
        for need in &needs {
            if !world.deps.contains(&(ids[i], *need)) {
                world.deps.push((ids[i], *need));
            }
        }
        deps.push(needs);
    }
    all.extend(ids.iter().copied());
    for (i, child) in children.iter().enumerate() {
        if child.children.is_empty() {
            continue;
        }
        let nested_scope = child
            .task
            .scope
            .clone()
            .or_else(|| scope.map(str::to_string));
        insert_children(
            world,
            Some(ids[i]),
            nested_scope.as_deref(),
            &child.children,
            &deps[i],
            all,
            now,
        )?;
    }
    Ok(ids)
}

fn add_kind_run(world: &mut World, task: &Task, now: i64) {
    let ms = task.started_ms.map_or(0, |started| (now - started).max(0));
    if let Some(stat) = world.kind_stats.iter_mut().find(|s| s.kind == task.kind) {
        stat.runs += 1;
        stat.ms += ms;
        stat.units += task.units;
    } else {
        world.kind_stats.push(KindStat {
            kind: task.kind.clone(),
            runs: 1,
            ms,
            units: task.units,
        });
    }
}

fn reconcile(world: &mut World, now: i64) {
    for _ in 0..32 {
        let refreshed = refresh_all(world, now);
        let cascaded = cascade(world, now);
        if !refreshed && !cascaded {
            break;
        }
    }
    write_pause_notes(world, now);
}

fn refresh_all(world: &mut World, now: i64) -> bool {
    let parent_of: HashMap<TaskId, Option<TaskId>> =
        world.tasks.iter().map(|t| (t.id, t.parent)).collect();
    let mut parents: Vec<TaskId> = world.tasks.iter().filter_map(|t| t.parent).collect();
    parents.sort_unstable();
    parents.dedup();
    parents.sort_by_key(|id| std::cmp::Reverse(depth_of(*id, &parent_of)));
    let mut changed = false;
    for parent in parents {
        if refresh(world, parent, now) {
            changed = true;
        }
    }
    changed
}

fn depth_of(id: TaskId, parent_of: &HashMap<TaskId, Option<TaskId>>) -> usize {
    let mut depth = 0;
    let mut current = parent_of.get(&id).copied().flatten();
    while let Some(parent) = current {
        depth += 1;
        if depth > 64 {
            break;
        }
        current = parent_of.get(&parent).copied().flatten();
    }
    depth
}

fn refresh(world: &mut World, parent: TaskId, now: i64) -> bool {
    let Some(parent_task) = world.task(parent).cloned() else {
        return false;
    };
    // Only a task that owns real children (a composite) derives its status. Inline steps
    // of an answer do not: the answer keeps the status of its run.
    if parent_task.inline || !world.has_owned_children(parent) {
        return false;
    }
    let children: Vec<Task> = world
        .tasks
        .iter()
        .filter(|t| t.parent == Some(parent))
        .cloned()
        .collect();
    if children.is_empty() {
        return false;
    }
    let states: Vec<ChildState> = children
        .iter()
        .map(|child| child_state(world, child))
        .collect();
    let errors: Vec<Option<&str>> = children
        .iter()
        .map(|child| child.error.as_deref())
        .collect();
    let notes: Vec<Option<&Note>> = children.iter().map(|child| child.note.as_ref()).collect();
    let Some(status) = composite_status(&states) else {
        return false;
    };
    let error = composite_error(&states, &errors);
    let note = composite_note(&states, &notes);
    let progress = if status == Status::Done {
        1.0
    } else {
        let weights: Vec<Option<f64>> = children
            .iter()
            .map(|child| weight(child, &world.tasks, now, &world.kind_stats))
            .collect();
        let value = if weights.iter().all(Option::is_some) {
            let total: f64 = weights.iter().flatten().sum();
            if total > 0.0 {
                children
                    .iter()
                    .zip(&weights)
                    .map(|(c, w)| f64::from(c.progress) * w.unwrap_or(0.0))
                    .sum::<f64>()
                    / total
            } else {
                children.iter().map(|c| f64::from(c.progress)).sum::<f64>() / children.len() as f64
            }
        } else {
            children.iter().map(|c| f64::from(c.progress)).sum::<f64>() / children.len() as f64
        };
        value as f32
    };
    let progress = progress.clamp(0.0, 1.0);
    let started = parent_task
        .started_ms
        .or_else(|| (status == Status::Running).then_some(now));
    let finished = if status.is_finished() {
        Some(parent_task.finished_ms.unwrap_or(now))
    } else {
        None
    };
    let same = parent_task.status == status
        && parent_task.error == error
        && parent_task.note == note
        && (parent_task.progress - progress).abs() < 0.000_1
        && parent_task.started_ms == started
        && parent_task.finished_ms == finished;
    if same {
        return false;
    }
    if let Some(task) = world.task_mut(parent) {
        task.status = status;
        task.error = error.clone();
        task.note = note.clone();
        task.progress = progress;
        task.started_ms = started;
        task.finished_ms = finished;
        task.updated_ms = now;
    }
    parent_task.status != status || parent_task.error != error || parent_task.note != note
}

pub fn child_state(world: &World, child: &Task) -> ChildState {
    let status = effective_status(child);
    let blocked = dep_hold(world, child.id).is_some();
    ChildState::of(status, blocked)
}

#[derive(Clone)]
enum CancelCause {
    Failure(Note),
    Plain,
}

fn cascade(world: &mut World, now: i64) -> bool {
    let mut any = false;
    for _ in 0..64 {
        // Read the state the rules decide on, then let the borrow end before writing anything.
        let doomed = {
            let needs = needs_map(world);
            let children = child_map(world);
            let by_id: HashMap<TaskId, &Task> =
                world.tasks.iter().map(|task| (task.id, task)).collect();
            let mut doomed: HashMap<TaskId, CancelCause> = HashMap::new();
            for item in &world.tasks {
                if effective_status(item).is_finished() || item.status.is_finished() {
                    continue;
                }
                if let Some(cause) = cancel_cause(item, &by_id, &needs) {
                    doomed.insert(item.id, cause);
                }
            }
            if doomed.is_empty() {
                break;
            }
            let mut stack: Vec<TaskId> = doomed.keys().copied().collect();
            while let Some(id) = stack.pop() {
                let Some(cause) = doomed.get(&id).cloned() else {
                    continue;
                };
                let Some(kids) = children.get(&id) else {
                    continue;
                };
                for child in kids {
                    let Some(found) = by_id.get(child).copied() else {
                        continue;
                    };
                    if found.status.is_finished() || effective_status(found).is_finished() {
                        continue;
                    }
                    match doomed.get(child) {
                        Some(CancelCause::Failure(_)) => {}
                        Some(CancelCause::Plain) => {
                            if let CancelCause::Failure(note) = &cause {
                                doomed.insert(*child, CancelCause::Failure(note.clone()));
                            }
                        }
                        None => {
                            doomed.insert(*child, cause.clone());
                            stack.push(*child);
                        }
                    }
                }
            }
            doomed
        };
        any = true;
        let mut ids: Vec<TaskId> = doomed.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let Some(cause) = doomed.get(&id).cloned() else {
                continue;
            };
            let (running, inline) = world.task(id).map_or((false, false), |t| {
                (
                    matches!(t.status, Status::Running | Status::Stopping),
                    t.inline,
                )
            });
            if let Some(task) = world.task_mut(id) {
                if running && !inline {
                    task.status = Status::Stopping;
                    task.stop_intent = Some(StopIntent::Cancel);
                    if let CancelCause::Failure(note) = &cause {
                        task.note = Some(note.clone());
                    }
                    task.updated_ms = now;
                } else if running {
                    if let CancelCause::Failure(note) = &cause {
                        task.note = Some(note.clone());
                    }
                    settle_stop(task, true, now);
                } else {
                    task.status = Status::Cancelled;
                    task.finished_ms = Some(now);
                    task.stop_intent = None;
                    task.note = match cause {
                        CancelCause::Failure(note) => Some(note),
                        CancelCause::Plain => None,
                    };
                }
                task.updated_ms = now;
            }
        }
    }
    any
}

fn cancel_cause(
    task: &Task,
    by_id: &HashMap<TaskId, &Task>,
    needs: &HashMap<TaskId, Vec<TaskId>>,
) -> Option<CancelCause> {
    if let Some(cause) = dependency_block(task.id, by_id, needs) {
        if matches!(cause, CancelCause::Failure(_)) {
            return Some(cause);
        }
        if let Some(CancelCause::Failure(note)) = ancestor_cancel(task, by_id) {
            return Some(CancelCause::Failure(note));
        }
        return Some(cause);
    }
    ancestor_cancel(task, by_id)
}

fn dependency_block(
    id: TaskId,
    by_id: &HashMap<TaskId, &Task>,
    needs: &HashMap<TaskId, Vec<TaskId>>,
) -> Option<CancelCause> {
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    if let Some(deps) = needs.get(&id) {
        queue.extend(deps.iter().copied());
    }
    let mut saw_cancelled = false;
    while let Some(dep) = queue.pop_front() {
        if !seen.insert(dep) {
            continue;
        }
        let Some(found) = by_id.get(&dep).copied() else {
            continue;
        };
        match effective_status(found) {
            Status::Failed => return Some(CancelCause::Failure(failure_note(&found.shown_name()))),
            Status::Cancelled => saw_cancelled = true,
            _ => {}
        }
        if let Some(more) = needs.get(&dep) {
            queue.extend(more.iter().copied());
        }
    }
    saw_cancelled.then_some(CancelCause::Plain)
}

fn ancestor_cancel(task: &Task, by_id: &HashMap<TaskId, &Task>) -> Option<CancelCause> {
    let mut parent = task.parent;
    let mut guard = 0;
    while let Some(id) = parent {
        guard += 1;
        if guard > 64 {
            break;
        }
        let Some(found) = by_id.get(&id).copied() else {
            break;
        };
        if effective_status(found) == Status::Cancelled || found.status == Status::Cancelled {
            if let Some(note) = found
                .note
                .clone()
                .filter(|note| is_failure_note(Some(note)))
            {
                return Some(CancelCause::Failure(note));
            }
            return Some(CancelCause::Plain);
        }
        parent = found.parent;
    }
    None
}

fn write_pause_notes(world: &mut World, now: i64) {
    // Decide the new notes first, then write them, so the world is borrowed only one way at a time.
    let notes: Vec<(TaskId, Option<Note>)> = {
        let needs = needs_map(world);
        let children = child_map(world);
        let by_id: HashMap<TaskId, &Task> =
            world.tasks.iter().map(|task| (task.id, task)).collect();
        let mut notes = Vec::new();
        for item in &world.tasks {
            if item.inline || children.get(&item.id).is_some_and(|kids| !kids.is_empty()) {
                continue;
            }
            if !matches!(item.status, Status::Queued | Status::Waiting) {
                continue;
            }
            let note = paused_dependency(item.id, &by_id, &needs).map(|title| pause_note(&title));
            let next = match &note {
                Some(note) => Some(note.clone()),
                None if is_pause_note(item.note.as_ref()) => None,
                None => continue,
            };
            if item.note == next {
                continue;
            }
            notes.push((item.id, next));
        }
        notes
    };
    for (id, next) in notes {
        if let Some(task) = world.task_mut(id) {
            task.note = next;
            task.updated_ms = now;
        }
    }
}

fn paused_dependency(
    id: TaskId,
    by_id: &HashMap<TaskId, &Task>,
    needs: &HashMap<TaskId, Vec<TaskId>>,
) -> Option<String> {
    let mut queue = VecDeque::new();
    let mut seen = HashSet::new();
    if let Some(deps) = needs.get(&id) {
        queue.extend(deps.iter().copied());
    }
    while let Some(dep) = queue.pop_front() {
        if !seen.insert(dep) {
            continue;
        }
        let Some(found) = by_id.get(&dep).copied() else {
            continue;
        };
        if effective_status(found) == Status::Paused {
            return Some(found.shown_name());
        }
        if let Some(more) = needs.get(&dep) {
            queue.extend(more.iter().copied());
        }
    }
    None
}

fn diff_events(before: &HashMap<TaskId, Status>, world: &World) -> Vec<Event> {
    let mut events = Vec::new();
    let mut tasks: Vec<&Task> = world.tasks.iter().collect();
    tasks.sort_by_key(|t| t.id);
    for task in tasks {
        let prev = before.get(&task.id).copied();
        let kind = match (prev, task.status) {
            (Some(a), b) if a == b => None,
            (_, Status::Running) => Some(EventKind::Started),
            (_, Status::Paused) => Some(EventKind::Paused),
            (Some(Status::Paused), Status::Queued) => Some(EventKind::Resumed),
            (_, Status::Done) => Some(EventKind::Done),
            (_, Status::Failed) => Some(EventKind::Failed),
            (_, Status::Cancelled) => Some(EventKind::Cancelled),
            _ => None,
        };
        if let Some(kind) = kind {
            events.push(Event {
                task: task.id,
                scope: task.scope.clone(),
                kind,
            });
        }
    }
    events
}

/// How long the task has been running: from its start to its finish, or to `now` while it is
/// still running. A `finished_ms` left by an earlier attempt does not end a running task.
pub fn elapsed(task: &Task, now: i64) -> Duration {
    let Some(started) = task.started_ms else {
        return Duration::ZERO;
    };
    let end = if matches!(task.status, Status::Running | Status::Stopping) { now } else { task.finished_ms.unwrap_or(now) };
    Duration::from_millis((end - started).max(0) as u64)
}

pub fn estimate(task: &Task, now: i64, kind_stats: &[KindStat]) -> Option<Duration> {
    let status = effective_status(task);
    match status {
        Status::Done => return Some(Duration::ZERO),
        Status::Failed | Status::Cancelled => return None,
        _ => {}
    }
    let p = task.progress.clamp(0.0, 1.0) as f64;
    if p >= 0.05 && task.started_ms.is_some() {
        let gone = elapsed(task, now);
        if gone >= Duration::from_secs(10) {
            return Some(Duration::from_secs_f64(gone.as_secs_f64() * (1.0 - p) / p));
        }
    }
    let per_unit = kind_stats
        .iter()
        .find(|s| s.kind == task.kind && s.units > 0.0)
        .map(|s| s.ms as f64 / s.units)?;
    if task.units <= 0.0 {
        return None;
    }
    Some(Duration::from_millis(
        (per_unit * task.units * (1.0 - p)).max(0.0) as u64,
    ))
}

pub fn estimate_tree(
    task: &Task,
    tasks: &[Task],
    now: i64,
    kind_stats: &[KindStat],
) -> Option<Duration> {
    let children: Vec<&Task> = tasks.iter().filter(|t| t.parent == Some(task.id)).collect();
    if children.is_empty() {
        return estimate(task, now, kind_stats);
    }
    if children.iter().all(|c| effective_status(c) == Status::Done) {
        return Some(Duration::ZERO);
    }
    let mut total = Duration::ZERO;
    let mut any = false;
    for child in children
        .into_iter()
        .filter(|c| effective_status(c) != Status::Done)
    {
        if let Some(left) = estimate_tree(child, tasks, now, kind_stats) {
            total += left;
            any = true;
        }
    }
    any.then_some(total)
}

fn weight(task: &Task, tasks: &[Task], now: i64, stats: &[KindStat]) -> Option<f64> {
    let left = estimate_tree(task, tasks, now, stats)?.as_secs_f64();
    Some(left + elapsed(task, now).as_secs_f64())
}

pub fn top_level(tasks: &[Task], filter: &Filter) -> Vec<Task> {
    let mut owned: Vec<Task> = tasks
        .iter()
        .filter(|t| t.parent.is_none() && in_group(t, filter.group))
        .filter(|t| {
            filter
                .scope
                .as_ref()
                .is_none_or(|scope| t.scope.as_ref() == Some(scope))
        })
        .cloned()
        .collect();
    sort_tasks(&mut owned);
    owned
}

/// Listing order: running, queued, waiting, then the rest; queued and waiting by priority.
pub fn sort_tasks(tasks: &mut [Task]) {
    fn rank(status: Status) -> u8 {
        match status {
            Status::Running | Status::Stopping => 0,
            Status::Queued => 1,
            Status::Waiting => 2,
            Status::Paused => 3,
            Status::Failed => 4,
            Status::Cancelled => 5,
            Status::Done => 6,
        }
    }
    tasks.sort_by(|a, b| {
        rank(a.status).cmp(&rank(b.status)).then_with(|| {
            if matches!(a.status, Status::Queued | Status::Waiting) && a.status == b.status {
                b.priority.cmp(&a.priority).then(a.id.cmp(&b.id))
            } else {
                b.updated_ms.cmp(&a.updated_ms).then(a.id.cmp(&b.id))
            }
        })
    });
}

pub fn in_group(task: &Task, group: KindGroup) -> bool {
    match group {
        KindGroup::All => true,
        KindGroup::Analysis => ANALYSIS_KINDS.contains(&task.kind.as_str()),
        KindGroup::Chat => task.inline || CHAT_KINDS.contains(&task.kind.as_str()),
    }
}

/// Models unfinished tasks still need, including a run that is stopping. The engine unloads these last.
pub fn models_needed(tasks: &[Task]) -> Vec<(String, Vec<TaskId>)> {
    let mut map: HashMap<String, Vec<TaskId>> = HashMap::new();
    for task in tasks {
        if task.status.is_finished() {
            continue;
        }
        if let Some(model) = &task.model {
            map.entry(model.clone()).or_default().push(task.id);
        }
    }
    let mut out: Vec<(String, Vec<TaskId>)> = map.into_iter().collect();
    for (_, ids) in &mut out {
        ids.sort_unstable();
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_running_task_counts_up_to_now_and_a_stale_finish_does_not_end_it() {
        let task = |status, started, finished| Task {
            id: 1,
            kind: "index".into(),
            title: "rec".into(),
            input: Value::Null,
            priority: 0,
            status,
            cursor: Value::Null,
            partial: Value::Null,
            result: None,
            progress: 0.0,
            error: None,
            note: None,
            attempts: 1,
            created_ms: 0,
            updated_ms: 0,
            parent: None,
            scope: None,
            inline: false,
            started_ms: started,
            finished_ms: finished,
            stats: Stats::default(),
            detail: Value::Null,
            units: 1.0,
            resource: None,
            memory: 0,
            model: None,
            role: Role::Work,
            gen: 0,
            yield_cont: false,
            heartbeat_ms: None,
            stop_intent: None,
        };
        // A finish stamp of an earlier attempt does not end the run that is going on.
        let running = task(Status::Running, Some(1_000), Some(2_000));
        assert_eq!(elapsed(&running, 91_000), Duration::from_secs(90));
        let done = task(Status::Done, Some(1_000), Some(4_000));
        assert_eq!(elapsed(&done, 91_000), Duration::from_secs(3));
        let queued = task(Status::Queued, None, None);
        assert_eq!(elapsed(&queued, 91_000), Duration::ZERO);
    }

    #[test]
    fn composite_status_follows_the_rule() {
        assert_eq!(composite_status(&[]), None);
        assert_eq!(
            composite_status(&[ChildState::Done, ChildState::Going]),
            Some(Status::Running)
        );
        assert_eq!(
            composite_status(&[ChildState::Paused, ChildState::Waiting]),
            Some(Status::Paused)
        );
        assert_eq!(
            composite_status(&[ChildState::Waiting, ChildState::Done]),
            Some(Status::Waiting)
        );
        assert_eq!(
            composite_status(&[ChildState::Done, ChildState::Done]),
            Some(Status::Done)
        );
        assert_eq!(
            composite_status(&[ChildState::Failed, ChildState::Cancelled]),
            Some(Status::Failed)
        );
        assert_eq!(
            composite_status(&[ChildState::Cancelled, ChildState::Done]),
            Some(Status::Cancelled)
        );
    }
}
