//! Model scheduler: shares the budget (GPU memory, RAM, slots per kind of work) between jobs.
//!
//! A job asks for a [`Ticket`] — a kind of work and, optionally, a model with its size and
//! preferred device — and waits for a [`Lease`]. Rules:
//!
//! * **Placement**: a model already loaded is shared; otherwise it goes to the preferred pool if
//!   it fits (GPU for `Auto` / `Gpu`, then the CPU as a fallback; `Cpu` never uses the GPU).
//!   A model larger than every allowed pool fails at once.
//! * **Unloading**: a model with no lease left is unloaded at once, unless a waiting job wants
//!   it (it is then handed over without reloading).
//! * **Order**: one line, first come first served; interactive work (chat answers) goes before
//!   background jobs. Jobs that need no new memory may pass a job waiting for memory; jobs that
//!   need memory may not (no starvation of big models).
//! * **Round robin**: a lease held longer than the quantum while others wait for memory in its
//!   pool reports [`Lease::should_yield`]; the job checkpoints, drops the lease and asks again —
//!   the waiting job gets the memory, the yielding one goes to the back of the line.
//!
//! Loading and unloading the model itself is up to the runtime: a lease says whether the model
//! has to be loaded ([`Lease::fresh`]), and the `on_unload` callback says when to free it.

use mediaa_core::agent::{Budget, Device, WorkKind};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pool {
    Gpu,
    Cpu,
}

/// A model a job needs.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelNeed {
    /// Identifies the model (same key: same loaded instance).
    pub key: String,
    /// Memory it takes once loaded (weights + working memory).
    pub bytes: u64,
    pub device: Device,
}

impl ModelNeed {
    fn pools(&self) -> &'static [Pool] {
        match self.device {
            Device::Auto | Device::Gpu => &[Pool::Gpu, Pool::Cpu],
            Device::Cpu => &[Pool::Cpu],
        }
    }
}

/// Why a ticket is not running yet.
#[derive(Debug, Clone, PartialEq)]
pub enum Wait {
    /// All slots of its kind of work are taken.
    Slot(WorkKind),
    /// Its model does not fit the free memory.
    Memory { key: String, bytes: u64 },
    /// An earlier job is waiting for memory first.
    Turn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LoadedModel {
    pub key: String,
    pub pool: Pool,
    pub bytes: u64,
    pub leases: u32,
}

/// What the scheduler is doing (for the queue page).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    pub loaded: Vec<LoadedModel>,
    /// Jobs running per kind of work.
    pub running: Vec<(WorkKind, u32)>,
    pub waiting: Vec<(u64, WorkKind, Wait)>,
    pub gpu_used: u64,
    pub ram_used: u64,
}

struct Waiting {
    id: u64,
    kind: WorkKind,
    model: Option<ModelNeed>,
    interactive: bool,
    reason: Option<Wait>,
}

struct Grant {
    pool: Option<Pool>,
    key: Option<String>,
    fresh: bool,
}

struct State {
    budget: Budget,
    running: HashMap<WorkKind, u32>,
    loaded: Vec<LoadedModel>,
    line: Vec<Waiting>,
    granted: HashMap<u64, Result<Grant, String>>,
    next: u64,
}

impl State {
    fn used(&self, pool: Pool) -> u64 {
        self.loaded.iter().filter(|m| m.pool == pool).map(|m| m.bytes).sum()
    }

    fn cap(&self, pool: Pool) -> u64 {
        match pool {
            Pool::Gpu => self.budget.gpu_bytes(),
            Pool::Cpu => self.budget.ram_bytes(),
        }
    }

