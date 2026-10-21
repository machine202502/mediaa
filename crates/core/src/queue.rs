//! Queue of neural-network tasks of the Agent section (`<media>/.ai/queue.db`). A task keeps
//! where it stopped (`cursor`) and what it has produced so far (`partial`), both JSON, written
//! together in one transaction by every checkpoint. After a crash of the program or of the
//! task's worker the task goes back to the queue and continues from its last checkpoint.
//!
//! Tasks form a graph (schema 2): a composite task (`parent`, created `waiting`, never claimed)
//! groups the child tasks of one logical piece of work; `task_deps` says which tasks have to be
//! `done` before a task may be claimed; `kind_stats` remembers how long a kind of work takes per
//! unit, so the queue can estimate what is left.

use anyhow::bail;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type TaskId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Running,
    /// A composite task that no child has started yet; never claimed.
    Waiting,
    Paused,
    Done,
    Failed,
    Cancelled,
}

impl TaskStatus {
    fn from_code(s: &str) -> TaskStatus {
        match s {
            "running" => TaskStatus::Running,
            "waiting" => TaskStatus::Waiting,
            "paused" => TaskStatus::Paused,
            "done" => TaskStatus::Done,
            "failed" => TaskStatus::Failed,
            "cancelled" => TaskStatus::Cancelled,
            _ => TaskStatus::Queued,
        }
    }

    /// Nothing more will happen to the task on its own.
    pub fn is_finished(self) -> bool {
        matches!(self, TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled)
    }
}

/// What the inner calls of a task used: the queue counts this for every task (inline ones — chat
/// answers, steps and tools — and background ones alike).
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
    /// The models called, once each.
    #[serde(default)]
    pub models: Vec<String>,
}

impl Stats {
    /// Adds another share of work (the models are merged, not added).
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

/// How long a kind of work took per unit of work, over all its runs (`kind_stats`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KindStat {
    pub kind: String,
    pub runs: i64,
    pub ms: i64,
    /// Minutes of recording, thousands of words, topics…
    pub units: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: TaskId,
    /// What to do (the worker picks the handler by it), e.g. `transcribe`.
    pub kind: String,
    /// Shown in the queue.
    pub title: String,
    /// The task's input (file, options…).
    pub input: Value,
    /// Higher first; equal priorities in order of creation.
    pub priority: i64,
    pub status: TaskStatus,
    /// Where the task stopped (`Null`: not started).
    pub cursor: Value,
    /// What it has produced so far.
    pub partial: Value,
    pub result: Option<Value>,
    /// 0..=1.
    pub progress: f32,
    pub error: Option<String>,
    /// How many times it was started.
    pub attempts: i64,
    pub created_ms: i64,
    pub updated_ms: i64,
    /// The composite task this task belongs to (`None` at the top level).
    pub parent: Option<TaskId>,
    /// The recording (file id) or folder the work is about; used for filtering and for stopping
    /// all the work of one recording at once.
    pub scope: Option<String>,
    /// Run by the caller itself (a chat answer), not by the worker; never claimed.
    pub inline: bool,
    /// The first start. Set once, so a continued task still tells how long it has been going.
    pub started_ms: Option<i64>,
    /// Set by `done`, `failed` and `cancelled`.
    pub finished_ms: Option<i64>,
    pub stats: Stats,
    /// For rows of tools: `{tool, args, result_chars}`.
    pub detail: Value,
    /// How much work it is (minutes of the recording, thousands of words, topics…), set when the
    /// task is enqueued; 0 when unknown.
    pub units: f64,
}

/// What a caller wants to enqueue. A plain `NewTask::new` is a normal queued task; the builders
/// add a parent, dependencies, work size and the rest.
#[derive(Debug, Clone)]
pub struct NewTask {
    kind: String,
    title: String,
    input: Value,
    priority: i64,
    parent: Option<TaskId>,
    scope: Option<String>,
    needs: Vec<TaskId>,
    units: f64,
    inline: bool,
    composite: bool,
    cursor: Option<Value>,
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

    /// The task waits until all these tasks are `done`.
    pub fn needs(mut self, needs: &[TaskId]) -> NewTask {
        self.needs = needs.to_vec();
        self
    }

    /// How much work it is, for the time estimate (see [`Task::units`]).
    pub fn units(mut self, units: f64) -> NewTask {
        self.units = units;
        self
    }

    /// Created `inline`: the caller runs it itself, the worker never takes it.
    pub fn inline(mut self) -> NewTask {
        self.inline = true;
        self
    }

    /// Created `waiting` and never claimed: only its children do the work.
    pub fn composite(mut self) -> NewTask {
        self.composite = true;
        self
    }

    /// Starts from this checkpoint instead of the beginning.
    pub fn cursor(mut self, cursor: &Value) -> NewTask {
        self.cursor = Some(cursor.clone());
        self
    }
}

/// A child enqueued by [`Queue::complete_with`] or [`Queue::enqueue_tree`]. `after` names its
/// siblings (by their index in the `children` slice) that it waits for; its own `needs` name
/// tasks already in the queue. A composite child brings its own children (a whole subtree).
#[derive(Debug, Clone)]
pub struct Child {
    pub task: NewTask,
    pub after: Vec<usize>,
    /// The children of this child; they go under it, defaulting to its scope.
    pub children: Vec<Child>,
}

impl Child {
    pub fn new(task: NewTask) -> Child {
        Child { task, after: Vec::new(), children: Vec::new() }
    }

    pub fn after(mut self, siblings: &[usize]) -> Child {
        self.after = siblings.to_vec();
        self
    }

    /// Gives this (composite) child its own children, enqueued under it in the same transaction.
    pub fn with_children(mut self, children: Vec<Child>) -> Child {
        self.children = children;
        self
    }
}

/// Inserts `children` under `parent` (a child's own `parent` and `scope` win) with `scope` as the
/// default, recursively: the children of a child go under it and inherit its dependencies (a
/// composite's own work cannot start before the composite could). Returns the direct ids; every
/// inserted id is pushed to `all`.
fn insert_children(
    conn: &Connection,
    parent: Option<TaskId>,
    scope: Option<&str>,
    children: &[Child],
    inherited: &[TaskId],
    all: &mut Vec<TaskId>,
) -> anyhow::Result<Vec<TaskId>> {
    let mut ids = Vec::with_capacity(children.len());
    for child in children {
        let status = if child.task.composite { "waiting" } else { "queued" };
        let mut insert = child.task.clone();
        if insert.parent.is_none() {
            insert.parent = parent;
        }
        if insert.scope.is_none() {
            insert.scope = scope.map(str::to_string);
        }
        ids.push(insert_task(conn, &insert, status)?);
    }
    let mut deps: Vec<Vec<TaskId>> = Vec::with_capacity(children.len());
    for (i, child) in children.iter().enumerate() {
        let mut needs = inherited.to_vec();
        needs.extend(child.task.needs.iter().copied());
        for &sibling in &child.after {
            let Some(need) = ids.get(sibling) else { bail!("child {i} waits for sibling {sibling}, which is not in the list") };
            needs.push(*need);
        }
        for need in &needs {
            conn.execute("INSERT OR IGNORE INTO task_deps (task, needs) VALUES (?1, ?2)", params![ids[i], need])?;
        }
        deps.push(needs);
    }
    all.extend(ids.iter().copied());
    for (i, child) in children.iter().enumerate() {
        if child.children.is_empty() {
            continue;
        }
        let nested_scope = child.task.scope.clone().or_else(|| scope.map(str::to_string));
        insert_children(conn, Some(ids[i]), nested_scope.as_deref(), &child.children, &deps[i], all)?;
    }
    Ok(ids)
}

/// Which tasks [`Queue::top_level`] returns: the work of recordings, of chat, or all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum KindGroup {
    #[default]
    All,
    /// Stages of the analysis of recordings.
    Analysis,
    /// Chat answers, their model steps and tools (inline work).
    Chat,
}

/// Top-level tasks of one recording (scope) and/or one kind group.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    pub scope: Option<String>,
    pub group: KindGroup,
}

impl Filter {
    pub fn scope(scope: &str) -> Filter {
        Filter { scope: Some(scope.to_string()), group: KindGroup::All }
    }

    pub fn group(group: KindGroup) -> Filter {
        Filter { scope: None, group }
    }
}

const ANALYSIS_KINDS: &[&str] = &["analysis", "transcribe", "diarize", "index", "topics", "topics-plan", "topic-digest", "topics-merge", "details", "topic-detail"];
const CHAT_KINDS: &[&str] = &["chat", "answer", "model-step", "tool"];

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn json(s: Option<String>) -> Value {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

fn stats(s: Option<String>) -> Stats {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

const COLUMNS: &str = "id, kind, title, input, priority, status, cursor, partial, result, progress, error, attempts, created_ms, updated_ms, \
     parent, scope, inline, started_ms, finished_ms, stats, detail, units";

fn task(r: &Row) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        kind: r.get(1)?,
        title: r.get(2)?,
        input: json(r.get(3)?),
        priority: r.get(4)?,
        status: TaskStatus::from_code(&r.get::<_, String>(5)?),
        cursor: json(r.get(6)?),
        partial: json(r.get(7)?),
        result: r.get::<_, Option<String>>(8)?.map(|s| json(Some(s))),
        progress: r.get(9)?,
        error: r.get(10)?,
        attempts: r.get(11)?,
        created_ms: r.get(12)?,
        updated_ms: r.get(13)?,
        parent: r.get(14)?,
        scope: r.get(15)?,
        inline: r.get(16)?,
        started_ms: r.get(17)?,
        finished_ms: r.get(18)?,
        stats: stats(r.get(19)?),
        detail: json(r.get(20)?),
        units: r.get(21)?,
    })
}

