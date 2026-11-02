use crossbeam_channel::Sender;
use mediaa_protocol::backend::{AgentRequestWire, AgentResponseStateWire, AgentResponseWire};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

static NEXT: AtomicU64 = AtomicU64::new(1);
static RESPONSES: Mutex<Option<HashMap<u64, AgentResponseStateWire>>> = Mutex::new(None);
static SENDER: OnceLock<Sender<AgentRequestWire>> = OnceLock::new();

pub fn install(sender: Sender<AgentRequestWire>) {
    let _ = SENDER.set(sender);
}

pub fn next_id() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub fn start(kind: &str, payload: serde_json::Value) -> u64 {
    let request = next_id();
    if let Some(sender) = SENDER.get() {
        let _ = sender.try_send(AgentRequestWire {
            request,
            kind: kind.to_string(),
            payload,
        });
    }
    request
}

pub fn apply(response: AgentResponseWire) {
    let mut guard = RESPONSES.lock();
    let responses = guard.get_or_insert_with(HashMap::new);
    responses.insert(response.request, response.state);
}

pub fn take(request: u64) -> Option<AgentResponseStateWire> {
    RESPONSES.lock().as_mut()?.remove(&request)
}