    /// Hands out every lease that can be given now; returns models to unload.
    fn schedule(&mut self) -> Vec<(String, Pool)> {
        let mut unload = Vec::new();
        // Pools an earlier ticket is waiting on memory for: later ones may not take memory there.
        let mut blocked: Vec<Pool> = Vec::new();
        let mut i = 0;
        while i < self.line.len() {
            let kind = self.line[i].kind;
            if self.running.get(&kind).copied().unwrap_or(0) >= self.budget.limit(kind) {
                self.line[i].reason = Some(Wait::Slot(kind));
                i += 1;
                continue;
            }
            let grant = match self.line[i].model.clone() {
                None => Some(Ok(Grant { pool: None, key: None, fresh: false })),
                Some(need) => match self.loaded.iter().position(|m| m.key == need.key) {
                    Some(at) => {
                        self.loaded[at].leases += 1;
                        Some(Ok(Grant { pool: Some(self.loaded[at].pool), key: Some(need.key.clone()), fresh: false }))
                    }
                    None if need.pools().iter().all(|&p| need.bytes > self.cap(p)) => {
                        Some(Err(format!("model needs {} MB, more than the budget allows", need.bytes >> 20)))
                    }
                    None if need.pools().iter().any(|p| blocked.contains(p)) => {
                        self.line[i].reason = Some(Wait::Turn);
                        None
                    }
                    None => match self.place(&need, &mut unload) {
                        Some(pool) => {
                            self.loaded.push(LoadedModel { key: need.key.clone(), pool, bytes: need.bytes, leases: 1 });
                            Some(Ok(Grant { pool: Some(pool), key: Some(need.key.clone()), fresh: true }))
                        }
                        None => {
                            self.line[i].reason = Some(Wait::Memory { key: need.key.clone(), bytes: need.bytes });
                            blocked.extend(need.pools());
                            None
                        }
                    },
                },
            };
            match grant {
                Some(g) => {
                    let t = self.line.remove(i);
                    if g.is_ok() {
                        *self.running.entry(kind).or_default() += 1;
                    }
                    self.granted.insert(t.id, g);
                }
                None => i += 1,
            }
        }
        unload
    }

    /// The first allowed pool with room (idle models are unloaded to make room).
    fn place(&mut self, need: &ModelNeed, unload: &mut Vec<(String, Pool)>) -> Option<Pool> {
        for &pool in need.pools() {
            let idle: u64 = self.loaded.iter().filter(|m| m.pool == pool && m.leases == 0).map(|m| m.bytes).sum();
            let free = self.cap(pool).saturating_sub(self.used(pool));
            if need.bytes <= free + idle {
                if need.bytes > free {
                    self.loaded.retain(|m| {
                        let evict = m.pool == pool && m.leases == 0;
                        if evict {
                            unload.push((m.key.clone(), m.pool));
                        }
                        !evict
                    });
                }
                return Some(pool);
            }
        }
        None
    }
}

/// Frees a model (key, pool) in the runtime.
type Unload = Box<dyn Fn(&str, Pool) + Send + Sync>;

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    quantum: Duration,
    on_unload: Unload,
}

impl Shared {
    /// Runs `f` on the state, schedules, unloads outside the lock and wakes waiters.
    fn update<R>(&self, f: impl FnOnce(&mut State) -> R) -> R {
        let (r, unload) = {
            let mut s = self.state.lock().expect("scheduler lock");
            let r = f(&mut s);
            let unload = s.schedule();
            (r, unload)
        };
        for (key, pool) in unload {
            (self.on_unload)(&key, pool);
        }
        self.changed.notify_all();
        r
    }
}

#[derive(Clone)]
pub struct Scheduler {
    shared: Arc<Shared>,
}

impl Scheduler {
    /// `quantum`: how long a lease may keep memory others wait for; `on_unload(key, pool)`:
    /// free that model now.
    pub fn new(budget: Budget, quantum: Duration, on_unload: impl Fn(&str, Pool) + Send + Sync + 'static) -> Scheduler {
        let state = State { budget, running: HashMap::new(), loaded: Vec::new(), line: Vec::new(), granted: HashMap::new(), next: 1 };
        Scheduler { shared: Arc::new(Shared { state: Mutex::new(state), changed: Condvar::new(), quantum, on_unload: Box::new(on_unload) }) }
    }

    /// New limits (take effect for the next leases).
    pub fn set_budget(&self, budget: Budget) {
        self.shared.update(|s| s.budget = budget);
    }

    /// Joins the line; `interactive` work goes before background jobs.
    pub fn request(&self, kind: WorkKind, model: Option<ModelNeed>, interactive: bool) -> Ticket {
        let id = self.shared.update(|s| {
            let id = s.next;
            s.next += 1;
            let at = if interactive { s.line.iter().position(|t| !t.interactive).unwrap_or(s.line.len()) } else { s.line.len() };
            s.line.insert(at, Waiting { id, kind, model, interactive, reason: None });
            id
        });
        Ticket { shared: self.shared.clone(), id, kind, taken: false }
    }

