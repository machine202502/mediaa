use mediaa_queue::state;
use mediaa_queue::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

fn script(chunks: u32, busy: u32, slow_stop: u32) -> Value {
    json!({ "chunks": chunks, "busy": busy, "slow_stop": slow_stop })
}

fn failing(chunks: u32, busy: u32) -> Value {
    json!({ "chunks": chunks, "busy": busy, "fail_at": 1 })
}

#[derive(serde::Deserialize)]
struct Script {
    chunks: u32,
    #[serde(default)]
    busy: u32,
    #[serde(default)]
    slow_stop: u32,
    #[serde(default)]
    hang: bool,
    #[serde(default)]
    fail_at: Option<u32>,
}

#[derive(Clone, Copy)]
struct RunState {
    busy_left: u32,
    slow_left: u32,
}

#[derive(Default)]
struct ScriptRunner {
    state: HashMap<(TaskId, i64), RunState>,
    failed: HashSet<TaskId>,
}

impl Runner for ScriptRunner {
    fn handles(&self, _kind: &str) -> bool {
        true
    }

    fn step(&mut self, task: &Task, ctx: &mut dyn RunCtx) -> std::result::Result<Step, String> {
        let script: Script =
            serde_json::from_value(task.input.clone()).map_err(|err| err.to_string())?;
        let committed = cursor_u32(&task.cursor);
        let key = (task.id, task.gen);
        let mut state = self.state.get(&key).copied().unwrap_or(RunState {
            busy_left: script.busy,
            slow_left: script.slow_stop,
        });
        let already_failed = self.failed.contains(&task.id);
        let step = next_step(&script, &mut state, committed, already_failed, ctx)?;
        if matches!(step, Step::Failed(_)) {
            self.failed.insert(task.id);
        }
        self.state.insert(key, state);
        Ok(step)
    }
}

fn next_step(
    script: &Script,
    state: &mut RunState,
    committed: u32,
    already_failed: bool,
    ctx: &mut dyn RunCtx,
) -> std::result::Result<Step, String> {
    if script.hang && committed >= script.chunks {
        return Ok(Step::Hang {
            idle: Duration::from_secs(900),
        });
    }
    if ctx.should_stop() {
        if state.slow_left > 0 {
            state.slow_left -= 1;
            return Ok(Step::Busy);
        }
        return Ok(Step::Stopped);
    }
    if state.busy_left > 0 {
        state.busy_left -= 1;
        return Ok(Step::Busy);
    }
    let next = committed + 1;
    if script.fail_at == Some(next) && !already_failed {
        return Ok(Step::Failed("model crashed".into()));
    }
    if next > script.chunks {
        return Ok(Step::Done(json!({ "chunks": committed })));
    }
    ctx.checkpoint(
        &json!(next),
        &json!(next),
        next as f32 / script.chunks.max(1) as f32,
    )?;
    if next == script.chunks {
        Ok(Step::Done(json!({ "chunks": next })))
    } else {
        Ok(Step::Chunk)
    }
}

fn cursor_u32(cursor: &Value) -> u32 {
    match cursor {
        Value::Number(number) => number.as_u64().unwrap_or(0) as u32,
        _ => 0,
    }
}

struct Probe {
    pools: Vec<Pool>,
    prev: HashMap<TaskId, (Status, Value)>,
    events_seen: usize,
}

impl Probe {
    fn new(sched: &Scheduler, pools: Vec<Pool>) -> Probe {
        let world = sched.store().world().unwrap();
        Probe {
            pools,
            prev: world
                .tasks
                .iter()
                .map(|task| (task.id, (task.status, task.cursor.clone())))
                .collect(),
            events_seen: sched.events().len(),
        }
    }

    fn check(&mut self, sched: &Scheduler) {
        self.check_snapshot(sched);
        self.check_events(sched);
    }

    fn check_snapshot(&self, sched: &Scheduler) {
        let world = sched.store().world().unwrap();
        let snapshot = sched.snapshot().unwrap();
        assert_eq!(
            snapshot.now_ms,
            sched.store().now_ms(),
            "E01: snapshot clock"
        );
        assert_eq!(
            snapshot.tasks.len(),
            world.tasks.len(),
            "E01: snapshot rows vs store rows"
        );
        for view in &snapshot.tasks {
            let stored = world.task(view.task.id).unwrap_or_else(|| {
                panic!(
                    "E01: task {} is in the snapshot but not in the store",
                    view.task.id
                )
            });
            assert_eq!(
                &view.task, stored,
                "E01: row {} differs from the store",
                view.task.id
            );
            if stored.status.is_finished() || stored.status == Status::Paused {
                assert!(
                    view.wait.is_none(),
                    "E01: task {} is {:?} but waits because {:?}",
                    stored.id,
                    stored.status,
                    view.wait
                );
            }
            if !stored.inline && world.has_owned_children(stored.id) {
                let children: Vec<Task> = world
                    .tasks
                    .iter()
                    .filter(|child| child.parent == Some(stored.id))
                    .cloned()
                    .collect();
                let states: Vec<ChildState> = children
                    .iter()
                    .map(|child| state::child_state(&world, child))
                    .collect();
                if let Some(expected) = composite_status(&states) {
                    assert_eq!(
                        stored.status, expected,
                        "E01: parent {} is {:?} but rule 13.7 says {:?}",
                        stored.id, stored.status, expected
                    );
                }
                if stored.status == Status::Waiting {
                    assert!(
                        view.wait.is_some(),
                        "E01: waiting composite {} has no waiting reason",
                        stored.id
                    );
                }
            } else if !stored.inline && stored.status == Status::Queued {
                match state::dep_hold(&world, stored.id) {
                    Some(DepHold::Paused(_)) => assert_eq!(
                        view.wait,
                        Some(WaitReason::Paused),
                        "E01: task {} waits on a paused dependency",
                        stored.id
                    ),
                    Some(DepHold::Dependency) => assert_eq!(
                        view.wait,
                        Some(WaitReason::Dependency),
                        "E01: task {} waits on an unfinished dependency",
                        stored.id
                    ),
                    None => assert!(
                        state::is_claimable(&world, stored.id),
                        "E01: task {} is queued with no hold but is not claimable",
                        stored.id
                    ),
                }
            }
        }
        let mut leaves: Vec<TaskId> = world
            .tasks
            .iter()
            .filter(|task| {
                !task.inline
                    && !world.has_owned_children(task.id)
                    && matches!(task.status, Status::Running | Status::Stopping)
            })
            .map(|task| task.id)
            .collect();
        leaves.sort_unstable();
        let mut running: Vec<TaskId> = snapshot.running.iter().map(|live| live.id).collect();
        running.sort_unstable();
        assert_eq!(
            leaves, running,
            "E01: what runs now is exactly the live leaves"
        );
        for live in &snapshot.running {
            let stored = world.task(live.id).unwrap();
            assert_eq!(
                Some(live.resource),
                stored.resource,
                "E01: run {} holds another resource",
                live.id
            );
            assert_eq!(
                live.chunk, stored.cursor,
                "E01: run {} shows another chunk",
                live.id
            );
            assert_eq!(
                live.stopping,
                stored.status == Status::Stopping,
                "E01: run {} stopping flag",
                live.id
            );
        }
        let runs = sched.live();
        for pool in &self.pools {
            let used: Vec<&LiveRun> = runs
                .iter()
                .filter(|run| run.resource == pool.resource)
                .collect();
            assert!(
                used.len() <= pool.capacity as usize,
                "E01: {:?} holds {} slots of {}",
                pool.resource,
                used.len(),
                pool.capacity
            );
            let memory: u64 = used.iter().map(|run| run.memory).sum();
            assert!(
                memory <= pool.memory,
                "E01: {:?} uses {memory} memory of {}",
                pool.resource,
                pool.memory
            );
        }
    }

