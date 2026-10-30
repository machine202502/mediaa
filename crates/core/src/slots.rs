use crate::agent::{AgentSettings, GROUP_LIMIT_MAX};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

const POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Key {
    Group(String),
    Web,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GroupLoad {
    pub name: String,
    pub busy: u32,
    pub limit: u32,
    pub waiting: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolLoad {
    pub busy: u32,
    pub limit: u32,
    pub waiting: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub groups: Vec<GroupLoad>,
    pub web: ToolLoad,
}

struct Waiting {
    id: u64,
    key: Key,
}

struct State {
    groups: Vec<(String, u32)>,
    web_limit: u32,
    places: HashMap<String, String>,
    busy: HashMap<Key, u32>,
    line: Vec<Waiting>,
    granted: HashSet<u64>,
    passed: HashSet<u64>,
    next: u64,
}

impl State {
    fn limit(&self, key: &Key) -> Option<u32> {
        match key {
            Key::Group(name) => self
                .groups
                .iter()
                .find(|(known, _)| known == name)
                .map(|(_, limit)| *limit),
            Key::Web => Some(self.web_limit),
        }
    }

    fn waiting(&self, key: &Key) -> usize {
        self.line.iter().filter(|item| &item.key == key).count()
    }

    fn schedule(&mut self) {
        let mut i = 0;
        while i < self.line.len() {
            let key = self.line[i].key.clone();
            let Some(limit) = self.limit(&key) else {
                let gone = self.line.remove(i);
                self.passed.insert(gone.id);
                continue;
            };
            let busy = self.busy.get(&key).copied().unwrap_or(0);
            if busy >= limit {
                i += 1;
                continue;
            }
            let granted = self.line.remove(i);
            *self.busy.entry(granted.key).or_default() += 1;
            self.granted.insert(granted.id);
        }
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut state = self.state.lock().expect("slots lock");
        let result = f(&mut state);
        state.schedule();
        drop(state);
        self.changed.notify_all();
        result
    }
}

#[derive(Clone)]
pub struct Slots {
    shared: Arc<Shared>,
}

impl Default for Slots {
    fn default() -> Slots {
        Slots::new()
    }
}

impl Slots {
    pub fn new() -> Slots {
        let slots = Slots {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    groups: Vec::new(),
                    web_limit: 0,
                    places: HashMap::new(),
                    busy: HashMap::new(),
                    line: Vec::new(),
                    granted: HashSet::new(),
                    passed: HashSet::new(),
                    next: 1,
                }),
                changed: Condvar::new(),
            }),
        };
        slots.configure(&AgentSettings::default());
        slots
    }

    pub fn configure(&self, settings: &AgentSettings) {
        let groups = settings
            .groups
            .iter()
            .map(|group| (group.name.clone(), group.limit.clamp(1, GROUP_LIMIT_MAX)))
            .collect();
        let places = settings
            .connections
            .iter()
            .filter_map(|connection| {
                let group = connection.group.trim();
                (!group.is_empty()).then(|| (connection.id.clone(), group.to_string()))
            })
            .collect();
        let web_limit = settings.web_limit.clamp(1, GROUP_LIMIT_MAX);
        self.shared.update(|state| {
            state.groups = groups;
            state.places = places;
            state.web_limit = web_limit;
        });
    }

    pub fn group(&self, name: &str) -> Ticket {
        self.request(Key::Group(name.to_string()))
    }

    pub fn web(&self) -> Ticket {
        self.request(Key::Web)
    }

    pub fn provider(&self, connection: &str) -> Option<Ticket> {
        let group = self
            .shared
            .state
            .lock()
            .expect("slots lock")
            .places
            .get(connection)
            .cloned();
        group.map(|group| self.request(Key::Group(group)))
    }

    fn request(&self, key: Key) -> Ticket {
        let id = self.shared.update(|state| {
            let id = state.next;
            state.next += 1;
            state.line.push(Waiting {
                id,
                key: key.clone(),
            });
            id
        });
        Ticket {
            shared: self.shared.clone(),
            id,
            key,
            taken: false,
        }
    }

    pub fn status(&self) -> Status {
        let state = self.shared.state.lock().expect("slots lock");
        Status {
            groups: state
                .groups
                .iter()
                .map(|(name, limit)| {
                    let key = Key::Group(name.clone());
                    GroupLoad {
                        name: name.clone(),
                        busy: state.busy.get(&key).copied().unwrap_or(0),
                        limit: *limit,
                        waiting: state.waiting(&key),
                    }
                })
                .collect(),
            web: ToolLoad {
                busy: state.busy.get(&Key::Web).copied().unwrap_or(0),
                limit: state.web_limit,
                waiting: state.waiting(&Key::Web),
            },
        }
    }
}

