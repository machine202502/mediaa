//! One runner for every queue kind. A chunk is a blocking function on its own thread;
//! [`StageRunner::step`] polls it (`Busy` while it runs) and returns `Chunk`, `Done`, `Failed`
//! or `Stopped`. The engine's cancel flag is the scheduler's stop flag, so pause and cancel
//! reach a model on the next token. The cursor is what [`crate::run::Work::More`] returns;
//! a run after a crash or a pause continues from it.

use crate::assistant::chat::Live;
use crate::slots::Slots;
use mediaa_core::agent::{AgentSettings, Budget};
use mediaa_engine::Engine;
use mediaa_queue::state::{Op, Outcome, World};
use mediaa_queue::{
    Clock, Event, Pool, Resource, RunCtx, Runner, Scheduler, Status, Step, Store, SystemClock, Task, TaskId, HISTORY,
};
use serde_json::Value;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// What one chunk of a task did. The runner checkpoints `More` on the scheduler's thread.
#[derive(Debug)]
pub enum Work {
    More { cursor: Value, partial: Value, progress: f32 },
    Done(Value),
    Stopped,
}

/// Events the studio and the queue page both see. A cursor past the kept log is a gap: refresh.
#[derive(Default)]
pub struct Feed {
    inner: Mutex<FeedInner>,
}

#[derive(Default)]
struct FeedInner {
    start: usize,
    events: Vec<Event>,
}

impl Feed {
    pub fn push(&self, event: Event) {
        let mut inner = self.inner.lock().expect("feed");
        inner.events.push(event);
        if inner.events.len() > 4000 {
            let drop_n = 2000;
            inner.events.drain(0..drop_n);
            inner.start += drop_n;
        }
    }

    /// `(next cursor, events since cursor, missed some)`.
    pub fn since(&self, cursor: usize) -> (usize, Vec<Event>, bool) {
        let inner = self.inner.lock().expect("feed");
        let end = inner.start + inner.events.len();
        let gap = cursor < inner.start;
        let from = cursor.saturating_sub(inner.start).min(inner.events.len());
        (end, inner.events[from..].to_vec(), gap)
    }
}

/// Shared by the chunk thread and the UI. Tokens stay here, not in the queue.
#[derive(Default)]
pub struct Streams {
    live: Mutex<HashMap<TaskId, Live>>,
}

impl Streams {
    pub fn insert(&self, id: TaskId, live: Live) {
        self.live.lock().expect("streams").insert(id, live);
    }

    pub fn get(&self, id: TaskId) -> Option<Live> {
        self.live.lock().expect("streams").get(&id).cloned()
    }

    pub fn update<R>(&self, id: TaskId, f: impl FnOnce(&mut Live) -> R) -> Option<R> {
        self.live.lock().expect("streams").get_mut(&id).map(f)
    }

    pub fn close(&self, id: TaskId) {
        if let Some(live) = self.live.lock().expect("streams").get_mut(&id) {
            live.close();
        }
    }

    pub fn remove(&self, id: TaskId) -> Option<Live> {
        self.live.lock().expect("streams").remove(&id)
    }
}

/// What a chunk needs besides the task: the store, the engine and the settings.
#[derive(Clone)]
pub struct Host {
    pub ai_dir: PathBuf,
    pub engine: Arc<dyn Engine>,
    pub store: Arc<dyn Store>,
    pub slots: Slots,
    pub settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    pub streams: Arc<Streams>,
}

/// One kind (or a family of kinds) of chunked work.
pub trait Stage: Send + Sync {
    fn handles(&self, kind: &str) -> bool;
    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work>;
}

struct Flight {
    gen: i64,
    cancel: Arc<AtomicBool>,
    /// Closed when this flight is given up on: its chunk may not write to the queue any more.
    closed: Arc<AtomicBool>,
    /// What the chunk's thread reports while it works (see `mediaa_core::pulse`).
    pulse: mediaa_core::pulse::Pulse,
    /// The pulse count and the moment it was last seen to move.
    seen_beats: u64,
    seen_at: Instant,
    handle: Option<JoinHandle<std::result::Result<Work, String>>>,
}

/// The store a chunk writes through. A flight that was given up on (reaped as a hang, or
/// detached by [`Drop for StageRunner`]) must not touch the queue file any more: the next
/// scheduler opens it. Its write is dropped, not failed; reads stay allowed.
struct GatedStore {
    inner: Arc<dyn Store>,
    closed: Arc<AtomicBool>,
}

impl Store for GatedStore {
    fn apply(&self, op: Op) -> mediaa_queue::Result<Outcome> {
        if self.closed.load(Ordering::Relaxed) {
            return Ok(Outcome::default());
        }
        self.inner.apply(op)
    }

    fn world(&self) -> mediaa_queue::Result<World> {
        self.inner.world()
    }

    fn now_ms(&self) -> i64 {
        self.inner.now_ms()
    }
}

/// How long a dropped runner gives its chunks to finish. A call still inside native code after
/// that is detached (see [`GatedStore`]).
const DROP_WAIT: Duration = Duration::from_secs(5);

/// Polls every stage. One flight per task; `step` returns before the chunk does.
pub struct StageRunner {
    stages: Vec<Arc<dyn Stage>>,
    flights: HashMap<TaskId, Flight>,
    host: Host,
    /// [`DROP_WAIT`]; a test may shorten it.
    drop_wait: Duration,
}

