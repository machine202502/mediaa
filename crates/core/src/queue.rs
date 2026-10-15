//! Queue of neural-network tasks of the Agent section (`<media>/.ai/queue.db`). A task keeps
//! where it stopped (`cursor`) and what it has produced so far (`partial`), both JSON, written
//! together in one transaction by every checkpoint. After a crash of the program or of the
//! task's worker the task goes back to the queue and continues from its last checkpoint.

use anyhow::bail;
use rusqlite::{params, Connection, OptionalExtension, Row};
use serde_json::Value;
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub type TaskId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Queued,
    Running,
    Paused,
    Done,
    Failed,
}

impl TaskStatus {
    fn from_code(s: &str) -> TaskStatus {
        match s {
            "running" => TaskStatus::Running,
            "paused" => TaskStatus::Paused,
            "done" => TaskStatus::Done,
            "failed" => TaskStatus::Failed,
            _ => TaskStatus::Queued,
        }
    }
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
}

fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn json(s: Option<String>) -> Value {
    s.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

const COLUMNS: &str = "id, kind, title, input, priority, status, cursor, partial, result, progress, error, attempts, created_ms, updated_ms";

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
    })
}

const SCHEMA: i64 = 1;

pub struct Queue {
    db: Connection,
}

impl Queue {
    /// Opens the queue in `dir` (the library's `.ai` folder). Any number of connections may be
    /// open (the worker, the pages); tasks keep their status.
    pub fn open(dir: &Path) -> anyhow::Result<Queue> {
        std::fs::create_dir_all(dir)?;
        let db = Connection::open(dir.join("queue.db"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA {
            if version != 0 {
                bail!("{} has schema {version}, expected {SCHEMA}", dir.join("queue.db").display());
            }
            db.execute_batch(
                "CREATE TABLE tasks (
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
                    updated_ms INTEGER NOT NULL
                );
                PRAGMA user_version = 1;",
            )?;
        }
        Ok(Queue { db })
    }

    pub fn enqueue(&self, kind: &str, title: &str, input: &Value, priority: i64) -> anyhow::Result<TaskId> {
        let now = now_ms();
        self.db.execute(
            "INSERT INTO tasks (kind, title, input, priority, status, created_ms, updated_ms) VALUES (?1, ?2, ?3, ?4, 'queued', ?5, ?5)",
            params![kind, title, input.to_string(), priority, now],
        )?;
        Ok(self.db.last_insert_rowid())
    }

    /// Like [`Queue::enqueue`], unless the same task (kind and input) is already waiting,
    /// running or paused: then its id.
    pub fn enqueue_once(&self, kind: &str, title: &str, input: &Value, priority: i64) -> anyhow::Result<TaskId> {
        let existing: Option<TaskId> = self
            .db
            .query_row(
                "SELECT id FROM tasks WHERE kind = ?1 AND input = ?2 AND status IN ('queued', 'running', 'paused') LIMIT 1",
                params![kind, input.to_string()],
                |r| r.get(0),
            )
            .optional()?;
        match existing {
            Some(id) => Ok(id),
            None => self.enqueue(kind, title, input, priority),
        }
    }

    pub fn get(&self, id: TaskId) -> anyhow::Result<Option<Task>> {
        Ok(self.db.query_row(&format!("SELECT {COLUMNS} FROM tasks WHERE id = ?1"), [id], task).optional()?)
    }

    /// Every task: running first, then queued by priority, then the rest, newest first.
    pub fn list(&self) -> anyhow::Result<Vec<Task>> {
        let mut q = self.db.prepare(&format!(
            "SELECT {COLUMNS} FROM tasks ORDER BY
                CASE status WHEN 'running' THEN 0 WHEN 'queued' THEN 1 WHEN 'paused' THEN 2 WHEN 'failed' THEN 3 ELSE 4 END,
                CASE status WHEN 'queued' THEN -priority ELSE 0 END,
                CASE status WHEN 'queued' THEN id ELSE -updated_ms END"
        ))?;
        let tasks = q.query_map([], task)?.collect::<rusqlite::Result<_>>()?;
        Ok(tasks)
    }

    /// Takes the next queued task of one of `kinds` and marks it running.
    pub fn claim(&self, kinds: &[&str]) -> anyhow::Result<Option<Task>> {
        if kinds.is_empty() {
            return Ok(None);
        }
        let tx = self.db.unchecked_transaction()?;
        let marks = vec!["?"; kinds.len()].join(",");
        let id: Option<TaskId> = tx
            .query_row(
                &format!("SELECT id FROM tasks WHERE status = 'queued' AND kind IN ({marks}) ORDER BY priority DESC, id LIMIT 1"),
                rusqlite::params_from_iter(kinds),
                |r| r.get(0),
            )
            .optional()?;
        let Some(id) = id else { return Ok(None) };
        tx.execute(
            "UPDATE tasks SET status = 'running', attempts = attempts + 1, error = NULL, updated_ms = ?2 WHERE id = ?1",
            params![id, now_ms()],
        )?;
        tx.commit()?;
        self.get(id)
    }

    /// Saves where a running task is and what it has so far (one transaction). Returns the
    /// task's status: anything but `Running` means it was paused or removed meanwhile and
    /// should stop.
    pub fn checkpoint(&self, id: TaskId, cursor: &Value, partial: &Value, progress: f32) -> anyhow::Result<TaskStatus> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute(
            "UPDATE tasks SET cursor = ?2, partial = ?3, progress = ?4, updated_ms = ?5 WHERE id = ?1 AND status = 'running'",
            params![id, cursor.to_string(), partial.to_string(), progress.clamp(0.0, 1.0), now_ms()],
        )?;
        let status: Option<String> = tx.query_row("SELECT status FROM tasks WHERE id = ?1", [id], |r| r.get(0)).optional()?;
        tx.commit()?;
        // A removed task reads as paused: the worker just stops.
        Ok(status.map_or(TaskStatus::Paused, |s| TaskStatus::from_code(&s)))
    }