    fn check_events(&mut self, sched: &Scheduler) {
        let world = sched.store().world().unwrap();
        let events = sched.events();
        assert!(
            events.len() >= self.events_seen,
            "E01: the event log shrank"
        );
        let new_events = &events[self.events_seen..];
        self.events_seen = events.len();
        for task in &world.tasks {
            let Some((before, cursor_before)) = self.prev.get(&task.id) else {
                continue;
            };
            if *before != task.status {
                if let Some(kind) = event_of(*before, task.status) {
                    assert!(
                        new_events
                            .iter()
                            .any(|event| event.task == task.id && event.kind == kind),
                        "E01: task {} went {:?} -> {:?} without a {:?} event; events: {:?}",
                        task.id,
                        before,
                        task.status,
                        kind,
                        new_events
                    );
                }
            }
            if *cursor_before != task.cursor && !task.cursor.is_null() {
                assert!(
                    new_events
                        .iter()
                        .any(|event| event.task == task.id && event.kind == EventKind::Chunk),
                    "E01: task {} moved its cursor to {} without a chunk event; events: {:?}",
                    task.id,
                    task.cursor,
                    new_events
                );
            }
        }
        self.prev = world
            .tasks
            .iter()
            .map(|task| (task.id, (task.status, task.cursor.clone())))
            .collect();
    }
}

fn event_of(before: Status, after: Status) -> Option<EventKind> {
    if before == after {
        return None;
    }
    Some(match after {
        Status::Running => EventKind::Started,
        Status::Paused => EventKind::Paused,
        Status::Queued if before == Status::Paused => EventKind::Resumed,
        Status::Done => EventKind::Done,
        Status::Failed => EventKind::Failed,
        Status::Cancelled => EventKind::Cancelled,
        _ => return None,
    })
}

fn drain_to_end(sched: &Scheduler, probe: &mut Probe) {
    for _ in 0..600 {
        let paused: Vec<TaskId> = sched
            .store()
            .world()
            .unwrap()
            .tasks
            .iter()
            .filter(|task| task.status == Status::Paused)
            .map(|task| task.id)
            .collect();
        for id in paused {
            let _ = sched.resume(id);
        }
        sched.tick().unwrap();
        probe.check(sched);
        let world = sched.store().world().unwrap();
        if world.tasks.iter().all(|task| task.status.is_finished()) && sched.live().is_empty() {
            return;
        }
    }
    let left: Vec<(TaskId, Status)> = sched
        .store()
        .world()
        .unwrap()
        .tasks
        .iter()
        .filter(|task| !task.status.is_finished())
        .map(|task| (task.id, task.status))
        .collect();
    panic!("E02: the queue did not reach the end; still {left:?}");
}

fn pools() -> Vec<Pool> {
    vec![
        Pool {
            resource: Resource::Speech,
            capacity: 1,
            memory: 1_000,
        },
        Pool {
            resource: Resource::Embed,
            capacity: 1,
            memory: 1_000,
        },
        Pool {
            resource: Resource::Llm,
            capacity: 1,
            memory: 1_000,
        },
        Pool {
            resource: Resource::Provider,
            capacity: 1,
            memory: 1_000,
        },
    ]
}

fn scheduler(clock: Arc<FakeClock>, pool_list: Vec<Pool>) -> Scheduler {
    let store: Arc<dyn Store> = Arc::new(MemStore::new(clock));
    let sched = Scheduler::new(store, pool_list);
    sched.register(Box::new(ScriptRunner::default()));
    sched
}

struct Fixture {
    sched: Scheduler,
    probe: Probe,
}

fn fixture() -> Fixture {
    fixture_with(pools())
}

fn fixture_with(pool_list: Vec<Pool>) -> Fixture {
    let clock = Arc::new(FakeClock::new(10_000));
    let sched = scheduler(clock, pool_list.clone());
    let probe = Probe::new(&sched, pool_list);
    Fixture { sched, probe }
}

impl Fixture {
    fn tick(&mut self) {
        self.sched.tick().unwrap();
        self.probe.check(&self.sched);
    }

