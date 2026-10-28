use crate::pages::agent::{AgentUi, Services};
use eframe::egui;
use mediaa_agent::run::{self, Feed, Streams};
use mediaa_agent::slots::Slots;
use mediaa_core::agent::AgentSettings;
use mediaa_engine::Engine;
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
    engine_budget: mediaa_engine::Budget,
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
    pub fn new(engine: Arc<dyn Engine>, budget: &mediaa_core::agent::Budget) -> AgentController {
        AgentController {
            ui: AgentUi::default(),
            engine,
            engine_budget: budget.clone().into(),
            slots: Slots::new(budget.clone()),
            hosted: None,
            checked: None,
            pending: false,
        }
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
        if active {
            self.ui.engine_state.tick(&self.engine, ctx);
        }
        self.slots.set_budget(agent.budget.clone());
        let budget: mediaa_engine::Budget = agent.budget.clone().into();
        if budget != self.engine_budget {
            self.engine.set_budget(budget);
            self.engine_budget = budget;
        }
        let wanted = with_language(agent, language);
        if let Some(hosted) = &self.hosted {
            if hosted.dir == ai_dir {
                let stale = *hosted.settings.read().expect("settings lock") != wanted;
                if stale {
                    *hosted.settings.write().expect("settings lock") = wanted;
                }
                hosted.sched.set_pools(run::pools(&agent.budget));
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
        match run::open_scheduler(ai_dir, &agent.budget) {
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
            slots: self.slots.clone(),
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
