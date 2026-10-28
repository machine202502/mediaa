pub mod analysis;
pub mod budget;
pub mod catalogs;
pub mod chat;
pub mod composer;
pub mod dialog;
pub mod markdown;
pub mod mentions;
pub mod models;
pub mod providers;
pub mod queue;
pub mod studio;
pub mod tab_strip;
pub mod task_details;
pub mod task_label;
pub mod timeline;
pub mod topics;

use mediaa_agent::run::{Feed, Streams};
use mediaa_agent::slots::Slots;
use mediaa_core::agent::AgentSettings;
use mediaa_engine::Engine;
use mediaa_queue::Scheduler;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

#[derive(Default)]
pub struct AgentUi {
    pub catalogs: catalogs::CatalogsUi,
    pub providers: providers::ProvidersUi,
    pub models: models::ModelsUi,
    pub queue: queue::QueueUi,
}

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