    fn check(&mut self) {
        self.probe.check(&self.sched);
    }

    fn task(&self, id: TaskId) -> Task {
        self.sched.store().get(id).unwrap().expect("task")
    }

    fn wait(&self, id: TaskId) -> Option<WaitReason> {
        self.sched
            .snapshot()
            .unwrap()
            .tasks
            .into_iter()
            .find(|view| view.task.id == id)
            .and_then(|view| view.wait)
    }
}

struct Db {
    _dir: tempfile::TempDir,
    path: PathBuf,
    clock: Arc<FakeClock>,
    pools: Vec<Pool>,
}

impl Db {
    fn new(pool_list: Vec<Pool>) -> Db {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.db");
        Db {
            _dir: dir,
            path,
            clock: Arc::new(FakeClock::new(10_000)),
            pools: pool_list,
        }
    }

    fn sched(&self) -> Scheduler {
        let store: Arc<dyn Store> =
            Arc::new(SqliteStore::open(&self.path, self.clock.clone()).unwrap());
        let sched = Scheduler::new(store, self.pools.clone());
        sched.register(Box::new(ScriptRunner::default()));
        sched
    }
}

fn leaf(kind: &str, title: &str, input: &Value, resource: Resource) -> NewTask {
    NewTask::new(kind, title, input).resource(resource)
}

fn composite(title: &str, scope: &str) -> NewTask {
    NewTask::new("analysis", title, &json!({}))
        .composite()
        .scope(scope)
        .analysis(scope)
}

#[test]
fn a01_a_paused_queued_task_is_not_taken_until_resumed() {
    let mut f = fixture();
    let holder = f
        .sched
        .enqueue(leaf(
            "topic-detail",
            "holder",
            &script(3, 0, 0),
            Resource::Llm,
        ))
        .unwrap();
    f.tick();
    let victim = f
        .sched
        .enqueue(leaf(
            "topic-detail",
            "victim",
            &script(2, 0, 0),
            Resource::Llm,
        ))
        .unwrap();
    f.check();

    f.sched.pause(victim).unwrap();
    f.check();
    assert_eq!(f.task(victim).status, Status::Paused);

    for _ in 0..3 {
        f.tick();
    }
    assert_eq!(f.task(holder).status, Status::Done);
    assert_eq!(
        f.task(victim).status,
        Status::Paused,
        "a paused task that was never taken is not taken"
    );
    assert!(f.sched.live().is_empty());

    assert!(f.sched.resume(victim).unwrap());
    f.check();
    assert_eq!(f.task(victim).status, Status::Queued);
    f.tick();
    assert_ne!(
        f.task(victim).status,
        Status::Paused,
        "resume makes it claimable"
    );

    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(victim).status, Status::Done);
}

#[test]
fn a04_cancelling_a_paused_task_cancels_its_dependents() {
    let mut f = fixture();
    let gate = f
        .sched
        .enqueue(leaf("topic-detail", "gate", &script(4, 0, 0), Resource::Llm).analysis("rec"))
        .unwrap();
    let dependent = f
        .sched
        .enqueue(
            leaf("index", "dependent", &script(2, 0, 0), Resource::Llm)
                .analysis("rec")
                .needs(&[gate]),
        )
        .unwrap();
    f.tick();

    f.sched.pause(gate).unwrap();
    f.check();
    assert_eq!(f.task(gate).status, Status::Paused);
    assert_eq!(f.task(dependent).status, Status::Queued);
    assert_eq!(f.wait(dependent), Some(WaitReason::Paused));
    assert_eq!(f.task(dependent).note, Some(pause_note("gate")));

    f.sched.cancel(gate).unwrap();
    f.check();
    assert_eq!(f.task(gate).status, Status::Cancelled);
    assert_eq!(f.task(dependent).status, Status::Cancelled);
    assert!(f.sched.live().is_empty());
    assert!(f
        .sched
        .snapshot()
        .unwrap()
        .tasks
        .iter()
        .all(|view| view.wait.is_none()));

    drain_to_end(&f.sched, &mut f.probe);
}

#[test]
fn a05_double_pause_double_resume_and_pausing_a_done_task_do_nothing() {
    let mut f = fixture();
    let holder = f
        .sched
        .enqueue(leaf(
            "topic-detail",
            "holder",
            &script(3, 0, 0),
            Resource::Llm,
        ))
        .unwrap();
    f.tick();
    let id = f
        .sched
        .enqueue(leaf("index", "victim", &script(1, 0, 0), Resource::Llm))
        .unwrap();
    f.check();

    f.sched.pause(id).unwrap();
    f.check();
    assert_eq!(f.task(id).status, Status::Paused);
    f.sched.pause(id).unwrap();
    f.check();
    assert_eq!(
        f.task(id).status,
        Status::Paused,
        "a double pause is a no-op"
    );

    assert!(f.sched.resume(id).unwrap());
    f.check();
    assert_eq!(f.task(id).status, Status::Queued);
    assert!(f.sched.resume(id).unwrap());
    f.check();
    assert_eq!(
        f.task(id).status,
        Status::Queued,
        "a double resume is a no-op"
    );

    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(holder).status, Status::Done);
    assert_eq!(f.task(id).status, Status::Done);

    f.sched.pause(id).unwrap();
    f.check();
    assert_eq!(
        f.task(id).status,
        Status::Done,
        "a done task cannot be paused"
    );
    assert!(f.sched.resume(id).unwrap());
    f.check();
    assert_eq!(
        f.task(id).status,
        Status::Done,
        "a done task cannot be resumed"
    );
    assert!(f.sched.live().is_empty());
    drain_to_end(&f.sched, &mut f.probe);
}