    pub fn status(&self) -> Status {
        let s = self.shared.state.lock().expect("scheduler lock");
        Status {
            loaded: s.loaded.clone(),
            running: WorkKind::ALL.iter().map(|k| (*k, s.running.get(k).copied().unwrap_or(0))).collect(),
            waiting: s.line.iter().filter_map(|t| t.reason.clone().map(|r| (t.id, t.kind, r))).collect(),
            gpu_used: s.used(Pool::Gpu),
            ram_used: s.used(Pool::Cpu),
        }
    }
}

/// A place in the line.
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

    /// The lease if granted by now: `Some(Err)` when the model can never fit.
    pub fn try_take(&mut self) -> Option<anyhow::Result<Lease>> {
        let grant = self.shared.state.lock().expect("scheduler lock").granted.remove(&self.id)?;
        self.taken = true;
        Some(match grant {
            Ok(g) => Ok(Lease { shared: self.shared.clone(), kind: self.kind, pool: g.pool, key: g.key, fresh: g.fresh, since: Instant::now() }),
            Err(e) => Err(anyhow::anyhow!(e)),
        })
    }

    /// Waits for the lease (or until `cancel`).
    pub fn wait(mut self, cancel: &AtomicBool) -> anyhow::Result<Lease> {
        loop {
            if let Some(r) = self.try_take() {
                return r;
            }
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let s = self.shared.state.lock().expect("scheduler lock");
            let _ = self.shared.changed.wait_timeout(s, Duration::from_millis(100));
        }
    }

    pub fn waiting_for(&self) -> Option<Wait> {
        self.shared.state.lock().expect("scheduler lock").line.iter().find(|t| t.id == self.id).and_then(|t| t.reason.clone())
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.taken {
            let id = self.id;
            let granted = self.shared.update(|s| {
                s.line.retain(|t| t.id != id);
                s.granted.remove(&id)
            });
            // Granted but never taken: give it back.
            if let Some(Ok(g)) = granted {
                drop(Lease { shared: self.shared.clone(), kind: self.kind, pool: g.pool, key: g.key, fresh: false, since: Instant::now() });
            }
        }
    }
}

/// Permission to run (and to use a model, in `pool`). Dropping it frees the slot and, when no
/// one else uses the model, the model.
pub struct Lease {
    shared: Arc<Shared>,
    kind: WorkKind,
    pool: Option<Pool>,
    key: Option<String>,
    fresh: bool,
    since: Instant,
}

impl Lease {
    /// Where the model runs (`None`: no model).
    pub fn pool(&self) -> Option<Pool> {
        self.pool
    }

    /// The model has to be loaded (it was not in memory).
    pub fn fresh(&self) -> bool {
        self.fresh
    }