const SCHEMA: i64 = 2;

/// How tasks are listed: what runs now, then what waits, then the rest; queued and waiting by
/// priority, the rest newest first.
const ORDER: &str = "CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 WHEN 'waiting' THEN 2 WHEN 'paused' THEN 3 WHEN 'failed' THEN 4 WHEN 'cancelled' THEN 5 ELSE 6 END,
     CASE WHEN status IN ('queued', 'waiting') THEN -priority ELSE 0 END,
     CASE WHEN status IN ('queued', 'waiting') THEN id ELSE -updated_ms END";

pub struct Queue {
    db: Connection,
    dir: PathBuf,
}

impl Queue {
    /// Opens the queue in `dir` (the library's `.ai` folder). Any number of connections may be
    /// open (the worker, the pages); tasks keep their status. A file of another schema is
    /// recreated empty (the project has no released versions, so there is nothing to migrate).
    pub fn open(dir: &Path) -> anyhow::Result<Queue> {
        std::fs::create_dir_all(dir)?;
        let db = Connection::open(dir.join("queue.db"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA {
            db.execute_batch(
                "DROP TABLE IF EXISTS task_deps;
                 DROP TABLE IF EXISTS kind_stats;
                 DROP TABLE IF EXISTS tasks;
                 CREATE TABLE tasks (
                    id INTEGER PRIMARY KEY,
                    kind TEXT NOT NULL,
                    title TEXT NOT NULL,
                    input TEXT NOT NULL,
                    priority INTEGER NOT NULL DEFAULT 0,
                    status TEXT NOT NULL,
                    cursor TEXT,
                    partial TEXT,
                    result TEXT,
                    progress REAL NOT NULL DEFAULT 0,
                    error TEXT,
                    attempts INTEGER NOT NULL DEFAULT 0,
                    created_ms INTEGER NOT NULL,
                    updated_ms INTEGER NOT NULL,
                    parent INTEGER,
                    scope TEXT,
                    inline INTEGER NOT NULL DEFAULT 0,
                    started_ms INTEGER,
                    finished_ms INTEGER,
                    stats TEXT,
                    detail TEXT,
                    units REAL NOT NULL DEFAULT 0
                );
                CREATE INDEX tasks_parent ON tasks(parent);
                CREATE INDEX tasks_status ON tasks(status);
                CREATE TABLE task_deps (
                    task INTEGER NOT NULL,
                    needs INTEGER NOT NULL,
                    PRIMARY KEY (task, needs)
                );
                CREATE INDEX task_deps_needs ON task_deps(needs);
                CREATE TABLE kind_stats (
                    kind TEXT PRIMARY KEY,
                    runs INTEGER NOT NULL DEFAULT 0,
                    ms INTEGER NOT NULL DEFAULT 0,
                    units REAL NOT NULL DEFAULT 0
                );
                PRAGMA user_version = 2;",
            )?;
        }
        Ok(Queue { db, dir: dir.to_path_buf() })
    }

    /// The folder of the queue file (a library's `.ai` folder).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Adds a task to the queue; returns its id.
    pub fn enqueue(&self, task: NewTask) -> anyhow::Result<TaskId> {
        let tx = self.db.unchecked_transaction()?;
        let status = if task.composite { "waiting" } else { "queued" };
        let id = insert_task(&tx, &task, status)?;
        for need in &task.needs {
            tx.execute("INSERT OR IGNORE INTO task_deps (task, needs) VALUES (?1, ?2)", params![id, need])?;
        }
        tx.commit()?;
        Ok(id)
    }

    /// Adds a task and its children in one transaction: the root is the parent of the children
    /// (unless a child names another parent), and the children may wait for each other by their
    /// position (`Child::after`). A child may bring its own children (a whole subtree). The
    /// whole graph appears at once, so nothing can be claimed before its children exist.
    pub fn enqueue_tree(&self, root: NewTask, children: &[Child]) -> anyhow::Result<(TaskId, Vec<TaskId>)> {
        let tx = self.db.unchecked_transaction()?;
        let status = if root.composite { "waiting" } else { "queued" };
        let id = insert_task(&tx, &root, status)?;
        for need in &root.needs {
            tx.execute("INSERT OR IGNORE INTO task_deps (task, needs) VALUES (?1, ?2)", params![id, need])?;
        }
        let mut all = Vec::new();
        let ids = insert_children(&tx, Some(id), root.scope.as_deref(), children, &root.needs, &mut all)?;
        tx.commit()?;
        Ok((id, ids))
    }

    /// Like [`Queue::enqueue`], unless the same task (kind and input) is already pending
    /// (queued, running, waiting or paused): then its id.
    pub fn enqueue_once(&self, task: NewTask) -> anyhow::Result<TaskId> {
        let existing: Option<TaskId> = self
            .db
            .query_row(
                "SELECT id FROM tasks WHERE kind = ?1 AND input = ?2 AND status IN ('queued', 'running', 'waiting', 'paused') LIMIT 1",
                params![task.kind, task.input.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        match existing {
            Some(id) => Ok(id),
            None => self.enqueue(task),
        }
    }

    pub fn get(&self, id: TaskId) -> anyhow::Result<Option<Task>> {
        Ok(self.db.query_row(&format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"), [id], task).optional()?)
    }

    /// Every task, in the listing order (see [`ORDER`]).
    pub fn list(&self) -> anyhow::Result<Vec<Task>> {
        let mut q = self.db.prepare(&format!("SELECT {COLUMNS} FROM tasks ORDER BY {ORDER}"))?;
        let tasks = q.query_map([], task)?.collect::<rusqlite::Result<_>>()?;
        Ok(tasks)
    }

    /// The top-level tasks of a filter (children are read with [`Queue::children`]).
    pub fn top_level(&self, filter: &Filter) -> anyhow::Result<Vec<Task>> {
        let mut sql = format!("SELECT {COLUMNS} FROM tasks WHERE parent IS NULL");
        match filter.group {
            KindGroup::All => {}
            KindGroup::Analysis => sql.push_str(&format!(" AND kind IN ({})", quoted(ANALYSIS_KINDS))),
            KindGroup::Chat => sql.push_str(&format!(" AND (inline = 1 OR kind IN ({}))", quoted(CHAT_KINDS))),
        }
        if filter.scope.is_some() {
            sql.push_str(" AND scope = ?1");
        }
        sql.push_str(&format!(" ORDER BY {ORDER}"));
        let mut q = self.db.prepare(&sql)?;
        let rows = match &filter.scope {
            Some(scope) => q.query_map([scope.as_str()], task)?.collect::<rusqlite::Result<_>>()?,
            None => q.query_map([], task)?.collect::<rusqlite::Result<_>>()?,
        };
        Ok(rows)
    }

    /// The direct children of a composite task.
    pub fn children(&self, id: TaskId) -> anyhow::Result<Vec<Task>> {
        let mut q = self.db.prepare(&format!("SELECT {COLUMNS} FROM tasks WHERE parent = ?1 ORDER BY id"))?;
        let tasks = q.query_map([id], task)?.collect::<rusqlite::Result<_>>()?;
        Ok(tasks)
    }

    /// `task` and everything below it.
    pub fn subtree(&self, id: TaskId) -> anyhow::Result<Vec<Task>> {
        Ok(subtree_at(&self.db, id)?)
    }

    /// Takes the next queued task of one of `kinds` and marks it running. Only tasks whose
    /// dependencies are all `done` are taken; inline and waiting (composite) tasks never are.
    pub fn claim(&self, kinds: &[&str]) -> anyhow::Result<Option<Task>> {
        if kinds.is_empty() {
            return Ok(None);
        }
        let tx = self.db.unchecked_transaction()?;
        let marks = vec!["?"; kinds.len()].join(",");
        let id: Option<TaskId> = tx
            .query_row(
                &format!(
                    "SELECT id FROM tasks t WHERE status = 'queued' AND inline = 0 AND kind IN ({marks})
                     AND NOT EXISTS (SELECT 1 FROM task_deps d JOIN tasks n ON n.id = d.needs WHERE d.task = t.id AND n.status != 'done')
                     ORDER BY priority DESC, id LIMIT 1"
                ),
                rusqlite::params_from_iter(kinds),
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        let now = now_ms();
        tx.execute(
            "UPDATE tasks SET status = 'running', attempts = attempts + 1, error = NULL, finished_ms = NULL,
                    started_ms = COALESCE(started_ms, ?2), updated_ms = ?2 WHERE id = ?1",
            params![id, now],
        )?;
        refresh_up(&tx, id, now)?;
        tx.commit()?;
        self.get(id)
    }

    /// Saves where a running task is and what it has so far (one transaction). Returns the
    /// task's status: anything but `Running` means it was paused or cancelled meanwhile and
    /// should stop.
    pub fn checkpoint(&self, id: TaskId, cursor: &Value, partial: &Value, progress: f32) -> anyhow::Result<TaskStatus> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        tx.execute(
            "UPDATE tasks SET cursor = ?2, partial = ?3, progress = ?4, updated_ms = ?5 WHERE id = ?1 AND status = 'running'",
            params![id, cursor.to_string(), partial.to_string(), progress.clamp(0.0, 1.0), now],
        )?;
        let status: Option<String> = tx.query_row("SELECT status FROM tasks WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        if status.as_deref() == Some("running") {
            refresh_up(&tx, id, now)?;
        }
        tx.commit()?;
        // A removed task reads as paused: the worker just stops.
        Ok(status.map_or(TaskStatus::Paused, |s| TaskStatus::from_code(&s)))
    }

    /// Completes the task and enqueues its children in one transaction. Children go under the
    /// task's parent when it has one (the composite they all belong to), else under the task
    /// itself; a child's own `parent` overrides that, and its scope defaults to the task's. A
    /// child may bring its own children (a whole subtree). Deps among the children are given by
    /// their position in `children`, deps on existing tasks by `NewTask::needs`.
    pub fn complete_with(&self, id: TaskId, result: &Value, children: &[Child]) -> anyhow::Result<Vec<TaskId>> {
        let tx = self.db.unchecked_transaction()?;
        let Some(task) = task_at(&tx, id)? else { bail!("task {id} is not in the queue") };
        let now = now_ms();
        let default_parent = task.parent.or(Some(task.id));
        if !has_children(&tx, id)? {
            add_kind_run(&tx, &task, now)?;
        }
        tx.execute(
            "UPDATE tasks SET status = 'done', result = ?2, progress = 1, error = NULL, finished_ms = ?3, updated_ms = ?3 WHERE id = ?1",
            params![id, result.to_string(), now],
        )?;
        let mut all = Vec::new();
        let ids = insert_children(&tx, default_parent, task.scope.as_deref(), children, &[], &mut all)?;
        refresh_up(&tx, id, now)?;
        for &child in &all {
            refresh_up(&tx, child, now)?;
        }
        tx.commit()?;
        Ok(ids)
    }

    pub fn complete(&self, id: TaskId, result: &Value) -> anyhow::Result<()> {
        self.complete_with(id, result, &[])?;
        Ok(())
    }

    /// The task stopped with an error; its checkpoint stays for a retry.
    pub fn fail(&self, id: TaskId, error: &str) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        tx.execute(
            "UPDATE tasks SET status = 'failed', error = ?2, finished_ms = ?3, updated_ms = ?3 WHERE id = ?1",
            params![id, error, now],
        )?;
        refresh_up(&tx, id, now)?;
        tx.commit()?;
        Ok(())
    }

    /// A running task goes back to the queue untouched (its worker is stopping).
    pub fn release(&self, id: TaskId) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        tx.execute("UPDATE tasks SET status = 'queued', updated_ms = ?2 WHERE id = ?1 AND status = 'running'", params![id, now])?;
        refresh_up(&tx, id, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Stops a task (a running one stops at its next checkpoint); a composite stops its whole
    /// subtree.
    pub fn pause(&self, id: TaskId) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        tx.execute(
            "WITH RECURSIVE sub(id) AS (SELECT id FROM tasks WHERE id = ?1 UNION ALL SELECT t.id FROM tasks t JOIN sub ON t.parent = sub.id)
             UPDATE tasks SET status = 'paused', updated_ms = ?2
             WHERE id IN (SELECT id FROM sub) AND status IN ('queued', 'running', 'waiting')",
            params![id, now],
        )?;
        refresh_up(&tx, id, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Back into the queue (paused or failed tasks), continuing from the checkpoint; a composite
    /// resumes its whole subtree and goes back to `waiting` until a child runs.
    pub fn resume(&self, id: TaskId) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        tx.execute(
            "WITH RECURSIVE sub(id) AS (SELECT id FROM tasks WHERE id = ?1 UNION ALL SELECT t.id FROM tasks t JOIN sub ON t.parent = sub.id)
             UPDATE tasks SET status = CASE WHEN EXISTS (SELECT 1 FROM tasks c WHERE c.parent = tasks.id) THEN 'waiting' ELSE 'queued' END,
                    error = NULL, finished_ms = NULL, updated_ms = ?2
             WHERE id IN (SELECT id FROM sub) AND status IN ('paused', 'failed')",
            params![id, now],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Starts over: drops the checkpoint and queues the task again; a composite starts over with
    /// its whole subtree (a task that is running now is left alone).
    pub fn restart(&self, id: TaskId) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        tx.execute(
            "WITH RECURSIVE sub(id) AS (SELECT id FROM tasks WHERE id = ?1 UNION ALL SELECT t.id FROM tasks t JOIN sub ON t.parent = sub.id)
             UPDATE tasks SET status = CASE WHEN EXISTS (SELECT 1 FROM tasks c WHERE c.parent = tasks.id) THEN 'waiting' ELSE 'queued' END,
                    cursor = NULL, partial = NULL, result = NULL, progress = 0, error = NULL,
                    started_ms = NULL, finished_ms = NULL, updated_ms = ?2
             WHERE id IN (SELECT id FROM sub) AND status != 'running'",
            params![id, now],
        )?;
        refresh_up(&tx, id, now)?;
        tx.commit()?;
        Ok(())
    }

    /// Cancels a task and its whole subtree; a running task stops at its next checkpoint.
    pub fn cancel(&self, id: TaskId) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "WITH RECURSIVE sub(id) AS (SELECT id FROM tasks WHERE id = ?1 UNION ALL SELECT t.id FROM tasks t JOIN sub ON t.parent = sub.id)
             UPDATE tasks SET status = 'cancelled', finished_ms = ?2, updated_ms = ?2
             WHERE id IN (SELECT id FROM sub) AND status IN ('queued', 'running', 'waiting', 'paused')",
            params![id, now_ms()],
        )?;
        refresh_up(&tx, id, now_ms())?;
        tx.commit()?;
        Ok(())
    }

    /// Cancels every unfinished task of one recording (or folder); returns how many.
    pub fn cancel_scope(&self, scope: &str) -> anyhow::Result<usize> {
        Ok(self.db.execute(
            "UPDATE tasks SET status = 'cancelled', finished_ms = ?2, updated_ms = ?2
             WHERE scope = ?1 AND status IN ('queued', 'running', 'waiting', 'paused')",
            params![scope, now_ms()],
        )?)
    }

    /// Cancels the analysis of one recording only — the top-level `analysis` composites of the
    /// scope with their whole subtrees — and leaves the rest of the scope's work alone (a chat
    /// answer about the same recording keeps running); returns how many.
    pub fn cancel_analysis(&self, scope: &str) -> anyhow::Result<usize> {
        Ok(self.db.execute(
            "WITH RECURSIVE sub(id) AS (
                 SELECT id FROM tasks WHERE scope = ?1 AND kind = 'analysis' AND parent IS NULL
                 UNION
                 SELECT t.id FROM tasks t JOIN sub ON t.parent = sub.id
             )
             UPDATE tasks SET status = 'cancelled', finished_ms = ?2, updated_ms = ?2
             WHERE id IN (SELECT id FROM sub) AND status IN ('queued', 'running', 'waiting', 'paused')",
            params![scope, now_ms()],
        )?)
    }

    /// Sets a task's priority (higher is taken first).
    pub fn set_priority(&self, id: TaskId, priority: i64) -> anyhow::Result<()> {
        self.db.execute("UPDATE tasks SET priority = ?2, updated_ms = ?3 WHERE id = ?1", params![id, priority, now_ms()])?;
        Ok(())
    }

    /// A task the caller runs itself (a chat answer, a model step, a tool): created running.
    pub fn begin_inline(&self, kind: &str, title: &str, input: &Value, parent: Option<TaskId>, scope: Option<&str>) -> anyhow::Result<TaskId> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        tx.execute(
            "INSERT INTO tasks (kind, title, input, status, attempts, created_ms, updated_ms, parent, scope, inline, started_ms)
             VALUES (?1, ?2, ?3, 'running', 1, ?4, ?4, ?5, ?6, 1, ?4)",
            params![kind, title, input.to_string(), now, parent, scope],
        )?;
        let id = tx.last_insert_rowid();
        refresh_up(&tx, id, now)?;
        tx.commit()?;
        Ok(id)
    }

    /// An inline task is done (same as [`Queue::complete`]).
    pub fn finish_inline(&self, id: TaskId, result: &Value) -> anyhow::Result<()> {
        self.complete(id, result)
    }

    /// An inline task failed (same as [`Queue::fail`]).
    pub fn fail_inline(&self, id: TaskId, error: &str) -> anyhow::Result<()> {
        self.fail(id, error)
    }

    /// An inline task was cancelled (same as [`Queue::cancel`]).
    pub fn cancel_inline(&self, id: TaskId) -> anyhow::Result<()> {
        self.cancel(id)
    }

    /// Adds what the task's model calls used (one model call, its tokens and time, its model).
    pub fn add_stats(&self, id: TaskId, delta: &Stats) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let current: Option<Option<String>> = tx.query_row("SELECT stats FROM tasks WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        let Some(current) = current else { return Ok(()) };
        let mut all = stats(current);
        all.add(delta);
        tx.execute(
            "UPDATE tasks SET stats = ?2, updated_ms = ?3 WHERE id = ?1",
            params![id, serde_json::to_string(&all)?, now_ms()],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The row of a tool in the queue: what it was called with and how much it returned.
    pub fn set_detail(&self, id: TaskId, detail: &Value) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE tasks SET detail = ?2, updated_ms = ?3 WHERE id = ?1",
            params![id, detail.to_string(), now_ms()],
        )?;
        Ok(())
    }

    /// Removes a task with its whole subtree.
    pub fn remove(&self, id: TaskId) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        let parent = task_at(&tx, id)?.and_then(|t| t.parent);
        tx.execute("WITH RECURSIVE sub(id) AS (SELECT id FROM tasks WHERE id = ?1 UNION ALL SELECT t.id FROM tasks t JOIN sub ON t.parent = sub.id) DELETE FROM tasks WHERE id IN (SELECT id FROM sub)", [id])?;
        tx.execute("DELETE FROM task_deps WHERE task NOT IN (SELECT id FROM tasks) OR needs NOT IN (SELECT id FROM tasks)", [])?;
        if let Some(parent) = parent {
            refresh(&tx, parent, now_ms())?;
            refresh_up(&tx, parent, now_ms())?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Removes the finished work: every top-level `done` task with its subtree (a child that is
    /// done under a task still going stays: it counts in «7 of 12»).
    pub fn clear_done(&self) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "WITH RECURSIVE doomed(id) AS (
                SELECT id FROM tasks WHERE parent IS NULL AND status = 'done'
                UNION ALL
                SELECT t.id FROM tasks t JOIN doomed d ON t.parent = d.id
             )
             DELETE FROM tasks WHERE id IN (SELECT id FROM doomed)",
            [],
        )?;
        tx.execute("DELETE FROM task_deps WHERE task NOT IN (SELECT id FROM tasks) OR needs NOT IN (SELECT id FROM tasks)", [])?;
        tx.commit()?;
        Ok(())
    }

    /// Removes `done`, `failed` and `cancelled` top-level tasks finished longer than `age` ago,
    /// with their subtrees and dependencies. `kind_stats` stays: it is the history of the work.
    /// Returns how many tasks were removed.
    pub fn purge_older_than(&self, age: Duration) -> anyhow::Result<usize> {
        let cutoff = now_ms() - age.as_millis() as i64;
        let tx = self.db.unchecked_transaction()?;
        let removed = tx.execute(
            "WITH RECURSIVE doomed(id) AS (
                SELECT id FROM tasks
                 WHERE parent IS NULL AND status IN ('done', 'failed', 'cancelled')
                   AND COALESCE(finished_ms, updated_ms) < ?1
                UNION ALL
                SELECT t.id FROM tasks t JOIN doomed d ON t.parent = d.id
             )
             DELETE FROM tasks WHERE id IN (SELECT id FROM doomed)",
            [cutoff],
        )?;
        tx.execute("DELETE FROM task_deps WHERE task NOT IN (SELECT id FROM tasks) OR needs NOT IN (SELECT id FROM tasks)", [])?;
        tx.commit()?;
        Ok(removed)
    }

    /// Tasks left running by a previous run of the worker (the program was closed or crashed)
    /// go back to the queue, keeping their checkpoint; inline tasks (a chat answer that the
    /// program was running) are cancelled instead — nobody can continue them. Returns how many.
    /// Only the worker calls it, when it starts.
    pub fn requeue_running(&self) -> anyhow::Result<usize> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        let running = tx.execute("UPDATE tasks SET status = 'queued', updated_ms = ?1 WHERE status = 'running' AND inline = 0 AND NOT EXISTS (SELECT 1 FROM tasks c WHERE c.parent = tasks.id)", [now])?;
        let inline = tx.execute(
            "UPDATE tasks SET status = 'cancelled', finished_ms = ?1, updated_ms = ?1 WHERE status = 'running' AND inline = 1",
            [now],
        )?;
        refresh_all(&tx, now)?;
        tx.commit()?;
        Ok(running + inline)
    }

