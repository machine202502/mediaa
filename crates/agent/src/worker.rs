//! The worker that runs queued tasks (`mediaa_core::queue`) one at a time with the handler of
//! their kind. A handler continues from the task's cursor and checkpoints as it goes; when it
//! fails or panics the task is marked failed with its checkpoint kept, so a retry continues
//! from there. Pausing a task or stopping the worker takes effect at the next checkpoint.

use crate::scope::Scope;
use mediaa_core::queue::{Queue, Task, TaskId, TaskStatus};
use mediaa_engine::Engine;
use serde_json::Value;
use std::cell::OnceCell;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// A running task without a checkpoint for this long counts as hung and is queued again.
pub const STALE: Duration = Duration::from_secs(15 * 60);
/// Finished work is forgotten after this long.
pub const HISTORY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How often the history is purged while the worker runs.
const PURGE_EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// Does the work of one task kind.
pub trait TaskHandler: Send + Sync {
    fn kind(&self) -> &'static str;
    /// Runs `task` from `task.cursor` (with `task.partial` produced so far), calling
    /// [`Checkpoints::save`] regularly; returns the final result.
    fn run(&self, task: &Task, checkpoints: &mut Checkpoints) -> anyhow::Result<Value>;
}

/// Returned by [`Checkpoints::save`] when the task must stop (paused, removed, worker stopping).
#[derive(Debug)]
pub struct Stop;

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("stopped")
    }
}

impl std::error::Error for Stop {}

pub struct Checkpoints<'a> {
    queue: &'a Queue,
    id: TaskId,
    stop: &'a AtomicBool,
    engine: &'a Arc<dyn Engine>,
    key: Option<String>,
    scope: OnceCell<Scope>,
}

impl<'a> Checkpoints<'a> {
    /// Set when the worker stops (for waits between checkpoints, e.g. for a model lease).
    pub fn stopping(&self) -> &'a AtomicBool {
        self.stop
    }

    /// This task's work: its queue, id, recording and the engine that counts its model calls.
    pub fn scope(&self) -> &Scope {
        self.scope.get_or_init(|| Scope::new(self.queue.dir(), self.id, self.engine.clone(), self.key.clone()))
    }

    /// The engine for this task (model calls go into its stats).
    pub fn engine(&self) -> Arc<dyn Engine> {
        self.scope().engine()
    }

    /// Saves where the task is and what it has; `Err(Stop)` (propagate it with `?`) when it
    /// has to stop.
    pub fn save(&mut self, cursor: &Value, partial: &Value, progress: f32) -> anyhow::Result<()> {
        let status = self.queue.checkpoint(self.id, cursor, partial, progress)?;
        if status != TaskStatus::Running || self.stop.load(Ordering::Relaxed) {
            return Err(Stop.into());
        }
        Ok(())
    }
}

/// Runs the next queued task a handler exists for; returns whether there was one.
pub fn run_next(queue: &Queue, engine: &Arc<dyn Engine>, handlers: &[Box<dyn TaskHandler>], stop: &AtomicBool) -> anyhow::Result<bool> {
    let kinds: Vec<&str> = handlers.iter().map(|h| h.kind()).collect();
    let Some(task) = queue.claim(&kinds)? else { return Ok(false) };
    let handler = handlers.iter().find(|h| h.kind() == task.kind).expect("claimed by kind");
    let mut checkpoints =
        Checkpoints { queue, id: task.id, stop, engine, key: task.scope.clone(), scope: OnceCell::new() };
    match catch_unwind(AssertUnwindSafe(|| handler.run(&task, &mut checkpoints))) {
        Ok(Ok(result)) => queue.complete(task.id, &result)?,
        Ok(Err(e)) if e.is::<Stop>() => {
            // Paused / removed: nothing to do. Worker stopping: back to the queue now.
            if stop.load(Ordering::Relaxed) {
                queue.release(task.id)?;
            }
        }
        Ok(Err(e)) => queue.fail(task.id, &format!("{e:#}"))?,
        Err(panic) => {
            let msg = panic.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| panic.downcast_ref::<String>().cloned()).unwrap_or_default();
            log::error!("task {} ({}) crashed: {msg}", task.id, task.kind);
            queue.fail(task.id, &format!("crashed: {msg}"))?;
        }
    }
    Ok(true)
}

