//! Scheduler scenarios and the store features the queue page already depends on.
//! The script runner commits chunk numbers 1, 2, 3, … and keeps its counters per (task, generation).

use mediaa_queue::state::{self, Op};
use mediaa_queue::*;
use proptest::prelude::*;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

fn job(kind: &str, title: &str, input: &Value) -> NewTask {
    NewTask::new(kind, title, input).resource(Resource::Llm)
}

fn script(chunks: u32, busy: u32, slow: u32, hang: bool, fail: bool, after: u32) -> Value {
    let mut value = json!({
        "chunks": chunks,
        "busy": busy,
        "slow_stop": slow,
        "hang": hang,
        "after": after,
    });
    if fail {
        value["fail_at"] = json!(1);
    }
    value
}

fn cursor_u32(cursor: &Value) -> u32 {
    match cursor {
        Value::Number(number) => number.as_u64().unwrap_or(0) as u32,
        _ => 0,
    }
}

#[derive(Clone, Copy)]
struct RunState {
    busy_left: u32,
    slow_left: u32,
    hold_left: u32,
}

#[derive(Default)]
struct ScriptRunner {
    state: HashMap<(i64, i64), RunState>,
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
        let mut st = self.state.get(&key).copied().unwrap_or(RunState {
            busy_left: script.busy,
            slow_left: script.slow_stop,
            hold_left: 0,
        });
        let step = next_step(&script, &mut st, committed, task.attempts, ctx)?;
        self.state.insert(key, st);
        Ok(step)
    }
}

#[derive(serde::Deserialize)]
struct Script {
    chunks: u32,
    #[serde(default)]
    fail_at: Option<u32>,
    #[serde(default)]
    hang: bool,
    #[serde(default)]
    busy: u32,
    #[serde(default)]
    slow_stop: u32,
    #[serde(default)]
    after: u32,
}

fn next_step(
    script: &Script,
    st: &mut RunState,
    committed: u32,
    attempts: i64,
    ctx: &mut dyn RunCtx,
) -> std::result::Result<Step, String> {
    // A stuck model call: the first attempt reports a hang, the retry (after the model was
    // unloaded) goes on — as a real one does when the fresh load works.
    let hang = script.hang && attempts <= 1;
    if st.hold_left > 0 {
        st.hold_left -= 1;
        return Ok(Step::Busy);
    }
    if hang && script.chunks > 0 && committed >= script.chunks {
        return Ok(Step::Hang {
            idle: Duration::from_secs(900),
        });
    }
    if ctx.should_stop() && !hang {
        if st.slow_left > 0 {
            st.slow_left -= 1;
            return Ok(Step::Busy);
        }
        return Ok(Step::Stopped);
    }
    if st.busy_left > 0 {
        st.busy_left -= 1;
        return Ok(Step::Busy);
    }
    let next = committed + 1;
    if script.fail_at == Some(next) {
        return Ok(Step::Failed("model crashed".into()));
    }
    if next > script.chunks {
        return Ok(Step::Done(json!({ "chunks": committed })));
    }
    let progress = next as f32 / script.chunks.max(1) as f32;
    ctx.checkpoint(&json!(next), &json!(next), progress)?;
    if next == script.chunks {
        if hang {
            return Ok(Step::Hang {
                idle: Duration::from_secs(900),
            });
        }
        return Ok(Step::Done(json!({ "chunks": next })));
    }
    if script.after > 0 {
        st.hold_left = script.after.saturating_sub(1);
        return Ok(Step::Busy);
    }
    Ok(Step::Chunk)
}

fn pools() -> Vec<Pool> {
    vec![
        Pool {
            resource: Resource::Speech,
            capacity: 1,
            memory: 1_000,
        },
        Pool {
            resource: Resource::Llm,
            capacity: 1,
            memory: 1_000,
        },
    ]
}

fn scheduler(clock: Arc<FakeClock>, pools: Vec<Pool>) -> Scheduler {
    let store: Arc<dyn Store> = Arc::new(MemStore::new(clock));
    let sched = Scheduler::new(store, pools);
    sched.register(Box::new(ScriptRunner::default()));
    sched
}

fn task(sched: &Scheduler, id: TaskId) -> Task {
    sched.store().get(id).unwrap().unwrap()
}