impl StageRunner {
    pub fn new(host: Host, stages: Vec<Arc<dyn Stage>>) -> StageRunner {
        StageRunner { stages, flights: HashMap::new(), host, drop_wait: DROP_WAIT }
    }
}

impl Drop for StageRunner {
    fn drop(&mut self) {
        // A chunk writes to the queue file and to the transcript. A scheduler that is gone
        // must leave no chunk behind: the next scheduler opens the same files, and a late
        // writer would replay a stale world (ids that are taken already, overwritten rows).
        // The wait is bounded: a call inside native code that never returns must not keep the
        // process alive. Such a call is detached, its writes are closed off and its model is
        // unloaded the way the watchdog does it.
        let mut flights = std::mem::take(&mut self.flights);
        for flight in flights.values() {
            flight.cancel.store(true, Ordering::Relaxed);
        }
        let deadline = Instant::now() + self.drop_wait;
        for flight in flights.values() {
            let Some(handle) = flight.handle.as_ref() else { continue };
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        for (id, flight) in flights.iter_mut() {
            let Some(handle) = flight.handle.take() else { continue };
            if handle.is_finished() {
                let _ = handle.join();
                continue;
            }
            flight.closed.store(true, Ordering::Relaxed);
            if let Some(model) = flight.pulse.model() {
                log::warn!("task {id}: the chunk did not return in {:.0} s, unloading {model}", self.drop_wait.as_secs_f32());
                self.host.engine.forget_loaded(&model);
            } else {
                log::warn!("task {id}: the chunk did not return in {:.0} s, detached", self.drop_wait.as_secs_f32());
            }
            drop(handle);
        }
    }
}

impl Runner for StageRunner {
    fn handles(&self, kind: &str) -> bool {
        self.stages.iter().any(|stage| stage.handles(kind))
    }

    fn step(&mut self, task: &Task, ctx: &mut dyn RunCtx) -> std::result::Result<Step, String> {
        if self.flights.get(&task.id).is_some_and(|flight| flight.gen != task.gen) {
            if let Some(flight) = self.flights.remove(&task.id) {
                // The task was restarted: the old chunk is detached and may not write any more.
                flight.cancel.store(true, Ordering::Relaxed);
                flight.closed.store(true, Ordering::Relaxed);
            }
        }
        if ctx.should_stop() {
            if let Some(flight) = self.flights.get(&task.id) {
                flight.cancel.store(true, Ordering::Relaxed);
            }
        }
        let running = self
            .flights
            .get(&task.id)
            .and_then(|flight| flight.handle.as_ref())
            .is_some_and(|handle| !handle.is_finished());
        if running {
            // The chunk is a blocking call of its own thread. It beats while it moves (a token,
            // an embedding, a downloaded chunk); silence longer than the resource's stale limit
            // is a call that will not return.
            let mut stuck: Option<(Duration, Option<String>)> = None;
            if let Some(flight) = self.flights.get_mut(&task.id) {
                let beats = flight.pulse.beats();
                if beats != flight.seen_beats {
                    flight.seen_beats = beats;
                    flight.seen_at = Instant::now();
                }
                let idle = flight.seen_at.elapsed();
                if idle > ctx.stale() {
                    flight.cancel.store(true, Ordering::Relaxed);
                    stuck = Some((idle, flight.pulse.model()));
                }
            }
            let Some((idle, model)) = stuck else {
                return Ok(Step::Busy);
            };
            if let Some(model) = model.as_deref() {
                log::warn!("task {} {}: no sign of life for {:.0} s, unloading {model}", task.id, task.kind, idle.as_secs_f32());
                self.host.engine.forget_loaded(model);
            } else {
                log::warn!("task {} {}: no sign of life for {:.0} s, the call is cancelled", task.id, task.kind, idle.as_secs_f32());
            }
            if let Some(flight) = self.flights.remove(&task.id) {
                // Never join: the call may never return. Its store writes are dropped from here
                // on and the transcript writes it may still make are idempotent.
                flight.cancel.store(true, Ordering::Relaxed);
                flight.closed.store(true, Ordering::Relaxed);
                drop(flight.handle);
            }
            return Ok(Step::Hang { idle });
        }
        if let Some(mut flight) = self.flights.remove(&task.id) {
            let handle = flight.handle.take().expect("a finished flight has a handle");
            return match handle.join() {
                Ok(Ok(Work::More { cursor, partial, progress })) => {
                    ctx.checkpoint(&cursor, &partial, progress)?;
                    Ok(Step::Chunk)
                }
                Ok(Ok(Work::Done(result))) => Ok(Step::Done(result)),
                Ok(Ok(Work::Stopped)) => Ok(Step::Stopped),
                Ok(Err(error)) => Err(error),
                Err(_) => Err("crashed".into()),
            };
        }
        if ctx.should_stop() {
            return Ok(Step::Stopped);
        }
        let stage = self
            .stages
            .iter()
            .find(|stage| stage.handles(&task.kind))
            .cloned()
            .ok_or_else(|| format!("no runner for {}", task.kind))?;
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancel);
        let closed = Arc::new(AtomicBool::new(false));
        let pulse = mediaa_core::pulse::Pulse::new();
        let attached = pulse.clone();
        let mut host = self.host.clone();
        let inner = host.store;
        host.store = Arc::new(GatedStore { inner, closed: Arc::clone(&closed) });
        let owned = task.clone();
        let handle = std::thread::Builder::new()
            .name(format!("queue-{}", owned.kind))
            .spawn(move || {
                // The pulse of this chunk: every model call of the stage below reports on it.
                let _pulse = attached.attach();
                let outcome = catch_unwind(AssertUnwindSafe(|| stage.chunk(&owned, &flag, &host)));
                match outcome {
                    Ok(Ok(work)) => Ok(work),
                    Ok(Err(error)) => Err(format!("{error:#}")),
                    Err(_) => Err("crashed".into()),
                }
            })
            .map_err(|error| error.to_string())?;
        self.flights.insert(task.id, Flight { gen: task.gen, cancel, closed, pulse, seen_beats: 0, seen_at: Instant::now(), handle: Some(handle) });
        Ok(Step::Busy)
    }
}

