use crate::events::EventLog;
use crate::frame::{Frame, PROTOCOL_VERSION};
use crate::state::StateHub;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandRequest {
    pub id: u64,
    pub name: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ServerAction {
    Frames(Vec<Frame>),
    Command(CommandRequest),
}

#[derive(Debug, Clone, PartialEq)]
struct CachedResult {
    ok: bool,
    payload: Vec<u8>,
    error: Option<String>,
}

pub struct Server {
    hub: StateHub,
    events: EventLog,
    session: u64,
    handshaken: bool,
    subscribed: BTreeSet<String>,
    sent_versions: BTreeMap<String, u64>,
    event_cursor: u64,
    results: HashMap<u64, CachedResult>,
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

impl Server {
    pub fn new() -> Self {
        Self {
            hub: StateHub::new(),
            events: EventLog::new(4096),
            session: 0,
            handshaken: false,
            subscribed: BTreeSet::new(),
            sent_versions: BTreeMap::new(),
            event_cursor: 0,
            results: HashMap::new(),
        }
    }

    pub fn hub_mut(&mut self) -> &mut StateHub {
        &mut self.hub
    }

    pub fn events_mut(&mut self) -> &mut EventLog {
        &mut self.events
    }

    pub fn on_frame(&mut self, frame: Frame) -> ServerAction {
        match frame {
            Frame::Hello { version, .. } => {
                if version != PROTOCOL_VERSION {
                    return ServerAction::Frames(vec![Frame::Error {
                        code: "protocol_version".into(),
                        message: format!("expected {PROTOCOL_VERSION}, got {version}"),
                    }]);
                }
                self.session += 1;
                self.handshaken = true;
                self.subscribed.clear();
                self.sent_versions.clear();
                self.event_cursor = 0;
                self.results.clear();
                let event_seq = self.events.next_seq().saturating_sub(1);
                ServerAction::Frames(vec![Frame::Welcome {
                    version: PROTOCOL_VERSION,
                    session: self.session,
                    event_seq,
                    metadata: BTreeMap::new(),
                }])
            }
            Frame::Subscribe {
                stores,
                versions,
                event_seq,
            } => {
                if !self.handshaken {
                    return ServerAction::Frames(vec![Frame::Error {
                        code: "handshake".into(),
                        message: "Hello is required".into(),
                    }]);
                }
                self.subscribed = stores.iter().cloned().collect();
                let mut out = Vec::new();
                for store in &stores {
                    let Some((version, payload)) = self.hub.snapshot(store) else {
                        continue;
                    };
                    let known = versions.get(store).copied().unwrap_or(0);
                    if known != version {
                        out.push(Frame::Snapshot {
                            store: store.clone(),
                            version,
                            payload: payload.to_vec(),
                        });
                    }
                    self.sent_versions.insert(store.clone(), version);
                }
                let cursor = if event_seq == 0 {
                    self.events.next_seq().saturating_sub(1)
                } else {
                    event_seq
                };
                let since = self.events.since(cursor);
                if since.gap {
                    out.push(Frame::Resync {
                        stores: stores.clone(),
                        event_seq: self.events.next_seq().saturating_sub(1),
                    });
                    self.event_cursor = self.events.next_seq().saturating_sub(1);
                } else {
                    self.event_cursor = cursor;
                    for event in since.events {
                        self.event_cursor = event.seq;
                        out.push(Frame::Event {
                            seq: event.seq,
                            kind: event.kind,
                            payload: event.payload,
                        });
                    }
                }
                ServerAction::Frames(out)
            }
            Frame::Resync { stores, .. } => {
                if !self.handshaken {
                    return ServerAction::Frames(vec![Frame::Error {
                        code: "handshake".into(),
                        message: "Hello is required".into(),
                    }]);
                }
                let mut out = Vec::new();
                for store in stores {
                    if let Some((version, payload)) = self.hub.snapshot(&store) {
                        out.push(Frame::Snapshot {
                            store: store.clone(),
                            version,
                            payload: payload.to_vec(),
                        });
                        self.sent_versions.insert(store, version);
                    }
                }
                ServerAction::Frames(out)
            }
            Frame::Command { id, name, payload } => {
                if !self.handshaken {
                    return ServerAction::Frames(vec![Frame::Error {
                        code: "handshake".into(),
                        message: "Hello is required".into(),
                    }]);
                }
                if let Some(cached) = self.results.get(&id) {
                    return ServerAction::Frames(vec![Frame::Result {
                        id,
                        ok: cached.ok,
                        payload: cached.payload.clone(),
                        error: cached.error.clone(),
                    }]);
                }
                ServerAction::Command(CommandRequest { id, name, payload })
            }
            Frame::Ping => ServerAction::Frames(vec![Frame::Pong]),
            Frame::Pong => ServerAction::Frames(Vec::new()),
            _ => ServerAction::Frames(vec![Frame::Error {
                code: "unexpected_frame".into(),
                message: "the server cannot accept this frame".into(),
            }]),
        }
    }

    pub fn complete_command(&mut self, id: u64, result: Result<Vec<u8>, String>) -> Frame {
        let (ok, payload, error) = match result {
            Ok(payload) => (true, payload, None),
            Err(error) => (false, Vec::new(), Some(error)),
        };
        self.results.insert(
            id,
            CachedResult {
                ok,
                payload: payload.clone(),
                error: error.clone(),
            },
        );
        Frame::Result {
            id,
            ok,
            payload,
            error,
        }
    }

    pub fn flush(&mut self) -> Vec<Frame> {
        if !self.handshaken {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (store, version, payload) in self.hub.changes(&self.sent_versions) {
            if !self.subscribed.contains(&store) {
                continue;
            }
            self.sent_versions.insert(store.clone(), version);
            out.push(Frame::Changed {
                store,
                version,
                payload,
            });
        }
        let since = self.events.since(self.event_cursor);
        if since.gap {
            out.push(Frame::Resync {
                stores: self.subscribed.iter().cloned().collect(),
                event_seq: self.events.next_seq().saturating_sub(1),
            });
            self.event_cursor = self.events.next_seq().saturating_sub(1);
        } else {
            for event in since.events {
                self.event_cursor = event.seq;
                out.push(Frame::Event {
                    seq: event.seq,
                    kind: event.kind,
                    payload: event.payload,
                });
            }
        }
        out
    }
}