    /// Running tasks without a checkpoint for `timeout` (their worker hung or died) go back
    /// to the queue; returns how many.
    pub fn requeue_stale(&self, timeout: Duration) -> anyhow::Result<usize> {
        let tx = self.db.unchecked_transaction()?;
        let now = now_ms();
        let changed = tx.execute(
            "UPDATE tasks SET status = 'queued', updated_ms = ?1 WHERE status = 'running' AND inline = 0 AND updated_ms < ?2 AND NOT EXISTS (SELECT 1 FROM tasks c WHERE c.parent = tasks.id)",
            params![now, now - timeout.as_millis() as i64],
        )?;
        refresh_all(&tx, now)?;
        tx.commit()?;
        Ok(changed)
    }

    /// Time the task has been going (up to `now`, or its finish).
    pub fn elapsed_of(&self, id: TaskId, now: i64) -> anyhow::Result<Duration> {
        Ok(self.get(id)?.map_or(Duration::ZERO, |t| elapsed(&t, now)))
    }

    /// Estimated time left for the task, children included (see [`estimate`]).
    pub fn estimate_of(&self, id: TaskId, now: i64) -> anyhow::Result<Option<Duration>> {
        let Some(task) = self.get(id)? else { return Ok(None) };
        let tree = subtree_at(&self.db, id)?;
        let stats = kind_stats(&self.db)?;
        Ok(estimate_tree(&task, &tree, now, &stats))
    }