/// Starts the worker thread on the queue in `dir` (a library's `.ai` folder) until `stop`.
pub fn spawn(dir: PathBuf, engine: Arc<dyn Engine>, handlers: Vec<Box<dyn TaskHandler>>, stop: Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new().name("agent-worker".into()).spawn(move || {
        let queue = match Queue::open(&dir) {
            Ok(q) => q,
            Err(e) => return log::error!("agent queue: {e:#}"),
        };
        // Tasks this worker ran before the program was closed (or crashed) continue; chat work
        // that was running is cancelled. The history of finished work is trimmed.
        if let Err(e) = queue.requeue_running() {
            log::warn!("agent queue: {e:#}");
        }
        if let Err(e) = queue.purge_older_than(HISTORY) {
            log::warn!("agent queue: {e:#}");
        }
        let mut stale_check = Instant::now();
        let mut purge_check = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            if stale_check.elapsed() > Duration::from_secs(60) {
                stale_check = Instant::now();
                if let Err(e) = queue.requeue_stale(STALE) {
                    log::warn!("agent queue: {e:#}");
                }
            }
            if purge_check.elapsed() > PURGE_EVERY {
                purge_check = Instant::now();
                if let Err(e) = queue.purge_older_than(HISTORY) {
                    log::warn!("agent queue: {e:#}");
                }
            }
            match run_next(&queue, &engine, &handlers, &stop) {
                Ok(true) => {}
                Ok(false) => std::thread::sleep(Duration::from_millis(500)),
                Err(e) => {
                    log::warn!("agent queue: {e:#}");
                    std::thread::sleep(Duration::from_secs(2));
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::queue::NewTask;
    use mediaa_engine::testing::FakeEngine;
    use serde_json::json;

    fn engine() -> Arc<dyn Engine> {
        Arc::new(FakeEngine::new())
    }

    /// Counts 0..10 one item per checkpoint; can crash once after a given item.
    struct Counter {
        crash_after: std::sync::Mutex<Option<u64>>,
    }

    impl TaskHandler for Counter {
        fn kind(&self) -> &'static str {
            "count"
        }

        fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
            let mut next = task.cursor.as_u64().unwrap_or(0);
            let mut done: Vec<u64> = serde_json::from_value(task.partial.clone()).unwrap_or_default();
            while next < 10 {
                done.push(next);
                next += 1;
                cp.save(&json!(next), &json!(done), next as f32 / 10.0)?;
                if *self.crash_after.lock().unwrap() == Some(next) {
                    *self.crash_after.lock().unwrap() = None;
                    panic!("model ran out of memory");
                }
            }
            Ok(json!({"items": done}))
        }
    }

    #[test]
    fn a_crashed_task_continues_from_its_checkpoint() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue(NewTask::new("count", "count to ten", &json!({}))).unwrap();
        let other = q.enqueue(NewTask::new("unknown", "no handler", &json!({})).priority(9)).unwrap();
        let handlers: Vec<Box<dyn TaskHandler>> = vec![Box::new(Counter { crash_after: std::sync::Mutex::new(Some(4)) })];
        let stop = AtomicBool::new(false);

        let engine = engine();
        assert!(run_next(&q, &engine, &handlers, &stop).unwrap());
        let t = q.get(id).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
        assert!(t.error.unwrap().contains("out of memory"));
        assert_eq!((t.cursor, t.partial), (json!(4), json!([0, 1, 2, 3])), "checkpoint kept");

        q.resume(id).unwrap();
        assert!(run_next(&q, &engine, &handlers, &stop).unwrap());
        let t = q.get(id).unwrap().unwrap();
        assert_eq!(t.status, TaskStatus::Done);
        assert_eq!(t.result, Some(json!({"items": [0, 1, 2, 3, 4, 5, 6, 7, 8, 9]})), "nothing lost or repeated");
        assert_eq!(t.attempts, 2);
        assert!(!run_next(&q, &engine, &handlers, &stop).unwrap(), "no task with a handler left");
        assert_eq!(q.get(other).unwrap().unwrap().status, TaskStatus::Queued);
    }

    #[test]
    fn stopping_the_worker_requeues_the_task_at_its_checkpoint() {
        struct StopAt3(Arc<AtomicBool>);
        impl TaskHandler for StopAt3 {
            fn kind(&self) -> &'static str {
                "count"
            }
            fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
                let mut next = task.cursor.as_u64().unwrap_or(0);
                loop {
                    next += 1;
                    if next == 3 {
                        self.0.store(true, Ordering::Relaxed); // the app is closing
                    }
                    cp.save(&json!(next), &Value::Null, 0.0)?;
                }
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue(NewTask::new("count", "t", &json!({}))).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handlers: Vec<Box<dyn TaskHandler>> = vec![Box::new(StopAt3(stop.clone()))];
        run_next(&q, &engine(), &handlers, &stop).unwrap();
        let t = q.get(id).unwrap().unwrap();
        assert_eq!((t.status, t.cursor), (TaskStatus::Queued, json!(3)));
    }

    #[test]
    fn the_worker_thread_runs_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let id = Queue::open(dir.path()).unwrap().enqueue(NewTask::new("count", "t", &json!({}))).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(dir.path().to_path_buf(), engine(), vec![Box::new(Counter { crash_after: std::sync::Mutex::new(None) })], stop.clone()).unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while q.get(id).unwrap().unwrap().status != TaskStatus::Done && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert_eq!(q.get(id).unwrap().unwrap().status, TaskStatus::Done);
    }

    #[test]
    fn starting_the_worker_cancels_running_inline_work() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.begin_inline("tool", "a tool", &json!({}), None, Some("rec")).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let handle = spawn(dir.path().to_path_buf(), engine(), vec![], stop.clone()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while q.get(id).unwrap().unwrap().status != TaskStatus::Cancelled && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        assert_eq!(q.get(id).unwrap().unwrap().status, TaskStatus::Cancelled, "nobody can continue it");
    }
}