/// Pools from the settings budget. Memory is the shared byte budget; a task asks for none,
/// so the slot counts are the limit and the engine keeps the models.
pub fn pools(budget: &Budget) -> Vec<Pool> {
    let memory = budget.gpu_bytes().saturating_add(budget.ram_bytes());
    vec![
        Pool { resource: Resource::Speech, capacity: budget.limit(mediaa_core::agent::WorkKind::Transcription), memory },
        Pool { resource: Resource::Embed, capacity: budget.limit(mediaa_core::agent::WorkKind::Indexing), memory },
        Pool { resource: Resource::Llm, capacity: budget.limit(mediaa_core::agent::WorkKind::Agent), memory },
        Pool { resource: Resource::Provider, capacity: budget.limit(mediaa_core::agent::WorkKind::ProviderCalls), memory: 0 },
    ]
}

/// The queue file of a library's `.ai` folder. An old schema is dropped.
pub fn open_store(ai_dir: &Path) -> anyhow::Result<Arc<dyn Store>> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    Ok(Arc::new(mediaa_queue::SqliteStore::open(&ai_dir.join("queue.db"), clock)?))
}

/// Registers every kind the app runs: speech, topics and chat.
pub fn register_stages(
    sched: &Scheduler,
    ai_dir: &Path,
    engine: Arc<dyn Engine>,
    slots: Slots,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    streams: Arc<Streams>,
) {
    let host = Host {
        ai_dir: ai_dir.to_path_buf(),
        engine,
        store: sched.store().clone(),
        slots: slots.clone(),
        settings: settings.clone(),
        streams,
    };
    let mut stages: Vec<Arc<dyn Stage>> = vec![
        Arc::new(crate::transcribe::Transcribe::new(ai_dir, settings.clone())),
        Arc::new(crate::diarize::Diarize::new(ai_dir, settings.clone())),
        Arc::new(crate::index::Index::new(ai_dir, settings.clone())),
        Arc::new(crate::names::SpeakerNames::new(ai_dir, settings.clone())),
    ];
    stages.extend(crate::outline::stages(ai_dir, settings));
    stages.extend(crate::assistant::queued::stages());
    sched.register(Box::new(StageRunner::new(host, stages)));
}

/// Opens the library queue, recovers runs the process left behind and forgets old history.
pub fn open_scheduler(ai_dir: &Path, budget: &Budget) -> anyhow::Result<Arc<Scheduler>> {
    let sched = Arc::new(Scheduler::new(open_store(ai_dir)?, pools(budget)));
    sched.recover()?;
    sched.purge_history()?;
    Ok(sched)
}

/// Ticks the scheduler and tells the engine which models waiting work still needs.
pub fn spawn_host(sched: Arc<Scheduler>, engine: Arc<dyn Engine>, feed: Arc<Feed>, stop: Arc<AtomicBool>) -> std::io::Result<JoinHandle<()>> {
    let rx = sched.subscribe();
    std::thread::Builder::new().name("agent-queue".into()).spawn(move || {
        let mut purged = Instant::now();
        while !stop.load(Ordering::Relaxed) {
            while let Ok(event) = rx.try_recv() {
                feed.push(event);
            }
            if let Err(error) = sched.tick() {
                log::warn!("agent queue: {error:#}");
            }
            while let Ok(event) = rx.try_recv() {
                feed.push(event);
            }
            if let Ok(needs) = sched.models_needed() {
                let specs: Vec<String> = needs.into_iter().map(|need| need.model).collect();
                engine.set_wanted(&specs);
            }
            if purged.elapsed() > Duration::from_secs(24 * 60 * 60) {
                purged = Instant::now();
                if let Err(error) = sched.purge_history() {
                    log::warn!("agent queue: {error:#}");
                }
            }
            let wait = if sched.live().is_empty() { 200 } else { 20 };
            std::thread::sleep(Duration::from_millis(wait));
        }
    })
}

