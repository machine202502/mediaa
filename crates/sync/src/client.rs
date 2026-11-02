use crate::frame::{Frame, PROTOCOL_VERSION};
use crate::state::StateView;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq)]
pub enum ClientUpdate {
    Welcome {
        version: u32,
        session: u64,
    },
    State {
        store: String,
        version: u64,
        payload: Vec<u8>,
    },
    Event {
        seq: u64,
        kind: String,
        payload: Vec<u8>,
    },
    Result {
        id: u64,
        ok: bool,
        payload: Vec<u8>,
        error: Option<String>,
    },
    Resync,
    Gap,
    Pong,
    Ping,
    Error {
        code: String,
        message: String,
    },
}

pub struct Client {
    view: StateView,
    stores: Vec<String>,
    event_cursor: u64,
    pending: BTreeSet<u64>,
    handshaken: bool,
    session: u64,
}

impl Default for Client {
    fn default() -> Self {
        Self::new()
    }
}

impl Client {
    pub fn new() -> Self {
        Self {
            view: StateView::new(),
            stores: Vec::new(),
            event_cursor: 0,
            pending: BTreeSet::new(),
            handshaken: false,
            session: 0,
        }
    }

    pub fn hello_frame(&self, client: &str) -> Frame {
        Frame::Hello {
            version: PROTOCOL_VERSION,
            client: client.to_string(),
        }
    }

    pub fn subscribe(&mut self, stores: Vec<String>) -> Frame {
        self.stores = stores;
        self.subscribe_frame()
    }

    pub fn subscribe_frame(&self) -> Frame {
        Frame::Subscribe {
            stores: self.stores.clone(),
            versions: self.view.versions(),
            event_seq: self.event_cursor,
        }
    }

    pub fn resync_frame(&self) -> Frame {
        Frame::Resync {
            stores: self.stores.clone(),
            event_seq: self.event_cursor,
        }
    }

    pub fn ping_frame(&self) -> Frame {
        Frame::Ping
    }

    pub fn command_frame(&mut self, id: u64, name: &str, payload: Vec<u8>) -> Frame {
        self.pending.insert(id);
        Frame::Command {
            id,
            name: name.to_string(),
            payload,
        }
    }

    pub fn pending(&self) -> &BTreeSet<u64> {
        &self.pending
    }

    pub fn view(&self) -> &StateView {
        &self.view
    }

    pub fn event_cursor(&self) -> u64 {
        self.event_cursor
    }

    pub fn handshaken(&self) -> bool {
        self.handshaken
    }

    pub fn session(&self) -> u64 {
        self.session
    }

    pub fn on_frame(&mut self, frame: Frame) -> Vec<ClientUpdate> {
        match frame {
            Frame::Welcome {
                version,
                session,
                event_seq,
                ..
            } => {
                self.handshaken = true;
                self.session = session;
                self.event_cursor = event_seq;
                vec![ClientUpdate::Welcome { version, session }]
            }
            Frame::Snapshot {
                store,
                version,
                payload,
            }
            | Frame::Changed {
                store,
                version,
                payload,
            } => {
                if !self.view.accept(&store, version) {
                    return Vec::new();
                }
                vec![ClientUpdate::State {
                    store,
                    version,
                    payload,
                }]
            }
            Frame::Event { seq, kind, payload } => {
                if seq <= self.event_cursor {
                    return Vec::new();
                }
                if seq != self.event_cursor + 1 {
                    return vec![ClientUpdate::Gap];
                }
                self.event_cursor = seq;
                vec![ClientUpdate::Event { seq, kind, payload }]
            }
            Frame::Result {
                id,
                ok,
                payload,
                error,
            } => {
                self.pending.remove(&id);
                vec![ClientUpdate::Result {
                    id,
                    ok,
                    payload,
                    error,
                }]
            }
            Frame::Resync { stores, event_seq } => {
                self.view.reset();
                self.stores = stores;
                self.event_cursor = event_seq;
                vec![ClientUpdate::Resync]
            }
            Frame::Pong => vec![ClientUpdate::Pong],
            Frame::Ping => vec![ClientUpdate::Ping],
            Frame::Error { code, message } => vec![ClientUpdate::Error { code, message }],
            _ => Vec::new(),
        }
    }
}
