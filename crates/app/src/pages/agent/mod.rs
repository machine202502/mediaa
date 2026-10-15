//! Agent section: virtual catalogs of files, LLM providers and the models used per role.

pub mod budget;
pub mod catalogs;
pub mod chat;
pub mod dialog;
pub mod models;
pub mod providers;
pub mod queue;
pub mod studio;

use mediaa_agent::runtime::Runtime;
use mediaa_agent::index::Index;
use mediaa_agent::outline::Outline;
use mediaa_agent::transcribe::Transcribe;
use mediaa_agent::worker::{self, TaskHandler};
use mediaa_core::agent::{AgentSettings, Budget};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

/// The queue worker of a library's `.ai` folder (transcription and the other background tasks);
/// stops when dropped (at the next checkpoint of its task).
struct Worker {
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    /// The settings its tasks see (Hugging Face token and proxy for downloads).
    settings: Arc<RwLock<AgentSettings>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub struct AgentUi {
    pub catalogs: catalogs::CatalogsUi,
    pub providers: providers::ProvidersUi,
    pub models: models::ModelsUi,
    pub queue: queue::QueueUi,
    pub budget: budget::BudgetUi,
    /// The model runtime of the library's `.ai` folder and the budget it runs under.
    runtime: Option<(PathBuf, Budget, Runtime)>,
    worker: Option<Worker>,
}

impl AgentUi {
    /// The model runtime of `ai_dir` (one per library), kept on the current budget.
    pub fn runtime(&mut self, ai_dir: &Path, budget: &Budget) -> &Runtime {
        let same = self.runtime.as_ref().is_some_and(|(dir, _, _)| dir == ai_dir);
        if !same {
            self.runtime = Some((ai_dir.to_path_buf(), budget.clone(), Runtime::new(ai_dir, budget.clone())));
        }
        let (_, current, runtime) = self.runtime.as_mut().expect("just set");
        if current != budget {
            runtime.set_budget(budget.clone());
            *current = budget.clone();
        }
        runtime
    }

    /// Keeps the runtime and the queue worker on the library's `.ai` folder and the current
    /// settings (called every frame; cheap when nothing changed).
    pub fn tick(&mut self, ai_dir: &Path, agent: &AgentSettings) {
        let scheduler = self.runtime(ai_dir, &agent.budget).scheduler().clone();
        if let Some(w) = &self.worker {
            if w.dir == ai_dir {
                let stale = *w.settings.read().expect("settings lock") != *agent;
                if stale {
                    *w.settings.write().expect("settings lock") = agent.clone();
                }
                return;
            }
        }
        // A new library (or the first frame): a worker of its own.
        self.worker = None;
        let settings = Arc::new(RwLock::new(agent.clone()));
        let s = settings.clone();
        let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(move || s.read().expect("settings lock").clone());
        let runtime = self.runtime(ai_dir, &agent.budget).clone();
        let handlers: Vec<Box<dyn TaskHandler>> = vec![
            Box::new(Transcribe::new(ai_dir, scheduler, current.clone())),
            Box::new(Index::new(ai_dir, runtime.clone(), current.clone())),
            Box::new(Outline::new(ai_dir, runtime, current)),
        ];
        let stop = Arc::new(AtomicBool::new(false));
        match worker::spawn(ai_dir.to_path_buf(), handlers, stop.clone()) {
            Ok(_) => self.worker = Some(Worker { dir: ai_dir.to_path_buf(), stop, settings }),
            Err(e) => log::error!("agent worker: {e}"),
        }
    }
}

/// What studios need for searching: the runtime (query embeddings) and the current settings.
#[derive(Clone)]
pub struct Services {
    pub ai_dir: PathBuf,
    pub runtime: Runtime,
    pub settings: Arc<RwLock<AgentSettings>>,
}

impl AgentUi {
    /// The services of the current library (after [`AgentUi::tick`]).
    pub fn services(&self) -> Option<Services> {
        let (dir, _, runtime) = self.runtime.as_ref()?;
        let worker = self.worker.as_ref().filter(|w| &w.dir == dir)?;
        Some(Services { ai_dir: dir.clone(), runtime: runtime.clone(), settings: worker.settings.clone() })
    }
}