#[test]
fn a06_pausing_the_analysis_pauses_every_leaf_and_resume_keeps_the_dependency_order() {
    let mut f = fixture();
    let (root, kids) = f
        .sched
        .enqueue_tree(
            composite("rec", "rec"),
            vec![
                Child::new(
                    leaf("transcribe", "speech", &script(2, 0, 0), Resource::Speech).scope("rec"),
                ),
                Child::new(leaf("index", "index", &script(2, 0, 0), Resource::Embed).scope("rec"))
                    .after(&[0]),
            ],
        )
        .unwrap();
    let (speech, index) = (kids[0], kids[1]);
    f.check();
    assert_eq!(f.task(root).status, Status::Running);

    f.tick();
    assert_eq!(f.task(speech).status, Status::Queued);
    assert_eq!(f.wait(index), Some(WaitReason::Dependency));

    f.sched.pause(root).unwrap();
    f.check();
    assert_eq!(f.task(root).status, Status::Paused);
    assert_eq!(
        f.task(speech).status,
        Status::Paused,
        "every leaf is paused"
    );
    assert_eq!(f.task(index).status, Status::Paused, "every leaf is paused");
    assert!(f.sched.live().is_empty());

    assert!(f.sched.resume(root).unwrap());
    f.check();
    assert_eq!(f.task(root).status, Status::Running);
    assert_eq!(f.task(speech).status, Status::Queued);
    assert_eq!(f.task(index).status, Status::Queued);
    assert_eq!(f.wait(index), Some(WaitReason::Dependency));

    f.tick();
    f.tick();
    assert_eq!(
        f.task(speech).status,
        Status::Done,
        "the first stage goes first"
    );
    while f.task(index).status != Status::Done {
        f.tick();
    }
    assert!(
        f.task(index).started_ms.unwrap() >= f.task(speech).finished_ms.unwrap(),
        "the dependent stage starts only after its prerequisite"
    );
    assert_eq!(f.task(root).status, Status::Done);
    drain_to_end(&f.sched, &mut f.probe);
}

#[test]
fn a07_pausing_one_stage_holds_dependents_and_leaves_independent_work_running() {
    let mut f = fixture();
    let (root, kids) = f
        .sched
        .enqueue_tree(
            composite("rec", "rec"),
            vec![
                Child::new(
                    leaf("transcribe", "gate", &script(3, 1, 0), Resource::Speech).scope("rec"),
                ),
                Child::new(
                    leaf("topic-detail", "after", &script(2, 0, 0), Resource::Llm).scope("rec"),
                )
                .after(&[0]),
                Child::new(leaf("details", "side", &script(3, 1, 0), Resource::Llm).scope("rec")),
            ],
        )
        .unwrap();
    let (gate, after, side) = (kids[0], kids[1], kids[2]);
    f.tick();
    assert_eq!(f.task(gate).status, Status::Running);
    assert_eq!(f.task(side).status, Status::Running);

    f.sched.pause(gate).unwrap();
    f.check();
    assert_eq!(f.task(gate).status, Status::Stopping);
    assert_eq!(
        f.wait(after),
        Some(WaitReason::Paused),
        "the dependent waits «на паузе»"
    );

    f.tick();
    assert_eq!(f.task(gate).status, Status::Paused);
    assert_eq!(f.task(after).status, Status::Queued);
    assert_eq!(f.wait(after), Some(WaitReason::Paused));
    assert_eq!(f.task(after).note, Some(pause_note("gate")));
    assert_eq!(
        f.task(side).status,
        Status::Running,
        "independent work goes on"
    );
    assert_eq!(cursor_u32(&f.task(side).cursor), 1);

    f.tick();
    assert_eq!(
        cursor_u32(&f.task(side).cursor),
        2,
        "independent work keeps going"
    );
    assert_eq!(f.task(root).status, Status::Running);

    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(root).status, Status::Done);
    assert_eq!(f.task(after).status, Status::Done);
}

#[test]
fn a08_cancelling_a_running_task_frees_the_slot_and_cancels_dependents() {
    let mut f = fixture();
    let gate = f
        .sched
        .enqueue(leaf("transcribe", "gate", &script(4, 1, 0), Resource::Speech).analysis("rec"))
        .unwrap();
    let dependent = f
        .sched
        .enqueue(
            leaf("index", "after", &script(2, 0, 0), Resource::Llm)
                .analysis("rec")
                .needs(&[gate]),
        )
        .unwrap();
    let next = f
        .sched
        .enqueue(leaf("transcribe", "next", &script(2, 1, 0), Resource::Speech).analysis("rec"))
        .unwrap();
    f.tick();
    assert_eq!(f.task(gate).status, Status::Running);

    f.sched.cancel(gate).unwrap();
    f.check();
    assert_eq!(f.task(gate).status, Status::Stopping);
    assert!(
        !f.sched.resume(gate).unwrap(),
        "the run has not returned yet"
    );

    f.tick();
    assert_eq!(f.task(gate).status, Status::Cancelled);
    assert_eq!(f.task(dependent).status, Status::Cancelled);
    assert_eq!(
        f.sched.live().iter().map(|run| run.id).collect::<Vec<_>>(),
        vec![next],
        "the freed slot goes to the next task in the same tick"
    );
    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(next).status, Status::Done);
}

#[test]
fn a09_failure_cancels_dependents_and_restart_brings_back_only_that_work() {
    let mut f = fixture();
    let (root, kids) = f
        .sched
        .enqueue_tree(
            composite("rec", "rec"),
            vec![
                Child::new(leaf("index", "keep", &script(1, 0, 0), Resource::Embed).scope("rec")),
                Child::new(
                    leaf("transcribe", "bad", &failing(2, 0), Resource::Speech).scope("rec"),
                ),
            ],
        )
        .unwrap();
    let (keep, bad) = (kids[0], kids[1]);
    let after = f
        .sched
        .enqueue(
            leaf("topic-detail", "after", &script(2, 0, 0), Resource::Llm)
                .scope("rec")
                .needs(&[bad]),
        )
        .unwrap();
    f.tick();
    assert_eq!(f.task(keep).status, Status::Done);
    assert_eq!(f.task(bad).status, Status::Failed);
    assert_eq!(f.task(after).status, Status::Cancelled);
    assert_eq!(f.task(after).note, Some(failure_note("bad")));
    assert_eq!(f.task(root).status, Status::Failed);
    let kept = f.task(keep).result.clone();

    f.sched.restart(bad).unwrap();
    f.check();
    assert_eq!(f.task(bad).status, Status::Queued);
    assert_eq!(
        f.task(after).status,
        Status::Queued,
        "cancelled because of it"
    );
    assert_eq!(f.task(keep).status, Status::Done, "done work is not redone");
    assert_eq!(f.task(keep).attempts, 1);
    assert_eq!(f.task(keep).result, kept);
    assert_eq!(f.task(root).status, Status::Running);

    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(bad).status, Status::Done);
    assert_eq!(f.task(after).status, Status::Done);
    assert_eq!(f.task(keep).attempts, 1, "the done stage never ran again");
    assert_eq!(f.task(root).status, Status::Done);
}

