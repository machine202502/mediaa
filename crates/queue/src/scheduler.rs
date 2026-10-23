use crate::events::Event;
use crate::runner::{RunCtx, Runner, Step};
use crate::snapshot::{self, ModelNeed, PoolLoad, Snapshot};
use crate::state::{
    self, is_claimable, participant, Child, NewTask, Op, Outcome, Resource, Stats, Status, Task,
};
use crate::store::Store;
use crate::{Error, Result, TaskId, HISTORY};
use crossbeam_channel::{Receiver, Sender};
use serde_json::Value;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const DEFAULT_STALE: Duration = Duration::from_secs(15 * 60);

fn default_stale(resource: Resource) -> Duration {
    match resource {
        Resource::Speech => Duration::from_secs(60 * 60),
        Resource::Embed => Duration::from_secs(5 * 60),
        Resource::Llm => Duration::from_secs(10 * 60),
        Resource::Provider => Duration::from_secs(10 * 60),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Pool {
    pub resource: Resource,
    pub capacity: u32,
    pub memory: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Turn {
    pub resource: Resource,
    pub participant: String,
    pub chat: bool,
    pub bg_ready: bool,
    pub chat_ready: bool,
    pub ready: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct LiveRun {
    pub id: TaskId,
    pub gen: i64,
    pub resource: Resource,
    pub memory: u64,
}

#[derive(Clone, Default)]
struct Fairness {
    last_chat: Option<String>,
    last_bg: Option<String>,
    issued: u64,
}

struct Run {
    gen: i64,
    resource: Resource,
    memory: u64,
    stop: Arc<AtomicBool>,
    participant: String,
    chat: bool,
    bg_ready: bool,
    chat_ready: bool,
    ready: Vec<String>,
    chunked: bool,
}

struct Inner {
    runners: Vec<Box<dyn Runner>>,
    runs: HashMap<TaskId, Run>,
    fair: HashMap<Resource, Fairness>,
    turns: Vec<Turn>,
    events: Vec<Event>,
    subs: Vec<Sender<Event>>,
    stale: Duration,
    stale_for: HashMap<Resource, Duration>,
}

pub struct Scheduler {
    store: Arc<dyn Store>,
    pools: Vec<Pool>,
    inner: Mutex<Inner>,
}

impl Scheduler {
    pub fn new(store: Arc<dyn Store>, pools: Vec<Pool>) -> Scheduler {
        Scheduler {
            store,
            pools,
            inner: Mutex::new(Inner {
                runners: Vec::new(),
                runs: HashMap::new(),
                fair: HashMap::new(),
                turns: Vec::new(),
                events: Vec::new(),
                subs: Vec::new(),
                stale: DEFAULT_STALE,
                stale_for: HashMap::new(),
            }),
        }
    }

    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    pub fn register(&self, runner: Box<dyn Runner>) {
        self.inner.lock().expect("queue lock").runners.push(runner);
    }

    pub fn set_stale(&self, stale: Duration) {
        self.inner.lock().expect("queue lock").stale = stale;
    }

    pub fn set_stale_for(&self, resource: Resource, stale: Duration) {
        self.inner.lock().expect("queue lock").stale_for.insert(resource, stale);
    }

    fn stale_limit(&self, resource: Resource) -> Duration {
        let guard = self.inner.lock().expect("queue lock");
        guard.stale_for.get(&resource).copied().unwrap_or_else(|| default_stale(resource))
    }

    pub fn events(&self) -> Vec<Event> {
        self.inner.lock().expect("queue lock").events.clone()
    }

    pub fn turns(&self) -> Vec<Turn> {
        self.inner.lock().expect("queue lock").turns.clone()
    }

    pub fn live(&self) -> Vec<LiveRun> {
        let guard = self.inner.lock().expect("queue lock");
        let mut runs: Vec<LiveRun> = guard
            .runs
            .iter()
            .map(|(id, run)| LiveRun {
                id: *id,
                gen: run.gen,
                resource: run.resource,
                memory: run.memory,
            })
            .collect();
        runs.sort_by_key(|run| run.id);
        runs
    }

    pub fn subscribe(&self) -> Receiver<Event> {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.inner.lock().expect("queue lock").subs.push(tx);
        rx
    }

    pub fn snapshot(&self) -> Result<Snapshot> {
        let world = self.store.world()?;
        let now = self.store.now_ms();
        let (runs, fair) = {
            let guard = self.inner.lock().expect("queue lock");
            let runs: Vec<(TaskId, Resource)> = guard
                .runs
                .iter()
                .map(|(id, run)| (*id, run.resource))
                .collect();
            (runs, guard.fair.clone())
        };
        let loads = pool_loads(&world, &self.pools, &runs, &fair);
        Ok(snapshot::build(&world, &runs, &loads, now))
    }

    pub fn models_needed(&self) -> Result<Vec<ModelNeed>> {
        let world = self.store.world()?;
        Ok(state::models_needed(&world.tasks)
            .into_iter()
            .map(|(model, tasks)| ModelNeed { model, tasks })
            .collect())
    }

    pub fn purge_history(&self) -> Result<usize> {
        let cutoff = self
            .store
            .now_ms()
            .saturating_sub(HISTORY.as_millis() as i64);
        Ok(self.exec(Op::Purge { cutoff_ms: cutoff })?.changed)
    }

    pub fn tick(&self) -> Result<Vec<Event>> {
        let start = self.inner.lock().expect("queue lock").events.len();
        let stale = self.inner.lock().expect("queue lock").stale;
        let older = self.store.now_ms().saturating_sub(stale.as_millis() as i64);
        self.exec(Op::Reap {
            older_than_ms: older,
        })?;
        self.sync_runs()?;
        let initial = self.live_ids();
        for id in initial {
            self.step_one(id)?;
        }
        let started = self.dispatch()?;
        for id in started {
            self.step_one(id)?;
        }
        Ok(self.events_since(start))
    }

    pub fn pause(&self, id: TaskId) -> Result<Outcome> {
        self.raise(&self.subtree(id)?);
        self.exec(Op::Pause(id))
    }

    pub fn resume(&self, id: TaskId) -> Result<bool> {
        Ok(self.exec(Op::Resume(id))?.accepted)
    }

    pub fn cancel(&self, id: TaskId) -> Result<Outcome> {
        self.raise(&self.subtree(id)?);
        self.exec(Op::Cancel(id))
    }

    pub fn cancel_scope(&self, scope: &str) -> Result<Outcome> {
        let world = self.store.world()?;
        let ids: Vec<TaskId> = world
            .tasks
            .iter()
            .filter(|task| task.scope.as_deref() == Some(scope))
            .map(|task| task.id)
            .collect();
        self.raise(&ids);
        self.exec(Op::CancelScope(scope.to_string()))
    }

    pub fn cancel_analysis(&self, scope: &str) -> Result<Outcome> {
        let world = self.store.world()?;
        let roots: Vec<TaskId> = world
            .tasks
            .iter()
            .filter(|task| {
                task.scope.as_deref() == Some(scope)
                    && task.kind == "analysis"
                    && task.parent.is_none()
            })
            .map(|task| task.id)
            .collect();
        let mut ids = Vec::new();
        for root in roots {
            ids.extend(state::subtree_ids(&world, root));
        }
        self.raise(&ids);
        self.exec(Op::CancelAnalysis(scope.to_string()))
    }

    pub fn restart(&self, id: TaskId) -> Result<Outcome> {
        self.exec(Op::Restart(id))
    }

    pub fn recover(&self) -> Result<Outcome> {
        let out = self.exec(Op::Recover)?;
        let mut guard = self.inner.lock().expect("queue lock");
        let ids: Vec<TaskId> = guard.runs.keys().copied().collect();
        for id in ids {
            take_run(&mut guard, id);
        }
        Ok(out)
    }

    pub fn enqueue(&self, task: NewTask) -> Result<TaskId> {
        let out = self.exec(Op::Enqueue(task))?;
        out.ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("enqueue returned no id".into()))
    }

    pub fn enqueue_tree(
        &self,
        root: NewTask,
        children: Vec<Child>,
    ) -> Result<(TaskId, Vec<TaskId>)> {
        let out = self.exec(Op::EnqueueTree { root, children })?;
        let root_id = out
            .ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("enqueue_tree returned no id".into()))?;
        let direct = out.ids.iter().skip(1).take(out.changed).copied().collect();
        Ok((root_id, direct))
    }

    pub fn enqueue_once(&self, task: NewTask) -> Result<TaskId> {
        let out = self.exec(Op::EnqueueOnce(task))?;
        out.ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("enqueue_once returned no id".into()))
    }

    pub fn begin_inline(
        &self,
        kind: &str,
        title: &str,
        input: &Value,
        parent: Option<TaskId>,
        scope: Option<&str>,
    ) -> Result<TaskId> {
        let out = self.exec(Op::BeginInline {
            kind: kind.to_string(),
            title: title.to_string(),
            input: input.clone(),
            parent,
            scope: scope.map(str::to_string),
        })?;
        out.ids
            .first()
            .copied()
            .ok_or_else(|| Error::Bad("inline returned no id".into()))
    }

    pub fn finish_inline(&self, id: TaskId, result: &Value) -> Result<Outcome> {
        let gen = self.store.gen_of(id)?;
        self.exec(Op::Complete {
            id,
            gen,
            result: result.clone(),
            children: Vec::new(),
        })
    }

    pub fn fail_inline(&self, id: TaskId, error: &str) -> Result<Outcome> {
        let gen = self.store.gen_of(id)?;
        self.exec(Op::Fail {
            id,
            gen,
            error: error.to_string(),
        })
    }

    pub fn add_stats(&self, id: TaskId, delta: &Stats) -> Result<Outcome> {
        self.exec(Op::AddStats {
            id,
            delta: delta.clone(),
        })
    }

    pub fn set_detail(&self, id: TaskId, detail: &Value) -> Result<Outcome> {
        self.exec(Op::SetDetail {
            id,
            detail: detail.clone(),
        })
    }

    pub fn set_priority(&self, id: TaskId, priority: i64) -> Result<Outcome> {
        self.exec(Op::SetPriority { id, priority })
    }

    pub fn remove(&self, id: TaskId) -> Result<Outcome> {
        let ids = self.subtree(id)?;
        self.raise(&ids);
        let out = self.exec(Op::Remove(id))?;
        let mut guard = self.inner.lock().expect("queue lock");
        for id in ids {
            take_run(&mut guard, id);
        }
        Ok(out)
    }

    pub fn clear_done(&self) -> Result<Outcome> {
        self.exec(Op::ClearDone)
    }

    fn exec(&self, op: Op) -> Result<Outcome> {
        let out = self.store.apply(op)?;
        self.push_events(&out.events);
        Ok(out)
    }

    fn push_events(&self, events: &[Event]) {
        if events.is_empty() {
            return;
        }
        let mut guard = self.inner.lock().expect("queue lock");
        for event in events {
            guard.events.push(event.clone());
            guard.subs.retain(|sub| sub.send(event.clone()).is_ok());
        }
    }

    fn push_event(&self, event: Event) {
        self.push_events(std::slice::from_ref(&event));
    }

    fn events_since(&self, start: usize) -> Vec<Event> {
        let guard = self.inner.lock().expect("queue lock");
        guard.events.get(start..).unwrap_or_default().to_vec()
    }

    fn subtree(&self, id: TaskId) -> Result<Vec<TaskId>> {
        let world = self.store.world()?;
        Ok(state::subtree_ids(&world, id))
    }

    fn raise(&self, ids: &[TaskId]) {
        let guard = self.inner.lock().expect("queue lock");
        for id in ids {
            if let Some(run) = guard.runs.get(id) {
                run.stop.store(true, Ordering::Relaxed);
            }
        }
    }

    fn live_ids(&self) -> Vec<TaskId> {
        let guard = self.inner.lock().expect("queue lock");
        let mut ids: Vec<TaskId> = guard.runs.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    fn sync_runs(&self) -> Result<()> {
        let world = self.store.world()?;
        let mut guard = self.inner.lock().expect("queue lock");
        let ids: Vec<TaskId> = guard.runs.keys().copied().collect();
        for id in ids {
            let gen = guard.runs.get(&id).map(|run| run.gen);
            let action = match (gen, world.task(id)) {
                (Some(gen), Some(task)) if task.gen == gen && task.status == Status::Stopping => {
                    Sync::Stop
                }
                (Some(gen), Some(task)) if task.gen == gen && task.status == Status::Running => {
                    Sync::Keep
                }
                _ => Sync::Drop,
            };
            match action {
                Sync::Stop => {
                    if let Some(run) = guard.runs.get(&id) {
                        run.stop.store(true, Ordering::Relaxed);
                    }
                }
                Sync::Drop => take_run(&mut guard, id),
                Sync::Keep => {}
            }
        }
        Ok(())
    }

    fn usage(&self, resource: Resource) -> (usize, u64) {
        let guard = self.inner.lock().expect("queue lock");
        let mut slots = 0;
        let mut memory = 0;
        for run in guard.runs.values() {
            if run.resource == resource {
                slots += 1;
                memory += run.memory;
            }
        }
        (slots, memory)
    }

    fn dispatch(&self) -> Result<Vec<TaskId>> {
        let mut started = Vec::new();
        for pool in self.pools.clone() {
            for _ in 0..pool.capacity.max(1) {
                let world = self.store.world()?;
                let (used_slots, used_mem) = self.usage(pool.resource);
                if used_slots >= pool.capacity as usize {
                    break;
                }
                let memory_left = pool.memory.saturating_sub(used_mem);
                let ready = ready_tasks(&world, pool.resource, memory_left);
                if ready.is_empty() {
                    break;
                }
                let (bg_ready, chat_ready, ready_keys) = ready_flags(&ready);
                let Some(task) = ({
                    let mut guard = self.inner.lock().expect("queue lock");
                    let fair = guard.fair.entry(pool.resource).or_default();
                    pick(fair, &ready)
                }) else {
                    break;
                };
                let out = self.exec(Op::Claim(task.id))?;
                if !out.accepted {
                    break;
                }
                let gen = self.store.gen_of(task.id)?;
                self.inner.lock().expect("queue lock").runs.insert(
                    task.id,
                    Run {
                        gen,
                        resource: pool.resource,
                        memory: task.memory,
                        stop: Arc::new(AtomicBool::new(false)),
                        participant: participant(&task),
                        chat: task.role.is_chat(),
                        bg_ready,
                        chat_ready,
                        ready: ready_keys,
                        chunked: false,
                    },
                );
                started.push(task.id);
            }
        }
        Ok(started)
    }

    fn step_one(&self, id: TaskId) -> Result<()> {
        let world = self.store.world()?;
        let Some(task) = world.task(id).cloned() else {
            self.remove_run(id);
            return Ok(());
        };
        let meta = {
            let guard = self.inner.lock().expect("queue lock");
            guard.runs.get(&id).and_then(|run| {
                if task.gen != run.gen || !matches!(task.status, Status::Running | Status::Stopping)
                {
                    None
                } else {
                    Some(StepMeta {
                        gen: run.gen,
                        stop: Arc::clone(&run.stop),
                        resource: run.resource,
                        participant: run.participant.clone(),
                        chat: run.chat,
                        bg_ready: run.bg_ready,
                        chat_ready: run.chat_ready,
                        ready: run.ready.clone(),
                    })
                }
            })
        };
        let Some(meta) = meta else {
            self.remove_run(id);
            return Ok(());
        };
        let before = task.cursor.clone();
        let Some((pos, mut runner)) = self.take_runner(&task.kind) else {
            self.apply_step(id, &meta, &before, Err("no runner for this kind".into()))?;
            return Ok(());
        };
        let stepped = {
            let emit = |event: Event| self.push_event(event);
            let mut ctx = SchedCtx {
                sched: self,
                id,
                gen: meta.gen,
                stop: Arc::clone(&meta.stop),
                resource: meta.resource,
                emit: &emit,
            };
            catch_unwind(AssertUnwindSafe(|| runner.step(&task, &mut ctx)))
        };
        self.put_runner(pos, runner);
        let stepped = match stepped {
            Ok(result) => result,
            Err(_) => Err("crashed".into()),
        };
        self.apply_step(id, &meta, &before, stepped)
    }

    fn apply_step(
        &self,
        id: TaskId,
        meta: &StepMeta,
        before: &Value,
        step: std::result::Result<Step, String>,
    ) -> Result<()> {
        let stop_now = meta.stop.load(Ordering::Relaxed)
            || self
                .store
                .world()?
                .task(id)
                .is_some_and(|task| task.status == Status::Stopping);
        match step {
            Ok(Step::Hang { idle }) => {
                if stop_now {
                    self.exec(Op::FinishStop { id, gen: meta.gen })?;
                } else {
                    self.exec(Op::ReapRun {
                        id,
                        gen: meta.gen,
                        note: Some(state::hang_note(meta.resource, idle)),
                    })?;
                }
                self.remove_run(id);
            }
            Ok(Step::Busy) => {
                self.exec(Op::Heartbeat { id, gen: meta.gen })?;
            }
            Ok(Step::Chunk) => {
                if stop_now {
                    self.exec(Op::FinishStop { id, gen: meta.gen })?;
                } else {
                    self.exec(Op::Yield { id, gen: meta.gen })?;
                }
                self.remove_run(id);
            }
            Ok(Step::Done(result)) => {
                let out = self.exec(Op::Complete {
                    id,
                    gen: meta.gen,
                    result,
                    children: Vec::new(),
                })?;
                if !out.accepted {
                    self.exec(Op::FinishStop { id, gen: meta.gen })?;
                }
                self.remove_run(id);
            }
            Ok(Step::Failed(error)) => {
                if stop_now {
                    self.exec(Op::FinishStop { id, gen: meta.gen })?;
                } else {
                    self.exec(Op::Fail {
                        id,
                        gen: meta.gen,
                        error,
                    })?;
                }
                self.remove_run(id);
            }
            Ok(Step::Stopped) => {
                self.exec(Op::FinishStop { id, gen: meta.gen })?;
                self.remove_run(id);
            }
            Err(error) => {
                if stop_now {
                    self.exec(Op::FinishStop { id, gen: meta.gen })?;
                } else {
                    self.exec(Op::Fail {
                        id,
                        gen: meta.gen,
                        error,
                    })?;
                }
                self.remove_run(id);
            }
        }
        self.note_turn(id, before, meta);
        Ok(())
    }

    fn note_turn(&self, id: TaskId, before: &Value, meta: &StepMeta) {
        let Ok(world) = self.store.world() else {
            return;
        };
        let Some(task) = world.task(id) else { return };
        if task.cursor == *before || task.cursor.is_null() {
            return;
        }
        self.inner.lock().expect("queue lock").turns.push(Turn {
            resource: meta.resource,
            participant: meta.participant.clone(),
            chat: meta.chat,
            bg_ready: meta.bg_ready,
            chat_ready: meta.chat_ready,
            ready: meta.ready.clone(),
        });
    }

    fn remove_run(&self, id: TaskId) {
        let mut guard = self.inner.lock().expect("queue lock");
        take_run(&mut guard, id);
    }

    fn take_runner(&self, kind: &str) -> Option<(usize, Box<dyn Runner>)> {
        let mut guard = self.inner.lock().expect("queue lock");
        let pos = guard
            .runners
            .iter()
            .position(|runner| runner.handles(kind))?;
        Some((pos, guard.runners.remove(pos)))
    }

    fn put_runner(&self, pos: usize, runner: Box<dyn Runner>) {
        let mut guard = self.inner.lock().expect("queue lock");
        let pos = pos.min(guard.runners.len());
        guard.runners.insert(pos, runner);
    }
}

struct StepMeta {
    gen: i64,
    stop: Arc<AtomicBool>,
    resource: Resource,
    participant: String,
    chat: bool,
    bg_ready: bool,
    chat_ready: bool,
    ready: Vec<String>,
}

enum Sync {
    Keep,
    Stop,
    Drop,
}

struct SchedCtx<'a> {
    sched: &'a Scheduler,
    id: TaskId,
    gen: i64,
    stop: Arc<AtomicBool>,
    resource: Resource,
    emit: &'a dyn Fn(Event),
}

impl RunCtx for SchedCtx<'_> {
    fn checkpoint(&mut self, cursor: &Value, partial: &Value, progress: f32) -> Result<()> {
        let out = self.sched.store.apply(Op::Checkpoint {
            id: self.id,
            gen: self.gen,
            cursor: cursor.clone(),
            partial: partial.clone(),
            progress,
        })?;
        if out.accepted {
            let mut guard = self.sched.inner.lock().expect("queue lock");
            if let Some(run) = guard.runs.get_mut(&self.id) {
                run.chunked = true;
            }
        }
        for event in out.events {
            (self.emit)(event);
        }
        Ok(())
    }

    fn should_stop(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn progress(&mut self, progress: f32) -> Result<()> {
        self.sched.store.apply(Op::Progress {
            id: self.id,
            gen: self.gen,
            progress,
        })?;
        Ok(())
    }

    fn stale(&self) -> Duration {
        self.sched.stale_limit(self.resource)
    }
}

fn take_run(guard: &mut Inner, id: TaskId) {
    let Some(run) = guard.runs.remove(&id) else {
        return;
    };
    if run.chunked {
        return;
    }
    if let Some(fair) = guard.fair.get_mut(&run.resource) {
        fair.issued = fair.issued.saturating_sub(1);
    }
}

fn pool_loads(
    world: &state::World,
    pools: &[Pool],
    runs: &[(TaskId, Resource)],
    fair: &HashMap<Resource, Fairness>,
) -> Vec<PoolLoad> {
    pools
        .iter()
        .map(|pool| {
            let used_slots = runs
                .iter()
                .filter(|(_, resource)| *resource == pool.resource)
                .count();
            let used_mem: u64 = runs
                .iter()
                .filter(|(_, resource)| *resource == pool.resource)
                .filter_map(|(id, _)| world.task(*id).map(|task| task.memory))
                .sum();
            let free_slots = (pool.capacity as usize).saturating_sub(used_slots);
            let memory_left = pool.memory.saturating_sub(used_mem);
            let ready = ready_tasks(world, pool.resource, memory_left);
            let fairness = fair.get(&pool.resource).cloned().unwrap_or_default();
            let winners = peek_winners(&fairness, &ready, free_slots);
            PoolLoad {
                resource: pool.resource,
                free_slots,
                memory_left,
                winners,
            }
        })
        .collect()
}

fn ready_tasks(world: &state::World, resource: Resource, memory_left: u64) -> Vec<Task> {
    let mut ready: Vec<Task> = world
        .tasks
        .iter()
        .filter(|task| {
            task.resource == Some(resource)
                && task.memory <= memory_left
                && is_claimable(world, task.id)
        })
        .cloned()
        .collect();
    ready.sort_by_key(|task| task.id);
    ready
}

fn ready_flags(ready: &[Task]) -> (bool, bool, Vec<String>) {
    let bg_ready = ready.iter().any(|task| !task.role.is_chat());
    let chat_ready = ready.iter().any(|task| task.role.is_chat());
    let mut keys: Vec<String> = ready.iter().map(participant).collect();
    keys.sort();
    keys.dedup();
    (bg_ready, chat_ready, keys)
}

fn pick(fair: &mut Fairness, ready: &[Task]) -> Option<Task> {
    let chats: Vec<&Task> = ready.iter().filter(|task| task.role.is_chat()).collect();
    let bgs: Vec<&Task> = ready.iter().filter(|task| !task.role.is_chat()).collect();
    let want_bg = fair.issued % 3 == 2;
    let chosen = if want_bg && !bgs.is_empty() {
        next_in(&bgs, fair.last_bg.as_deref())
    } else if !chats.is_empty() {
        next_in(&chats, fair.last_chat.as_deref())
    } else {
        next_in(&bgs, fair.last_bg.as_deref())
    }?;
    let key = participant(&chosen);
    if chosen.role.is_chat() {
        fair.last_chat = Some(key);
    } else {
        fair.last_bg = Some(key);
    }
    fair.issued += 1;
    Some(chosen)
}

fn next_in(tasks: &[&Task], last: Option<&str>) -> Option<Task> {
    let mut keys: Vec<String> = tasks.iter().map(|task| participant(task)).collect();
    keys.sort();
    keys.dedup();
    if keys.is_empty() {
        return None;
    }
    let index = match last {
        Some(last) => keys.iter().position(|key| key.as_str() > last).unwrap_or(0),
        None => 0,
    };
    let key = keys[index].clone();
    tasks
        .iter()
        .filter(|task| participant(task) == key)
        .min_by_key(|task| task.id)
        .map(|task| (*task).clone())
}

fn peek_winners(fair: &Fairness, ready: &[Task], slots: usize) -> Vec<TaskId> {
    let mut fair = fair.clone();
    let mut left = ready.to_vec();
    let mut winners = Vec::new();
    for _ in 0..slots {
        let Some(task) = pick(&mut fair, &left) else {
            break;
        };
        left.retain(|item| item.id != task.id);
        winners.push(task.id);
    }
    winners
}