    /// Others wait for memory this lease holds and its quantum is over: checkpoint and drop it.
    pub fn should_yield(&self) -> bool {
        let Some(pool) = self.pool else { return false };
        if self.since.elapsed() < self.shared.quantum {
            return false;
        }
        let s = self.shared.state.lock().expect("scheduler lock");
        s.line.iter().any(|t| matches!(t.reason, Some(Wait::Memory { .. }) | Some(Wait::Turn)) && t.model.as_ref().is_some_and(|m| m.pools().contains(&pool)))
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let (kind, key) = (self.kind, self.key.clone());
        let unloaded = self.shared.update(|s| {
            if let Some(n) = s.running.get_mut(&kind) {
                *n = n.saturating_sub(1);
            }
            let mut unloaded = None;
            if let Some(key) = &key {
                if let Some(i) = s.loaded.iter().position(|m| &m.key == key) {
                    s.loaded[i].leases = s.loaded[i].leases.saturating_sub(1);
                    let wanted = s.line.iter().any(|t| t.model.as_ref().is_some_and(|m| &m.key == key));
                    if s.loaded[i].leases == 0 && !wanted {
                        let m = s.loaded.remove(i);
                        unloaded = Some((m.key, m.pool));
                    }
                }
            }
            unloaded
        });
        if let Some((key, pool)) = unloaded {
            (self.shared.on_unload)(&key, pool);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn sched(gpu: f32, ram: f32, quantum_ms: u64) -> (Scheduler, Arc<Mutex<Vec<String>>>) {
        let unloaded = Arc::new(Mutex::new(Vec::new()));
        let u = unloaded.clone();
        // Generous slots: these tests are about memory unless they set limits.
        let budget = Budget { gpu_memory_gb: gpu, ram_memory_gb: ram, agent: 8, transcription: 8, indexing: 8, web_search: 8, provider_calls: 8 };
        (Scheduler::new(budget, Duration::from_millis(quantum_ms), move |key, _| u.lock().unwrap().push(key.to_string())), unloaded)
    }

    fn model(key: &str, gb: u64, device: Device) -> Option<ModelNeed> {
        Some(ModelNeed { key: key.into(), bytes: gb * GB, device })
    }

    fn take(t: &mut Ticket) -> Lease {
        t.try_take().expect("granted").expect("fits")
    }

    #[test]
    fn placement_gpu_first_then_cpu_and_too_big() {
        let (s, _) = sched(8.0, 16.0, 1000);
        let mut a = s.request(WorkKind::Agent, model("llm", 5, Device::Auto), false);
        let la = take(&mut a);
        assert_eq!((la.pool(), la.fresh()), (Some(Pool::Gpu), true));
        // 5 GB taken of 8: a 4 GB model falls back to the CPU…
        let mut b = s.request(WorkKind::Transcription, model("asr", 4, Device::Gpu), false);
        assert_eq!(take(&mut b).pool(), Some(Pool::Cpu));
        // …a CPU-only model never lands on the GPU, the same model is shared, not reloaded.
        let mut c = s.request(WorkKind::Indexing, model("emb", 1, Device::Cpu), false);
        assert_eq!(take(&mut c).pool(), Some(Pool::Cpu));
        let mut d = s.request(WorkKind::Agent, model("llm", 5, Device::Auto), false);
        let ld = take(&mut d);
        assert_eq!((ld.pool(), ld.fresh()), (Some(Pool::Gpu), false));
        // Larger than any pool it may use: fails at once instead of waiting forever.
        let mut e = s.request(WorkKind::Agent, model("huge", 20, Device::Auto), false);
        assert!(e.try_take().unwrap().is_err());
        let mut f = s.request(WorkKind::Agent, model("cpu-big", 10, Device::Cpu), false);
        let _ = f.try_take(); // 10 GB fits 16 GB RAM in principle: waits (RAM partly used) or runs
        assert_eq!(s.status().gpu_used, 5 * GB);
    }

    #[test]
    fn waits_for_memory_then_runs_and_unloads_at_once() {
        let (s, unloaded) = sched(8.0, 0.0, 1000);
        let mut a = s.request(WorkKind::Agent, model("big", 6, Device::Gpu), false);
        let la = take(&mut a);
        let mut b = s.request(WorkKind::Transcription, model("asr", 4, Device::Gpu), false);
        assert!(b.try_take().is_none());
        assert_eq!(b.waiting_for(), Some(Wait::Memory { key: "asr".into(), bytes: 4 * GB }));
        drop(la);
        assert_eq!(*unloaded.lock().unwrap(), ["big"], "no one needs it: unloaded at once");
        let lb = take(&mut b);
        assert!(lb.fresh());
        drop(lb);
        assert_eq!(*unloaded.lock().unwrap(), ["big", "asr"]);
        assert!(s.status().loaded.is_empty());
    }

    #[test]
    fn a_waiting_job_gets_the_model_without_a_reload() {
        let (s, unloaded) = sched(8.0, 0.0, 1000);
        let mut a = s.request(WorkKind::Agent, model("llm", 6, Device::Gpu), false);
        let la = take(&mut a);
        // Same model, but its slot (agent: 1) is busy: it waits for the slot.
        s.set_budget(Budget { gpu_memory_gb: 8.0, ram_memory_gb: 0.0, agent: 1, transcription: 8, indexing: 8, web_search: 8, provider_calls: 8 });
        let mut b = s.request(WorkKind::Agent, model("llm", 6, Device::Gpu), false);
        assert_eq!(b.waiting_for(), Some(Wait::Slot(WorkKind::Agent)));
        drop(la);
        assert!(unloaded.lock().unwrap().is_empty(), "kept: a waiting job wants it");
        assert!(!take(&mut b).fresh());
    }

    #[test]
    fn round_robin_makes_a_long_holder_yield() {
        let (s, _) = sched(8.0, 0.0, 30);
        let mut a = s.request(WorkKind::Transcription, model("asr", 6, Device::Gpu), false);
        let la = take(&mut a);
        assert!(!la.should_yield(), "no one is waiting");
        let mut b = s.request(WorkKind::Agent, model("llm", 6, Device::Gpu), false);
        assert!(b.try_take().is_none());
        assert!(!la.should_yield(), "within its quantum");
        std::thread::sleep(Duration::from_millis(40));
        assert!(la.should_yield(), "quantum over while another job waits for this memory");
        // The job checkpoints, drops its lease and asks again: the other model runs first.
        drop(la);
        let mut again = s.request(WorkKind::Transcription, model("asr", 6, Device::Gpu), false);
        let lb = take(&mut b);
        assert!(again.try_take().is_none(), "back of the line");
        drop(lb);
        assert!(take(&mut again).fresh());
    }

    #[test]
    fn slots_limit_each_kind_and_light_work_passes_a_memory_wait() {
        let (s, _) = sched(4.0, 0.0, 1000);
        let budget = Budget { gpu_memory_gb: 4.0, ram_memory_gb: 0.0, web_search: 2, agent: 8, transcription: 8, indexing: 8, provider_calls: 8 };
        s.set_budget(budget);
        let mut w1 = s.request(WorkKind::WebSearch, None, false);
        let mut w2 = s.request(WorkKind::WebSearch, None, false);
        let w3 = s.request(WorkKind::WebSearch, None, false);
        let (_l1, _l2) = (take(&mut w1), take(&mut w2));
        assert_eq!(w3.waiting_for(), Some(Wait::Slot(WorkKind::WebSearch)));
        // A model job waits for memory; a later job without a model still runs,
        // a later job that needs memory does not jump the line.
        let mut big = s.request(WorkKind::Agent, model("a", 3, Device::Gpu), false);
        let lbig = take(&mut big);
        let mut waiting = s.request(WorkKind::Agent, model("b", 3, Device::Gpu), false);
        let small = s.request(WorkKind::Indexing, model("c", 1, Device::Gpu), false);
        let mut light = s.request(WorkKind::Transcription, None, false);
        assert!(waiting.try_take().is_none());
        assert_eq!(small.waiting_for(), Some(Wait::Turn), "1 GB is free, but b asked first");
        assert!(light.try_take().is_some());
        drop(lbig);
        assert!(waiting.try_take().is_some());
    }

    #[test]
    fn interactive_work_goes_first_and_tickets_can_be_cancelled() {
        let (s, _) = sched(4.0, 0.0, 1000);
        let mut a = s.request(WorkKind::Agent, model("a", 4, Device::Gpu), false);
        let la = take(&mut a);
        let mut bg = s.request(WorkKind::Indexing, model("emb", 4, Device::Gpu), false);
        let mut chat = s.request(WorkKind::Agent, model("llm", 4, Device::Gpu), true);
        drop(la);
        let _answer = take(&mut chat); // the chat answer runs first
        assert!(bg.try_take().is_none());
        let cancel = AtomicBool::new(true);
        let t = s.request(WorkKind::Agent, model("x", 4, Device::Gpu), false);
        let id = t.id();
        assert!(t.wait(&cancel).is_err());
        assert!(s.status().waiting.iter().all(|(w, _, _)| *w != id), "a cancelled ticket leaves the line");
    }

    #[test]
    fn wait_blocks_until_memory_is_free() {
        let (s, _) = sched(4.0, 0.0, 1000);
        let mut a = s.request(WorkKind::Agent, model("a", 4, Device::Gpu), false);
        let la = take(&mut a);
        let s2 = s.clone();
        let h = std::thread::spawn(move || s2.request(WorkKind::Transcription, model("b", 4, Device::Gpu), false).wait(&AtomicBool::new(false)).map(|l| l.fresh()));
        std::thread::sleep(Duration::from_millis(50));
        drop(la);
        assert!(h.join().unwrap().unwrap());
    }
}