#[test]
fn a10_restart_of_a_done_stage_reruns_it_and_leaves_the_rest_done() {
    let mut f = fixture();
    let (root, kids) = f
        .sched
        .enqueue_tree(
            composite("rec", "rec"),
            vec![
                Child::new(
                    leaf("transcribe", "a", &script(2, 0, 0), Resource::Speech).scope("rec"),
                ),
                Child::new(leaf("index", "b", &script(2, 0, 0), Resource::Embed).scope("rec"))
                    .after(&[0]),
                Child::new(leaf("topic-detail", "c", &script(1, 0, 0), Resource::Llm).scope("rec")),
                Child::new(leaf("details", "d", &script(2, 0, 0), Resource::Llm).scope("rec"))
                    .after(&[1]),
            ],
        )
        .unwrap();
    let (a, b, c, d) = (kids[0], kids[1], kids[2], kids[3]);
    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(root).status, Status::Done);
    for id in [a, b, c, d] {
        assert_eq!(f.task(id).status, Status::Done);
    }
    let d_result = f.task(d).result.clone();

    f.sched.restart(b).unwrap();
    f.check();
    assert_eq!(
        f.task(b).status,
        Status::Queued,
        "the stage itself is redone"
    );
    assert!(
        f.task(b).cursor.is_null(),
        "заново starts from the first chunk"
    );
    assert_eq!(
        f.task(root).status,
        Status::Running,
        "the parent is recomputed"
    );
    for id in [a, c, d] {
        assert_eq!(f.task(id).status, Status::Done, "the rest is not rerun");
        assert_eq!(f.task(id).attempts, 1);
    }

    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(b).status, Status::Done);
    assert_eq!(f.task(b).attempts, 2);
    assert_eq!(f.task(d).result, d_result);
    assert_eq!(f.task(root).status, Status::Done);
}

#[test]
fn a12_a_crash_while_paused_leaves_the_task_paused() {
    let db = Db::new(pools());
    let id;
    {
        let sched = db.sched();
        let mut probe = Probe::new(&sched, db.pools.clone());
        id = sched
            .enqueue(leaf("topic-detail", "paused", &script(3, 0, 0), Resource::Llm).scope("rec"))
            .unwrap();
        probe.check(&sched);
        sched.tick().unwrap();
        probe.check(&sched);
        sched.tick().unwrap();
        probe.check(&sched);
        assert_eq!(
            cursor_u32(&sched.store().get(id).unwrap().unwrap().cursor),
            2
        );
        sched.pause(id).unwrap();
        probe.check(&sched);
        assert_eq!(
            sched.store().get(id).unwrap().unwrap().status,
            Status::Paused
        );
    }

    let sched = db.sched();
    let mut probe = Probe::new(&sched, db.pools.clone());
    sched.recover().unwrap();
    probe.check(&sched);
    let restored = sched.store().get(id).unwrap().unwrap();
    assert_eq!(
        restored.status,
        Status::Paused,
        "a crash does not resume a pause"
    );
    assert_eq!(cursor_u32(&restored.cursor), 2, "the paused chunk is kept");
    assert!(sched.live().is_empty());

    drain_to_end(&sched, &mut probe);
    assert_eq!(sched.store().get(id).unwrap().unwrap().status, Status::Done);
}

#[test]
fn a13_a_crash_while_stopping_settles_each_task_by_its_intent() {
    let db = Db::new(pools());
    let (paused, cancelled);
    {
        let sched = db.sched();
        let mut probe = Probe::new(&sched, db.pools.clone());
        paused = sched
            .enqueue(leaf(
                "transcribe",
                "pause",
                &script(3, 1, 0),
                Resource::Speech,
            ))
            .unwrap();
        cancelled = sched
            .enqueue(leaf(
                "topic-detail",
                "cancel",
                &script(3, 1, 0),
                Resource::Llm,
            ))
            .unwrap();
        probe.check(&sched);
        sched.tick().unwrap();
        probe.check(&sched);
        sched.pause(paused).unwrap();
        probe.check(&sched);
        sched.cancel(cancelled).unwrap();
        probe.check(&sched);
        assert_eq!(
            sched.store().get(paused).unwrap().unwrap().status,
            Status::Stopping
        );
        assert_eq!(
            sched.store().get(cancelled).unwrap().unwrap().status,
            Status::Stopping
        );
    }

    let sched = db.sched();
    let mut probe = Probe::new(&sched, db.pools.clone());
    sched.recover().unwrap();
    probe.check(&sched);
    assert_eq!(
        sched.store().get(paused).unwrap().unwrap().status,
        Status::Paused,
        "a pause that was still stopping settles as paused"
    );
    assert_eq!(
        sched.store().get(cancelled).unwrap().unwrap().status,
        Status::Cancelled,
        "a cancel that was still stopping settles as cancelled"
    );
    assert!(sched.live().is_empty());

    drain_to_end(&sched, &mut probe);
    assert_eq!(
        sched.store().get(paused).unwrap().unwrap().status,
        Status::Done
    );
}