fn view_matches_store(sched: &Scheduler) {
    let snap = sched.snapshot().unwrap();
    let world = sched.store().world().unwrap();
    assert_eq!(snap.tasks.len(), world.tasks.len());
    for view in &snap.tasks {
        let stored = world.task(view.task.id).unwrap();
        assert_eq!(view.task.status, stored.status);
        assert_eq!(view.task.cursor, stored.cursor);
        assert_eq!(view.task.result, stored.result);
        assert_eq!(view.task.started_ms, stored.started_ms);
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
    let mut running: Vec<TaskId> = snap.running.iter().map(|live| live.id).collect();
    running.sort_unstable();
    assert_eq!(leaves, running, "what runs now is exactly the live leaves");
    assert_eq!(snap.now_ms, sched.store().now_ms());
}

fn stage_ready(sched: &Scheduler, scope: &str) -> bool {
    sched.snapshot().unwrap().tasks.iter().any(|view| {
        view.task.kind == "analysis"
            && view.task.scope.as_deref() == Some(scope)
            && view.task.status == Status::Done
    })
}

fn wait_of(sched: &Scheduler, id: TaskId) -> Option<WaitReason> {
    sched
        .snapshot()
        .unwrap()
        .tasks
        .into_iter()
        .find(|view| view.task.id == id)
        .and_then(|view| view.wait)
}

#[test]
fn pause_mid_call_frees_the_slot_and_does_not_repeat_a_chunk() {
    let clock = Arc::new(FakeClock::new(5_000));
    let sched = scheduler(clock, pools());
    let first = sched
        .enqueue(
            job("transcribe", "first", &script(3, 1, 0, false, false, 0))
                .resource(Resource::Speech)
                .scope("rec"),
        )
        .unwrap();
    sched.tick().unwrap();
    assert_eq!(task(&sched, first).status, Status::Running);
    assert!(task(&sched, first).cursor.is_null());
    assert_eq!(sched.live().len(), 1);
    view_matches_store(&sched);

    sched.pause(first).unwrap();
    assert_eq!(task(&sched, first).status, Status::Stopping);
    assert!(
        !sched.resume(first).unwrap(),
        "resume waits until the run has returned"
    );
    let other = sched
        .enqueue(
            job("transcribe", "other", &script(1, 1, 0, false, false, 0))
                .resource(Resource::Speech)
                .scope("rec"),
        )
        .unwrap();
    assert!(
        sched.live().iter().all(|run| run.id == first),
        "the slot stays taken"
    );
    assert_eq!(task(&sched, other).status, Status::Queued);

    sched.tick().unwrap();
    assert_eq!(task(&sched, first).status, Status::Paused);
    assert!(
        task(&sched, first).cursor.is_null(),
        "the unfinished model call is discarded"
    );
    assert_eq!(
        sched.live().iter().map(|run| run.id).collect::<Vec<_>>(),
        vec![other],
        "the slot is given to the next task in the same tick"
    );
    view_matches_store(&sched);

    assert!(sched.resume(first).unwrap());
    let mut seen = Vec::new();
    for _ in 0..12 {
        sched.tick().unwrap();
        view_matches_store(&sched);
        let cursor = task(&sched, first).cursor.clone();
        if !cursor.is_null() {
            seen.push(cursor_u32(&cursor));
        }
        if task(&sched, first).status == Status::Done {
            break;
        }
    }
    assert_eq!(task(&sched, first).status, Status::Done);
    assert_eq!(seen, vec![1, 2, 3], "chunks continue from 1 with no repeat");
    assert_eq!(task(&sched, first).result, Some(json!({ "chunks": 3 })));
    assert!(task(&sched, first).started_ms.is_some());
}

#[test]
fn slow_stop_holds_the_slot_until_the_run_returns() {
    let clock = Arc::new(FakeClock::new(5_000));
    let sched = scheduler(clock, pools());
    let id = sched
        .enqueue(
            job("transcribe", "slow", &script(2, 1, 2, false, false, 0))
                .resource(Resource::Speech),
        )
        .unwrap();
    let other = sched
        .enqueue(
            job("transcribe", "next", &script(1, 1, 0, false, false, 0))
                .resource(Resource::Speech),
        )
        .unwrap();
    sched.tick().unwrap();
    sched.pause(id).unwrap();
    for _ in 0..2 {
        sched.tick().unwrap();
        assert_eq!(task(&sched, id).status, Status::Stopping);
        assert_eq!(task(&sched, other).status, Status::Queued);
        assert_eq!(sched.live().len(), 1);
    }
    sched.tick().unwrap();
    assert_eq!(task(&sched, id).status, Status::Paused);
    assert_eq!(
        sched.live().iter().map(|run| run.id).collect::<Vec<_>>(),
        vec![other]
    );
}

#[test]
fn two_recordings_run_on_different_pools() {
    let clock = Arc::new(FakeClock::new(5_000));
    let sched = scheduler(clock, pools());
    let speech = sched
        .enqueue(
            job("transcribe", "speech", &script(2, 1, 0, false, false, 0))
                .resource(Resource::Speech)
                .scope("r1")
                .analysis("r1"),
        )
        .unwrap();
    let llm = sched
        .enqueue(
            job("topic-detail", "llm", &script(2, 1, 0, false, false, 0))
                .resource(Resource::Llm)
                .scope("r2")
                .analysis("r2"),
        )
        .unwrap();
    sched.tick().unwrap();
    assert_eq!(
        (task(&sched, speech).status, task(&sched, llm).status),
        (Status::Running, Status::Running)
    );
    let snap = sched.snapshot().unwrap();
    assert_eq!(snap.running.len(), 2);
    let resources: HashSet<_> = snap.running.iter().map(|live| live.resource).collect();
    assert_eq!(resources.len(), 2);
    view_matches_store(&sched);
}

#[test]
fn parents_follow_13_7_through_pause_resume_restart_and_cancel() {
    let clock = Arc::new(FakeClock::new(8_000));
    let sched = scheduler(clock, pools());
    let top = sched
        .enqueue(
            job("analysis", "top", &json!({}))
                .composite()
                .scope("rec")
                .analysis("rec"),
        )
        .unwrap();
    let middle = sched
        .enqueue(
            job("group", "middle", &json!({}))
                .composite()
                .parent(top)
                .scope("rec"),
        )
        .unwrap();
    let leaf = sched
        .enqueue(
            job("transcribe", "leaf", &script(3, 1, 0, false, false, 0))
                .resource(Resource::Speech)
                .parent(middle)
                .scope("rec")
                .analysis("rec"),
        )
        .unwrap();
    let later = sched
        .enqueue(
            job("topic-detail", "later", &json!({}))
                .resource(Resource::Llm)
                .needs(&[leaf])
                .scope("rec"),
        )
        .unwrap();

    assert_eq!(
        (task(&sched, top).status, task(&sched, middle).status),
        (Status::Running, Status::Running)
    );
    sched.tick().unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Running);
    view_matches_store(&sched);

    sched.pause(leaf).unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Stopping);
    assert_eq!(
        (task(&sched, middle).status, task(&sched, top).status),
        (Status::Paused, Status::Paused)
    );
    assert_eq!(task(&sched, later).note, Some(pause_note("leaf")));
    assert!(!sched.resume(leaf).unwrap());
    view_matches_store(&sched);

    sched.tick().unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Paused);
    assert!(task(&sched, leaf).cursor.is_null());
    assert_eq!(
        (task(&sched, middle).status, task(&sched, top).status),
        (Status::Paused, Status::Paused)
    );
    assert!(sched.resume(leaf).unwrap());
    assert_eq!(
        (
            task(&sched, leaf).status,
            task(&sched, middle).status,
            task(&sched, top).status
        ),
        (Status::Queued, Status::Running, Status::Running)
    );
    assert_eq!(task(&sched, later).note, None);
    assert_eq!(task(&sched, later).status, Status::Queued);
    assert!(sched.live().is_empty() || sched.live().iter().all(|run| run.id != later));

    sched.cancel(leaf).unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Cancelled);
    assert_eq!(task(&sched, later).status, Status::Cancelled);
    assert_eq!(
        task(&sched, later).note,
        None,
        "a plain cancel has no failure note"
    );
    assert_eq!(
        (task(&sched, middle).status, task(&sched, top).status),
        (Status::Cancelled, Status::Cancelled)
    );
    view_matches_store(&sched);

    sched.restart(middle).unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Queued);
    assert_eq!(
        task(&sched, later).status,
        Status::Queued,
        "it was cancelled because of the leaf"
    );
    assert_eq!(
        (task(&sched, middle).status, task(&sched, top).status),
        (Status::Running, Status::Running)
    );
    assert!(task(&sched, top).note.is_none());

    sched.tick().unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Running);
    sched.pause(top).unwrap();
    assert_eq!(
        task(&sched, leaf).status,
        Status::Stopping,
        "pausing the composite stops its running leaf"
    );
    assert_eq!(
        (task(&sched, middle).status, task(&sched, top).status),
        (Status::Paused, Status::Paused)
    );
    assert!(!sched.resume(top).unwrap());
    sched.tick().unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Paused);
    assert!(sched.resume(top).unwrap());
    assert_eq!(task(&sched, leaf).status, Status::Queued);
    assert_eq!(task(&sched, top).status, Status::Running);
    view_matches_store(&sched);
}

#[test]
fn a_failed_task_keeps_its_start_and_fails_its_parents() {
    let clock = Arc::new(FakeClock::new(9_000));
    let sched = scheduler(clock.clone(), pools());
    let top = sched
        .enqueue(
            job("analysis", "top", &json!({}))
                .composite()
                .scope("rec"),
        )
        .unwrap();
    let middle = sched
        .enqueue(
            job("group", "middle", &json!({}))
                .composite()
                .parent(top)
                .scope("rec"),
        )
        .unwrap();
    let leaf = sched
        .enqueue(
            job("transcribe", "leaf", &script(3, 1, 0, false, true, 0))
                .resource(Resource::Speech)
                .parent(middle)
                .scope("rec"),
        )
        .unwrap();
    let later = sched
        .enqueue(
            job("topic-detail", "later", &json!({}))
                .resource(Resource::Llm)
                .needs(&[leaf]),
        )
        .unwrap();
    sched.tick().unwrap();
    assert_eq!(
        task(&sched, leaf).status,
        Status::Running,
        "the model call has started"
    );
    clock.advance(1_500);
    sched.tick().unwrap();

    let failed = task(&sched, leaf);
    assert_eq!(failed.status, Status::Failed);
    assert_eq!(failed.error.as_deref(), Some("model crashed"));
    assert_eq!(failed.started_ms, Some(9_000));
    assert!(failed.finished_ms.unwrap() >= failed.started_ms.unwrap());
    for id in [middle, top] {
        let parent = task(&sched, id);
        assert_eq!(parent.status, Status::Failed, "{id}");
        assert_eq!(parent.error.as_deref(), Some("model crashed"));
    }
    let waiting = task(&sched, later);
    assert_eq!(waiting.status, Status::Cancelled);
    assert_eq!(waiting.note, Some(failure_note("leaf")));
    let snap = sched.snapshot().unwrap();
    let shown = snap.tasks.iter().find(|view| view.task.id == leaf).unwrap();
    assert_eq!(shown.task.started_ms, Some(9_000));
    assert!(shown.elapsed_ms >= 1_500);
    view_matches_store(&sched);

    sched.restart(leaf).unwrap();
    assert_eq!(task(&sched, leaf).status, Status::Queued);
    assert!(
        task(&sched, leaf).started_ms.is_none(),
        "заново starts over"
    );
    assert_eq!(task(&sched, later).status, Status::Queued);
    assert_eq!(task(&sched, top).status, Status::Running);
}

