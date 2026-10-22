//! Agent section: virtual catalogs of files, LLM providers and the models used per role.

pub mod analysis;
pub mod budget;
pub mod catalogs;
pub mod chat;
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
use mediaa_agent::diarize::Diarize;
use mediaa_agent::index::Index;
use mediaa_agent::outline;
use mediaa_agent::slots::Slots;
use mediaa_agent::transcribe::Transcribe;
use mediaa_agent::worker::{self, TaskHandler};
use mediaa_core::agent::AgentSettings;
use mediaa_engine::Engine;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// The queue worker of a library's `.ai` folder (transcription and the other background tasks);
/// stops when dropped (at the next checkpoint of its task).
struct Worker {
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    /// The settings its tasks see.
    settings: Arc<RwLock<AgentSettings>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
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
    worker: Option<Worker>,
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
            worker: None,
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
        if let Some(w) = &self.worker {
            if w.dir == ai_dir {
                let stale = *w.settings.read().expect("settings lock") != *agent;
                if stale {
                    *w.settings.write().expect("settings lock") = agent.clone();
                }
                return;
            }
        }
        // A new library: a worker of its own.
        self.worker = None;
        let settings = Arc::new(RwLock::new(agent.clone()));
        let s = settings.clone();
        let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(move || s.read().expect("settings lock").clone());
        let slots = self.slots.clone();
        let engine = self.engine.clone();
        let mut handlers: Vec<Box<dyn TaskHandler>> = vec![
            Box::new(Transcribe::new(ai_dir, slots.clone(), current.clone())),
            Box::new(Diarize::new(ai_dir, slots.clone(), current.clone())),
            Box::new(Index::new(ai_dir, slots.clone(), current.clone())),
        ];
        // The topics stage: the plan, a digest per topic, the merge and a detail per topic.
        handlers.extend(outline::handlers(ai_dir, slots, current));
        let stop = Arc::new(AtomicBool::new(false));
        match worker::spawn(ai_dir.to_path_buf(), engine, handlers, stop.clone()) {
            Ok(_) => self.worker = Some(Worker { dir: ai_dir.to_path_buf(), stop, settings }),
            Err(e) => log::error!("agent worker: {e}"),
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
}

impl AgentUi {
    /// The services of the current library (after [`AgentUi::tick`]).
    pub fn services(&self) -> Option<Services> {
        let worker = self.worker.as_ref()?;
        Some(Services { ai_dir: worker.dir.clone(), engine: self.engine.clone(), slots: self.slots.clone(), settings: worker.settings.clone() })
    }
}