#[test]
fn b02_two_recordings_on_one_pool_take_turns_by_chunk() {
    let mut f = fixture();
    let mut ids = Vec::new();
    for key in ["r1", "r2"] {
        ids.push(
            f.sched
                .enqueue(leaf("topic-detail", key, &script(3, 0, 0), Resource::Llm).analysis(key))
                .unwrap(),
        );
        f.check();
    }
    for _ in 0..6 {
        f.tick();
    }
    let turns: Vec<String> = f
        .sched
        .turns()
        .into_iter()
        .map(|turn| turn.participant)
        .collect();
    assert_eq!(
        turns,
        vec!["a:r1", "a:r2", "a:r1", "a:r2", "a:r1", "a:r2"],
        "one chunk each, strictly by turn"
    );
    assert_eq!(f.task(ids[0]).status, Status::Done);
    assert_eq!(f.task(ids[1]).status, Status::Done);
    drain_to_end(&f.sched, &mut f.probe);
}

#[test]
fn b04_cancelling_analysis_a_leaves_analysis_b_alone() {
    let mut f = fixture();
    let (root, kids) = f
        .sched
        .enqueue_tree(
            composite("a", "a"),
            vec![Child::new(
                leaf("topic-detail", "a", &script(5, 1, 0), Resource::Llm).analysis("a"),
            )],
        )
        .unwrap();
    let a = kids[0];
    let b = f
        .sched
        .enqueue(leaf("topic-detail", "b", &script(2, 0, 0), Resource::Llm).analysis("b"))
        .unwrap();
    f.tick();
    assert_eq!(f.task(a).status, Status::Running);
    assert_eq!(f.task(b).status, Status::Queued);

    f.sched.cancel(root).unwrap();
    f.check();
    f.tick();
    assert_eq!(f.task(a).status, Status::Cancelled);
    assert_eq!(f.task(root).status, Status::Cancelled);
    assert_ne!(f.task(b).status, Status::Cancelled, "B is not touched");
    assert_eq!(f.task(b).note, None);

    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(b).status, Status::Done);
    assert_eq!(f.task(root).status, Status::Cancelled);
}

#[test]
fn b05_an_error_in_a_does_not_stop_b() {
    let mut f = fixture();
    let a = f
        .sched
        .enqueue(leaf("topic-detail", "a", &failing(3, 1), Resource::Llm).analysis("a"))
        .unwrap();
    let b = f
        .sched
        .enqueue(leaf("topic-detail", "b", &script(2, 0, 0), Resource::Llm).analysis("b"))
        .unwrap();
    f.tick();
    assert_eq!(f.task(a).status, Status::Running);

    f.tick();
    assert_eq!(f.task(a).status, Status::Failed);
    assert_ne!(f.task(b).status, Status::Failed);
    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(b).status, Status::Done);
    assert_eq!(f.task(a).status, Status::Failed);
}

#[test]
fn b06_resuming_a_while_b_is_mid_chunk_waits_for_b_and_does_not_interrupt_it() {
    let mut f = fixture();
    let a = f
        .sched
        .enqueue(leaf("topic-detail", "a", &script(3, 1, 0), Resource::Llm).analysis("a"))
        .unwrap();
    let b = f
        .sched
        .enqueue(leaf("topic-detail", "b", &script(3, 1, 0), Resource::Llm).analysis("b"))
        .unwrap();
    f.tick();
    assert_eq!(f.task(a).status, Status::Running);

    f.sched.pause(a).unwrap();
    f.check();
    assert!(!f.sched.resume(a).unwrap(), "the run is still stopping");
    f.tick();
    assert_eq!(f.task(a).status, Status::Paused);
    assert_eq!(f.task(b).status, Status::Running, "B took the pool");
    assert!(f.task(b).cursor.is_null(), "B is inside its model call");

    assert!(f.sched.resume(a).unwrap());
    f.check();
    assert_eq!(f.task(a).status, Status::Queued);
    assert_eq!(
        f.wait(a),
        Some(WaitReason::Resource(Resource::Llm)),
        "A waits for B's model call"
    );
    assert_eq!(f.task(b).status, Status::Running, "B was not interrupted");

    f.tick();
    assert_eq!(cursor_u32(&f.task(b).cursor), 1, "B finished its chunk");
    assert_ne!(f.task(b).status, Status::Paused);
    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(a).status, Status::Done);
    assert_eq!(f.task(b).status, Status::Done);
}

#[test]
fn b07_a_task_that_does_not_fit_in_memory_waits_and_is_taken_on_release() {
    let mut f = fixture_with(vec![Pool {
        resource: Resource::Llm,
        capacity: 2,
        memory: 10,
    }]);
    let a = f
        .sched
        .enqueue(leaf("embed", "a", &script(3, 1, 0), Resource::Llm).memory(8))
        .unwrap();
    let b = f
        .sched
        .enqueue(leaf("embed", "b", &script(3, 1, 0), Resource::Llm).memory(8))
        .unwrap();
    f.tick();
    assert_eq!(f.sched.live().len(), 1);
    let waiting = if f.sched.live()[0].id == a { b } else { a };
    assert_eq!(f.wait(waiting), Some(WaitReason::Memory));

    f.tick();
    assert_eq!(
        f.task(waiting).status,
        Status::Running,
        "the waiting task took the released slot"
    );
    let other = if waiting == a { b } else { a };
    assert_eq!(
        f.wait(other),
        Some(WaitReason::Memory),
        "the other still does not fit"
    );
    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(a).status, Status::Done);
    assert_eq!(f.task(b).status, Status::Done);
}