#[test]
fn studio_shows_one_recording_ready_when_its_analysis_is_done() {
    let clock = Arc::new(FakeClock::new(4_000));
    let sched = scheduler(clock, pools());
    let rx = sched.subscribe();
    let v1 = sched
        .enqueue(
            job("analysis", "v1", &json!({}))
                .composite()
                .scope("v1")
                .analysis("v1"),
        )
        .unwrap();
    sched
        .enqueue(
            job("transcribe", "v1", &script(1, 0, 0, false, false, 0))
                .resource(Resource::Speech)
                .parent(v1)
                .scope("v1")
                .analysis("v1"),
        )
        .unwrap();
    let v2 = sched
        .enqueue(
            job("analysis", "v2", &json!({}))
                .composite()
                .scope("v2")
                .analysis("v2"),
        )
        .unwrap();
    sched
        .enqueue(
            job("transcribe", "v2", &script(4, 0, 0, false, false, 0))
                .resource(Resource::Speech)
                .parent(v2)
                .scope("v2")
                .analysis("v2"),
        )
        .unwrap();

    sched.tick().unwrap();
    view_matches_store(&sched);
    assert!(stage_ready(&sched, "v1"));
    assert!(!stage_ready(&sched, "v2"));
    assert_eq!(task(&sched, v1).status, Status::Done);
    assert_ne!(task(&sched, v2).status, Status::Done);
    assert!(sched
        .events()
        .iter()
        .any(|event| event.kind == EventKind::Done && event.scope.as_deref() == Some("v1")));
    assert!(!sched
        .events()
        .iter()
        .any(|event| event.kind == EventKind::Done && event.scope.as_deref() == Some("v2")));

    for _ in 0..8 {
        sched.tick().unwrap();
        view_matches_store(&sched);
        if stage_ready(&sched, "v2") {
            break;
        }
    }
    assert!(stage_ready(&sched, "v2"));
    assert_eq!(
        task(&sched, v2).result,
        None,
        "the composite's result stays empty; the leaf holds the chunks"
    );
    assert!(sched
        .events()
        .iter()
        .any(|event| event.kind == EventKind::Done && event.scope.as_deref() == Some("v2")));
    let mut received = Vec::new();
    while let Ok(event) = rx.try_recv() {
        received.push(event);
    }
    assert_eq!(received, sched.events());
}

