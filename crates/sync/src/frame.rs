use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "frame", rename_all = "snake_case")]
pub enum Frame {
    Hello {
        version: u32,
        client: String,
    },
    Welcome {
        version: u32,
        session: u64,
        event_seq: u64,
        metadata: BTreeMap<String, String>,
    },
    Subscribe {
        stores: Vec<String>,
        versions: BTreeMap<String, u64>,
        event_seq: u64,
    },
    Snapshot {
        store: String,
        version: u64,
        payload: Vec<u8>,
    },
    Changed {
        store: String,
        version: u64,
        payload: Vec<u8>,
    },
    Resync {
        stores: Vec<String>,
        event_seq: u64,
    },
    Command {
        id: u64,
        name: String,
        payload: Vec<u8>,
    },
    Result {
        id: u64,
        ok: bool,
        payload: Vec<u8>,
        error: Option<String>,
    },
    Event {
        seq: u64,
        kind: String,
        payload: Vec<u8>,
    },
    Reject {
        reason: String,
    },
    Ping,
    Pong,
    Error {
        code: String,
        message: String,
    },
}