    pub fn complete(&self, id: TaskId, result: &Value) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE tasks SET status = 'done', result = ?2, progress = 1, updated_ms = ?3 WHERE id = ?1",
            params![id, result.to_string(), now_ms()],
        )?;
        Ok(())
    }

    /// The task stopped with an error; its checkpoint stays for a retry.
    pub fn fail(&self, id: TaskId, error: &str) -> anyhow::Result<()> {
        self.db.execute("UPDATE tasks SET status = 'failed', error = ?2, updated_ms = ?3 WHERE id = ?1", params![id, error, now_ms()])?;
        Ok(())
    }

    /// A running task goes back to the queue untouched (its worker is stopping).
    pub fn release(&self, id: TaskId) -> anyhow::Result<()> {
        self.db.execute("UPDATE tasks SET status = 'queued', updated_ms = ?2 WHERE id = ?1 AND status = 'running'", params![id, now_ms()])?;
        Ok(())
    }

    /// Stops a queued or running task (a running one stops at its next checkpoint).
    pub fn pause(&self, id: TaskId) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE tasks SET status = 'paused', updated_ms = ?2 WHERE id = ?1 AND status IN ('queued', 'running')",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Back into the queue (paused or failed tasks), continuing from the checkpoint.
    pub fn resume(&self, id: TaskId) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE tasks SET status = 'queued', error = NULL, updated_ms = ?2 WHERE id = ?1 AND status IN ('paused', 'failed')",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    /// Starts over: drops the checkpoint and queues the task again.
    pub fn restart(&self, id: TaskId) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE tasks SET status = 'queued', cursor = NULL, partial = NULL, result = NULL, progress = 0, error = NULL, updated_ms = ?2
             WHERE id = ?1 AND status != 'running'",
            params![id, now_ms()],
        )?;
        Ok(())
    }

    pub fn remove(&self, id: TaskId) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM tasks WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Removes finished tasks.
    pub fn clear_done(&self) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM tasks WHERE status = 'done'", [])?;
        Ok(())
    }

    /// Tasks left running by a previous run of the worker (the program was closed or crashed)
    /// go back to the queue, keeping their checkpoint; returns how many. Only the worker calls
    /// it, when it starts.
    pub fn requeue_running(&self) -> anyhow::Result<usize> {
        Ok(self.db.execute("UPDATE tasks SET status = 'queued' WHERE status = 'running'", [])?)
    }

    /// Running tasks without a checkpoint for `timeout` (their worker hung or died) go back
    /// to the queue; returns how many.
    pub fn requeue_stale(&self, timeout: Duration) -> anyhow::Result<usize> {
        let before = now_ms() - timeout.as_millis() as i64;
        Ok(self.db.execute("UPDATE tasks SET status = 'queued' WHERE status = 'running' AND updated_ms < ?1", [before])?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_same_task_is_queued_once_while_it_is_pending() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let a = q.enqueue_once("index", "a", &json!({"path": "/a"}), 0).unwrap();
        assert_eq!(q.enqueue_once("index", "a", &json!({"path": "/a"}), 0).unwrap(), a);
        assert_ne!(q.enqueue_once("index", "b", &json!({"path": "/b"}), 0).unwrap(), a);
        assert_ne!(q.enqueue_once("outline", "a", &json!({"path": "/a"}), 0).unwrap(), a);
        q.claim(&["index"]).unwrap();
        q.complete(a, &json!({})).unwrap();
        assert_ne!(q.enqueue_once("index", "a", &json!({"path": "/a"}), 0).unwrap(), a, "again once done");
    }

    #[test]
    fn priority_order_and_claiming() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let low = q.enqueue("transcribe", "low", &json!({"file": "a.mp3"}), 0).unwrap();
        let high = q.enqueue("transcribe", "high", &json!({}), 5).unwrap();
        let other = q.enqueue("embed", "other kind", &json!({}), 9).unwrap();
        assert!(q.claim(&[]).unwrap().is_none());
        let t = q.claim(&["transcribe"]).unwrap().unwrap();
        assert_eq!((t.id, t.status, t.attempts), (high, TaskStatus::Running, 1));
        assert_eq!(q.claim(&["transcribe"]).unwrap().unwrap().id, low);
        assert!(q.claim(&["transcribe"]).unwrap().is_none(), "nothing queued of that kind");
        assert_eq!(q.get(other).unwrap().unwrap().status, TaskStatus::Queued);
        assert_eq!(q.get(low).unwrap().unwrap().input, json!({"file": "a.mp3"}));
        assert_eq!(q.list().unwrap().iter().map(|t| t.id).collect::<Vec<_>>(), [low, high, other], "running first");
    }

    #[test]
    fn checkpoints_survive_a_restart_of_the_program() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let q = Queue::open(dir.path()).unwrap();
            let id = q.enqueue("transcribe", "t", &json!({}), 0).unwrap();
            q.claim(&["transcribe"]).unwrap();
            assert_eq!(q.checkpoint(id, &json!({"at": 3}), &json!(["a", "b", "c"]), 0.3).unwrap(), TaskStatus::Running);
            id
            // Dropped while running: the program "crashed".
        };
        let q = Queue::open(dir.path()).unwrap();
        assert_eq!(q.get(id).unwrap().unwrap().status, TaskStatus::Running, "opening (e.g. by a page) changes nothing");
        assert_eq!(q.requeue_running().unwrap(), 1, "the worker starting");
        let t = q.get(id).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Queued, "back in the queue");
        assert_eq!((t.cursor, t.partial, t.progress), (json!({"at": 3}), json!(["a", "b", "c"]), 0.3));
        let again = q.claim(&["transcribe"]).unwrap().unwrap();
        assert_eq!((again.cursor, again.attempts), (json!({"at": 3}), 2), "continues where it stopped");
    }

    #[test]
    fn pause_resume_fail_restart_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue("k", "t", &json!({}), 0).unwrap();
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
        q.resume(id).unwrap();
        assert_eq!(q.get(id).unwrap().unwrap().error, None);
        q.restart(id).unwrap();
        let t = q.get(id).unwrap().unwrap();
        assert_eq!((t.cursor, t.progress), (Value::Null, 0.0), "starts over");
        q.claim(&["k"]).unwrap();
        q.complete(id, &json!({"text": "done"})).unwrap();
        assert_eq!(q.get(id).unwrap().unwrap().result, Some(json!({"text": "done"})));
        q.clear_done().unwrap();
        assert!(q.get(id).unwrap().is_none());
        let gone = q.enqueue("k", "t", &json!({}), 0).unwrap();
        q.remove(gone).unwrap();
        assert_eq!(q.checkpoint(gone, &json!(1), &json!(1), 1.0).unwrap(), TaskStatus::Paused, "removed: stop");
    }

    #[test]
    fn stale_running_tasks_are_requeued() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue("k", "t", &json!({}), 0).unwrap();
        q.claim(&["k"]).unwrap();
        assert_eq!(q.requeue_stale(Duration::from_secs(60)).unwrap(), 0, "fresh");
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(q.requeue_stale(Duration::from_millis(10)).unwrap(), 1);
        assert_eq!(q.get(id).unwrap().unwrap().status, TaskStatus::Queued);
    }
}