#[test]
fn studio_pause_and_resume_keeps_the_cursor_and_the_result() {
    let clock = Arc::new(FakeClock::new(4_000));
    let sched = scheduler(clock, pools());
    let analysis = sched
        .enqueue(
            job("analysis", "one", &json!({}))
                .composite()
                .scope("rec")
                .analysis("rec"),
        )
        .unwrap();
    let leaf = sched
        .enqueue(
            job("transcribe", "speech", &script(3, 1, 0, false, false, 0))
                .resource(Resource::Speech)
                .parent(analysis)
                .scope("rec")
                .analysis("rec"),
        )
        .unwrap();
    sched.tick().unwrap();
    view_matches_store(&sched);
    assert_eq!(task(&sched, leaf).status, Status::Running);
    let cursor = task(&sched, leaf).cursor.clone();
    sched.pause(leaf).unwrap();
    assert!(!sched.resume(leaf).unwrap());
    sched.tick().unwrap();
    view_matches_store(&sched);
    assert_eq!(task(&sched, leaf).status, Status::Paused);
    assert_eq!(task(&sched, leaf).cursor, cursor);
    assert_eq!(task(&sched, analysis).status, Status::Paused);
    assert!(sched.resume(leaf).unwrap());

    let mut cursors = Vec::new();
    for _ in 0..16 {
        sched.tick().unwrap();
        view_matches_store(&sched);
        let current = task(&sched, leaf).cursor.clone();
        if let Some(prev) = cursors.last() {
            assert!(cursor_u32(&current) >= cursor_u32(prev));
        }
        if !current.is_null() {
            cursors.push(current);
        }
        if task(&sched, leaf).status == Status::Done {
            break;
        }
    }
    assert_eq!(task(&sched, leaf).status, Status::Done);
    assert_eq!(
        cursors.iter().map(cursor_u32).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert_eq!(task(&sched, leaf).result, Some(json!({ "chunks": 3 })));
    assert_eq!(task(&sched, analysis).status, Status::Done);
    assert!(sched
        .events()
        .iter()
        .any(|event| event.kind == EventKind::Done && event.scope.as_deref() == Some("rec")));
    let snap = sched.snapshot().unwrap();
    assert_eq!(
        snap.tasks
            .iter()
            .find(|view| view.task.id == leaf)
            .unwrap()
            .task
            .result,
        Some(json!({ "chunks": 3 }))
    );
}

#[test]
fn two_chats_interleave_and_background_gets_every_third_chunk() {
    let clock = Arc::new(FakeClock::new(3_000));
    let sched = scheduler(
        clock,
        vec![Pool {
            resource: Resource::Llm,
            capacity: 1,
            memory: 100,
        }],
    );
    let chat = |key: &str| {
        job("answer", key, &script(6, 1, 0, false, false, 0))
            .resource(Resource::Llm)
            .chat(key)
            .scope(key)
    };
    let a = sched.enqueue(chat("a")).unwrap();
    let b = sched.enqueue(chat("b")).unwrap();
    let bg = sched
        .enqueue(
            job("transcribe", "bg", &script(6, 1, 0, false, false, 0))
                .resource(Resource::Llm)
                .analysis("bg")
                .scope("bg"),
        )
        .unwrap();
    sched.tick().unwrap();
    assert!(sched.turns().is_empty(), "a model call is not a chunk yet");
    assert_eq!(wait_of(&sched, b), Some(WaitReason::Turn));
    assert_eq!(
        wait_of(&sched, bg),
        Some(WaitReason::Resource(Resource::Llm))
    );
    assert_eq!(sched.live().len(), 1);
    view_matches_store(&sched);

    for _ in 0..40 {
        sched.tick().unwrap();
        view_matches_store(&sched);
        if sched.turns().len() >= 18 {
            break;
        }
    }
    let got: Vec<_> = sched
        .turns()
        .into_iter()
        .map(|turn| turn.participant)
        .collect();
    let expect: Vec<_> = (0..6)
        .flat_map(|_| ["c:a", "c:b", "a:bg"])
        .map(str::to_string)
        .collect();
    assert_eq!(got, expect);
    assert_eq!(task(&sched, a).status, Status::Done);
    assert_eq!(task(&sched, b).status, Status::Done);
    assert_eq!(task(&sched, bg).status, Status::Done);
}

#[test]
fn memory_budget_blocks_the_task_that_does_not_fit() {
    let clock = Arc::new(FakeClock::new(2_000));
    let sched = scheduler(
        clock,
        vec![Pool {
            resource: Resource::Llm,
            capacity: 2,
            memory: 10,
        }],
    );
    let first = sched
        .enqueue(
            job("embed", "big", &script(2, 1, 0, false, false, 0))
                .resource(Resource::Llm)
                .memory(8),
        )
        .unwrap();
    let second = sched
        .enqueue(
            job("embed", "also", &script(2, 1, 0, false, false, 0))
                .resource(Resource::Llm)
                .memory(8),
        )
        .unwrap();
    sched.tick().unwrap();
    let running = sched.live();
    assert_eq!(running.len(), 1);
    let waiting = if running[0].id == first {
        second
    } else {
        first
    };
    assert_eq!(wait_of(&sched, waiting), Some(WaitReason::Memory));
    view_matches_store(&sched);
}

#[test]
fn a_dependency_is_the_waiting_reason() {
    let clock = Arc::new(FakeClock::new(2_000));
    let sched = scheduler(clock, pools());
    let first = sched
        .enqueue(
            job("transcribe", "first", &json!({}))
                .resource(Resource::Llm)
                .model("speech-model"),
        )
        .unwrap();
    let second = sched
        .enqueue(
            job("index", "second", &json!({}))
                .resource(Resource::Llm)
                .needs(&[first])
                .model("embed-model"),
        )
        .unwrap();
    assert_eq!(wait_of(&sched, first), None);
    assert_eq!(wait_of(&sched, second), Some(WaitReason::Dependency));
    let models = sched.models_needed().unwrap();
    assert_eq!(models.len(), 2);
    assert!(models
        .iter()
        .any(|need| need.model == "speech-model" && need.tasks == vec![first]));
}

#[test]
fn a_note_is_plain_data_with_no_sentence_in_it() {
    assert_eq!(
        json!(pause_note("Темы: rec")),
        json!({"kind": "paused_by", "title": "Темы: rec"})
    );
    assert_eq!(
        json!(failure_note("Темы: rec")),
        json!({"kind": "failed_by", "title": "Темы: rec"})
    );
    assert_eq!(
        json!(hang_note(Resource::Llm, Duration::from_secs(90))),
        json!({"kind": "hang", "resource": "llm", "idle_s": 90})
    );
}

#[test]
fn a_hang_goes_back_to_its_cursor_and_a_late_write_is_ignored() {
    let clock = Arc::new(FakeClock::new(10_000));
    let sched = scheduler(clock.clone(), pools());
    let id = sched
        .enqueue(
            job("transcribe", "hang", &script(2, 0, 0, true, false, 0))
                .resource(Resource::Speech),
        )
        .unwrap();
    sched.tick().unwrap();
    // The second chunk is committed, the model call after it hangs: the task goes back to the
    // queue at its cursor at once, with the reason in its details.
    sched.tick().unwrap();
    let after = task(&sched, id);
    assert_eq!(
        (after.status, cursor_u32(&after.cursor)),
        (Status::Queued, 2),
        "the hung chunks stay committed"
    );
    assert!(sched.live().is_empty(), "the stuck run has left the slot");
    assert_eq!(after.heartbeat_ms, None);
    assert_eq!(
        serde_json::from_value::<Note>(after.detail["hang"].clone()).unwrap(),
        hang_note(Resource::Speech, Duration::from_secs(900))
    );
    let stale_gen = after.gen - 1;
    let rejected = sched
        .store()
        .apply(Op::Checkpoint {
            id,
            gen: stale_gen,
            cursor: json!(99),
            partial: json!(99),
            progress: 1.0,
        })
        .unwrap();
    assert!(!rejected.accepted, "the stuck call cannot write any more");
    assert_eq!(cursor_u32(&task(&sched, id).cursor), 2);

    // The retry (the model was unloaded) finishes, and the note goes with the finished task.
    sched.tick().unwrap();
    let done = task(&sched, id);
    assert_eq!(done.status, Status::Done);
    assert!(done.error.is_none());
    assert_eq!(done.detail.get("hang"), None);

    // A hang of a stopping task is settled by the runner's report instead of holding the slot.
    let again = sched
        .enqueue(
            job("transcribe", "hang2", &script(1, 1, 0, true, false, 0))
                .resource(Resource::Llm),
        )
        .unwrap();
    sched.tick().unwrap();
    assert_eq!(task(&sched, again).status, Status::Running);
    sched.cancel(again).unwrap();
    assert_eq!(task(&sched, again).status, Status::Stopping);
    sched.tick().unwrap();
    assert_eq!(task(&sched, again).status, Status::Cancelled);
    assert!(sched.live().iter().all(|run| run.id != again));
}

#[test]
fn a_crash_continues_from_the_last_chunk() {
    let clock = Arc::new(FakeClock::new(6_000));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.db");
    let id = {
        let store: Arc<dyn Store> = Arc::new(SqliteStore::open(&path, clock.clone()).unwrap());
        let sched = Scheduler::new(store, pools());
        sched.register(Box::new(ScriptRunner::default()));
        let id = sched
            .enqueue(
                job("transcribe", "crash", &script(3, 0, 0, false, false, 1))
                    .resource(Resource::Speech)
                    .scope("rec"),
            )
            .unwrap();
        sched.tick().unwrap();
        let task = sched.store().get(id).unwrap().unwrap();
        assert_eq!(
            (task.status, cursor_u32(&task.cursor)),
            (Status::Running, 1)
        );
        id
    };
    let store: Arc<dyn Store> = Arc::new(SqliteStore::open(&path, clock).unwrap());
    let sched = Scheduler::new(store, pools());
    sched.register(Box::new(ScriptRunner::default()));
    assert_eq!(
        task(&sched, id).status,
        Status::Running,
        "opening the file does not touch a live row"
    );
    sched.recover().unwrap();
    let restored = task(&sched, id);
    assert_eq!(restored.status, Status::Queued);
    assert_eq!(cursor_u32(&restored.cursor), 1);
    assert!(sched.live().is_empty());
    let mut cursors = vec![1];
    for _ in 0..8 {
        sched.tick().unwrap();
        view_matches_store(&sched);
        let current = cursor_u32(&task(&sched, id).cursor);
        if cursors.last().copied() != Some(current) {
            cursors.push(current);
        }
        if task(&sched, id).status == Status::Done {
            break;
        }
    }
    assert_eq!(task(&sched, id).status, Status::Done);
    assert_eq!(cursors, vec![1, 2, 3]);
    assert_eq!(task(&sched, id).result, Some(json!({ "chunks": 3 })));
    assert!(sched
        .events()
        .iter()
        .any(|event| event.kind == EventKind::Done && event.scope.as_deref() == Some("rec")));
}

/// An answer is one run. Its inline model steps are not a composite, so a crash requeues it
/// and the next claim continues.
#[test]
fn an_answer_with_inline_steps_is_still_one_run() {
    let clock = Arc::new(FakeClock::new(1_000));
    let store: Arc<dyn Store> = Arc::new(MemStore::new(clock));
    let id = {
        let sched = Scheduler::new(store.clone(), pools());
        sched.register(Box::new(ScriptRunner::default()));
        let id = sched
            .enqueue(job("chat-answer", "q", &script(2, 2, 0, false, false, 0)).chat("c:1"))
            .unwrap();
        sched.tick().unwrap();
        assert_eq!(task(&sched, id).status, Status::Running);
        let step = sched
            .begin_inline("model-step", "1", &json!({}), Some(id), Some("rec"))
            .unwrap();
        assert!(task(&sched, step).inline);
        assert_eq!(task(&sched, id).status, Status::Running, "the step does not take over the answer");
        sched.pause(id).unwrap();
        assert_eq!(task(&sched, id).status, Status::Stopping);
        id
    };
    let sched = Scheduler::new(store, pools());
    sched.recover().unwrap();
    assert_eq!(task(&sched, id).status, Status::Paused, "a pause that had not returned is settled");
    let steps: Vec<_> = sched
        .store()
        .children(id)
        .unwrap()
        .into_iter()
        .filter(|child| child.inline)
        .collect();
    assert!(steps.iter().all(|child| child.status == Status::Paused), "pause settles an inline step at once");
    sched.resume(id).unwrap();
    assert_eq!(task(&sched, id).status, Status::Queued);
    sched.register(Box::new(ScriptRunner::default()));
    for _ in 0..8 {
        sched.tick().unwrap();
        if task(&sched, id).status == Status::Done {
            break;
        }
    }
    assert_eq!(task(&sched, id).status, Status::Done);
}

#[test]
fn a_leaf_without_a_resource_is_rejected() {
    let clock = Arc::new(FakeClock::new(0));
    let store = MemStore::new(clock);
    let err = store
        .enqueue(NewTask::new("index", "x", &json!({})))
        .unwrap_err();
    assert!(err.to_string().contains("resource"), "{err}");
    store
        .enqueue(NewTask::new("analysis", "a", &json!({})).composite())
        .unwrap();
    store
        .enqueue(NewTask::new("tool", "t", &json!({})).inline())
        .unwrap();
}

#[test]
fn pausing_or_cancelling_an_inline_task_settles_it_at_once() {
    let clock = Arc::new(FakeClock::new(1_000));
    let store = MemStore::new(clock);
    let running = store
        .begin_inline("model-step", "step", &json!({}), None, None)
        .unwrap();
    assert!(store.pause(running).unwrap().changed > 0);
    assert_eq!(store.get(running).unwrap().unwrap().status, Status::Paused);
    store.resume(running).unwrap();
    let again = store
        .begin_inline("model-step", "step", &json!({}), None, None)
        .unwrap();
    assert!(store.cancel(again).unwrap().changed > 0);
    assert_eq!(
        store.get(again).unwrap().unwrap().status,
        Status::Cancelled
    );
}

#[test]
fn no_runner_fails_the_task_on_the_tick_that_claimed_it() {
    let clock = Arc::new(FakeClock::new(1_000));
    let store: Arc<dyn Store> = Arc::new(MemStore::new(clock));
    let sched = Scheduler::new(store, pools());
    let id = sched
        .enqueue(job("transcribe", "x", &json!({})))
        .unwrap();
    sched.tick().unwrap();
    let task = task(&sched, id);
    assert_eq!(task.status, Status::Failed);
    assert_eq!(task.error.as_deref(), Some("no runner for this kind"));
    assert!(task.started_ms.is_some());
    assert!(sched.live().is_empty());
}

fn both(test: fn(&dyn Store, &Arc<FakeClock>)) {
    let clock = Arc::new(FakeClock::new(1_000));
    let mem = MemStore::new(clock.clone());
    test(&mem, &clock);
    let clock = Arc::new(FakeClock::new(1_000));
    let dir = tempfile::tempdir().unwrap();
    let sql = SqliteStore::open(&dir.path().join("queue.db"), clock.clone()).unwrap();
    test(&sql, &clock);
}

#[test]
fn store_features_hold_for_memory_and_sqlite() {
    both(store_features);
}

fn store_features(store: &dyn Store, clock: &Arc<FakeClock>) {
    clock.set(1_000);
    let once = store
        .enqueue_once(job("index", "a", &json!({ "path": "/a" })))
        .unwrap();
    assert_eq!(
        store
            .enqueue_once(job("index", "a", &json!({ "path": "/a" })))
            .unwrap(),
        once
    );
    assert_ne!(
        store
            .enqueue_once(job("index", "b", &json!({ "path": "/b" })))
            .unwrap(),
        once
    );
    assert_ne!(
        store
            .enqueue_once(job("outline", "a", &json!({ "path": "/a" })))
            .unwrap(),
        once
    );
    store.claim(once).unwrap();
    store.pause(once).unwrap();
    assert_eq!(store.get(once).unwrap().unwrap().status, Status::Stopping);
    assert_eq!(
        store
            .enqueue_once(job("index", "a", &json!({ "path": "/a" })))
            .unwrap(),
        once,
        "still the same while stopping"
    );
    let gen = store.gen_of(once).unwrap();
    store.apply(Op::FinishStop { id: once, gen }).unwrap();
    store.resume(once).unwrap();
    store.claim(once).unwrap();
    store.complete(once, &json!({})).unwrap();
    assert_ne!(
        store
            .enqueue_once(job("index", "a", &json!({ "path": "/a" })))
            .unwrap(),
        once
    );

    let low = store
        .enqueue(job("transcribe", "low", &json!({})).priority(0))
        .unwrap();
    let high = store
        .enqueue(job("transcribe", "high", &json!({})).priority(5))
        .unwrap();
    let running = store
        .enqueue(job("embed", "run", &json!({})))
        .unwrap();
    assert!(store.claim(running).unwrap().accepted);
    let order: Vec<_> = store.list().unwrap().iter().map(|task| task.id).collect();
    let pos = |id: TaskId| order.iter().position(|item| *item == id).unwrap();
    assert!(pos(running) < pos(high) && pos(high) < pos(low));

    let gate = store
        .enqueue(job("index", "gate", &json!({})))
        .unwrap();
    let (topics, direct) = store
        .enqueue_tree(
            job("topics", "topics", &json!({}))
                .composite()
                .scope("f1")
                .needs(&[gate]),
            vec![
                Child::new(job("details", "details", &json!({})).composite())
                    .with_children(vec![Child::new(job("detail", "x", &json!({})))]),
            ],
        )
        .unwrap();
    let details = store.get(direct[0]).unwrap().unwrap();
    assert_eq!(
        (details.parent, details.scope.as_deref(), details.status),
        (Some(topics), Some("f1"), Status::Waiting)
    );
    let nested = store.children(details.id).unwrap()[0].id;
    assert!(
        !store.claim(nested).unwrap().accepted,
        "the root's needs hold the nested leaf"
    );
    store.claim(gate).unwrap();
    store.complete(gate, &json!({})).unwrap();
    assert!(store.claim(nested).unwrap().accepted);

    let composite = store
        .enqueue(job("analysis", "analysis", &json!({})).composite())
        .unwrap();
    assert_eq!(
        store.get(composite).unwrap().unwrap().status,
        Status::Waiting
    );
    let first = store
        .enqueue(job("step", "first", &json!({})).parent(composite))
        .unwrap();
    store.claim(first).unwrap();
    let added = store
        .complete_with(
            first,
            &json!({ "planned": 2 }),
            vec![
                Child::new(job("step", "second", &json!({}))),
                Child::new(job("step", "third", &json!({}))).after(&[0]),
            ],
        )
        .unwrap();
    assert!(added.accepted);
    assert_eq!(added.ids.len(), 2);
    assert_ne!(store.get(composite).unwrap().unwrap().status, Status::Done);
    assert!(store.claim(added.ids[0]).unwrap().accepted);
    store.complete(added.ids[0], &json!({})).unwrap();
    assert!(store.claim(added.ids[1]).unwrap().accepted);
    store.complete(added.ids[1], &json!({})).unwrap();
    assert_eq!(store.get(composite).unwrap().unwrap().status, Status::Done);
    assert!(
        !store
            .complete(added.ids[1], &json!({ "again": true }))
            .unwrap()
            .accepted
    );

    let top = store
        .enqueue(job("group", "top", &json!({})).composite())
        .unwrap();
    let mid = store
        .enqueue(
            job("group", "middle", &json!({}))
                .parent(top)
                .composite(),
        )
        .unwrap();
    let leaf = store
        .enqueue(job("k", "leaf", &json!({})).parent(mid))
        .unwrap();
    let after = store
        .enqueue(job("k", "later", &json!({})).needs(&[leaf]))
        .unwrap();
    store.claim(leaf).unwrap();
    store.pause(leaf).unwrap();
    assert_eq!(store.get(leaf).unwrap().unwrap().status, Status::Stopping);
    assert!(!store.resume(leaf).unwrap().accepted);
    assert_eq!(
        store.get(after).unwrap().unwrap().note,
        Some(pause_note("leaf"))
    );
    assert_eq!(
        (
            store.get(mid).unwrap().unwrap().status,
            store.get(top).unwrap().unwrap().status
        ),
        (Status::Paused, Status::Paused)
    );
    let leaf_gen = store.gen_of(leaf).unwrap();
    store
        .apply(Op::FinishStop {
            id: leaf,
            gen: leaf_gen,
        })
        .unwrap();
    assert_eq!(store.get(leaf).unwrap().unwrap().status, Status::Paused);
    assert!(store.resume(leaf).unwrap().accepted);
    assert_eq!(store.get(after).unwrap().unwrap().note, None);
    assert_eq!(store.get(top).unwrap().unwrap().status, Status::Running);
    store.cancel(leaf).unwrap();
    assert_eq!(store.get(after).unwrap().unwrap().status, Status::Cancelled);
    assert_eq!(store.get(top).unwrap().unwrap().status, Status::Cancelled);
    store.restart(mid).unwrap();
    assert_eq!(store.get(leaf).unwrap().unwrap().status, Status::Queued);
    assert_eq!(store.get(after).unwrap().unwrap().status, Status::Queued);
    assert_eq!(store.get(top).unwrap().unwrap().status, Status::Running);

    let early = store
        .enqueue(job("k", "early", &json!({})))
        .unwrap();
    let done_mid = store
        .enqueue(job("k", "done-middle", &json!({})).needs(&[early]))
        .unwrap();
    let held = store
        .enqueue(job("k", "held", &json!({})).needs(&[done_mid]))
        .unwrap();
    store.complete(early, &json!({})).unwrap();
    store.complete(done_mid, &json!({})).unwrap();
    store.restart(early).unwrap();
    store.pause(early).unwrap();
    assert_eq!(
        store.get(held).unwrap().unwrap().note,
        Some(pause_note("early"))
    );
    assert!(!store.claim(held).unwrap().accepted);
    assert_eq!(store.get(done_mid).unwrap().unwrap().status, Status::Done);

    store.claim(leaf).unwrap();
    clock.advance(2_000);
    store.fail(leaf, "model crashed").unwrap();
    let failed = store.get(leaf).unwrap().unwrap();
    assert!(failed.started_ms.is_some());
    assert!(failed.finished_ms.unwrap() >= failed.started_ms.unwrap());
    assert_eq!(
        store.get(after).unwrap().unwrap().note,
        Some(failure_note("leaf"))
    );
    assert_eq!(store.get(top).unwrap().unwrap().status, Status::Failed);
    store.restart(leaf).unwrap();
    assert!(store.get(leaf).unwrap().unwrap().started_ms.is_none());

    let analysis = store
        .enqueue(
            job("analysis", "analysis", &json!({}))
                .composite()
                .scope("rec"),
        )
        .unwrap();
    let topics = store
        .enqueue(
            job("topics", "topics", &json!({}))
                .parent(analysis)
                .composite()
                .scope("rec"),
        )
        .unwrap();
    let digest = store
        .enqueue(
            job("topic-digest", "digest", &json!({}))
                .parent(topics)
                .scope("rec"),
        )
        .unwrap();
    let answer = store
        .begin_inline("answer", "answer", &json!({}), None, Some("rec"))
        .unwrap();
    let other = store
        .enqueue(
            job("analysis", "other", &json!({}))
                .composite()
                .scope("rec-2"),
        )
        .unwrap();
    let standalone = store
        .enqueue(job("transcribe", "old", &json!({})).scope("rec"))
        .unwrap();
    assert_eq!(store.cancel_analysis("rec").unwrap().changed, 3);
    for id in [analysis, topics, digest] {
        assert_eq!(store.get(id).unwrap().unwrap().status, Status::Cancelled);
    }
    assert_eq!(store.get(answer).unwrap().unwrap().status, Status::Running);
    assert_eq!(store.get(other).unwrap().unwrap().status, Status::Waiting);
    assert_eq!(
        store.get(standalone).unwrap().unwrap().status,
        Status::Queued
    );
    assert!(!store.claim(answer).unwrap().accepted);
    store.recover().unwrap();
    assert_eq!(
        store.get(answer).unwrap().unwrap().status,
        Status::Cancelled
    );

    let inline = store
        .begin_inline("tool", "search", &json!({}), None, None)
        .unwrap();
    store
        .add_stats(
            inline,
            &Stats {
                model_calls: 1,
                prompt_tokens: 100,
                completion_tokens: 20,
                model_ms: 400,
                models: vec!["qwen".into()],
            },
        )
        .unwrap();
    store
        .add_stats(
            inline,
            &Stats {
                model_calls: 2,
                prompt_tokens: 50,
                completion_tokens: 5,
                model_ms: 100,
                models: vec!["qwen".into(), "bge".into()],
            },
        )
        .unwrap();
    let stats = store.get(inline).unwrap().unwrap().stats;
    assert_eq!(
        (
            stats.model_calls,
            stats.prompt_tokens,
            stats.completion_tokens,
            stats.model_ms
        ),
        (3, 150, 25, 500)
    );
    assert_eq!(stats.models, ["qwen", "bge"]);
    store
        .set_detail(inline, &json!({ "tool": "search" }))
        .unwrap();
    assert_eq!(store.get(inline).unwrap().unwrap().detail["tool"], "search");
    assert!(
        store
            .complete(inline, &json!({ "ok": true }))
            .unwrap()
            .accepted
    );
    assert_eq!(store.get(inline).unwrap().unwrap().status, Status::Done);

    clock.set(1_000);
    let sized = store
        .enqueue(job("k", "sized", &json!({})).units(10.0))
        .unwrap();
    store.claim(sized).unwrap();
    clock.set(21_000);
    store.complete(sized, &json!({})).unwrap();
    let stat = store
        .kind_stats()
        .unwrap()
        .into_iter()
        .find(|stat| stat.kind == "k")
        .unwrap();
    assert_eq!((stat.runs, stat.ms, stat.units), (1, 20_000, 10.0));
    let plain = store
        .enqueue(job("k", "plain", &json!({})))
        .unwrap();
    store.claim(plain).unwrap();
    store.complete(plain, &json!({})).unwrap();
    assert_eq!(
        store
            .kind_stats()
            .unwrap()
            .into_iter()
            .find(|stat| stat.kind == "k")
            .unwrap()
            .runs,
        1
    );

    clock.set(50_000);
    let rate = store
        .enqueue(job("rate", "rate", &json!({})))
        .unwrap();
    store.claim(rate).unwrap();
    store.checkpoint(rate, &json!(1), &json!([]), 0.5).unwrap();
    clock.set(70_000);
    let left = estimate(&store.get(rate).unwrap().unwrap(), clock.now_ms(), &[]).unwrap();
    assert!((left.as_millis() as i64 - 20_000).abs() < 50, "{left:?}");
    let fresh = store
        .enqueue(job("probe", "fresh", &json!({})).units(5.0))
        .unwrap();
    let history = [KindStat {
        kind: "probe".into(),
        runs: 1,
        ms: 60_000,
        units: 10.0,
    }];
    assert_eq!(
        estimate(
            &store.get(fresh).unwrap().unwrap(),
            clock.now_ms(),
            &history
        )
        .unwrap(),
        Duration::from_secs(30)
    );
    let unknown = store
        .enqueue(job("new", "unknown", &json!({})))
        .unwrap();
    assert_eq!(
        estimate(&store.get(unknown).unwrap().unwrap(), clock.now_ms(), &[]),
        None
    );

    let group = store
        .enqueue(job("group", "group", &json!({})).composite())
        .unwrap();
    let done_child = store
        .enqueue(job("probe", "done", &json!({})).parent(group))
        .unwrap();
    let left_child = store
        .enqueue(
            job("probe", "a", &json!({}))
                .parent(group)
                .units(5.0),
        )
        .unwrap();
    let right_child = store
        .enqueue(
            job("probe", "b", &json!({}))
                .parent(group)
                .units(10.0),
        )
        .unwrap();
    store.complete(done_child, &json!({})).unwrap();
    let tree = store.subtree(group).unwrap();
    let summed = estimate_tree(
        &store.get(group).unwrap().unwrap(),
        &tree,
        clock.now_ms(),
        &history,
    )
    .unwrap();
    assert_eq!(summed, Duration::from_secs(90));
    assert_eq!(store.get(group).unwrap().unwrap().status, Status::Running);
    store.complete(left_child, &json!({})).unwrap();
    store.complete(right_child, &json!({})).unwrap();
    assert_eq!(
        estimate_tree(
            &store.get(group).unwrap().unwrap(),
            &store.subtree(group).unwrap(),
            clock.now_ms(),
            &history
        ),
        Some(Duration::ZERO)
    );

    clock.set(0);
    let old = store
        .enqueue(job("old", "old", &json!({})).units(4.0))
        .unwrap();
    store.claim(old).unwrap();
    store.complete(old, &json!({})).unwrap();
    let old_group = store
        .enqueue(job("group", "old-group", &json!({})).composite())
        .unwrap();
    let old_child = store
        .enqueue(job("k", "old-child", &json!({})).parent(old_group))
        .unwrap();
    store.claim(old_child).unwrap();
    store.complete(old_child, &json!({})).unwrap();
    let young_at = 31 * 24 * 60 * 60 * 1_000;
    clock.set(young_at);
    let young = store
        .enqueue(job("k", "young", &json!({})))
        .unwrap();
    store.claim(young).unwrap();
    store.complete(young, &json!({})).unwrap();
    let unfinished = store
        .enqueue(job("k", "unfinished", &json!({})))
        .unwrap();
    clock.set(young_at + 1_000);
    let cutoff = clock.now_ms() - HISTORY.as_millis() as i64;
    store.purge_older_than(cutoff).unwrap();
    assert!(store.get(old).unwrap().is_none());
    assert!(store.get(old_group).unwrap().is_none());
    assert!(store.get(old_child).unwrap().is_none());
    assert!(store.get(young).unwrap().is_some());
    assert!(store.get(unfinished).unwrap().is_some());
    assert!(store
        .kind_stats()
        .unwrap()
        .iter()
        .any(|stat| stat.kind == "old" && stat.runs == 1));

    let parent = store
        .enqueue(job("group", "keep", &json!({})).composite())
        .unwrap();
    let child = store
        .enqueue(job("k", "child", &json!({})).parent(parent))
        .unwrap();
    let pending = store
        .enqueue(job("k", "pending", &json!({})).parent(parent))
        .unwrap();
    store.claim(child).unwrap();
    store.complete(child, &json!({})).unwrap();
    assert_ne!(store.get(parent).unwrap().unwrap().status, Status::Done);
    store.clear_done().unwrap();
    assert!(
        store.get(child).unwrap().is_some(),
        "a done child of an unfinished parent stays"
    );
    assert!(store.get(pending).unwrap().is_some());
    assert!(store.get(parent).unwrap().is_some());

    let before = store.list().unwrap().len();
    let err = store.enqueue_tree(
        job("group", "bad", &json!({})).composite(),
        vec![Child::new(job("k", "a", &json!({}))).after(&[3])],
    );
    assert!(err.is_err());
    assert_eq!(store.list().unwrap().len(), before);

    let stopping = store
        .enqueue(job("k", "model", &json!({})).model("kept"))
        .unwrap();
    store.claim(stopping).unwrap();
    store.pause(stopping).unwrap();
    let finished = store
        .enqueue(job("k", "finished-model", &json!({})).model("dropped"))
        .unwrap();
    store.claim(finished).unwrap();
    store.complete(finished, &json!({})).unwrap();
    let needed = state::models_needed(&store.world().unwrap().tasks);
    assert!(needed
        .iter()
        .any(|(model, ids)| model == "kept" && ids.contains(&stopping)));
    assert!(!needed.iter().any(|(model, _)| model == "dropped"));
}

#[test]
fn a_file_of_another_schema_is_recreated_and_ids_are_not_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.db");
    {
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TABLE tasks (id INTEGER PRIMARY KEY); INSERT INTO tasks (id) VALUES (7); PRAGMA user_version = 3;").unwrap();
    }
    let clock = Arc::new(FakeClock::new(1_000));
    let id = {
        let store = SqliteStore::open(&path, clock.clone()).unwrap();
        assert!(
            store.list().unwrap().is_empty(),
            "the old file is dropped, not migrated"
        );
        let id = store
            .enqueue(job("k", "new", &json!({})).scope("s").units(3.0))
            .unwrap();
        let task = store.get(id).unwrap().unwrap();
        assert_eq!(
            (task.status, task.scope.as_deref(), task.units),
            (Status::Queued, Some("s"), 3.0)
        );
        assert!(store.kind_stats().unwrap().is_empty());
        id
    };
    let store = SqliteStore::open(&path, clock).unwrap();
    assert!(store.get(id).unwrap().is_some(), "version 2 is kept");
    let next = store
        .enqueue(job("k", "next", &json!({})))
        .unwrap();
    assert!(next > id, "ids survive a reopen");
}