    /// The history of run times by kind of work.
    pub fn kind_stats(&self) -> anyhow::Result<Vec<KindStat>> {
        Ok(kind_stats(&self.db)?)
    }
}

/// Time the task has been going: from its first start to `now` (or to its finish).
pub fn elapsed(task: &Task, now: i64) -> Duration {
    let Some(started) = task.started_ms else { return Duration::ZERO };
    let end = task.finished_ms.unwrap_or(now);
    Duration::from_millis((end - started).max(0) as u64)
}

/// Estimated time left for one task (no children involved): by its progress rate when it is
/// running with `p ≥ 5 %` and 10 s behind it, else by the history (`ms per unit × units ×
/// (1 − p)`), else there is no estimate. `Done` is zero left; `failed` and `cancelled` are
/// unknown.
pub fn estimate(task: &Task, now: i64, kind_stats: &[KindStat]) -> Option<Duration> {
    match task.status {
        TaskStatus::Done => return Some(Duration::ZERO),
        TaskStatus::Failed | TaskStatus::Cancelled => return None,
        _ => {}
    }
    let p = task.progress.clamp(0.0, 1.0) as f64;
    if p >= 0.05 && task.started_ms.is_some() {
        let gone = elapsed(task, now);
        if gone >= Duration::from_secs(10) {
            return Some(Duration::from_secs_f64(gone.as_secs_f64() * (1.0 - p) / p));
        }
    }
    let per_unit = kind_stats.iter().find(|s| s.kind == task.kind && s.units > 0.0).map(|s| s.ms as f64 / s.units)?;
    if task.units <= 0.0 {
        return None;
    }
    Some(Duration::from_millis((per_unit * task.units * (1.0 - p)).max(0.0) as u64))
}

/// Estimated time left for `task`, composites included; `tasks` is a flat list containing its
/// descendants ([`Queue::subtree`]). A composite is the sum of its unfinished children (children
/// without an estimate are skipped; `None` when none has one).
pub fn estimate_tree(task: &Task, tasks: &[Task], now: i64, kind_stats: &[KindStat]) -> Option<Duration> {
    let children: Vec<&Task> = tasks.iter().filter(|t| t.parent == Some(task.id)).collect();
    if children.is_empty() {
        return estimate(task, now, kind_stats);
    }
    if children.iter().all(|c| c.status == TaskStatus::Done) {
        return Some(Duration::ZERO);
    }
    let mut total = Duration::ZERO;
    let mut any = false;
    for child in children.into_iter().filter(|c| c.status != TaskStatus::Done) {
        if let Some(left) = estimate_tree(child, tasks, now, kind_stats) {
            total += left;
            any = true;
        }
    }
    any.then_some(total)
}

/// The expected total time of a child: what is left plus what it has already taken. Used as its
/// weight in a composite's progress.
fn weight(task: &Task, tasks: &[Task], now: i64, stats: &[KindStat]) -> Option<f64> {
    let left = estimate_tree(task, tasks, now, stats)?.as_secs_f64();
    Some(left + elapsed(task, now).as_secs_f64())
}