#[test]
fn c01_three_recordings_on_one_pool_keep_a_strict_circle() {
    let mut f = fixture();
    for key in ["r1", "r2", "r3"] {
        f.sched
            .enqueue(leaf("topic-detail", key, &script(4, 0, 0), Resource::Llm).analysis(key))
            .unwrap();
        f.check();
    }
    for _ in 0..12 {
        f.tick();
    }
    let turns: Vec<String> = f
        .sched
        .turns()
        .into_iter()
        .map(|turn| turn.participant)
        .collect();
    let expected: Vec<String> = (0..4)
        .flat_map(|_| ["a:r1", "a:r2", "a:r3"])
        .map(str::to_string)
        .collect();
    assert_eq!(
        turns, expected,
        "strict round-robin, nobody waits more than 3 chunks"
    );
    for _ in 0..12 {
        f.tick();
    }
    let world = f.sched.store().world().unwrap();
    assert!(world.tasks.iter().all(|task| task.status == Status::Done));
    drain_to_end(&f.sched, &mut f.probe);
}

#[test]
fn c02_pausing_the_middle_of_three_lets_two_share_and_the_circle_returns_on_resume() {
    let mut f = fixture();
    let mut ids = Vec::new();
    for key in ["r1", "r2", "r3"] {
        ids.push(
            f.sched
                .enqueue(leaf("topic-detail", key, &script(6, 0, 0), Resource::Llm).analysis(key))
                .unwrap(),
        );
        f.check();
    }
    for _ in 0..3 {
        f.tick();
    }
    f.sched.pause(ids[1]).unwrap();
    f.check();
    assert_eq!(f.task(ids[1]).status, Status::Paused);

    for _ in 0..4 {
        f.tick();
    }
    let turns: Vec<String> = f
        .sched
        .turns()
        .into_iter()
        .map(|turn| turn.participant)
        .collect();
    assert_eq!(turns.len(), 7);
    assert_eq!(
        turns[3..],
        ["a:r1", "a:r3", "a:r1", "a:r3"],
        "the two remaining share the pool"
    );

    assert!(f.sched.resume(ids[1]).unwrap());
    f.check();
    for _ in 0..3 {
        f.tick();
    }
    let after: HashSet<String> = f
        .sched
        .turns()
        .into_iter()
        .skip(7)
        .map(|turn| turn.participant)
        .collect();
    assert_eq!(
        after,
        HashSet::from(["a:r1".to_string(), "a:r2".to_string(), "a:r3".to_string()]),
        "the circle of three is back"
    );
    drain_to_end(&f.sched, &mut f.probe);
    let world = f.sched.store().world().unwrap();
    assert!(world.tasks.iter().all(|task| task.status == Status::Done));
}

#[test]
fn c03_cancelling_all_three_in_turn_leaves_nothing_running_or_waiting() {
    let mut f = fixture();
    let mut ids = Vec::new();
    for key in ["r1", "r2", "r3"] {
        ids.push(
            f.sched
                .enqueue(leaf("topic-detail", key, &script(6, 1, 0), Resource::Llm).analysis(key))
                .unwrap(),
        );
        f.check();
    }
    f.tick();
    assert_eq!(f.task(ids[0]).status, Status::Running);

    for id in ids {
        f.sched.cancel(id).unwrap();
        f.check();
        f.tick();
    }
    let world = f.sched.store().world().unwrap();
    assert!(world
        .tasks
        .iter()
        .all(|task| task.status == Status::Cancelled));
    assert!(f.sched.live().is_empty());
    assert!(!world
        .tasks
        .iter()
        .any(|task| state::is_claimable(&world, task.id)));
    assert!(f
        .sched
        .snapshot()
        .unwrap()
        .tasks
        .iter()
        .all(|view| view.wait.is_none()));
    drain_to_end(&f.sched, &mut f.probe);
}

#[test]
fn c04_a_crash_with_three_in_work_continues_each_from_its_place() {
    let db = Db::new(vec![Pool {
        resource: Resource::Llm,
        capacity: 3,
        memory: 1_000,
    }]);
    let mut ids = Vec::new();
    {
        let sched = db.sched();
        let mut probe = Probe::new(&sched, db.pools.clone());
        for key in ["r1", "r2", "r3"] {
            ids.push(
                sched
                    .enqueue(
                        leaf("topic-detail", key, &script(3, 1, 0), Resource::Llm).analysis(key),
                    )
                    .unwrap(),
            );
            probe.check(&sched);
        }
        for _ in 0..3 {
            sched.tick().unwrap();
            probe.check(&sched);
        }
        for id in &ids {
            let task = sched.store().get(*id).unwrap().unwrap();
            assert_eq!(task.status, Status::Running);
            assert_eq!(cursor_u32(&task.cursor), 2);
        }
    }

    let sched = db.sched();
    let mut probe = Probe::new(&sched, db.pools.clone());
    sched.recover().unwrap();
    probe.check(&sched);
    for id in &ids {
        let task = sched.store().get(*id).unwrap().unwrap();
        assert_eq!(task.status, Status::Queued);
        assert_eq!(cursor_u32(&task.cursor), 2, "the last chunk is kept");
    }

    drain_to_end(&sched, &mut probe);
    for id in &ids {
        let task = sched.store().get(*id).unwrap().unwrap();
        assert_eq!(task.status, Status::Done);
        assert_eq!(cursor_u32(&task.cursor), 3, "it went on from its place");
    }
}

#[test]
fn d01_a_chat_question_during_an_analysis_goes_first_and_background_gets_every_third_chunk() {
    let mut f = fixture();
    let background = f
        .sched
        .enqueue(
            leaf(
                "topic-detail",
                "background",
                &script(3, 0, 0),
                Resource::Llm,
            )
            .analysis("rec"),
        )
        .unwrap();
    f.tick();
    let chat = f
        .sched
        .enqueue(
            leaf("chat-answer", "question", &script(3, 0, 0), Resource::Llm)
                .chat("chat-1")
                .scope("chat-1"),
        )
        .unwrap();
    f.check();

    for _ in 0..12 {
        f.tick();
        if f.task(chat).status.is_finished() && f.task(background).status.is_finished() {
            break;
        }
    }
    let turns: Vec<String> = f
        .sched
        .turns()
        .into_iter()
        .map(|turn| turn.participant)
        .collect();
    assert_eq!(turns[0], "a:rec", "the analysis was already running");
    assert_eq!(turns[1], "c:chat-1", "the chat goes first when it arrives");
    let mut streak = 0;
    for turn in &turns {
        if turn == "c:chat-1" {
            streak += 1;
            assert!(
                streak <= 2,
                "the background was starved for {streak} chunks: {turns:?}"
            );
        } else {
            streak = 0;
        }
    }
    assert!(
        turns.iter().filter(|turn| *turn == "a:rec").count() >= 2,
        "the background keeps getting its every third chunk: {turns:?}"
    );
    assert_eq!(f.task(chat).status, Status::Done);
    assert_eq!(f.task(background).status, Status::Done);
    drain_to_end(&f.sched, &mut f.probe);
}