fn check_invariants(
    sched: &Scheduler,
    pool_list: &[Pool],
    cursors: &mut HashMap<TaskId, Value>,
) -> std::result::Result<(), String> {
    let world = sched.store().world().map_err(|err| err.to_string())?;
    let now = sched.store().now_ms();
    let live = sched.live();
    for task in &world.tasks {
        let leaf = !task.inline && !world.has_owned_children(task.id);
        let hot = leaf && matches!(task.status, Status::Running | Status::Stopping);
        let runs: Vec<_> = live.iter().filter(|run| run.id == task.id).collect();
        if hot && runs.len() != 1 {
            return Err(format!(
                "task {} status {:?} has {} runs",
                task.id,
                task.status,
                runs.len()
            ));
        }
        if hot && runs[0].gen != task.gen {
            return Err(format!(
                "task {} gen {} run {}",
                task.id, task.gen, runs[0].gen
            ));
        }
        if !hot && !runs.is_empty() {
            return Err(format!(
                "task {} status {:?} still holds a slot",
                task.id, task.status
            ));
        }
        if task.input.get("chunks").is_some() && task.partial != task.cursor {
            return Err(format!(
                "task {} partial {:?} cursor {:?}",
                task.id, task.partial, task.cursor
            ));
        }
        if let Some(previous) = cursors.get(&task.id) {
            if let (Some(before), Some(after)) = (previous.as_u64(), task.cursor.as_u64()) {
                if after < before {
                    return Err(format!(
                        "task {} cursor went from {before} to {after}",
                        task.id
                    ));
                }
            }
        }
        cursors.insert(task.id, task.cursor.clone());
        // Заново clears the start but keeps attempts. A pause before the next claim is paused with no start.
        let needs_start = matches!(task.status, Status::Running | Status::Stopping)
            || (matches!(task.status, Status::Done | Status::Failed) && task.attempts > 0);
        if needs_start && task.started_ms.is_none() {
            return Err(format!("task {} {:?} has no start", task.id, task.status));
        }
        if task.status.is_finished() && task.finished_ms.is_none() {
            return Err(format!("task {} {:?} has no finish", task.id, task.status));
        }
        if !task.status.is_finished() && task.finished_ms.is_some() {
            return Err(format!(
                "task {} {:?} finished while live",
                task.id, task.status
            ));
        }
        if let (Some(started), Some(finished)) = (task.started_ms, task.finished_ms) {
            if finished < started {
                return Err(format!("task {} finished before it started", task.id));
            }
        }
        for stamp in [
            Some(task.created_ms),
            Some(task.updated_ms),
            task.started_ms,
            task.finished_ms,
        ]
        .into_iter()
        .flatten()
        {
            if stamp > now {
                return Err(format!("task {} stamp {stamp} is after {now}", task.id));
            }
        }
        if task.updated_ms < task.created_ms {
            return Err(format!("task {} updated before it was created", task.id));
        }
        if !effective_status(task).is_finished() && depends_on_dead(&world, task.id) {
            return Err(format!(
                "task {} still waits on a failed or cancelled task",
                task.id
            ));
        }
        if !task.inline && world.has_owned_children(task.id) {
            let children: Vec<Task> = world
                .tasks
                .iter()
                .filter(|child| child.parent == Some(task.id))
                .cloned()
                .collect();
            let states: Vec<_> = children
                .iter()
                .map(|child| state::child_state(&world, child))
                .collect();
            let expected = composite_status(&states)
                .ok_or_else(|| format!("task {} has no composite status", task.id))?;
            if task.status != expected {
                return Err(format!(
                    "task {} is {:?} but rule 13.7 says {:?}",
                    task.id, task.status, expected
                ));
            }
        }
    }
    for pool in pool_list {
        let slots: Vec<_> = live
            .iter()
            .filter(|run| run.resource == pool.resource)
            .collect();
        if slots.len() > pool.capacity as usize {
            return Err(format!("{:?} has {} slots", pool.resource, slots.len()));
        }
        let memory: u64 = slots.iter().map(|run| run.memory).sum();
        if memory > pool.memory {
            return Err(format!(
                "{:?} uses {memory} of {}",
                pool.resource, pool.memory
            ));
        }
    }
    let turns = sched.turns();
    let mut chat_streak: HashMap<Resource, u32> = HashMap::new();
    for (index, turn) in turns.iter().enumerate() {
        let streak = chat_streak.entry(turn.resource).or_insert(0);
        if turn.bg_ready && turn.chat {
            *streak += 1;
            if *streak > 2 {
                return Err(format!(
                    "{:?} background was waiting and chats took {streak} chunks",
                    turn.resource
                ));
            }
        } else {
            *streak = 0;
        }
        let limit = 3 * turn.ready.len().max(1);
        for participant in &turn.ready {
            let mut waited = 0;
            for later in turns.iter().skip(index) {
                if later.resource != turn.resource {
                    continue;
                }
                if later.participant == *participant {
                    break;
                }
                if !later.ready.iter().any(|ready| ready == participant) {
                    break;
                }
                waited += 1;
                if waited > limit {
                    return Err(format!(
                        "{participant} waited {waited} chunks on {:?}, limit {limit}",
                        turn.resource
                    ));
                }
            }
        }
    }
    Ok(())
}