fn insert_task(conn: &Connection, task: &NewTask, status: &str) -> rusqlite::Result<TaskId> {
    let now = now_ms();
    conn.execute(
        "INSERT INTO tasks (kind, title, input, priority, status, cursor, created_ms, updated_ms, parent, scope, inline, units)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?9, ?10, ?11)",
        params![
            task.kind,
            task.title,
            task.input.to_string(),
            task.priority,
            status,
            task.cursor.as_ref().map(|c| c.to_string()),
            now,
            task.parent,
            task.scope,
            task.inline,
            task.units
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn quoted(kinds: &[&str]) -> String {
    kinds.iter().map(|k| format!("'{k}'")).collect::<Vec<_>>().join(",")
}

fn task_at(conn: &Connection, id: TaskId) -> rusqlite::Result<Option<Task>> {
    conn.query_row(&format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"), [id], task).optional()
}

fn has_children(conn: &Connection, id: TaskId) -> rusqlite::Result<bool> {
    conn.query_row("SELECT EXISTS (SELECT 1 FROM tasks WHERE parent = ?1)", [id], |r| r.get(0))
}

fn subtree_at(conn: &Connection, id: TaskId) -> rusqlite::Result<Vec<Task>> {
    let mut q = conn.prepare(&format!(
        "WITH RECURSIVE sub(id) AS (SELECT id FROM tasks WHERE id = ?1 UNION ALL SELECT t.id FROM tasks t JOIN sub ON t.parent = sub.id)
         SELECT {COLUMNS} FROM tasks WHERE id IN (SELECT id FROM sub)"
    ))?;
    let tasks = q.query_map([id], task)?.collect::<rusqlite::Result<_>>()?;
    Ok(tasks)
}

fn kind_stats(conn: &Connection) -> rusqlite::Result<Vec<KindStat>> {
    let mut q = conn.prepare("SELECT kind, runs, ms, units FROM kind_stats")?;
    let stats = q
        .query_map([], |r| Ok(KindStat { kind: r.get(0)?, runs: r.get(1)?, ms: r.get(2)?, units: r.get(3)? }))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(stats)
}

/// On `done` of a non-composite task that knows its size: remember its run time and units.
fn add_kind_run(conn: &Connection, task: &Task, now: i64) -> rusqlite::Result<()> {
    if task.units <= 0.0 {
        return Ok(());
    }
    let ms = task.started_ms.map_or(0, |started| (now - started).max(0));
    conn.execute(
        "INSERT INTO kind_stats (kind, runs, ms, units) VALUES (?1, 1, ?2, ?3)
         ON CONFLICT(kind) DO UPDATE SET runs = runs + 1, ms = ms + excluded.ms, units = units + excluded.units",
        params![task.kind, ms, task.units],
    )?;
    Ok(())
}

/// Brings one composite in step with its children: `running` while a child runs, `failed` when a
/// child failed (with its error), `done` when all are done, `waiting` otherwise. Its
/// progress is the children's weighted mean (equal weights when there is nothing to estimate).
fn refresh(conn: &Connection, parent: TaskId, now: i64) -> rusqlite::Result<()> {
    let Some(parent_task) = task_at(conn, parent)? else { return Ok(()) };
    if !matches!(parent_task.status, TaskStatus::Waiting | TaskStatus::Running | TaskStatus::Queued) {
        return Ok(());
    }
    let children: Vec<Task> = {
        let mut q = conn.prepare(&format!("SELECT {COLUMNS} FROM tasks WHERE parent = ?1 ORDER BY id"))?;
        let rows = q.query_map([parent], task)?.collect::<rusqlite::Result<_>>()?;
        rows
    };
    if children.is_empty() {
        return Ok(());
    }
    // An inline task (a chat answer) is ended by its caller only: between its steps all its
    // children are done while it still runs.
    if parent_task.inline {
        return Ok(());
    }
    let failed = children.iter().find(|c| c.status == TaskStatus::Failed);
    let running = children.iter().any(|c| c.status == TaskStatus::Running);
    let all_done = children.iter().all(|c| c.status == TaskStatus::Done);
    let (status, error, finished) = if let Some(child) = failed {
        (TaskStatus::Failed, Some(child.error.clone().unwrap_or_else(|| "failed".into())), Some(now))
    } else if running {
        (TaskStatus::Running, None, None)
    } else if all_done {
        (TaskStatus::Done, None, Some(now))
    } else {
        (TaskStatus::Waiting, None, None)
    };
    let progress = if all_done {
        1.0
    } else {
        let tree = subtree_at(conn, parent)?;
        let stats = kind_stats(conn)?;
        let weights: Vec<Option<f64>> = children.iter().map(|c| weight(c, &tree, now, &stats)).collect();
        let weighted = if weights.iter().all(Option::is_some) {
            let total: f64 = weights.iter().flatten().sum();
            if total > 0.0 {
                children.iter().zip(&weights).map(|(c, w)| f64::from(c.progress) * w.unwrap_or(0.0)).sum::<f64>() / total
            } else {
                children.iter().map(|c| f64::from(c.progress)).sum::<f64>() / children.len() as f64
            }
        } else {
            children.iter().map(|c| f64::from(c.progress)).sum::<f64>() / children.len() as f64
        };
        weighted as f32
    };
    let started = parent_task.started_ms.or_else(|| (status == TaskStatus::Running).then_some(now));
    conn.execute(
        "UPDATE tasks SET status = ?2, error = ?3, progress = ?4, started_ms = ?5, finished_ms = ?6, updated_ms = ?7 WHERE id = ?1",
        params![parent, status_code(status), error, progress.clamp(0.0, 1.0), started, finished, now],
    )?;
    Ok(())
}

fn status_code(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Queued => "queued",
        TaskStatus::Running => "running",
        TaskStatus::Waiting => "waiting",
        TaskStatus::Paused => "paused",
        TaskStatus::Done => "done",
        TaskStatus::Failed => "failed",
        TaskStatus::Cancelled => "cancelled",
    }
}

/// Refreshes the composite tasks above `id`, nearest first.
fn refresh_up(conn: &Connection, id: TaskId, now: i64) -> rusqlite::Result<()> {
    let mut parent = task_at(conn, id)?.and_then(|t| t.parent);
    while let Some(id) = parent {
        refresh(conn, id, now)?;
        parent = task_at(conn, id)?.and_then(|t| t.parent);
    }
    Ok(())
}

/// Refreshes every composite task, deepest first (children are created after their parent, so
/// their ids are higher).
fn refresh_all(conn: &Connection, now: i64) -> rusqlite::Result<()> {
    let parents: Vec<TaskId> = {
        let mut q = conn.prepare("SELECT DISTINCT parent FROM tasks WHERE parent IS NOT NULL ORDER BY parent DESC")?;
        let ids = q.query_map([], |r| r.get(0))?.collect::<rusqlite::Result<_>>()?;
        ids
    };
    for parent in parents {
        refresh(conn, parent, now)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use serde_json::json;

    fn plain(kind: &str, title: &str) -> NewTask {
        NewTask::new(kind, title, &json!({}))
    }

    fn status(q: &Queue, id: TaskId) -> TaskStatus {
        q.get(id).unwrap().unwrap().status
    }

    #[test]
    fn the_same_task_is_queued_once_while_it_is_pending() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let a = q.enqueue_once(NewTask::new("index", "a", &json!({"path": "/a"}))).unwrap();
        assert_eq!(q.enqueue_once(NewTask::new("index", "a", &json!({"path": "/a"}))).unwrap(), a);
        assert_ne!(q.enqueue_once(NewTask::new("index", "b", &json!({"path": "/b"}))).unwrap(), a);
        assert_ne!(q.enqueue_once(NewTask::new("outline", "a", &json!({"path": "/a"}))).unwrap(), a);
        q.claim(&["index"]).unwrap();
        q.complete(a, &json!({})).unwrap();
        assert_ne!(q.enqueue_once(NewTask::new("index", "a", &json!({"path": "/a"}))).unwrap(), a, "again once done");
    }

    #[test]
    fn priority_order_and_claiming() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let low = q.enqueue(plain("transcribe", "low").units(0.0)).unwrap();
        let high = q.enqueue(plain("transcribe", "high").priority(5)).unwrap();
        let other = q.enqueue(plain("embed", "other kind").priority(9)).unwrap();
        assert!(q.claim(&[]).unwrap().is_none());
        let t = q.claim(&["transcribe"]).unwrap().unwrap();
        assert_eq!((t.id, t.status, t.attempts), (high, TaskStatus::Running, 1));
        assert!(t.started_ms.is_some(), "the first start is remembered");
        assert_eq!(q.claim(&["transcribe"]).unwrap().unwrap().id, low);
        assert!(q.claim(&["transcribe"]).unwrap().is_none(), "nothing queued of that kind");
        assert_eq!(status(&q, other), TaskStatus::Queued);
        assert_eq!(q.get(low).unwrap().unwrap().input, json!({}));
        assert_eq!(q.list().unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [low, high, other], "running first");
    }

    #[test]
    fn a_task_can_start_from_a_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let cursor = json!({"stage": "merge", "topic": 0});
        let id = q.enqueue(plain("outline", "t").cursor(&cursor)).unwrap();
        let t = q.claim(&["outline"]).unwrap().unwrap();
        assert_eq!((t.id, t.cursor), (id, cursor));
    }

    #[test]
    fn checkpoints_survive_a_restart_of_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let q = Queue::open(dir.path()).unwrap();
            let id = q.enqueue(plain("transcribe", "t")).unwrap();
            q.claim(&["transcribe"]).unwrap();
            assert_eq!(q.checkpoint(id, &json!({"at": 3}), &json!(["a", "b", "c"]), 0.3).unwrap(), TaskStatus::Running);
            id
            // Dropped while running: the program "crashed".
        };
        let q = Queue::open(dir.path()).unwrap();
        assert_eq!(status(&q, id), TaskStatus::Running, "opening (e.g. by a page) changes nothing");
        assert_eq!(q.requeue_running().unwrap(), 1, "the worker starting");
        let t = q.get(id).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Queued, "back in the queue");
        assert_eq!((t.cursor, t.partial, t.progress), (json!({"at": 3}), json!(["a", "b", "c"]), 0.3));
        let again = q.claim(&["transcribe"]).unwrap().unwrap();
        assert_eq!((again.cursor, again.attempts), (json!({"at": 3}), 2), "continues where it stopped");
        assert!(again.started_ms.is_some(), "the first start stays");
    }

    #[test]
    fn pause_resume_fail_restart_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue(plain("k", "t")).unwrap();
        q.claim(&["k"]).unwrap();
        q.pause(id).unwrap();
        assert_eq!(q.checkpoint(id, &json!(1), &json!([]), 0.1).unwrap(), TaskStatus::Paused, "the worker learns to stop");
        assert_eq!(q.get(id).unwrap().unwrap().cursor, Value::Null, "a paused task's checkpoint is not overwritten");
        q.resume(id).unwrap();
        q.claim(&["k"]).unwrap();
        q.checkpoint(id, &json!(2), &json!(["x"]), 0.5).unwrap();
        q.fail(id, "model crashed").unwrap();
        let t = q.get(id).unwrap().unwrap();
        assert_eq!((t.status, t.error.as_deref(), t.cursor.clone()), (TaskStatus::Failed, Some("model crashed"), json!(2)));
        assert!(t.finished_ms.is_some());
        q.resume(id).unwrap();
        assert_eq!(q.get(id).unwrap().unwrap().error, None);
        q.restart(id).unwrap();
        let t = q.get(id).unwrap().unwrap();
        assert_eq!((t.cursor, t.progress, t.started_ms, t.finished_ms), (Value::Null, 0.0, None, None), "starts over");
        q.claim(&["k"]).unwrap();
        q.complete(id, &json!({"text": "done"})).unwrap();
        let t = q.get(id).unwrap().unwrap();
        assert_eq!(t.result, Some(json!({"text": "done"})));
        assert_eq!(t.progress, 1.0);
        assert!(t.finished_ms.is_some());
        q.clear_done().unwrap();
        assert!(q.get(id).unwrap().is_none());
        let gone = q.enqueue(plain("k", "t")).unwrap();
        q.remove(gone).unwrap();
        assert_eq!(q.checkpoint(gone, &json!(1), &json!(1), 1.0).unwrap(), TaskStatus::Paused, "removed: stop");
    }

    #[test]
    fn stale_running_tasks_are_requeued() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue(plain("k", "t")).unwrap();
        q.claim(&["k"]).unwrap();
        assert_eq!(q.requeue_stale(Duration::from_secs(60)).unwrap(), 0, "fresh");
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(q.requeue_stale(Duration::from_millis(10)).unwrap(), 1);
        assert_eq!(status(&q, id), TaskStatus::Queued);
    }

    #[test]
    fn claim_waits_for_all_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let a = q.enqueue(plain("step", "a")).unwrap();
        let b = q.enqueue(plain("step", "b").needs(&[a])).unwrap();
        let c = q.enqueue(plain("step", "c").needs(&[a, b])).unwrap();
        assert_eq!(q.claim(&["step"]).unwrap().unwrap().id, a, "only a is free");
        assert!(q.claim(&["step"]).unwrap().is_none(), "b waits for a");
        q.complete(a, &json!({})).unwrap();
        assert_eq!(q.claim(&["step"]).unwrap().unwrap().id, b);
        q.complete(b, &json!({})).unwrap();
        assert_eq!(q.claim(&["step"]).unwrap().unwrap().id, c);
    }

    #[test]
    fn complete_with_adds_children_and_the_composite_does_not_finish_early() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let composite = q.enqueue(plain("analysis", "analysis").composite()).unwrap();
        assert_eq!(status(&q, composite), TaskStatus::Waiting, "a composite is created waiting");
        let first = q.enqueue(plain("step", "first").parent(composite)).unwrap();
        assert!(q.claim(&["step"]).unwrap().is_some());
        let ids = q
            .complete_with(
                first,
                &json!({"planned": 2}),
                &[Child::new(plain("step", "second")), Child::new(plain("step", "third")).after(&[0])],
            )
            .unwrap();
        assert_eq!(ids.len(), 2);
        let children = q.children(composite).unwrap();
        assert_eq!(children.len(), 3, "children of a child join the composite it belongs to");
        assert_eq!(children.iter().map(|t| t.id).collect::<Vec<_>>(), [first, ids[0], ids[1]]);
        assert_ne!(status(&q, composite), TaskStatus::Done, "a child was added in the same transaction: not done yet");
        assert_eq!(q.claim(&["step"]).unwrap().unwrap().id, ids[0], "second has no deps");
        q.complete(ids[0], &json!({})).unwrap();
        assert_eq!(q.claim(&["step"]).unwrap().unwrap().id, ids[1], "third waited for second");
        q.complete(ids[1], &json!({})).unwrap();
        assert_eq!(status(&q, composite), TaskStatus::Done, "all children are done");
    }

    #[test]
    fn composite_status_and_progress_follow_the_children() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let parent = q.enqueue(plain("group", "group").composite()).unwrap();
        let a = q.enqueue(plain("k", "a").parent(parent).units(10.0)).unwrap();
        let b = q.enqueue(plain("k", "b").parent(parent).units(10.0)).unwrap();
        let claim_a = q.claim(&["k"]).unwrap().unwrap();
        assert_eq!((claim_a.id, status(&q, parent)), (a, TaskStatus::Running), "a running child makes the composite running");
        let t = q.get(parent).unwrap().unwrap();
        assert!(t.started_ms.is_some(), "the composite remembers when it started");

        q.checkpoint(a, &json!(1), &json!([]), 0.5).unwrap();
        let t = q.get(parent).unwrap().unwrap();
        assert!((t.progress - 0.25).abs() < 0.001, "half of one of two equal children: {}", t.progress);

        q.complete(a, &json!({})).unwrap();
        assert_eq!(status(&q, parent), TaskStatus::Waiting, "one child done, one queued: waiting");
        let t = q.get(parent).unwrap().unwrap();
        assert!((t.progress - 0.5).abs() < 0.001, "{}", t.progress);

        q.claim(&["k"]).unwrap();
        q.fail(b, "model crashed").unwrap();
        let t = q.get(parent).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
        assert_eq!(t.error.as_deref(), Some("model crashed"));
        assert!(t.finished_ms.is_some());
    }

    #[test]
    fn a_composite_finishes_when_all_children_are_done() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let parent = q.enqueue(plain("group", "group").composite()).unwrap();
        let a = q.enqueue(plain("k", "a").parent(parent)).unwrap();
        let b = q.enqueue(plain("k", "b").parent(parent)).unwrap();
        for id in [a, b] {
            q.claim(&["k"]).unwrap();
            q.complete(id, &json!({})).unwrap();
        }
        let t = q.get(parent).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Done);
        assert_eq!(t.progress, 1.0);
        assert!(t.finished_ms.is_some());
    }

    #[test]
    fn pause_and_cancel_cascade_over_the_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let top = q.enqueue(plain("group", "top").composite()).unwrap();
        let middle = q.enqueue(plain("group", "middle").parent(top).composite()).unwrap();
        let leaf = q.enqueue(plain("k", "leaf").parent(middle)).unwrap();
        q.claim(&["k"]).unwrap();
        assert_eq!(status(&q, top), TaskStatus::Running);
        assert_eq!(status(&q, middle), TaskStatus::Running);

        q.pause(top).unwrap();
        for id in [top, middle, leaf] {
            assert_eq!(status(&q, id), TaskStatus::Paused, "task {id}");
        }
        assert_eq!(q.checkpoint(leaf, &json!(1), &json!([]), 0.5).unwrap(), TaskStatus::Paused, "the worker stops");

        q.resume(top).unwrap();
        assert_eq!(status(&q, top), TaskStatus::Waiting);
        assert_eq!(status(&q, middle), TaskStatus::Waiting);
        assert_eq!(status(&q, leaf), TaskStatus::Queued);

        q.claim(&["k"]).unwrap();
        q.cancel(top).unwrap();
        for id in [top, middle, leaf] {
            let t = q.get(id).unwrap().unwrap();
            assert_eq!(t.status, TaskStatus::Cancelled, "task {id}");
            assert!(t.finished_ms.is_some());
        }
    }

    #[test]
    fn a_child_composite_brings_its_own_children() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        // Through `enqueue_tree`: the scope comes through both levels, the parent is the child,
        // and what the root waits for holds its children too.
        let gate = q.enqueue(plain("index", "gate")).unwrap();
        let (topics, direct) = q
            .enqueue_tree(
                plain("topics", "topics").composite().scope("f1").needs(&[gate]),
                &[Child::new(plain("details", "details").composite()).with_children(vec![Child::new(plain("detail", "x"))])],
            )
            .unwrap();
        let details = q.get(direct[0]).unwrap().unwrap();
        assert_eq!((details.parent, details.scope.as_deref(), details.status), (Some(topics), Some("f1"), TaskStatus::Waiting));
        let leaf = &q.children(details.id).unwrap()[0];
        assert_eq!((leaf.parent, leaf.scope.as_deref()), (Some(details.id), Some("f1")));
        assert!(q.claim(&["detail"]).unwrap().is_none(), "the root's needs hold its children too");
        q.claim(&["index"]).unwrap();
        q.complete(gate, &json!({})).unwrap();
        assert_eq!(q.claim(&["detail"]).unwrap().unwrap().id, leaf.id);

        // Through `complete_with`: the child composite joins the task's parent composite, and its
        // own children go under it — all in one transaction.
        let (root, ids) =
            q.enqueue_tree(plain("topics", "topics").composite().scope("f2"), &[Child::new(plain("topics-plan", "plan"))]).unwrap();
        let plan = ids[0];
        assert_eq!(q.claim(&["topics-plan"]).unwrap().unwrap().id, plan);
        let made = q
            .complete_with(
                plan,
                &json!({"topics": 2}),
                &[
                    Child::new(plain("topic-digest", "given")),
                    Child::new(plain("topic-digest", "report")),
                    Child::new(plain("details", "details").composite())
                        .after(&[0, 1])
                        .with_children(vec![Child::new(plain("topic-detail", "detail"))]),
                ],
            )
            .unwrap();
        let details = q.get(made[2]).unwrap().unwrap();
        assert_eq!((details.parent, details.scope.as_deref()), (Some(root), Some("f2")), "a sibling of the plan, not its child");
        let leaf = q.children(details.id).unwrap()[0].id;
        assert_eq!(q.claim(&["topic-digest"]).unwrap().unwrap().id, made[0]);
        assert_eq!(q.claim(&["topic-digest"]).unwrap().unwrap().id, made[1]);
        assert!(q.claim(&["topic-detail"]).unwrap().is_none(), "a child of a composite waits for what the composite waits for");
        q.complete(made[0], &json!({})).unwrap();
        q.complete(made[1], &json!({})).unwrap();
        assert_eq!(q.claim(&["topic-detail"]).unwrap().unwrap().id, leaf);
    }

    #[test]
    fn cancel_analysis_stops_only_the_analysis_of_the_recording() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let analysis = q.enqueue(plain("analysis", "analysis").composite().scope("rec")).unwrap();
        let topics = q.enqueue(plain("topics", "topics").parent(analysis).composite().scope("rec")).unwrap();
        let digest = q.enqueue(plain("topic-digest", "digest").parent(topics).scope("rec")).unwrap();
        let answer = q.begin_inline("answer", "answer", &json!({}), None, Some("rec")).unwrap();
        let other = q.enqueue(plain("analysis", "other").composite().scope("rec-2")).unwrap();
        let standalone = q.enqueue(plain("transcribe", "old").scope("rec")).unwrap();

        assert_eq!(q.cancel_analysis("rec").unwrap(), 3);
        for id in [analysis, topics, digest] {
            assert_eq!(status(&q, id), TaskStatus::Cancelled, "task {id}");
        }
        assert_eq!(status(&q, answer), TaskStatus::Running, "a chat answer of the same recording keeps running");
        assert_eq!(status(&q, other), TaskStatus::Waiting, "another recording is untouched");
        assert_eq!(status(&q, standalone), TaskStatus::Queued, "only the analysis subtrees of the scope");
    }

    #[test]
    fn a_raised_priority_is_claimed_first() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let first = q.enqueue(plain("k", "first")).unwrap();
        let second = q.enqueue(plain("k", "second")).unwrap();
        q.set_priority(second, 100).unwrap();
        assert_eq!(q.claim(&["k"]).unwrap().unwrap().id, second);
        assert_eq!(q.claim(&["k"]).unwrap().unwrap().id, first);
    }

    #[test]
    fn cancel_scope_stops_every_unfinished_task_of_a_recording() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let a = q.enqueue(plain("k", "a").scope("file-1")).unwrap();
        let b = q.enqueue(plain("k", "b").scope("file-1")).unwrap();
        let done = q.enqueue(plain("k", "done").scope("file-1")).unwrap();
        q.complete(done, &json!({})).unwrap();
        let other = q.enqueue(plain("k", "other").scope("file-2")).unwrap();
        assert_eq!(q.cancel_scope("file-1").unwrap(), 2);
        for id in [a, b] {
            assert_eq!(status(&q, id), TaskStatus::Cancelled);
        }
        assert_eq!(status(&q, done), TaskStatus::Done, "finished work stays");
        assert_eq!(status(&q, other), TaskStatus::Queued);
    }

    #[test]
    fn inline_tasks_are_never_claimed_and_are_cancelled_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let q = Queue::open(dir.path()).unwrap();
            let id = q.begin_inline("model-step", "step", &json!({"question": "why"}), None, Some("rec-1")).unwrap();
            let t = q.get(id).unwrap().unwrap();
            assert_eq!((t.status, t.inline), (TaskStatus::Running, true));
            assert!(t.started_ms.is_some());
            assert!(q.claim(&["model-step"]).unwrap().is_none(), "the worker does not take inline work");
            id
        };
        let q = Queue::open(dir.path()).unwrap();
        assert_eq!(q.requeue_running().unwrap(), 1);
        assert_eq!(status(&q, id), TaskStatus::Cancelled, "nobody can continue a chat answer");
        assert!(q.get(id).unwrap().unwrap().finished_ms.is_some());

        let done = q.begin_inline("tool", "tool", &json!({}), None, None).unwrap();
        q.finish_inline(done, &json!({"ok": true})).unwrap();
        assert_eq!(status(&q, done), TaskStatus::Done);
        let failed = q.begin_inline("tool", "tool", &json!({}), None, None).unwrap();
        q.fail_inline(failed, "boom").unwrap();
        assert_eq!(status(&q, failed), TaskStatus::Failed);
        let cancelled = q.begin_inline("tool", "tool", &json!({}), None, None).unwrap();
        q.cancel_inline(cancelled).unwrap();
        assert_eq!(status(&q, cancelled), TaskStatus::Cancelled);
    }

    #[test]
    fn an_inline_answer_runs_until_its_caller_ends_it() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let answer = q.begin_inline("chat", "why", &json!({}), None, Some("rec-1")).unwrap();
        let step = q.begin_inline("model-step", "1", &json!({}), Some(answer), Some("rec-1")).unwrap();
        q.finish_inline(step, &json!({})).unwrap();
        assert_eq!(status(&q, answer), TaskStatus::Running, "all steps so far are done, the answer goes on");
        assert!(q.get(answer).unwrap().unwrap().finished_ms.is_none());
        q.finish_inline(answer, &json!({})).unwrap();
        assert_eq!(status(&q, answer), TaskStatus::Done);
    }

    #[test]
    fn stats_accumulate() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.begin_inline("tool", "search", &json!({}), None, None).unwrap();
        q.add_stats(id, &Stats { model_calls: 1, prompt_tokens: 100, completion_tokens: 20, model_ms: 400, models: vec!["qwen".into()] })
            .unwrap();
        q.add_stats(id, &Stats { model_calls: 2, prompt_tokens: 50, completion_tokens: 5, model_ms: 100, models: vec!["qwen".into(), "bge".into()] })
            .unwrap();
        let s = q.get(id).unwrap().unwrap().stats;
        assert_eq!((s.model_calls, s.prompt_tokens, s.completion_tokens, s.model_ms), (3, 150, 25, 500));
        assert_eq!(s.models, ["qwen", "bge"], "each model once");

        q.set_detail(id, &json!({"tool": "search", "args": "cats", "result_chars": 120})).unwrap();
        assert_eq!(q.get(id).unwrap().unwrap().detail["tool"], "search");

        let task = q.enqueue(plain("k", "t")).unwrap();
        q.add_stats(task, &Stats { model_calls: 4, ..Stats::default() }).unwrap();
        assert_eq!(q.get(task).unwrap().unwrap().stats.model_calls, 4, "also for background tasks");
    }

    #[test]
    fn kind_stats_remember_the_run_time_per_unit() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue(plain("k", "t").units(10.0)).unwrap();
        q.claim(&["k"]).unwrap();
        std::thread::sleep(Duration::from_millis(15));
        q.complete(id, &json!({})).unwrap();
        let stats = q.kind_stats().unwrap();
        let k = stats.iter().find(|s| s.kind == "k").expect("the run is remembered");
        assert_eq!((k.runs, k.units), (1, 10.0));
        assert!(k.ms >= 10, "the run took at least the sleep: {}", k.ms);

        let again = q.enqueue(plain("k", "t2").units(20.0)).unwrap();
        q.claim(&["k"]).unwrap();
        q.complete(again, &json!({})).unwrap();
        let k = q.kind_stats().unwrap().into_iter().find(|s| s.kind == "k").unwrap();
        assert_eq!((k.runs, k.units), (2, 30.0), "hours add up with the units");

        let no_units = q.enqueue(plain("k", "t3")).unwrap();
        q.claim(&["k"]).unwrap();
        q.complete(no_units, &json!({})).unwrap();
        assert_eq!(q.kind_stats().unwrap().into_iter().find(|s| s.kind == "k").unwrap().runs, 2, "unknown size is not counted");
    }

    #[test]
    fn estimate_by_rate_by_history_and_none() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        // By the progress rate: 20 s gone, half done → 20 s left.
        let id = q.enqueue(plain("k", "rate")).unwrap();
        q.claim(&["k"]).unwrap();
        q.checkpoint(id, &json!(1), &json!([]), 0.5).unwrap();
        q.db.execute("UPDATE tasks SET started_ms = ?2 WHERE id = ?1", params![id, now_ms() - 20_000]).unwrap();
        let task = q.get(id).unwrap().unwrap();
        let left = estimate(&task, now_ms(), &[]).unwrap();
        assert!((left.as_secs_f64() - 20.0).abs() < 1.5, "{left:?}");

        // Too early for the rate (under 10 s): the history: 60 s per 10 units, 5 units, 0 done → 30 s.
        let fresh = q.get(q.enqueue(plain("probe", "fresh").units(5.0)).unwrap()).unwrap().unwrap();
        let stats = [KindStat { kind: "probe".into(), runs: 1, ms: 60_000, units: 10.0 }];
        assert!((estimate(&fresh, now_ms(), &stats).unwrap().as_secs_f64() - 30.0).abs() < 0.01);

        // No history and no size: no estimate.
        let unknown = q.get(q.enqueue(plain("new", "unknown")).unwrap()).unwrap().unwrap();
        assert_eq!(estimate(&unknown, now_ms(), &[]), None);

        // A composite is the sum of what its unfinished children have left.
        let parent = q.enqueue(plain("group", "group").composite()).unwrap();
        let done = q.enqueue(plain("sum", "done").parent(parent)).unwrap();
        let a = q.enqueue(plain("sum", "a").parent(parent).units(5.0)).unwrap();
        let b = q.enqueue(plain("sum", "b").parent(parent).units(10.0)).unwrap();
        q.claim(&["sum"]).unwrap();
        q.complete(done, &json!({})).unwrap();
        q.db.execute("INSERT INTO kind_stats (kind, runs, ms, units) VALUES ('sum', 1, 60000, 10)", []).unwrap();
        let sum_stats = [KindStat { kind: "sum".into(), runs: 1, ms: 60_000, units: 10.0 }];
        let tree = q.subtree(parent).unwrap();
        let parent_task = q.get(parent).unwrap().unwrap();
        let left = estimate_tree(&parent_task, &tree, now_ms(), &sum_stats).unwrap();
        assert!((left.as_secs_f64() - 90.0).abs() < 0.01, "30 s + 60 s, the done one skipped: {left:?}");
        assert_eq!(q.estimate_of(parent, now_ms()).unwrap().unwrap().as_secs(), 90);

        // All children done: nothing left.
        for id in [a, b] {
            q.claim(&["sum"]).unwrap();
            q.complete(id, &json!({})).unwrap();
        }
        assert_eq!(q.estimate_of(parent, now_ms()).unwrap(), Some(Duration::ZERO));
    }

    #[test]
    fn purge_removes_old_subtrees_but_keeps_kind_stats() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        // An old sized run (it must stay in `kind_stats`) and an old composite with a child.
        let old = q.enqueue(plain("k", "old").units(10.0)).unwrap();
        q.claim(&["k"]).unwrap();
        q.complete(old, &json!({})).unwrap();
        let parent = q.enqueue(plain("group", "old group").composite()).unwrap();
        let child = q.enqueue(plain("k", "child").parent(parent)).unwrap();
        q.claim(&["k"]).unwrap();
        q.complete(child, &json!({})).unwrap();
        let young = q.enqueue(plain("k", "young")).unwrap();
        q.claim(&["k"]).unwrap();
        q.complete(young, &json!({})).unwrap();
        let unfinished = q.enqueue(plain("k", "unfinished")).unwrap();
        let old_ms = now_ms() - 40 * 24 * 60 * 60 * 1000;
        q.db.execute("UPDATE tasks SET finished_ms = ?2, updated_ms = ?2 WHERE id IN (?1, ?3, ?4)", params![old, old_ms, parent, child]).unwrap();
        q.db.execute("UPDATE tasks SET created_ms = ?2, updated_ms = ?2 WHERE id = ?1", params![unfinished, old_ms]).unwrap();
        assert_eq!(q.kind_stats().unwrap().iter().map(|s| s.runs).sum::<i64>(), 1, "the sized run is remembered");

        assert_eq!(q.purge_older_than(Duration::from_secs(30 * 24 * 60 * 60)).unwrap(), 3, "the old run and the old subtree");
        assert!(q.get(old).unwrap().is_none());
        assert!(q.get(parent).unwrap().is_none());
        assert!(q.get(child).unwrap().is_none(), "the whole subtree goes");
        assert!(q.get(young).unwrap().is_some(), "recent work stays");
        assert!(q.get(unfinished).unwrap().is_some(), "unfinished work stays");
        assert_eq!(q.kind_stats().unwrap().iter().map(|s| s.runs).sum::<i64>(), 1, "the history stays");
    }

    #[test]
    fn a_file_of_another_schema_is_recreated() {
        let dir = tempfile::tempdir().unwrap();
        {
            let db = Connection::open(dir.path().join("queue.db")).unwrap();
            db.execute_batch(
                "CREATE TABLE tasks (id INTEGER PRIMARY KEY, kind TEXT NOT NULL, title TEXT NOT NULL, input TEXT NOT NULL, priority INTEGER NOT NULL,
                    status TEXT NOT NULL, cursor TEXT, partial TEXT, result TEXT, progress REAL NOT NULL, error TEXT, attempts INTEGER NOT NULL,
                    created_ms INTEGER NOT NULL, updated_ms INTEGER NOT NULL);
                 INSERT INTO tasks (kind, title, input, priority, status, progress, attempts, created_ms, updated_ms)
                    VALUES ('old', 'old', '{}', 0, 'queued', 0, 0, 0, 0);
                 PRAGMA user_version = 1;",
            )
            .unwrap();
        }
        let q = Queue::open(dir.path()).unwrap();
        assert!(q.list().unwrap().is_empty(), "the old file is dropped, not migrated");
        let id = q.enqueue(plain("k", "new").scope("s").units(3.0)).unwrap();
        let t = q.get(id).unwrap().unwrap();
        assert_eq!((t.status, t.scope.as_deref(), t.units), (TaskStatus::Queued, Some("s"), 3.0));
        assert_eq!(q.kind_stats().unwrap(), Vec::new(), "the new tables exist");
    }

    #[test]
    fn top_level_and_children_read_the_tree() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let analysis = q.enqueue(plain("analysis", "analysis").composite().scope("rec-1")).unwrap();
        let step = q.enqueue(plain("transcribe", "step").parent(analysis).scope("rec-1")).unwrap();
        let chat = q.begin_inline("answer", "answer", &json!({}), None, Some("rec-2")).unwrap();
        let tool = q.begin_inline("tool", "tool", &json!({}), Some(chat), Some("rec-2")).unwrap();

        let all = q.top_level(&Filter::default()).unwrap();
        assert_eq!(all.iter().map(|t| t.id).collect::<Vec<_>>(), [chat, analysis], "children are not top-level; running first");
        assert!(!all.iter().any(|t| t.id == step || t.id == tool));
        let analysis_only = q.top_level(&Filter::group(KindGroup::Analysis)).unwrap();
        assert_eq!(analysis_only.iter().map(|t| t.id).collect::<Vec<_>>(), [analysis]);
        let chat_only = q.top_level(&Filter::group(KindGroup::Chat)).unwrap();
        assert_eq!(chat_only.iter().map(|t| t.id).collect::<Vec<_>>(), [chat]);
        let scope = q.top_level(&Filter::scope("rec-1")).unwrap();
        assert_eq!(scope.iter().map(|t| t.id).collect::<Vec<_>>(), [analysis]);
        assert_eq!(q.children(analysis).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [step]);
        assert_eq!(q.children(chat).unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [tool]);
        assert_eq!(q.subtree(analysis).unwrap().len(), 2);
    }

    #[test]
    fn restart_remove_and_clear_act_on_the_subtree() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let top = q.enqueue(plain("group", "top").composite()).unwrap();
        let a = q.enqueue(plain("k", "a").parent(top)).unwrap();
        let b = q.enqueue(plain("k", "b").parent(top)).unwrap();
        q.claim(&["k"]).unwrap();
        q.complete(a, &json!({})).unwrap();
        q.claim(&["k"]).unwrap();
        q.fail(b, "boom").unwrap();
        assert_eq!(status(&q, top), TaskStatus::Failed);
        // Clearing the finished work keeps a done child under a task that did not finish.
        q.clear_done().unwrap();
        assert!(q.get(a).unwrap().is_some(), "a done child of an unfinished task stays");
        // Restart of the composite starts every child over.
        q.restart(top).unwrap();
        for id in [a, b] {
            let t = q.get(id).unwrap().unwrap();
            assert_eq!((t.status, t.progress, t.error), (TaskStatus::Queued, 0.0, None), "task {id}");
        }
        assert_eq!(status(&q, top), TaskStatus::Waiting);
        // All done: clearing removes the whole tree.
        for _ in 0..2 {
            let t = q.claim(&["k"]).unwrap().unwrap();
            q.complete(t.id, &json!({})).unwrap();
        }
        assert_eq!(status(&q, top), TaskStatus::Done);
        q.clear_done().unwrap();
        assert!(q.list().unwrap().is_empty());
        // Removing a composite removes its children; removing a child updates the composite.
        let top = q.enqueue(plain("group", "top").composite()).unwrap();
        let a = q.enqueue(plain("k", "a").parent(top)).unwrap();
        let b = q.enqueue(plain("k", "b").parent(top)).unwrap();
        q.claim(&["k"]).unwrap();
        q.complete(a, &json!({})).unwrap();
        q.remove(b).unwrap();
        assert_eq!(status(&q, top), TaskStatus::Done, "only the done child is left");
        q.remove(top).unwrap();
        assert!(q.list().unwrap().is_empty(), "{:?}", q.list().unwrap().len());
    }

    #[test]
    fn a_composite_survives_a_restart_of_the_program_and_still_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let top = q.enqueue(plain("group", "top").composite()).unwrap();
        let mid = q.enqueue(plain("group", "mid").parent(top).composite()).unwrap();
        let leaf = q.enqueue(plain("k", "leaf").parent(mid)).unwrap();
        q.claim(&["k"]).unwrap();
        assert_eq!((status(&q, top), status(&q, mid)), (TaskStatus::Running, TaskStatus::Running));
        // The program was closed while the leaf ran.
        q.requeue_running().unwrap();
        assert_eq!(status(&q, leaf), TaskStatus::Queued);
        assert_eq!((status(&q, top), status(&q, mid)), (TaskStatus::Waiting, TaskStatus::Waiting), "composites are never queued");
        q.claim(&["k"]).unwrap();
        q.complete(leaf, &json!({})).unwrap();
        assert_eq!((status(&q, top), status(&q, mid)), (TaskStatus::Done, TaskStatus::Done));
    }
}