pub struct Ticket {
    shared: Arc<Shared>,
    id: u64,
    key: Key,
    taken: bool,
}

impl Ticket {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn try_take(&mut self) -> Option<Lease> {
        let mut state = self.shared.state.lock().expect("slots lock");
        if state.passed.remove(&self.id) {
            self.taken = true;
            return Some(Lease {
                shared: self.shared.clone(),
                key: None,
            });
        }
        if !state.granted.remove(&self.id) {
            return None;
        }
        self.taken = true;
        Some(Lease {
            shared: self.shared.clone(),
            key: Some(self.key.clone()),
        })
    }

    pub fn wait(mut self, cancel: &AtomicBool) -> anyhow::Result<Lease> {
        loop {
            if let Some(lease) = self.try_take() {
                return Ok(lease);
            }
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let state = self.shared.state.lock().expect("slots lock");
            let _ = self.shared.changed.wait_timeout(state, POLL);
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if self.taken {
            return;
        }
        let id = self.id;
        let key = self.key.clone();
        self.shared.update(|state| {
            state.line.retain(|item| item.id != id);
            state.passed.remove(&id);
            if state.granted.remove(&id) {
                if let Some(busy) = state.busy.get_mut(&key) {
                    *busy = busy.saturating_sub(1);
                }
            }
        });
    }
}

pub struct Lease {
    shared: Arc<Shared>,
    key: Option<Key>,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let Some(key) = self.key.clone() else {
            return;
        };
        self.shared.update(|state| {
            if let Some(busy) = state.busy.get_mut(&key) {
                *busy = busy.saturating_sub(1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{Connection, ConnectionKind, Group};

    fn configured(groups: &[(&str, u32)], web: u32, places: &[&str]) -> (Slots, Vec<String>) {
        let mut settings = AgentSettings {
            groups: groups
                .iter()
                .map(|(name, limit)| Group::new(*name, *limit))
                .collect(),
            web_limit: web,
            ..AgentSettings::default()
        };
        for group in places {
            let mut connection = Connection::new(ConnectionKind::OpenAiCompatible, "Local");
            connection.group = (*group).to_string();
            settings.connections.push(connection);
        }
        let ids = settings
            .connections
            .iter()
            .map(|connection| connection.id.clone())
            .collect();
        let slots = Slots::new();
        slots.configure(&settings);
        (slots, ids)
    }

    fn take(ticket: &mut Ticket) -> Lease {
        ticket.try_take().expect("the ticket has a slot")
    }

    #[test]
    fn a_provider_without_a_group_never_takes_a_slot() {
        let (slots, ids) = configured(&[("Single", 1)], 2, &[""]);
        assert!(
            slots.provider(&ids[0]).is_none(),
            "an unattached provider has no place"
        );
    }

    #[test]
    fn providers_of_one_group_share_its_limit() {
        let (slots, ids) = configured(&[("Single", 1)], 2, &["Single", "Single"]);
        let mut first = slots.provider(&ids[0]).unwrap();
        let mut second = slots.provider(&ids[1]).unwrap();
        let lease = take(&mut first);
        assert!(second.try_take().is_none(), "the second provider waits");
        drop(lease);
        let _second = take(&mut second);
    }

    #[test]
    fn another_group_does_not_wait_for_a_full_one() {
        let (slots, ids) = configured(&[("Single", 1), ("Local", 1)], 2, &["Single", "Local"]);
        let mut first = slots.provider(&ids[0]).unwrap();
        let _lease = take(&mut first);
        let mut other = slots.provider(&ids[1]).unwrap();
        assert!(other.try_take().is_some(), "the other group is free");
    }

    #[test]
    fn the_line_gives_the_slot_in_the_order_of_arrival() {
        let slots = Slots::new();
        slots.configure(&AgentSettings {
            groups: vec![Group::new("Single", 1)],
            ..AgentSettings::default()
        });
        let mut first = slots.group("Single");
        let busy = take(&mut first);
        let mut a = slots.group("Single");
        let mut b = slots.group("Single");
        let mut c = slots.group("Single");
        assert!(a.try_take().is_none() && b.try_take().is_none() && c.try_take().is_none());
        let mut d = slots.group("Single");
        drop(busy);
        let first = take(&mut a);
        assert!(
            b.try_take().is_none() && d.try_take().is_none(),
            "the newer tickets did not jump"
        );
        drop(first);
        let second = take(&mut b);
        assert!(c.try_take().is_none());
        drop(second);
        let third = take(&mut c);
        assert!(d.try_take().is_none(), "the last ticket waits its turn");
        drop(third);
        assert!(d.try_take().is_some());
    }

    #[test]
    fn removing_a_group_lets_its_waiters_go_without_a_slot() {
        let (slots, ids) = configured(&[("Single", 1)], 2, &["Single"]);
        let mut busy = slots.provider(&ids[0]).unwrap();
        let held = take(&mut busy);
        let mut waiting = slots.group("Single");
        assert!(waiting.try_take().is_none());
        slots.configure(&AgentSettings {
            groups: vec![Group::new("Other", 1)],
            ..AgentSettings::default()
        });
        assert!(
            waiting.try_take().is_some(),
            "the waiter goes without a slot"
        );
        assert!(
            slots
                .status()
                .groups
                .iter()
                .all(|group| group.name != "Single"),
            "the removed group is gone from the status"
        );
        drop(held);
    }

    #[test]
    fn a_lower_limit_admits_only_the_remaining_slots() {
        let slots = Slots::new();
        slots.configure(&AgentSettings {
            groups: vec![Group::new("Single", 2)],
            ..AgentSettings::default()
        });
        let mut first = slots.group("Single");
        let mut second = slots.group("Single");
        let one = take(&mut first);
        let two = take(&mut second);
        slots.configure(&AgentSettings {
            groups: vec![Group::new("Single", 1)],
            ..AgentSettings::default()
        });
        let mut third = slots.group("Single");
        assert!(third.try_take().is_none(), "the old slots are kept");
        drop(one);
        assert!(third.try_take().is_none(), "the lower limit still holds");
        drop(two);
        assert!(third.try_take().is_some());
    }

    #[test]
    fn the_web_tool_has_its_own_limit() {
        let (slots, _) = configured(&[("Single", 1)], 1, &[]);
        let mut first = slots.web();
        let mut second = slots.web();
        let lease = take(&mut first);
        assert!(second.try_take().is_none(), "the second search waits");
        let mut group = slots.group("Single");
        assert!(group.try_take().is_some(), "the group does not wait for it");
        drop(lease);
        assert!(second.try_take().is_some());
    }

    #[test]
    fn the_status_lists_every_group_even_when_idle() {
        let (slots, _) = configured(&[("Single", 1), ("Local", 4)], 2, &[]);
        let mut busy = slots.group("Single");
        let lease = take(&mut busy);
        let mut waiting = slots.group("Single");
        let status = slots.status();
        assert_eq!(status.web.limit, 2);
        assert_eq!(
            status.groups,
            [
                GroupLoad {
                    name: "Single".into(),
                    busy: 1,
                    limit: 1,
                    waiting: 1
                },
                GroupLoad {
                    name: "Local".into(),
                    busy: 0,
                    limit: 4,
                    waiting: 0
                },
            ]
        );
        assert_eq!(status.web.waiting, 0);
        assert!(waiting.try_take().is_none());
        drop(lease);
    }

    #[test]
    fn a_cancelled_ticket_leaves_the_line() {
        let (slots, _) = configured(&[("Single", 1)], 2, &[]);
        let mut busy = slots.group("Single");
        let lease = take(&mut busy);
        let ticket = slots.group("Single");
        assert_eq!(slots.status().groups[0].waiting, 1);
        let cancel = AtomicBool::new(true);
        assert!(ticket.wait(&cancel).is_err());
        let status = slots.status();
        assert_eq!(status.groups[0].waiting, 0, "the ticket left the line");
        assert_eq!(status.groups[0].busy, 1, "the slot is still held");
        drop(lease);
        assert_eq!(slots.status().groups[0].busy, 0);
    }

    #[test]
    fn wait_blocks_until_a_slot_is_free() {
        let (slots, _) = configured(&[("Single", 1)], 2, &[]);
        let mut first = slots.group("Single");
        let lease = take(&mut first);
        let second = slots.clone();
        let handle = std::thread::spawn(move || {
            second.group("Single").wait(&AtomicBool::new(false)).is_ok()
        });
        std::thread::sleep(Duration::from_millis(50));
        assert!(!handle.is_finished(), "the only slot is taken");
        drop(lease);
        assert!(handle.join().unwrap());
    }
}