fn depends_on_dead(world: &state::World, id: TaskId) -> bool {
    let mut queue: VecDeque<TaskId> = world
        .deps
        .iter()
        .filter(|(task, _)| *task == id)
        .map(|(_, need)| *need)
        .collect();
    let mut seen = HashSet::new();
    while let Some(dep) = queue.pop_front() {
        if !seen.insert(dep) {
            continue;
        }
        let Some(found) = world.task(dep) else {
            continue;
        };
        if matches!(effective_status(found), Status::Failed | Status::Cancelled) {
            return true;
        }
        queue.extend(
            world
                .deps
                .iter()
                .filter(|(task, _)| *task == dep)
                .map(|(_, need)| *need),
        );
    }
    false
}

fn quiet(sched: &Scheduler) -> bool {
    let world = sched.store().world().unwrap();
    let hot = world
        .tasks
        .iter()
        .any(|task| matches!(task.status, Status::Running | Status::Stopping));
    let claimable = world
        .tasks
        .iter()
        .any(|task| state::is_claimable(&world, task.id));
    !hot && !claimable && sched.live().is_empty()
}

fn drain(sched: &Scheduler, clock: &FakeClock) {
    // A hang ignores the stop flag and holds its slot until the stale reap. That reap can start
    // the next hang in the same tick, so cancel-and-reap has to run once per stuck task.
    for _ in 0..12 {
        for _ in 0..40 {
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
            if quiet(sched) {
                return;
            }
        }
        let stuck: Vec<TaskId> = sched
            .store()
            .world()
            .unwrap()
            .tasks
            .iter()
            .filter(|task| matches!(task.status, Status::Running | Status::Stopping))
            .map(|task| task.id)
            .collect();
        for id in stuck {
            let _ = sched.cancel(id);
        }
        clock.advance(120_000);
        sched.tick().unwrap();
        if quiet(sched) {
            return;
        }
    }
}

