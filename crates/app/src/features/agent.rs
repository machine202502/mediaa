use crate::pages::agent::{AgentUi, Services};
use eframe::egui;
use mediaa_agent::run::{self, Feed, Streams};
use mediaa_core::agent::{AgentSettings, Connection};
use mediaa_core::slots::Slots;
use mediaa_engine::{Connections, Engine};
use mediaa_queue::Scheduler;
use std::ops::{Deref, DerefMut};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

struct Hosted {
    dir: PathBuf,
    stop: Arc<AtomicBool>,
    settings: Arc<RwLock<AgentSettings>>,
    sched: Arc<Scheduler>,
    streams: Arc<Streams>,
    feed: Arc<Feed>,
}

impl Drop for Hosted {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn with_language(agent: &AgentSettings, language: &str) -> AgentSettings {
    let mut wanted = agent.clone();
    wanted.ui_language = Some(language.to_string());
    wanted
}

pub struct AgentController {
    ui: AgentUi,
    engine: Arc<dyn Engine>,
    connections: Option<Arc<Connections>>,
    applied: Vec<Connection>,
    slots: Slots,
    hosted: Option<Hosted>,
    checked: Option<PathBuf>,
    pending: bool,
}

impl Deref for AgentController {
    type Target = AgentUi;

    fn deref(&self) -> &AgentUi {
        &self.ui
    }
}

impl DerefMut for AgentController {
    fn deref_mut(&mut self) -> &mut AgentUi {
        &mut self.ui
    }
}

impl AgentController {
    pub fn new(engine: Arc<dyn Engine>, slots: Slots) -> AgentController {
        AgentController {
            ui: AgentUi::default(),
            engine,
            connections: None,
            applied: Vec::new(),
            slots,
            hosted: None,
            checked: None,
            pending: false,
        }
    }

    pub fn with_connections(connections: Arc<Connections>, slots: Slots) -> AgentController {
        let engine: Arc<dyn Engine> = Arc::new(mediaa_engine::Gated::new(
            connections.clone(),
            slots.clone(),
        ));
        let mut controller = AgentController::new(engine, slots);
        controller.connections = Some(connections);
        controller
    }

    pub fn engine(&self) -> &Arc<dyn Engine> {
        &self.engine
    }

    pub fn slots(&self) -> &Slots {
        &self.slots
    }

    fn pending_work(&mut self, ai_dir: &Path) -> bool {
        if self.checked.as_deref() != Some(ai_dir) {
            self.checked = Some(ai_dir.to_path_buf());
            self.pending = run::has_unfinished(ai_dir);
        }
        self.pending
    }

    pub fn tick(
        &mut self,
        ai_dir: &Path,
        agent: &AgentSettings,
        language: &str,
        active: bool,
        ctx: &egui::Context,
    ) {
        if !active && !self.pending_work(ai_dir) {
            return;
        }
        let _ = ctx;
        self.slots.configure(agent);
        if self.applied != agent.connections {
            if let Some(connections) = &self.connections {
                connections.set_connections(agent.connections.clone());
            }
            self.applied = agent.connections.clone();
        }
        let wanted = with_language(agent, language);
        if let Some(hosted) = &self.hosted {
            if hosted.dir == ai_dir {
                let stale = *hosted.settings.read().expect("settings lock") != wanted;
                if stale {
                    *hosted.settings.write().expect("settings lock") = wanted;
                }
                hosted.sched.set_pools(run::pools());
                return;
            }
        }
        self.hosted = None;
        let settings = Arc::new(RwLock::new(wanted));
        let s = settings.clone();
        let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> =
            Arc::new(move || s.read().expect("settings lock").clone());
        let slots = self.slots.clone();
        let engine = self.engine.clone();
        let streams = Arc::new(Streams::default());
        let feed = Arc::new(Feed::default());
        let stop = Arc::new(AtomicBool::new(false));
        match run::open_scheduler(ai_dir) {
            Ok(sched) => {
                run::register_stages(
                    &sched,
                    ai_dir,
                    engine.clone(),
                    slots,
                    current,
                    streams.clone(),
                );
                if let Err(e) = run::spawn_host(sched.clone(), engine, feed.clone(), stop.clone()) {
                    log::error!("agent queue: {e}");
                    return;
                }
                self.hosted = Some(Hosted {
                    dir: ai_dir.to_path_buf(),
                    stop,
                    settings,
                    sched,
                    streams,
                    feed,
                });
            }
            Err(e) => log::error!("agent queue: {e:#}"),
        }
    }

    pub fn services(&self) -> Option<Services> {
        let hosted = self.hosted.as_ref()?;
        Some(Services {
            ai_dir: hosted.dir.clone(),
            engine: self.engine.clone(),
            settings: hosted.settings.clone(),
            sched: hosted.sched.clone(),
            streams: hosted.streams.clone(),
            feed: hosted.feed.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stages_get_the_interface_language_with_the_settings() {
        let agent = AgentSettings {
            verify_answers: false,
            ..AgentSettings::default()
        };
        let wanted = with_language(&agent, "ru");
        assert_eq!(wanted.ui_language.as_deref(), Some("ru"));
        assert_eq!(wanted.language_name(), "Russian");
        assert!(!wanted.verify_answers, "the rest of the settings is kept");
        assert!(
            agent.ui_language.is_none(),
            "the saved settings are not touched"
        );
    }
}
