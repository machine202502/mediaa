//! Agent section: virtual catalogs of files, LLM providers and the models used per role.

pub mod analysis;
pub mod budget;
pub mod catalogs;
pub mod chat;
pub mod composer;
pub mod dialog;
pub mod engine_state;
pub mod markdown;
pub mod mentions;
pub mod models;
pub mod providers;
pub mod queue;
pub mod studio;
pub mod task_details;
pub mod task_label;
pub mod timeline;
pub mod topics;

use eframe::egui;
use mediaa_agent::run::{self, Feed, Streams};
use mediaa_agent::slots::Slots;
use mediaa_core::agent::{AgentSettings, Budget, WorkKind};
use mediaa_engine::Engine;
use mediaa_queue::Scheduler;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// The queue of a library's `.ai` folder. Dropping it asks the host thread to stop.
struct Hosted {
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    settings: Arc<RwLock<AgentSettings>>,
    sched: Arc<Scheduler>,
    streams: Arc<Streams>,
    feed: Arc<Feed>,
    /// Pool sizes the scheduler was opened with. A change opens it again.
    pools: (u32, u32, u32, u32, u64),
}

impl Drop for Hosted {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn pool_key(budget: &Budget) -> (u32, u32, u32, u32, u64) {
    (
        budget.limit(WorkKind::Transcription),
        budget.limit(WorkKind::Indexing),
        budget.limit(WorkKind::Agent),
        budget.limit(WorkKind::ProviderCalls),
        budget.gpu_bytes().saturating_add(budget.ram_bytes()),
    )
}

pub struct AgentUi {
    pub catalogs: catalogs::CatalogsUi,
    pub providers: providers::ProvidersUi,
    pub models: models::ModelsUi,
    pub queue: queue::QueueUi,
    /// The engine's state, shared by every page (and the section header).
    pub engine_state: engine_state::EngineState,
    /// The models' engine, shared with the agent's services.
    engine: Arc<dyn Engine>,
    /// The memory budget the engine was last told (settings may change it).
    engine_budget: mediaa_engine::Budget,
    /// Limits of the app's own model work (the engine has its own queue of loaded models).
    slots: Slots,
    hosted: Option<Hosted>,
}

impl AgentUi {
    pub fn new(engine: Arc<dyn Engine>, budget: &mediaa_core::agent::Budget) -> AgentUi {
        AgentUi {
            catalogs: Default::default(),
            providers: Default::default(),
            models: Default::default(),
            queue: Default::default(),
            engine_state: Default::default(),
            engine,
            engine_budget: budget.clone().into(),
            slots: Slots::new(budget.clone()),
            hosted: None,
        }
    }

    /// The models' engine of the app (one for the whole process).
    pub fn engine(&self) -> &Arc<dyn Engine> {
        &self.engine
    }

    /// Keeps the engine's budget, the slots of model work and the queue worker on the library's
    /// `.ai` folder and the current settings (called every frame; cheap when nothing changed).
    /// Also asks the engine for its state in the background (one poller for every page).
    pub fn tick(&mut self, ai_dir: &Path, agent: &AgentSettings, ctx: &egui::Context) {
        self.engine_state.tick(&self.engine, ctx);
        self.slots.set_budget(agent.budget.clone());
        let budget: mediaa_engine::Budget = agent.budget.clone().into();
        if budget != self.engine_budget {
            // The engine keeps its models under the same memory budget.
            self.engine.set_budget(budget);
            self.engine_budget = budget;
        }
        let pools = pool_key(&agent.budget);
        if let Some(hosted) = &self.hosted {
            if hosted.dir == ai_dir && hosted.pools == pools {
                let stale = *hosted.settings.read().expect("settings lock") != *agent;
                if stale {
                    *hosted.settings.write().expect("settings lock") = agent.clone();
                }
                return;
            }
        }
        // A new library, or new pool sizes: a host of its own. Recover continues runs the
        // previous process left in the file.
        self.hosted = None;
        let settings = Arc::new(RwLock::new(agent.clone()));
        let s = settings.clone();
        let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(move || s.read().expect("settings lock").clone());
        let slots = self.slots.clone();
        let engine = self.engine.clone();
        let streams = Arc::new(Streams::default());
        let feed = Arc::new(Feed::default());
        let stop = Arc::new(AtomicBool::new(false));
        match run::open_scheduler(ai_dir, &agent.budget) {
            Ok(sched) => {
                run::register_stages(&sched, ai_dir, engine.clone(), slots, current, streams.clone());
                if let Err(e) = run::spawn_host(sched.clone(), engine, feed.clone(), stop.clone()) {
                    log::error!("agent queue: {e}");
                    return;
                }
                self.hosted = Some(Hosted { dir: ai_dir.to_path_buf(), stop, settings, sched, streams, feed, pools });
            }
            Err(e) => log::error!("agent queue: {e:#}"),
        }
    }
}

/// What studios need for searching and asking: the engine, the slots of model work and the
/// current settings.
#[derive(Clone)]
pub struct Services {
    pub ai_dir: PathBuf,
    pub engine: Arc<dyn Engine>,
    pub slots: Slots,
    pub settings: Arc<RwLock<AgentSettings>>,
    pub sched: Arc<Scheduler>,
    pub streams: Arc<Streams>,
    pub feed: Arc<Feed>,
}

impl AgentUi {
    /// The services of the current library (after [`AgentUi::tick`]).
    pub fn services(&self) -> Option<Services> {
        let hosted = self.hosted.as_ref()?;
        Some(Services {
            ai_dir: hosted.dir.clone(),
            engine: self.engine.clone(),
            slots: self.slots.clone(),
            settings: hosted.settings.clone(),
            sched: hosted.sched.clone(),
            streams: hosted.streams.clone(),
            feed: hosted.feed.clone(),
        })
    }
}