#[derive(Clone, Debug)]
enum Act {
    Tick,
    Advance(u64),
    Pause(usize),
    Resume(usize),
    Cancel(usize),
    Restart(usize),
    Crash,
    Enqueue(Job),
}

#[derive(Clone, Debug)]
struct Job {
    chunks: u32,
    busy: u32,
    slow: u32,
    hang: bool,
    fail: bool,
    speech: bool,
    chat: bool,
    scope: u8,
    tree: bool,
}

fn id_at(world: &state::World, index: usize) -> Option<TaskId> {
    let mut ids: Vec<TaskId> = world.tasks.iter().map(|task| task.id).collect();
    ids.sort_unstable();
    if ids.is_empty() {
        None
    } else {
        Some(ids[index % ids.len()])
    }
}

fn enqueue_job(sched: &Scheduler, spec: &Job) {
    let leaves = sched
        .store()
        .world()
        .unwrap()
        .tasks
        .iter()
        .filter(|task| task.input.get("chunks").is_some())
        .count();
    if leaves >= 6 {
        return;
    }
    let scope = format!("s{}", spec.scope);
    let input = script(
        spec.chunks,
        spec.busy,
        spec.slow,
        spec.hang && !spec.fail,
        spec.fail,
        0,
    );
    let mut leaf = job(if spec.chat { "answer" } else { "transcribe" }, "t", &input)
        .resource(if spec.speech {
            Resource::Speech
        } else {
            Resource::Llm
        })
        .memory(1)
        .scope(&scope);
    if spec.chat {
        leaf = leaf.chat(&format!("c{}", spec.scope));
    } else {
        leaf = leaf.analysis(&format!("r{}", spec.scope));
    }
    if spec.tree && !spec.chat {
        let root = job("analysis", "analysis", &json!({}))
            .composite()
            .scope(&scope)
            .analysis(&format!("r{}", spec.scope));
        sched.enqueue_tree(root, vec![Child::new(leaf)]).unwrap();
    } else {
        sched.enqueue(leaf).unwrap();
    }
}