#[test]
fn d02_two_chats_interleave_and_stopping_one_leaves_the_other_running() {
    let mut f = fixture();
    let a = f
        .sched
        .enqueue(leaf("chat-answer", "a", &script(4, 1, 0), Resource::Llm).chat("a"))
        .unwrap();
    let b = f
        .sched
        .enqueue(leaf("chat-answer", "b", &script(4, 1, 0), Resource::Llm).chat("b"))
        .unwrap();
    f.tick();
    while f.sched.turns().len() < 6 {
        f.tick();
    }
    let turns: Vec<String> = f
        .sched
        .turns()
        .into_iter()
        .map(|turn| turn.participant)
        .collect();
    for pair in turns[..6].windows(2) {
        assert_ne!(pair[0], pair[1], "two chats take turns by step: {turns:?}");
    }
    let live = f.sched.live();
    assert_eq!(live.len(), 1);
    let stopped = live[0].id;
    let other = if stopped == a { b } else { a };

    f.sched.cancel(stopped).unwrap();
    f.check();
    assert_eq!(f.task(stopped).status, Status::Stopping);
    f.tick();
    assert_eq!(f.task(stopped).status, Status::Cancelled);
    assert_ne!(
        f.task(other).status,
        Status::Cancelled,
        "the other chat is not touched"
    );

    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(other).status, Status::Done);
    assert_eq!(f.task(stopped).status, Status::Cancelled);
}

#[test]
fn d03_stopping_an_answer_mid_step_frees_the_slot_for_the_background() {
    let mut f = fixture();
    let answer = f
        .sched
        .enqueue(leaf("chat-answer", "q", &script(4, 1, 0), Resource::Llm).chat("chat"))
        .unwrap();
    let background = f
        .sched
        .enqueue(leaf("topic-detail", "bg", &script(4, 1, 0), Resource::Llm).analysis("bg"))
        .unwrap();
    f.tick();
    assert_eq!(f.task(answer).status, Status::Running);

    f.sched.cancel(answer).unwrap();
    f.check();
    assert_eq!(f.task(answer).status, Status::Stopping);

    f.tick();
    assert_eq!(
        f.task(answer).status,
        Status::Cancelled,
        "the stop returns at once"
    );
    assert_eq!(
        f.sched.live().iter().map(|run| run.id).collect::<Vec<_>>(),
        vec![background],
        "the slot goes to the background"
    );
    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(background).status, Status::Done);
}

#[test]
fn d04_a_crash_mid_answer_continues_from_the_next_step() {
    let db = Db::new(pools());
    let id;
    {
        let sched = db.sched();
        let mut probe = Probe::new(&sched, db.pools.clone());
        id = sched
            .enqueue(leaf("chat-answer", "q", &script(4, 1, 0), Resource::Llm).chat("chat"))
            .unwrap();
        probe.check(&sched);
        sched.tick().unwrap();
        probe.check(&sched);
        sched.tick().unwrap();
        probe.check(&sched);
        assert_eq!(
            cursor_u32(&sched.store().get(id).unwrap().unwrap().cursor),
            1
        );
    }

    let sched = db.sched();
    let mut probe = Probe::new(&sched, db.pools.clone());
    sched.recover().unwrap();
    probe.check(&sched);
    let restored = sched.store().get(id).unwrap().unwrap();
    assert_eq!(restored.status, Status::Queued);
    assert_eq!(
        cursor_u32(&restored.cursor),
        1,
        "the finished steps stay done"
    );

    let mut cursors = vec![1];
    for _ in 0..20 {
        sched.tick().unwrap();
        probe.check(&sched);
        let task = sched.store().get(id).unwrap().unwrap();
        let cursor = cursor_u32(&task.cursor);
        if cursors.last().copied() != Some(cursor) {
            cursors.push(cursor);
        }
        if task.status == Status::Done {
            break;
        }
    }
    assert_eq!(
        cursors,
        vec![1, 2, 3, 4],
        "the answer continues with the next step"
    );
    assert_eq!(sched.store().get(id).unwrap().unwrap().status, Status::Done);
    drain_to_end(&sched, &mut probe);
}

#[test]
fn d05_a_provider_answer_and_a_local_analysis_run_in_parallel() {
    let mut f = fixture();
    let answer = f
        .sched
        .enqueue(leaf("chat-answer", "q", &script(2, 1, 0), Resource::Provider).chat("chat"))
        .unwrap();
    let local = f
        .sched
        .enqueue(leaf("topic-detail", "bg", &script(2, 1, 0), Resource::Llm).analysis("bg"))
        .unwrap();
    f.tick();
    assert_eq!(f.task(answer).status, Status::Running);
    assert_eq!(f.task(local).status, Status::Running);
    let resources: HashSet<Resource> = f
        .sched
        .snapshot()
        .unwrap()
        .running
        .into_iter()
        .map(|live| live.resource)
        .collect();
    assert_eq!(
        resources,
        HashSet::from([Resource::Provider, Resource::Llm]),
        "different pools run at the same time"
    );
    drain_to_end(&f.sched, &mut f.probe);
    assert_eq!(f.task(answer).status, Status::Done);
    assert_eq!(f.task(local).status, Status::Done);
}