/// Runs until every task is finished or paused, or `limit` elapses.
pub fn drain(sched: &Scheduler, limit: Duration) {
    let start = Instant::now();
    loop {
        if sched.tick().is_err() {
            break;
        }
        let unfinished = sched.snapshot().ok().is_some_and(|snap| {
            snap.tasks.iter().any(|view| {
                !view.task.status.is_finished() && view.task.status != Status::Paused && view.task.status != Status::Stopping
            })
        });
        if !unfinished && sched.live().is_empty() {
            break;
        }
        if start.elapsed() > limit {
            break;
        }
        if !sched.live().is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// History kept by the queue. The host purges with [`Scheduler::purge_history`].
pub const QUEUE_HISTORY: Duration = HISTORY;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::assistant::queued::{answer_task, ANSWER_KIND};
    use crate::slots::Slots;
    use mediaa_core::agent::{AgentSettings, ModelChoices, ModelSource, WebSettings};
    use mediaa_core::chat::Author;
    use mediaa_core::transcript::Transcript;
    use mediaa_engine::testing::FakeEngine;
    use mediaa_engine::{ChatMessage, ChatRequest, ChatRole, Engine};
    use mediaa_queue::{failure_note, NewTask, Note, Resource, Status, Store, WaitReason};
    use serde_json::json;

    fn sched_with(stage: Arc<dyn Stage>) -> (tempfile::TempDir, Arc<Scheduler>) {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        let settings = AgentSettings::default();
        let sched = Arc::new(Scheduler::new(store.clone(), pools(&settings.budget)));
        let host = Host {
            ai_dir: dir.path().to_path_buf(),
            engine: Arc::new(FakeEngine::new()),
            store,
            slots: Slots::new(settings.budget),
            settings: Arc::new(AgentSettings::default),
            streams: Arc::new(Streams::default()),
        };
        sched.register(Box::new(StageRunner::new(host, vec![stage])));
        (dir, sched)
    }

    fn until(sched: &Scheduler, id: TaskId, pred: impl Fn(&mediaa_queue::Task) -> bool) -> mediaa_queue::Task {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let task = sched.store().get(id).unwrap().unwrap();
            if pred(&task) {
                return task;
            }
            assert!(Instant::now() < deadline, "{:?} {:?}", task.status, task.cursor);
            sched.tick().unwrap();
            if !sched.live().is_empty() {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    /// Waits for a fact about a chunk thread (it entered a gate or a model call). The deadline
    /// only fails a broken test; no test relies on how long anything takes.
    fn until_true(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !ready() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// The chunk is inside the model call `model`. Dropping or pausing the scheduler before
    /// this point races the chunk: it may cancel before it ever calls the engine.
    fn until_call(engine: &FakeEngine, model: &str) {
        until_true(&format!("the {model} call has started"), || engine.chats().iter().any(|chat| chat.model == model));
    }

    /// One number per chunk. `block` holds the chunk whose cursor is already `at`.
    struct Count {
        block_at: Option<u64>,
        hold: Arc<AtomicBool>,
    }

    impl Stage for Count {
        fn handles(&self, kind: &str) -> bool {
            kind == "count"
        }
        fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, _: &Host) -> anyhow::Result<Work> {
            let n = task.cursor.as_u64().unwrap_or(0);
            if self.block_at == Some(n) && self.hold.load(Ordering::Relaxed) {
                while self.hold.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(5));
                }
                if cancel.load(Ordering::Relaxed) {
                    return Ok(Work::Stopped);
                }
            }
            if n >= 10 {
                return Ok(Work::Done(json!({ "n": n })));
            }
            let next = n + 1;
            Ok(Work::More { cursor: json!(next), partial: json!(next), progress: next as f32 / 10.0 })
        }
    }

    #[test]
    fn a_crash_keeps_the_cursor_and_the_next_run_goes_on() {
        let hold = Arc::new(AtomicBool::new(true));
        let (_dir, sched) = sched_with(Arc::new(Count { block_at: Some(4), hold: hold.clone() }));
        let id = sched.enqueue(NewTask::new("count", "count", &json!({})).resource(Resource::Llm)).unwrap();
        until(&sched, id, |task| task.status == Status::Running && task.cursor.as_u64() == Some(4));
        let store = sched.store().clone();
        drop(sched);
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Running, "the dropped runner does not finish the row");
        let sched = Arc::new(Scheduler::new(store, pools(&AgentSettings::default().budget)));
        sched.recover().unwrap();
        let restored = sched.store().get(id).unwrap().unwrap();
        assert_eq!((restored.status, restored.cursor.as_u64()), (Status::Queued, Some(4)));
        hold.store(false, Ordering::Relaxed);
        sched.register(Box::new(StageRunner::new(
            Host {
                ai_dir: std::env::temp_dir(),
                engine: Arc::new(FakeEngine::new()),
                store: sched.store().clone(),
                slots: Slots::new(AgentSettings::default().budget),
                settings: Arc::new(AgentSettings::default),
                streams: Arc::new(Streams::default()),
            },
            vec![Arc::new(Count { block_at: Some(4), hold })],
        )));
        drain(&sched, Duration::from_secs(10));
        let done = sched.store().get(id).unwrap().unwrap();
        assert_eq!(done.status, Status::Done, "{:?}", done.error);
        assert_eq!(done.result, Some(json!({ "n": 10 })));
        assert!(done.cursor.as_u64().unwrap_or(0) >= 4, "the finished chunks were not repeated");
    }

    /// A dropped scheduler takes no chunk with it: the chunk writes the last rows of its work
    /// after the cancel, and the next scheduler opens the same queue file.
    #[test]
    fn dropping_the_scheduler_waits_for_the_chunk_to_finish() {
        struct Late { exited: Arc<AtomicBool> }
        impl Stage for Late {
            fn handles(&self, kind: &str) -> bool {
                kind == "count"
            }
            fn chunk(&self, _: &Task, cancel: &Arc<AtomicBool>, _: &Host) -> anyhow::Result<Work> {
                while !cancel.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                // The write a real chunk may still make when it has seen the cancel.
                std::thread::sleep(Duration::from_millis(150));
                self.exited.store(true, Ordering::SeqCst);
                Ok(Work::Stopped)
            }
        }
        let exited = Arc::new(AtomicBool::new(false));
        let (_dir, sched) = sched_with(Arc::new(Late { exited: exited.clone() }));
        let id = sched.enqueue(NewTask::new("count", "count", &json!({})).resource(Resource::Llm)).unwrap();
        until(&sched, id, |task| task.status == Status::Running);
        drop(sched);
        assert!(exited.load(Ordering::SeqCst), "the drop waits for the chunk to leave");
    }

    struct Hung {
        engine: Arc<FakeEngine>,
    }

    impl Stage for Hung {
        fn handles(&self, kind: &str) -> bool {
            kind == "model"
        }
        fn chunk(&self, _task: &Task, cancel: &Arc<AtomicBool>, _: &Host) -> anyhow::Result<Work> {
            let request = ChatRequest { model: "hangs/model".into(), messages: vec![ChatMessage::new(ChatRole::User, "q")], ..ChatRequest::default() };
            let answer = self.engine.chat(&request, cancel, &mut |_| true)?;
            Ok(Work::Done(json!({ "text": answer.content })))
        }
    }

    /// A call inside native code that never returns does not keep the process alive: the drop
    /// waits no longer than its bound, then stops the chunk's writes and unloads its model.
    #[test]
    fn dropping_the_scheduler_does_not_wait_for_a_call_that_never_returns() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        let engine = Arc::new(FakeEngine::new());
        let sched = Arc::new(Scheduler::new(store.clone(), pools(&AgentSettings::default().budget)));
        let mut runner = StageRunner::new(host_of(dir.path(), store.clone(), engine.clone()), vec![Arc::new(Hung { engine: engine.clone() })]);
        let wait = Duration::from_millis(200);
        runner.drop_wait = wait;
        sched.register(Box::new(runner));
        let id = sched.enqueue(NewTask::new("model", "ask", &json!({})).resource(Resource::Llm).model("hangs/model")).unwrap();
        sched.tick().unwrap();
        until_call(&engine, "hangs/model");
        let started = Instant::now();
        drop(sched);
        let took = started.elapsed();
        assert!(took >= wait, "the drop gave the chunk its bound: {took:?}");
        assert!(took < wait + Duration::from_secs(2), "the drop waited for a call that never returns: {took:?}");
        assert_eq!(engine.forgotten(), vec!["hangs/model".to_string()], "the detached call's model is unloaded");
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Running, "the dropped runner does not finish the row");
        engine.release_hang();
    }

    /// A chunk that only leaves after the drop gave up on it may not write to the queue.
    #[test]
    fn a_detached_chunk_does_not_write_after_the_scheduler_is_dropped() {
        struct Late {
            engine: Arc<FakeEngine>,
            wrote: Arc<AtomicBool>,
        }
        impl Stage for Late {
            fn handles(&self, kind: &str) -> bool {
                kind == "model"
            }
            fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
                let request =
                    ChatRequest { model: "hangs/model".into(), messages: vec![ChatMessage::new(ChatRole::User, "q")], ..ChatRequest::default() };
                let _ = self.engine.chat(&request, cancel, &mut |_| true)?;
                // The scheduler is gone by now: this write must not reach the queue file.
                let _ = host.store.set_detail(task.id, &json!({ "late": true }));
                self.wrote.store(true, Ordering::SeqCst);
                Ok(Work::Stopped)
            }
        }
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        let engine = Arc::new(FakeEngine::new());
        let wrote = Arc::new(AtomicBool::new(false));
        let sched = Arc::new(Scheduler::new(store.clone(), pools(&AgentSettings::default().budget)));
        let mut runner = StageRunner::new(
            host_of(dir.path(), store.clone(), engine.clone()),
            vec![Arc::new(Late { engine: engine.clone(), wrote: wrote.clone() })],
        );
        runner.drop_wait = Duration::from_millis(200);
        sched.register(Box::new(runner));
        let id = sched.enqueue(NewTask::new("model", "ask", &json!({})).resource(Resource::Llm).model("hangs/model")).unwrap();
        sched.tick().unwrap();
        until_call(&engine, "hangs/model");
        drop(sched);
        engine.release_hang();
        until_true("the detached chunk ran its late write", || wrote.load(Ordering::SeqCst));
        let task = store.get(id).unwrap().unwrap();
        assert_eq!(task.detail.get("late"), None, "a detached chunk wrote to the queue file");
    }

    #[test]
    fn pause_mid_chunk_stops_quickly_and_resume_keeps_the_cursor() {
        let hold = Arc::new(AtomicBool::new(true));
        let (_dir, sched) = sched_with(Arc::new(Count { block_at: Some(3), hold: hold.clone() }));
        let id = sched.enqueue(NewTask::new("count", "count", &json!({})).resource(Resource::Llm)).unwrap();
        until(&sched, id, |task| task.status == Status::Running && task.cursor.as_u64() == Some(3));
        let started = Instant::now();
        sched.pause(id).unwrap();
        let paused = until(&sched, id, |task| task.status == Status::Paused);
        assert!(sched.live().is_empty(), "the chunk has left the slot");
        assert!(started.elapsed() < Duration::from_millis(500), "stop took {:?}", started.elapsed());
        assert_eq!(paused.cursor.as_u64(), Some(3));
        hold.store(false, Ordering::Relaxed);
        assert!(sched.resume(id).unwrap());
        drain(&sched, Duration::from_secs(10));
        let done = sched.store().get(id).unwrap().unwrap();
        assert_eq!(done.status, Status::Done, "{:?}", done.error);
        assert_eq!(done.cursor.as_u64(), Some(10));
    }

    #[test]
    fn a_failure_cancels_the_dependent() {
        struct Boom;
        impl Stage for Boom {
            fn handles(&self, kind: &str) -> bool {
                kind == "count"
            }
            fn chunk(&self, _: &Task, _: &Arc<AtomicBool>, _: &Host) -> anyhow::Result<Work> {
                anyhow::bail!("model exploded")
            }
        }
        let (_dir, sched) = sched_with(Arc::new(Boom));
        let first = sched.enqueue(NewTask::new("count", "first", &json!({})).resource(Resource::Llm)).unwrap();
        let second = sched.enqueue(NewTask::new("count", "second", &json!({})).resource(Resource::Llm).needs(&[first])).unwrap();
        drain(&sched, Duration::from_secs(10));
        assert_eq!(sched.store().get(first).unwrap().unwrap().status, Status::Failed);
        let blocked = sched.store().get(second).unwrap().unwrap();
        assert_eq!(blocked.status, Status::Cancelled);
        assert_eq!(blocked.note, Some(failure_note("first")));
    }

    #[test]
    fn opening_the_scheduler_cancels_inline_work() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let store = open_store(dir.path()).unwrap();
            store.begin_inline("tool", "a tool", &json!({}), None, Some("rec")).unwrap()
        };
        let sched = open_scheduler(dir.path(), &AgentSettings::default().budget).unwrap();
        assert_eq!(sched.store().get(id).unwrap().unwrap().status, Status::Cancelled);
    }

    struct Gate {
        kind: &'static str,
        entered: Arc<Mutex<Vec<String>>>,
        release: Arc<AtomicBool>,
    }

    impl Stage for Gate {
        fn handles(&self, kind: &str) -> bool {
            kind == self.kind
        }
        fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, _: &Host) -> anyhow::Result<Work> {
            self.entered.lock().expect("entered").push(format!("{}:{}", self.kind, task.scope.as_deref().unwrap_or("")));
            while !self.release.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(Work::Stopped);
            }
            Ok(Work::Done(json!({})))
        }
    }

    fn host_of(dir: &std::path::Path, store: Arc<dyn Store>, engine: Arc<dyn Engine>) -> Host {
        Host {
            ai_dir: dir.to_path_buf(),
            engine,
            store,
            slots: Slots::new(AgentSettings::default().budget),
            settings: Arc::new(AgentSettings::default),
            streams: Arc::new(Streams::default()),
        }
    }

    #[test]
    fn speech_and_topics_run_at_the_same_time() {
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        let sched = Arc::new(Scheduler::new(store.clone(), pools(&AgentSettings::default().budget)));
        let entered = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(AtomicBool::new(false));
        let speech = Arc::new(Gate { kind: "transcribe", entered: entered.clone(), release: release.clone() });
        let topics = Arc::new(Gate { kind: "topic-digest", entered: entered.clone(), release: release.clone() });
        sched.register(Box::new(StageRunner::new(host_of(dir.path(), store, Arc::new(FakeEngine::new())), vec![speech, topics])));
        let speech_id = sched.enqueue(NewTask::new("transcribe", "a", &json!({})).resource(Resource::Speech).analysis("rec-a")).unwrap();
        let topic_id = sched.enqueue(NewTask::new("topic-digest", "b", &json!({})).resource(Resource::Llm).analysis("rec-b")).unwrap();
        sched.tick().unwrap();
        until_true("both chunks have entered their gates", || {
            let seen = entered.lock().expect("entered");
            seen.iter().any(|item| item.starts_with("transcribe")) && seen.iter().any(|item| item.starts_with("topic-digest"))
        });
        assert_eq!(sched.store().get(speech_id).unwrap().unwrap().status, Status::Running);
        assert_eq!(sched.store().get(topic_id).unwrap().unwrap().status, Status::Running);
        let seen = entered.lock().expect("entered").clone();
        assert!(seen.iter().any(|item| item.starts_with("transcribe")), "{seen:?}");
        assert!(seen.iter().any(|item| item.starts_with("topic-digest")), "{seen:?}");
        release.store(true, Ordering::Relaxed);
        drain(&sched, Duration::from_secs(10));
        assert_eq!(sched.store().get(speech_id).unwrap().unwrap().status, Status::Done);
        assert_eq!(sched.store().get(topic_id).unwrap().unwrap().status, Status::Done);
    }

    /// Pause during a model call returns within a fraction of a second. The chunk already
    /// saved stays; the call that was in flight is done again only from there.
    #[test]
    fn pause_mid_model_call_stops_and_resume_does_not_redo_the_chunk() {
        let engine = Arc::new(FakeEngine::new());
        let finish = Arc::new(AtomicBool::new(false));
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        let sched = Arc::new(Scheduler::new(store.clone(), pools(&AgentSettings::default().budget)));
        sched.register(Box::new(StageRunner::new(
            host_of(dir.path(), store, engine.clone()),
            vec![Arc::new(SlowModel { engine: engine.clone(), finish: finish.clone() })],
        )));
        let id = sched.enqueue(NewTask::new("model", "ask", &json!({})).resource(Resource::Llm).model("slow/model")).unwrap();
        until(&sched, id, |task| task.status == Status::Running && task.cursor.as_u64() == Some(1));
        until_call(&engine, "slow/model");
        let started = Instant::now();
        sched.pause(id).unwrap();
        let paused = until(&sched, id, |task| task.status == Status::Paused);
        assert!(started.elapsed() < Duration::from_millis(500), "stop took {:?}", started.elapsed());
        assert!(engine.cancel_observed());
        assert_eq!(paused.cursor, json!(1));
        assert_eq!(paused.partial, json!([1]));
        finish.store(true, Ordering::Relaxed);
        assert!(sched.resume(id).unwrap());
        drain(&sched, Duration::from_secs(10));
        let done = sched.store().get(id).unwrap().unwrap();
        assert_eq!(done.status, Status::Done, "{:?}", done.error);
        assert_eq!(done.result, Some(json!({ "done": [1] })));
        assert_eq!(engine.chats().len(), 1, "the resumed run does not call the model again");
    }

    /// A model call that never returns: the runner reports a hang, the model is unloaded, the
    /// task goes back to the queue at its cursor and the retry (on the fresh load) finishes.
    #[test]
    fn a_stuck_model_call_is_reaped_and_its_model_unloaded() {
        // The first `hangs/model` call never returns until the test lets it go.
        let dir = tempfile::tempdir().unwrap();
        let store = open_store(dir.path()).unwrap();
        let engine = Arc::new(FakeEngine::new());
        let sched = Arc::new(Scheduler::new(store.clone(), pools(&AgentSettings::default().budget)));
        sched.set_stale_for(Resource::Llm, Duration::from_millis(50));
        sched.register(Box::new(StageRunner::new(host_of(dir.path(), store, engine.clone()), vec![Arc::new(Hung { engine: engine.clone() })])));
        let id = sched.enqueue(NewTask::new("model", "ask", &json!({})).resource(Resource::Llm).model("hangs/model")).unwrap();
        sched.tick().unwrap();
        until_call(&engine, "hangs/model");
        let reaped = until(&sched, id, |task| task.detail.get("hang").is_some());
        assert_eq!(reaped.cursor, Value::Null, "the task kept its cursor");
        let note = serde_json::from_value::<Note>(reaped.detail["hang"].clone()).unwrap();
        assert!(matches!(note, Note::Hang { resource: Resource::Llm, .. }), "{note:?}");
        assert_eq!(engine.forgotten(), vec!["hangs/model".to_string()], "the stuck call's model is unloaded");

        engine.release_hang();
        drain(&sched, Duration::from_secs(10));
        let done = sched.store().get(id).unwrap().unwrap();
        assert_eq!(done.status, Status::Done, "{:?}", done.error);
        assert_eq!(done.result, Some(json!({ "text": "Привет, мир" })));
        assert_eq!(engine.chats().len(), 2, "the retry called the model again");
        assert_eq!(done.detail.get("hang"), None, "the note goes with the finished task");
    }

    struct SlowModel {
        engine: Arc<FakeEngine>,
        finish: Arc<AtomicBool>,
    }

    impl Stage for SlowModel {
        fn handles(&self, kind: &str) -> bool {
            kind == "model"
        }
        fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, _: &Host) -> anyhow::Result<Work> {
            let n = task.cursor.as_u64().unwrap_or(0);
            if n >= 1 && self.finish.load(Ordering::Relaxed) {
                return Ok(Work::Done(json!({ "done": [1] })));
            }
            if n == 0 {
                return Ok(Work::More { cursor: json!(1), partial: json!([1]), progress: 0.5 });
            }
            let request = ChatRequest { model: "slow/model".into(), messages: vec![ChatMessage::new(ChatRole::User, "q")], ..ChatRequest::default() };
            let _ = self.engine.chat(&request, cancel, &mut |_| true);
            if cancel.load(Ordering::Relaxed) {
                return Ok(Work::Stopped);
            }
            Ok(Work::More { cursor: json!(2), partial: json!([1, 2]), progress: 1.0 })
        }
    }

    struct ChatHold {
        entered: Arc<Mutex<Vec<i64>>>,
        release: Arc<AtomicBool>,
    }

    impl Stage for ChatHold {
        fn handles(&self, kind: &str) -> bool {
            kind == ANSWER_KIND
        }
        fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
            let chat = task.input["chat"].as_i64().unwrap_or(0);
            self.entered.lock().expect("entered").push(chat);
            while !self.release.load(Ordering::Relaxed) && !cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(5));
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(Work::Stopped);
            }
            let store = task.input["store"].as_str().unwrap_or("");
            let text = task.input["question"].as_str().unwrap_or("");
            Transcript::open(&host.ai_dir, store)?.add_message(chat, Author::Agent, text, &json!({}))?;
            Ok(Work::Done(json!({})))
        }
    }

    #[test]
    fn a_second_chat_waits_and_both_answers_stay_in_their_chats() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let t = Transcript::open(ai, "rec").unwrap();
        let first = t.create_chat("Первый").unwrap();
        let second = t.create_chat("Второй").unwrap();
        drop(t);
        let mut budget = AgentSettings::default().budget;
        budget.agent = 1;
        let store = open_store(ai).unwrap();
        let sched = Arc::new(Scheduler::new(store.clone(), pools(&budget)));
        let entered = Arc::new(Mutex::new(Vec::new()));
        let release = Arc::new(AtomicBool::new(false));
        sched.register(Box::new(StageRunner::new(
            host_of(ai, store, Arc::new(FakeEngine::new())),
            vec![Arc::new(ChatHold { entered: entered.clone(), release: release.clone() })],
        )));
        let ask = |chat: i64, question: &str| {
            let input = json!({ "chat": chat, "store": "rec", "question": question });
            sched.enqueue(answer_task("rec", chat, question, input, Resource::Llm, None)).unwrap()
        };
        let a = ask(first, "первый ответ");
        let b = ask(second, "второй ответ");
        sched.tick().unwrap();
        until_true("the first answer has started", || !entered.lock().expect("entered").is_empty());
        assert_eq!(sched.store().get(a).unwrap().unwrap().status, Status::Running);
        assert_eq!(sched.store().get(b).unwrap().unwrap().status, Status::Queued);
        let waiting = sched.snapshot().unwrap().tasks.into_iter().find(|view| view.task.id == b).unwrap();
        assert_eq!(waiting.wait, Some(WaitReason::Turn));
        assert_eq!(*entered.lock().expect("entered"), vec![first], "the second chat has not started");
        release.store(true, Ordering::Relaxed);
        drain(&sched, Duration::from_secs(10));
        let t = Transcript::open(ai, "rec").unwrap();
        let text = |chat| t.messages(chat).unwrap().into_iter().map(|m| m.text).collect::<Vec<_>>();
        assert_eq!(text(first), ["первый ответ"]);
        assert_eq!(text(second), ["второй ответ"]);
        assert_eq!(sched.store().get(a).unwrap().unwrap().status, Status::Done);
        assert_eq!(sched.store().get(b).unwrap().unwrap().status, Status::Done);
    }

    #[test]
    fn a_chat_answer_continues_after_the_scheduler_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let engine = Arc::new(FakeEngine::new());
        engine.chat_answers(&[r#"{"thought":"","action":"answer","answer":"Готово."}"#]);
        let slow = Arc::new(AtomicBool::new(true));
        let flag = slow.clone();
        let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(move || {
            let mut models = ModelChoices::default();
            if flag.load(Ordering::SeqCst) {
                models.agent = Some(ModelSource::HuggingFace { repo: "slow/model".into() });
            }
            AgentSettings {
                verify_answers: false,
                models,
                budget: Budget { agent: 1, ..Default::default() },
                web: WebSettings { enabled: false, ..Default::default() },
                ..Default::default()
            }
        });
        Transcript::open(ai, "rec").unwrap().create_chat("").unwrap();
        let chat = Transcript::open(ai, "rec").unwrap().chats().unwrap()[0].id;
        let input = json!({ "chat": chat, "store": "rec", "question": "Когда?", "path": "rec.mp4", "file_id": "rec" });
        let open = |engine: Arc<dyn Engine>| {
            let sched = open_scheduler(ai, &current().budget).unwrap();
            register_stages(&sched, ai, engine, Slots::new(current().budget), current.clone(), Arc::new(Streams::default()));
            sched
        };
        let sched = open(engine.clone());
        let id = sched.enqueue(answer_task("rec", chat, "Когда?", input, Resource::Llm, Some("slow/model"))).unwrap();
        until(&sched, id, |task| task.status == Status::Running);
        until_call(&engine, "slow/model");
        // The drop waits for the chunk, so by here nothing of the old scheduler can write.
        drop(sched);
        assert!(engine.cancel_observed(), "dropping the scheduler cancels the model call");
        assert!(Transcript::open(ai, "rec").unwrap().messages(chat).unwrap().is_empty());
        slow.store(false, Ordering::SeqCst);
        let sched = open(engine.clone());
        drain(&sched, Duration::from_secs(30));
        let task = sched.store().get(id).unwrap().unwrap();
        assert_eq!(task.status, Status::Done, "{:?}", task.error);
        let messages = Transcript::open(ai, "rec").unwrap().messages(chat).unwrap();
        let answer = messages.iter().find(|m| m.author == Author::Agent).expect("the answer");
        assert_eq!(answer.text, "Готово.");
        assert_eq!(answer.details["continuing"], json!(true));
    }
}