fn simulation_config() -> proptest::test_runner::Config {
    proptest::test_runner::Config {
        cases: 200,
        failure_persistence: None,
        ..proptest::test_runner::Config::default()
    }
}

proptest::proptest! {
    #![proptest_config(simulation_config())]
    #[test]
    fn simulation(actions in proptest::collection::vec(action_strategy(), 1..30)) {
        let clock = Arc::new(FakeClock::new(10_000));
        let pool_list = vec![
            Pool { resource: Resource::Speech, capacity: 1, memory: 1_000 },
            Pool { resource: Resource::Llm, capacity: 1, memory: 1_000 },
        ];
        let sched = scheduler(clock.clone(), pool_list.clone());
        sched.set_stale(Duration::from_millis(5_000));
        let mut cursors = HashMap::new();
        for action in &actions {
            match action {
                Act::Tick => { sched.tick().unwrap(); }
                Act::Advance(ms) => clock.advance(*ms as i64),
                Act::Pause(index) => {
                    if let Some(id) = id_at(&sched.store().world().unwrap(), *index) { sched.pause(id).unwrap(); }
                }
                Act::Resume(index) => {
                    if let Some(id) = id_at(&sched.store().world().unwrap(), *index) { let _ = sched.resume(id); }
                }
                Act::Cancel(index) => {
                    if let Some(id) = id_at(&sched.store().world().unwrap(), *index) { sched.cancel(id).unwrap(); }
                }
                Act::Restart(index) => {
                    if let Some(id) = id_at(&sched.store().world().unwrap(), *index) { sched.restart(id).unwrap(); }
                }
                Act::Crash => { sched.recover().unwrap(); }
                Act::Enqueue(job) => enqueue_job(&sched, job),
            }
            if let Err(err) = check_invariants(&sched, &pool_list, &mut cursors) {
                prop_assert!(false, "{err} after {action:?}");
            }
        }
        drain(&sched, &clock);
        if let Err(err) = check_invariants(&sched, &pool_list, &mut cursors) {
            prop_assert!(false, "{err} after drain");
        }
        let world = sched.store().world().unwrap();
        let stuck: Vec<_> = world.tasks.iter().filter(|task| matches!(task.status, Status::Running | Status::Stopping) || state::is_claimable(&world, task.id)).map(|task| (task.id, task.status)).collect();
        prop_assert!(stuck.is_empty() && sched.live().is_empty(), "not drained: {stuck:?}");
    }
}

fn action_strategy() -> impl proptest::strategy::Strategy<Value = Act> {
    use proptest::prelude::*;
    let job = (
        1u32..=3,
        0u32..=1,
        0u32..=1,
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        0u8..=1,
        any::<bool>(),
    )
        .prop_map(
            |(chunks, busy, slow, hang, fail, speech, chat, scope, tree)| Job {
                chunks,
                busy,
                slow,
                hang,
                fail,
                speech,
                chat,
                scope,
                tree,
            },
        );
    prop_oneof![
        Just(Act::Tick),
        (0u64..3_000).prop_map(Act::Advance),
        (0usize..8).prop_map(Act::Pause),
        (0usize..8).prop_map(Act::Resume),
        (0usize..8).prop_map(Act::Cancel),
        (0usize..8).prop_map(Act::Restart),
        Just(Act::Crash),
        job.prop_map(Act::Enqueue),
    ]
}
