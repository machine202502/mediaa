use mediaa_core::assistant::Live;
use mediaa_protocol::backend::{AgentEventWire, AgentSnapshotWire, AgentStateWire};
use parking_lot::Mutex;
use std::collections::HashMap;

#[derive(Default)]
struct LiveBuffer {
    live: HashMap<i64, Live>,
    state: Option<AgentStateWire>,
    snapshot: Option<AgentSnapshotWire>,
}

static LIVE: Mutex<Option<LiveBuffer>> = Mutex::new(None);

fn buffer() -> parking_lot::MutexGuard<'static, Option<LiveBuffer>> {
    LIVE.lock()
}

pub fn apply_event(event: &AgentEventWire) {
    let mut guard = buffer();
    let live = guard
        .get_or_insert_with(LiveBuffer::default)
        .live
        .entry(event.task)
        .or_default();
    live.apply(&event.event);
}

pub fn apply_snapshot(snapshot: AgentSnapshotWire) {
    buffer().get_or_insert_with(LiveBuffer::default).snapshot = Some(snapshot);
}

pub fn apply_state(state: AgentStateWire) {
    buffer().get_or_insert_with(LiveBuffer::default).state = Some(state);
}

#[allow(dead_code, reason = "used by the live chat rewrite")]
pub fn live(task: i64) -> Option<Live> {
    buffer()
        .as_ref()
        .and_then(|live| live.live.get(&task).cloned())
}

#[allow(dead_code, reason = "used by the queue tab rewrite")]
pub fn snapshot() -> Option<AgentSnapshotWire> {
    buffer().as_ref().and_then(|live| live.snapshot.clone())
}

#[allow(dead_code, reason = "used by the live chat rewrite")]
pub fn state() -> Option<AgentStateWire> {
    buffer().as_ref().and_then(|live| live.state.clone())
}

#[allow(dead_code, reason = "used by the live chat rewrite")]
pub fn forget(task: i64) {
    if let Some(live) = buffer().as_mut() {
        live.live.remove(&task);
    }
}
