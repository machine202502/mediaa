use mediaa_core::agent::{Budget, WorkKind};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    pub running: Vec<(WorkKind, u32)>,
    pub waiting: Vec<(u64, WorkKind)>,
}

struct Waiting {
    id: u64,
    kind: WorkKind,
    interactive: bool,
}

struct State {
    budget: Budget,
    running: HashMap<WorkKind, u32>,
    line: Vec<Waiting>,
    granted: HashSet<u64>,
    next: u64,
}

impl State {
    fn schedule(&mut self) {
        let mut i = 0;
        while i < self.line.len() {
            let kind = self.line[i].kind;
            if self.running.get(&kind).copied().unwrap_or(0) >= self.budget.limit(kind) {
                i += 1;
                continue;
            }
            let t = self.line.remove(i);
            *self.running.entry(kind).or_default() += 1;
            self.granted.insert(t.id);
        }
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let mut s = self.state.lock().expect("slots lock");
        let r = f(&mut s);
        s.schedule();
        drop(s);
        self.changed.notify_all();
        r
    }
}

#[derive(Clone)]
pub struct Slots {
    shared: Arc<Shared>,
}

impl Default for Slots {
    fn default() -> Slots {
        Slots::new(Budget::default())
    }
}

impl Slots {
    pub fn new(budget: Budget) -> Slots {
        let state = State { budget, running: HashMap::new(), line: Vec::new(), granted: HashSet::new(), next: 1 };
        Slots { shared: Arc::new(Shared { state: Mutex::new(state), changed: Condvar::new() }) }
    }

    pub fn set_budget(&self, budget: Budget) {
        self.shared.update(|s| s.budget = budget);
    }

    pub fn request(&self, kind: WorkKind, interactive: bool) -> Ticket {
        let id = self.shared.update(|s| {
            let id = s.next;
            s.next += 1;
            let at = if interactive {
                s.line.iter().position(|t| !t.interactive && t.kind == kind).unwrap_or(s.line.len())
            } else {
                s.line.len()
            };
            s.line.insert(at, Waiting { id, kind, interactive });
            id
        });
        Ticket { shared: self.shared.clone(), id, kind, taken: false }
    }

    pub fn status(&self) -> Status {
        let s = self.shared.state.lock().expect("slots lock");
        Status {
            running: WorkKind::ALL.iter().map(|k| (*k, s.running.get(k).copied().unwrap_or(0))).collect(),
            waiting: s.line.iter().map(|t| (t.id, t.kind)).collect(),
        }
    }
}

pub struct Ticket {
    shared: Arc<Shared>,
    id: u64,
    kind: WorkKind,
    taken: bool,
}

impl Ticket {
    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn try_take(&mut self) -> Option<Lease> {
        let granted = self.shared.state.lock().expect("slots lock").granted.remove(&self.id);
        if !granted {
            return None;
        }
        self.taken = true;
        Some(Lease { shared: self.shared.clone(), kind: self.kind })
    }

    pub fn wait(mut self, cancel: &AtomicBool) -> anyhow::Result<Lease> {
        loop {
            if let Some(lease) = self.try_take() {
                return Ok(lease);
            }
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let s = self.shared.state.lock().expect("slots lock");
            let _ = self.shared.changed.wait_timeout(s, Duration::from_millis(100));
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.taken {
            let id = self.id;
            self.shared.update(|s| {
                s.line.retain(|t| t.id != id);
                s.granted.remove(&id);
            });
        }
    }
}

pub struct Lease {
    shared: Arc<Shared>,
    kind: WorkKind,
}

impl Drop for Lease {
    fn drop(&mut self) {
        let kind = self.kind;
        self.shared.update(|s| {
            if let Some(n) = s.running.get_mut(&kind) {
                *n = n.saturating_sub(1);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slots() -> Slots {
        Slots::new(Budget { agent: 8, transcription: 8, indexing: 8, web_search: 8, provider_calls: 8, ..Budget::default() })
    }

    fn take(t: &mut Ticket) -> Lease {
        t.try_take().expect("granted")
    }

    #[test]
    fn slots_limit_each_kind_of_work() {
        let s = slots();
        s.set_budget(Budget { web_search: 2, ..Budget::default() });
        let mut w1 = s.request(WorkKind::WebSearch, false);
        let mut w2 = s.request(WorkKind::WebSearch, false);
        let mut w3 = s.request(WorkKind::WebSearch, false);
        let (l1, l2) = (take(&mut w1), take(&mut w2));
        assert!(w3.try_take().is_none());
        assert_eq!(s.status().running.iter().find(|(k, _)| *k == WorkKind::WebSearch), Some(&(WorkKind::WebSearch, 2)));
        assert_eq!(s.status().waiting, [(w3.id(), WorkKind::WebSearch)]);
        let mut a = s.request(WorkKind::Agent, false);
        assert!(a.try_take().is_some());
        drop(l1);
        assert_eq!(take(&mut w3).kind, WorkKind::WebSearch);
        drop((l2, a));
        assert_eq!(s.status().running.iter().map(|(_, n)| *n).sum::<u32>(), 0);
    }

    #[test]
    fn interactive_work_goes_first_and_tickets_can_be_cancelled() {
        let s = slots();
        s.set_budget(Budget { agent: 1, ..Budget::default() });
        let mut first = s.request(WorkKind::Agent, false);
        let l = take(&mut first);
        let mut bg = s.request(WorkKind::Agent, false);
        let mut chat = s.request(WorkKind::Agent, true);
        assert!(bg.try_take().is_none() && chat.try_take().is_none());
        drop(l);
        let _answer = take(&mut chat);
        assert!(bg.try_take().is_none());
        let cancel = AtomicBool::new(true);
        let t = s.request(WorkKind::Agent, false);
        let id = t.id();
        assert!(t.wait(&cancel).is_err());
        assert!(s.status().waiting.iter().all(|(w, _)| *w != id), "a cancelled ticket leaves the line");
    }

    #[test]
    fn wait_blocks_until_a_slot_is_free() {
        let s = slots();
        s.set_budget(Budget { transcription: 1, ..Budget::default() });
        let mut a = s.request(WorkKind::Transcription, false);
        let la = take(&mut a);
        let s2 = s.clone();
        let h = std::thread::spawn(move || s2.request(WorkKind::Transcription, false).wait(&AtomicBool::new(false)).is_ok());
        std::thread::sleep(Duration::from_millis(50));
        assert!(!h.is_finished(), "the only slot is taken");
        drop(la);
        assert!(h.join().unwrap());
    }
}
