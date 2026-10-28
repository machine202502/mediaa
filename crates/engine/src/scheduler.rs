use mediaa_core::agent::{Budget, Device, WorkKind};
use mediaa_core::pulse;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Pool {
    Gpu,
    Cpu,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelNeed {
    pub key: String,
    pub name: String,
    pub bytes: u64,
    pub device: Device,
    pub n_ctx: u32,
}

impl ModelNeed {
    fn pools(&self) -> &'static [Pool] {
        match self.device {
            Device::Auto | Device::Gpu => &[Pool::Gpu, Pool::Cpu],
            Device::Cpu => &[Pool::Cpu],
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Wait {
    Slot(WorkKind),
    Memory {
        key: String,
        name: String,
        bytes: u64,
    },
    Turn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    pub key: String,
    pub name: String,
    pub pool: Pool,
    pub bytes: u64,
    pub leases: u32,
    pub n_ctx: u32,
    pub idle: Duration,
    pub unloads_in: Option<Duration>,
}

struct Resident {
    key: String,
    name: String,
    pool: Pool,
    bytes: u64,
    leases: u32,
    holders: HashMap<u64, WorkKind>,
    n_ctx: u32,
    idle_since: Option<Instant>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Status {
    pub loaded: Vec<LoadedModel>,
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
    n_ctx: u32,
    lease: u64,
}

struct State {
    budget: Budget,
    keep_alive: Duration,
    wanted: HashSet<String>,
    running: HashMap<WorkKind, u32>,
    loaded: Vec<Resident>,
    line: Vec<Waiting>,
    granted: HashMap<u64, Result<Grant, String>>,
    next: u64,
    next_lease: u64,
    abandoned: HashSet<u64>,
}

struct Clock {
    manual: Mutex<Option<Instant>>,
    offset_ms: AtomicU64,
}

impl Clock {
    fn real() -> Clock {
        Clock {
            manual: Mutex::new(None),
            offset_ms: AtomicU64::new(0),
        }
    }

    #[cfg(test)]
    fn manual() -> Clock {
        Clock {
            manual: Mutex::new(Some(Instant::now())),
            offset_ms: AtomicU64::new(0),
        }
    }

    fn now(&self) -> Instant {
        if let Some(now) = *self.manual.lock().expect("clock lock") {
            return now;
        }
        Instant::now() + Duration::from_millis(self.offset_ms.load(Ordering::Relaxed))
    }

    fn advance(&self, by: Duration) {
        let mut manual = self.manual.lock().expect("clock lock");
        if let Some(now) = manual.as_mut() {
            *now += by;
            return;
        }
        let add = by.as_millis().min(u128::from(u64::MAX)) as u64;
        self.offset_ms.fetch_add(add, Ordering::Relaxed);
    }
}

impl State {
    fn used(&self, pool: Pool) -> u64 {
        self.loaded
            .iter()
            .filter(|m| m.pool == pool)
            .map(|m| m.bytes)
            .sum()
    }

    fn cap(&self, pool: Pool) -> u64 {
        match pool {
            Pool::Gpu => self.budget.gpu_bytes(),
            Pool::Cpu => self.budget.ram_bytes(),
        }
    }

    fn line_wants(&self, key: &str) -> bool {
        self.line
            .iter()
            .any(|t| t.model.as_ref().is_some_and(|m| m.key == key))
    }

    fn fits_a_pool(&self, need: &ModelNeed, bytes: u64) -> bool {
        need.pools().iter().any(|&pool| bytes <= self.cap(pool))
    }

    fn candidates(&self, need: &ModelNeed) -> Vec<(u32, u64)> {
        let mut best: HashMap<u32, u64> = HashMap::new();
        let mut consider = |n_ctx: u32, bytes: u64| {
            let slot = best.entry(n_ctx).or_insert(0);
            if bytes > *slot {
                *slot = bytes;
            }
        };
        consider(need.n_ctx, need.bytes);
        for ticket in &self.line {
            if let Some(model) = &ticket.model {
                if model.key == need.key {
                    consider(model.n_ctx, model.bytes);
                }
            }
        }
        let mut options: Vec<(u32, u64)> = best.into_iter().collect();
        options.sort_by_key(|option| std::cmp::Reverse(option.0));
        options
    }

    fn time_until_expiry(&self, now: Instant) -> Option<Duration> {
        self.loaded
            .iter()
            .filter(|m| m.leases == 0 && !self.line_wants(&m.key))
            .filter_map(|m| m.idle_since)
            .map(|since| (since + self.keep_alive).saturating_duration_since(now))
            .min()
    }

    fn expire(&mut self, now: Instant) -> Vec<(String, Pool)> {
        let keep = self.keep_alive;
        let mut unload = Vec::new();
        let mut kept = Vec::new();
        for model in std::mem::take(&mut self.loaded) {
            let idle = model
                .idle_since
                .map(|since| now.saturating_duration_since(since))
                .unwrap_or(Duration::ZERO);
            let wanted = self
                .line
                .iter()
                .any(|t| t.model.as_ref().is_some_and(|m| m.key == model.key));
            if model.leases == 0 && idle >= keep && !wanted {
                unload.push((model.key, model.pool));
            } else {
                kept.push(model);
            }
        }
        self.loaded = kept;
        unload
    }

    fn schedule(&mut self, now: Instant) -> Vec<(String, Pool)> {
        let mut unload = self.expire(now);
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
                None => Some(Ok(Grant {
                    pool: None,
                    key: None,
                    fresh: false,
                    n_ctx: 0,
                    lease: 0,
                })),
                Some(need) => self.grant_model(i, &need, &mut blocked, &mut unload),
            };
            match grant {
                Some(g) => {
                    let ticket = self.line.remove(i);
                    let g = match g {
                        Ok(mut g) => {
                            let id = self.next_lease;
                            self.next_lease += 1;
                            g.lease = id;
                            *self.running.entry(kind).or_default() += 1;
                            if let Some(key) = &g.key {
                                if let Some(model) = self.loaded.iter_mut().find(|m| &m.key == key)
                                {
                                    model.holders.insert(id, kind);
                                }
                            }
                            Ok(g)
                        }
                        Err(error) => Err(error),
                    };
                    self.granted.insert(ticket.id, g);
                }
                None => i += 1,
            }
        }
        unload
    }

    fn grant_model(
        &mut self,
        index: usize,
        need: &ModelNeed,
        blocked: &mut Vec<Pool>,
        unload: &mut Vec<(String, Pool)>,
    ) -> Option<Result<Grant, String>> {
        if let Some(at) = self.loaded.iter().position(|m| m.key == need.key) {
            if need.n_ctx <= self.loaded[at].n_ctx {
                self.loaded[at].leases += 1;
                self.loaded[at].idle_since = None;
                return Some(Ok(Grant {
                    pool: Some(self.loaded[at].pool),
                    key: Some(need.key.clone()),
                    fresh: false,
                    n_ctx: self.loaded[at].n_ctx,
                    lease: 0,
                }));
            }
            if self.loaded[at].leases > 0 {
                return self.wait_memory(index, need, blocked);
            }
        }
        if need.pools().iter().any(|pool| blocked.contains(pool)) {
            self.line[index].reason = Some(Wait::Turn);
            return None;
        }
        let mut fits_somewhere = false;
        for (n_ctx, bytes) in self.candidates(need) {
            if n_ctx < need.n_ctx || !self.fits_a_pool(need, bytes) {
                continue;
            }
            fits_somewhere = true;
            if self.acquire(need, n_ctx, bytes, unload) {
                let at = self
                    .loaded
                    .iter()
                    .position(|m| m.key == need.key)
                    .expect("just acquired");
                return Some(Ok(Grant {
                    pool: Some(self.loaded[at].pool),
                    key: Some(need.key.clone()),
                    fresh: true,
                    n_ctx: self.loaded[at].n_ctx,
                    lease: 0,
                }));
            }
        }
        if !fits_somewhere {
            return Some(Err(format!(
                "model needs {} MB, more than the budget allows",
                need.bytes >> 20
            )));
        }
        self.wait_memory(index, need, blocked)
    }

    fn wait_memory(
        &mut self,
        index: usize,
        need: &ModelNeed,
        blocked: &mut Vec<Pool>,
    ) -> Option<Result<Grant, String>> {
        self.line[index].reason = Some(Wait::Memory {
            key: need.key.clone(),
            name: need.name.clone(),
            bytes: need.bytes,
        });
        blocked.extend(need.pools());
        None
    }

    fn acquire(
        &mut self,
        need: &ModelNeed,
        n_ctx: u32,
        bytes: u64,
        unload: &mut Vec<(String, Pool)>,
    ) -> bool {
        if let Some(at) = self.loaded.iter().position(|m| m.key == need.key) {
            let pool = self.loaded[at].pool;
            let old = self.loaded[at].bytes;
            if bytes <= self.cap(pool)
                && self.evict(pool, bytes.saturating_sub(old), Some(&need.key), unload)
            {
                let model = self
                    .loaded
                    .iter_mut()
                    .find(|m| m.key == need.key)
                    .expect("still resident");
                model.bytes = bytes.max(model.bytes);
                model.n_ctx = n_ctx.max(model.n_ctx);
                model.leases = 1;
                model.idle_since = None;
                return true;
            }
            return self.move_pool(need, n_ctx, bytes, unload);
        }
        let Some(pool) = self.place(need, bytes, unload) else {
            return false;
        };
        self.loaded.push(Resident {
            key: need.key.clone(),
            name: need.name.clone(),
            pool,
            bytes,
            leases: 1,
            holders: HashMap::new(),
            n_ctx,
            idle_since: None,
        });
        true
    }

    fn move_pool(
        &mut self,
        need: &ModelNeed,
        n_ctx: u32,
        bytes: u64,
        unload: &mut Vec<(String, Pool)>,
    ) -> bool {
        let Some(at) = self.loaded.iter().position(|m| m.key == need.key) else {
            return false;
        };
        let old = self.loaded.remove(at);
        if let Some(pool) = self.place(need, bytes, unload) {
            unload.push((old.key.clone(), old.pool));
            self.loaded.push(Resident {
                key: old.key,
                name: old.name,
                pool,
                bytes,
                leases: 1,
                holders: HashMap::new(),
                n_ctx,
                idle_since: None,
            });
            true
        } else {
            self.loaded.insert(at.min(self.loaded.len()), old);
            false
        }
    }

    fn place(
        &mut self,
        need: &ModelNeed,
        bytes: u64,
        unload: &mut Vec<(String, Pool)>,
    ) -> Option<Pool> {
        need.pools()
            .iter()
            .copied()
            .find(|&pool| bytes <= self.cap(pool) && self.evict(pool, bytes, None, unload))
    }

    fn evict(
        &mut self,
        pool: Pool,
        need_free: u64,
        skip: Option<&str>,
        unload: &mut Vec<(String, Pool)>,
    ) -> bool {
        let free = self.cap(pool).saturating_sub(self.used(pool));
        if need_free <= free {
            return true;
        }
        let mut deficit = need_free - free;
        let mut order: Vec<usize> = self
            .loaded
            .iter()
            .enumerate()
            .filter(|(_, model)| {
                model.pool == pool && model.leases == 0 && skip != Some(model.key.as_str())
            })
            .map(|(index, _)| index)
            .collect();
        order.sort_by(|&a, &b| {
            let rank = |index: usize| {
                let model = &self.loaded[index];
                let wanted = self.wanted.contains(&model.key) || self.line_wants(&model.key);
                (wanted, model.idle_since)
            };
            rank(a).cmp(&rank(b))
        });
        let mut doomed = Vec::new();
        for index in order {
            if deficit == 0 {
                break;
            }
            deficit = deficit.saturating_sub(self.loaded[index].bytes);
            doomed.push(index);
        }
        if deficit > 0 {
            return false;
        }
        doomed.sort_unstable();
        for index in doomed.into_iter().rev() {
            let model = self.loaded.remove(index);
            unload.push((model.key, model.pool));
        }
        true
    }

    fn snapshot(&self, now: Instant) -> Status {
        Status {
            loaded: self
                .loaded
                .iter()
                .map(|model| {
                    let idle = model
                        .idle_since
                        .map(|since| now.saturating_duration_since(since))
                        .unwrap_or(Duration::ZERO);
                    let unloads_in =
                        (model.leases == 0).then(|| self.keep_alive.saturating_sub(idle));
                    LoadedModel {
                        key: model.key.clone(),
                        name: model.name.clone(),
                        pool: model.pool,
                        bytes: model.bytes,
                        leases: model.leases,
                        n_ctx: model.n_ctx,
                        idle,
                        unloads_in,
                    }
                })
                .collect(),
            running: WorkKind::ALL
                .iter()
                .map(|kind| (*kind, self.running.get(kind).copied().unwrap_or(0)))
                .collect(),
            waiting: self
                .line
                .iter()
                .filter_map(|ticket| {
                    ticket
                        .reason
                        .clone()
                        .map(|reason| (ticket.id, ticket.kind, reason))
                })
                .collect(),
            gpu_used: self.used(Pool::Gpu),
            ram_used: self.used(Pool::Cpu),
        }
    }
}

type Unload = Box<dyn Fn(&str, Pool) + Send + Sync>;

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    quantum: Duration,
    clock: Clock,
    on_unload: Unload,
}

impl Shared {
    fn update<R>(&self, f: impl FnOnce(&mut State, Instant) -> R) -> R {
        let now = self.clock.now();
        let (r, unload) = {
            let mut s = self.state.lock().expect("scheduler lock");
            let r = f(&mut s, now);
            let unload = s.schedule(now);
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
    pub fn new(
        budget: Budget,
        quantum: Duration,
        keep_alive: Duration,
        on_unload: impl Fn(&str, Pool) + Send + Sync + 'static,
    ) -> Scheduler {
        Self::build(budget, quantum, keep_alive, Clock::real(), on_unload)
    }

    #[cfg(test)]
    pub(crate) fn new_manual(
        budget: Budget,
        quantum: Duration,
        keep_alive: Duration,
        on_unload: impl Fn(&str, Pool) + Send + Sync + 'static,
    ) -> Scheduler {
        Self::build(budget, quantum, keep_alive, Clock::manual(), on_unload)
    }

    fn build(
        budget: Budget,
        quantum: Duration,
        keep_alive: Duration,
        clock: Clock,
        on_unload: impl Fn(&str, Pool) + Send + Sync + 'static,
    ) -> Scheduler {
        let state = State {
            budget,
            keep_alive,
            wanted: HashSet::new(),
            running: HashMap::new(),
            loaded: Vec::new(),
            line: Vec::new(),
            granted: HashMap::new(),
            next: 1,
            next_lease: 1,
            abandoned: HashSet::new(),
        };
        Scheduler {
            shared: Arc::new(Shared {
                state: Mutex::new(state),
                changed: Condvar::new(),
                quantum,
                clock,
                on_unload: Box::new(on_unload),
            }),
        }
    }

    pub fn set_budget(&self, budget: Budget) {
        self.shared.update(|s, _| s.budget = budget);
    }

    pub fn set_keep_alive(&self, keep_alive: Duration) {
        self.shared.update(|s, _| s.keep_alive = keep_alive);
    }

    pub fn set_wanted(&self, keys: HashSet<String>) {
        self.shared.update(|s, _| s.wanted = keys);
    }

    pub(crate) fn forget_failed(&self, key: &str) {
        self.shared.update(|s, _| {
            if let Some(i) = s.loaded.iter().position(|m| m.key == key && m.leases <= 1) {
                s.loaded.remove(i);
            }
        });
    }

    pub fn reclaim(&self, key: &str) -> bool {
        let gone = self.shared.update(|s, _| {
            let at = s.loaded.iter().position(|m| m.key == key)?;
            let resident = s.loaded.remove(at);
            for (id, kind) in resident.holders {
                if let Some(n) = s.running.get_mut(&kind) {
                    *n = n.saturating_sub(1);
                }
                s.abandoned.insert(id);
            }
            Some((resident.key, resident.pool))
        });
        if let Some((key, pool)) = &gone {
            (self.shared.on_unload)(key, *pool);
            log::warn!("model dropped after a stuck call: {key}");
        }
        self.shared.changed.notify_all();
        gone.is_some()
    }

    pub fn advance(&self, by: Duration) {
        self.shared.clock.advance(by);
        self.shared.update(|_, _| {});
    }

    pub fn wake(&self) {
        self.shared.changed.notify_all();
    }

    pub fn sweep_wait(&self, stop: &AtomicBool) {
        let guard = self.shared.state.lock().expect("scheduler lock");
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let wait = guard
            .time_until_expiry(self.shared.clock.now())
            .unwrap_or(Duration::from_secs(30));
        drop(
            self.shared
                .changed
                .wait_timeout(guard, wait.min(Duration::from_secs(30)))
                .expect("scheduler lock"),
        );
        if stop.load(Ordering::Relaxed) {
            return;
        }
        self.shared.update(|_, _| {});
    }

    pub fn request(&self, kind: WorkKind, model: Option<ModelNeed>, interactive: bool) -> Ticket {
        let id = self.shared.update(|s, _| {
            let id = s.next;
            s.next += 1;
            let at = if interactive {
                s.line
                    .iter()
                    .position(|t| !t.interactive)
                    .unwrap_or(s.line.len())
            } else {
                s.line.len()
            };
            s.line.insert(
                at,
                Waiting {
                    id,
                    kind,
                    model,
                    interactive,
                    reason: None,
                },
            );
            id
        });
        Ticket {
            shared: self.shared.clone(),
            id,
            kind,
            taken: false,
        }
    }

    pub fn status(&self) -> Status {
        let now = self.shared.clock.now();
        let (status, unload) = {
            let mut state = self.shared.state.lock().expect("scheduler lock");
            let unload = state.schedule(now);
            let status = state.snapshot(now);
            (status, unload)
        };
        for (key, pool) in unload {
            (self.shared.on_unload)(&key, pool);
        }
        self.shared.changed.notify_all();
        status
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

    pub fn try_take(&mut self) -> Option<anyhow::Result<Lease>> {
        let grant = self
            .shared
            .state
            .lock()
            .expect("scheduler lock")
            .granted
            .remove(&self.id)?;
        self.taken = true;
        Some(match grant {
            Ok(g) => Ok(Lease {
                shared: self.shared.clone(),
                id: g.lease,
                kind: self.kind,
                pool: g.pool,
                key: g.key,
                fresh: g.fresh,
                n_ctx: g.n_ctx,
                since: self.shared.clock.now(),
            }),
            Err(e) => Err(anyhow::anyhow!(e)),
        })
    }

    pub fn wait(mut self, cancel: &AtomicBool) -> anyhow::Result<Lease> {
        loop {
            if let Some(r) = self.try_take() {
                return r;
            }
            pulse::beat();
            if cancel.load(Ordering::Relaxed) {
                anyhow::bail!("cancelled");
            }
            let s = self.shared.state.lock().expect("scheduler lock");
            let _ = self
                .shared
                .changed
                .wait_timeout(s, Duration::from_millis(100));
        }
    }

    pub fn waiting_for(&self) -> Option<Wait> {
        self.shared
            .state
            .lock()
            .expect("scheduler lock")
            .line
            .iter()
            .find(|t| t.id == self.id)
            .and_then(|t| t.reason.clone())
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        if !self.taken {
            let id = self.id;
            let granted = self.shared.update(|s, _| {
                s.line.retain(|t| t.id != id);
                s.granted.remove(&id)
            });
            if let Some(Ok(g)) = granted {
                drop(Lease {
                    shared: self.shared.clone(),
                    id: g.lease,
                    kind: self.kind,
                    pool: g.pool,
                    key: g.key,
                    fresh: false,
                    n_ctx: g.n_ctx,
                    since: self.shared.clock.now(),
                });
            }
        }
    }
}

pub struct Lease {
    shared: Arc<Shared>,
    id: u64,
    kind: WorkKind,
    pool: Option<Pool>,
    key: Option<String>,
    fresh: bool,
    n_ctx: u32,
    since: Instant,
}

impl Lease {
    pub fn pool(&self) -> Option<Pool> {
        self.pool
    }

    pub fn fresh(&self) -> bool {
        self.fresh
    }

    pub fn n_ctx(&self) -> u32 {
        self.n_ctx
    }

    pub fn should_yield(&self) -> bool {
        let Some(pool) = self.pool else { return false };
        if self
            .shared
            .clock
            .now()
            .saturating_duration_since(self.since)
            < self.shared.quantum
        {
            return false;
        }
        let s = self.shared.state.lock().expect("scheduler lock");
        s.line.iter().any(|t| {
            matches!(t.reason, Some(Wait::Memory { .. }) | Some(Wait::Turn))
                && t.model.as_ref().is_some_and(|m| m.pools().contains(&pool))
        })
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let (kind, key, agent) = (self.kind, self.key.clone(), self.id);
        self.shared.update(|s, now| {
            let abandoned = s.abandoned.remove(&agent);
            if !abandoned {
                if let Some(n) = s.running.get_mut(&kind) {
                    *n = n.saturating_sub(1);
                }
            }
            if let Some(key) = &key {
                if let Some(i) = s.loaded.iter().position(|m| &m.key == key) {
                    s.loaded[i].holders.remove(&agent);
                    s.loaded[i].leases = s.loaded[i].leases.saturating_sub(1);
                    if s.loaded[i].leases == 0 {
                        s.loaded[i].idle_since = Some(now);
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn sched(gpu: f32, ram: f32, quantum_ms: u64) -> (Scheduler, Arc<Mutex<Vec<String>>>) {
        let unloaded = Arc::new(Mutex::new(Vec::new()));
        let u = unloaded.clone();
        let budget = Budget {
            gpu_memory_gb: gpu,
            ram_memory_gb: ram,
            agent: 8,
            transcription: 8,
            indexing: 8,
            web_search: 8,
            provider_calls: 8,
            ..Budget::default()
        };
        (
            Scheduler::new(
                budget,
                Duration::from_millis(quantum_ms),
                Duration::ZERO,
                move |key, _| u.lock().unwrap().push(key.to_string()),
            ),
            unloaded,
        )
    }

    fn manual(gpu: f32, keep: Duration) -> (Scheduler, Arc<Mutex<Vec<String>>>) {
        manual_quantum(gpu, keep, Duration::from_secs(1000))
    }

    fn manual_quantum(
        gpu: f32,
        keep: Duration,
        quantum: Duration,
    ) -> (Scheduler, Arc<Mutex<Vec<String>>>) {
        let unloaded = Arc::new(Mutex::new(Vec::new()));
        let u = unloaded.clone();
        let budget = Budget {
            gpu_memory_gb: gpu,
            ram_memory_gb: 0.0,
            agent: 8,
            transcription: 8,
            indexing: 8,
            web_search: 8,
            provider_calls: 8,
            ..Budget::default()
        };
        (
            Scheduler::new_manual(budget, quantum, keep, move |key, _| {
                u.lock().unwrap().push(key.to_string())
            }),
            unloaded,
        )
    }

    fn model(key: &str, gb: u64, device: Device) -> Option<ModelNeed> {
        Some(ModelNeed {
            key: key.into(),
            name: key.into(),
            bytes: gb * GB,
            device,
            n_ctx: 0,
        })
    }

    fn model_ctx(key: &str, gb: u64, n_ctx: u32) -> Option<ModelNeed> {
        Some(ModelNeed {
            key: key.into(),
            name: key.into(),
            bytes: gb * GB,
            device: Device::Gpu,
            n_ctx,
        })
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
        let mut b = s.request(WorkKind::Transcription, model("asr", 4, Device::Gpu), false);
        assert_eq!(take(&mut b).pool(), Some(Pool::Cpu));
        let mut c = s.request(WorkKind::Indexing, model("emb", 1, Device::Cpu), false);
        assert_eq!(take(&mut c).pool(), Some(Pool::Cpu));
        let mut d = s.request(WorkKind::Agent, model("llm", 5, Device::Auto), false);
        let ld = take(&mut d);
        assert_eq!((ld.pool(), ld.fresh()), (Some(Pool::Gpu), false));
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
        assert_eq!(
            b.waiting_for(),
            Some(Wait::Memory {
                key: "asr".into(),
                name: "asr".into(),
                bytes: 4 * GB
            })
        );
        drop(la);
        assert_eq!(
            *unloaded.lock().unwrap(),
            ["big"],
            "no one needs it: unloaded at once"
        );
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
        s.set_budget(Budget {
            gpu_memory_gb: 8.0,
            ram_memory_gb: 0.0,
            agent: 1,
            transcription: 8,
            indexing: 8,
            web_search: 8,
            provider_calls: 8,
            ..Budget::default()
        });
        let mut b = s.request(WorkKind::Agent, model("llm", 6, Device::Gpu), false);
        assert_eq!(b.waiting_for(), Some(Wait::Slot(WorkKind::Agent)));
        drop(la);
        assert!(
            unloaded.lock().unwrap().is_empty(),
            "kept: a waiting job wants it"
        );
        assert!(!take(&mut b).fresh());
    }

    #[test]
    fn round_robin_makes_a_long_holder_yield() {
        let (s, _) = manual_quantum(8.0, Duration::ZERO, Duration::from_millis(30));
        let mut a = s.request(WorkKind::Transcription, model("asr", 6, Device::Gpu), false);
        let la = take(&mut a);
        assert!(!la.should_yield(), "no one is waiting");
        let mut b = s.request(WorkKind::Agent, model("llm", 6, Device::Gpu), false);
        assert!(b.try_take().is_none());
        assert!(!la.should_yield(), "within its quantum");
        s.advance(Duration::from_millis(30));
        assert!(
            la.should_yield(),
            "quantum over while another job waits for this memory"
        );
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
        let budget = Budget {
            gpu_memory_gb: 4.0,
            ram_memory_gb: 0.0,
            web_search: 2,
            agent: 8,
            transcription: 8,
            indexing: 8,
            provider_calls: 8,
            ..Budget::default()
        };
        s.set_budget(budget);
        let mut w1 = s.request(WorkKind::WebSearch, None, false);
        let mut w2 = s.request(WorkKind::WebSearch, None, false);
        let w3 = s.request(WorkKind::WebSearch, None, false);
        let (_l1, _l2) = (take(&mut w1), take(&mut w2));
        assert_eq!(w3.waiting_for(), Some(Wait::Slot(WorkKind::WebSearch)));
        let mut big = s.request(WorkKind::Agent, model("a", 3, Device::Gpu), false);
        let lbig = take(&mut big);
        let mut waiting = s.request(WorkKind::Agent, model("b", 3, Device::Gpu), false);
        let small = s.request(WorkKind::Indexing, model("c", 1, Device::Gpu), false);
        let mut light = s.request(WorkKind::Transcription, None, false);
        assert!(waiting.try_take().is_none());
        assert_eq!(
            small.waiting_for(),
            Some(Wait::Turn),
            "1 GB is free, but b asked first"
        );
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
        let _answer = take(&mut chat);
        assert!(bg.try_take().is_none());
        let cancel = AtomicBool::new(true);
        let t = s.request(WorkKind::Agent, model("x", 4, Device::Gpu), false);
        let id = t.id();
        assert!(t.wait(&cancel).is_err());
        assert!(
            s.status().waiting.iter().all(|(w, _, _)| *w != id),
            "a cancelled ticket leaves the line"
        );
    }

    #[test]
    fn wait_blocks_until_memory_is_free() {
        let (s, _) = sched(4.0, 0.0, 1000);
        let mut a = s.request(WorkKind::Agent, model("a", 4, Device::Gpu), false);
        let la = take(&mut a);
        let s2 = s.clone();
        let h = std::thread::spawn(move || {
            s2.request(WorkKind::Transcription, model("b", 4, Device::Gpu), false)
                .wait(&AtomicBool::new(false))
                .map(|l| l.fresh())
        });
        std::thread::sleep(Duration::from_millis(50));
        drop(la);
        assert!(h.join().unwrap().unwrap());
    }

    #[test]
    fn the_keep_alive_sweep_unloads_and_returns() {
        let keep = Duration::from_millis(50);
        let (s, unloaded) = manual(8.0, keep);
        let mut first = s.request(WorkKind::Agent, model("llm", 1, Device::Gpu), false);
        drop(take(&mut first));
        s.shared.clock.advance(keep);
        let swept = s.clone();
        let stop = AtomicBool::new(false);
        let (tx, rx) = std::sync::mpsc::channel();
        let sweep = std::thread::spawn(move || {
            swept.sweep_wait(&stop);
            let _ = tx.send(());
        });
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the sweep returns instead of deadlocking");
        sweep.join().unwrap();
        assert_eq!(*unloaded.lock().unwrap(), ["llm"], "the idle model expired");
        let mut again = s.request(WorkKind::Agent, model("llm", 1, Device::Gpu), false);
        assert!(
            take(&mut again).fresh(),
            "the scheduler still hands out leases"
        );
    }

    #[test]
    fn the_keeper_does_not_spin_on_a_model_a_queued_job_wants() {
        let (s, unloaded) = manual(8.0, Duration::from_secs(10));
        let mut idle = s.request(WorkKind::Agent, model("llm", 1, Device::Gpu), false);
        drop(take(&mut idle));
        s.set_budget(Budget {
            gpu_memory_gb: 8.0,
            ram_memory_gb: 0.0,
            agent: 1,
            transcription: 8,
            indexing: 8,
            web_search: 8,
            provider_calls: 8,
            ..Budget::default()
        });
        let mut holder = s.request(WorkKind::Agent, model("other", 1, Device::Gpu), false);
        let held = take(&mut holder);
        let mut queued = s.request(WorkKind::Agent, model("llm", 1, Device::Gpu), false);
        assert_eq!(queued.waiting_for(), Some(Wait::Slot(WorkKind::Agent)));
        assert!(
            queued.try_take().is_none(),
            "it waits for the slot, not fails"
        );
        s.advance(Duration::from_secs(10));
        assert!(
            unloaded.lock().unwrap().is_empty(),
            "a wanted model is not unloaded"
        );
        let state = s.shared.state.lock().unwrap();
        assert_eq!(
            state.time_until_expiry(s.shared.clock.now()),
            None,
            "the keeper has nothing to wake for"
        );
        drop(state);
        drop(held);
    }

    #[test]
    fn an_idle_model_is_reused_within_keep_alive_and_unloaded_after_it() {
        let (s, unloaded) = manual(8.0, Duration::from_secs(300));
        let mut first = s.request(WorkKind::Agent, model("llm", 1, Device::Gpu), false);
        drop(take(&mut first));
        s.advance(Duration::from_secs(299));
        assert!(
            unloaded.lock().unwrap().is_empty(),
            "still inside keep-alive"
        );
        let resident = &s.status().loaded;
        assert_eq!(resident.len(), 1);
        assert_eq!(resident[0].idle, Duration::from_secs(299));
        assert_eq!(resident[0].unloads_in, Some(Duration::from_secs(1)));
        let mut again = s.request(WorkKind::Agent, model("llm", 1, Device::Gpu), false);
        let lease = take(&mut again);
        assert!(!lease.fresh(), "reused, not reloaded");
        drop(lease);
        s.advance(Duration::from_secs(300));
        assert_eq!(*unloaded.lock().unwrap(), ["llm"]);
        assert!(s.status().loaded.is_empty());
    }

    #[test]
    fn memory_pressure_evicts_the_least_recently_used_idle_model_and_wanted_last() {
        let (s, unloaded) = manual(8.0, Duration::from_secs(300));
        let mut older = s.request(WorkKind::Agent, model("older", 3, Device::Gpu), false);
        drop(take(&mut older));
        s.advance(Duration::from_secs(10));
        let mut newer = s.request(WorkKind::Indexing, model("newer", 3, Device::Gpu), false);
        drop(take(&mut newer));
        s.advance(Duration::from_secs(10));
        let mut third = s.request(
            WorkKind::Transcription,
            model("third", 3, Device::Gpu),
            false,
        );
        let _lease = take(&mut third);
        assert_eq!(*unloaded.lock().unwrap(), ["older"]);
        let left: Vec<_> = s.status().loaded.iter().map(|m| m.key.clone()).collect();
        assert!(
            left.contains(&"newer".into()) && left.contains(&"third".into()),
            "{left:?}"
        );

        drop(_lease);
        s.set_keep_alive(Duration::ZERO);
        assert!(s.status().loaded.is_empty());
        unloaded.lock().unwrap().clear();
        s.set_keep_alive(Duration::from_secs(300));

        let mut older = s.request(WorkKind::Agent, model("older", 3, Device::Gpu), false);
        drop(take(&mut older));
        s.advance(Duration::from_secs(10));
        let mut newer = s.request(WorkKind::Indexing, model("newer", 3, Device::Gpu), false);
        drop(take(&mut newer));
        s.set_wanted(HashSet::from(["older".into()]));
        let mut third = s.request(
            WorkKind::Transcription,
            model("third", 3, Device::Gpu),
            false,
        );
        let _lease = take(&mut third);
        assert_eq!(
            *unloaded.lock().unwrap(),
            ["newer"],
            "the wanted model is evicted last"
        );
        assert!(s.status().loaded.iter().any(|m| m.key == "older"));
    }

    #[test]
    fn different_contexts_share_one_load_and_a_larger_one_reloads_once() {
        let (s, unloaded) = manual(8.0, Duration::from_secs(300));
        let mut pad = s.request(WorkKind::Agent, model("pad", 5, Device::Gpu), false);
        let held = take(&mut pad);
        let mut big = s.request(WorkKind::Agent, model_ctx("llm", 4, 12_288), false);
        let mut small = s.request(WorkKind::Indexing, model_ctx("llm", 2, 8192), false);
        assert!(big.try_take().is_none() && small.waiting_for().is_some());
        drop(held);
        let large = take(&mut big);
        assert!(large.fresh());
        assert_eq!(large.n_ctx(), 12_288, "the largest context that fits");
        drop(large);
        let reused = take(&mut small);
        assert!(!reused.fresh());
        assert_eq!(reused.n_ctx(), 12_288);
        drop(reused);
        assert!(
            unloaded.lock().unwrap().iter().all(|key| key == "pad"),
            "the model itself was not reloaded"
        );

        let mut bigger = s.request(WorkKind::Agent, model_ctx("llm", 6, 16_384), false);
        let grown = take(&mut bigger);
        assert!(grown.fresh());
        assert_eq!(grown.n_ctx(), 16_384);
        drop(grown);
        let mut back = s.request(WorkKind::Agent, model_ctx("llm", 2, 8192), false);
        let again = take(&mut back);
        assert!(!again.fresh(), "no ping-pong back to the smaller context");
        assert_eq!(again.n_ctx(), 16_384);
    }
}
