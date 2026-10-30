use super::super::header;
use crate::app::App;
use crate::jobs::Job;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::{WebEngine, WebSettings, GROUP_LIMIT_MAX};
use rust_i18n::t;
use std::sync::Arc;

pub struct BudgetUi {
    name: String,
    limit: u32,
    error: Option<String>,
    web_check: Option<Job<Result<String, String>>>,
    web_status: Option<Result<String, String>>,
}

impl Default for BudgetUi {
    fn default() -> BudgetUi {
        BudgetUi {
            name: String::new(),
            limit: 1,
            error: None,
            web_check: None,
            web_status: None,
        }
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("budget.title"), "");
    let mut changed = false;
    let agent = &mut app.settings.agent;
    let state = &mut app.agent.budget;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.scope(|ui| theme::section(ui, icon::STACK, &t!("budget.pools")))
            .response
            .on_hover_text(t!("budget.pools_hint"));
        let mut remove = None;
        for index in 0..agent.groups.len() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&agent.groups[index].name).strong());
                changed |= ui
                    .add(
                        egui::DragValue::new(&mut agent.groups[index].limit)
                            .range(1..=GROUP_LIMIT_MAX),
                    )
                    .on_hover_text(t!("budget.pools_hint"))
                    .changed();
                if ui.button(icon::TRASH).clicked() {
                    remove = Some(agent.groups[index].name.clone());
                }
            });
        }
        if let Some(name) = remove {
            agent.remove_group(&name);
            changed = true;
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.name)
                    .hint_text(t!("budget.pool_name"))
                    .desired_width(140.0),
            );
            ui.add(egui::DragValue::new(&mut state.limit).range(1..=GROUP_LIMIT_MAX))
                .on_hover_text(t!("budget.pools_hint"));
            if ui.button(t!("budget.add")).clicked() {
                if agent.add_group(state.name.trim(), state.limit) {
                    state.name.clear();
                    state.error = None;
                    changed = true;
                } else {
                    state.error = Some(t!("budget.pool_error").into_owned());
                }
            }
        });
        if let Some(error) = &state.error {
            theme::hint(ui, error);
        }
    });
    ui.add_space(6.0);
    changed |= web(
        ui,
        &mut app.settings.agent.web,
        &mut app.settings.agent.web_limit,
        state,
    );
    if changed {
        app.settings_changed();
    }
}

fn engine_label(engine: WebEngine) -> &'static str {
    match engine {
        WebEngine::DuckDuckGo => "DuckDuckGo",
        WebEngine::SearXng => "SearXNG",
        WebEngine::Brave => "Brave",
        WebEngine::Tavily => "Tavily",
    }
}

fn web(ui: &mut egui::Ui, web: &mut WebSettings, limit: &mut u32, state: &mut BudgetUi) -> bool {
    let mut changed = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.scope(|ui| theme::section(ui, icon::GLOBE, &t!("budget.tools")))
            .response
            .on_hover_text(t!("budget.tools_hint"));
        changed |= ui
            .checkbox(&mut web.enabled, t!("agent.web_enabled"))
            .changed();
        theme::row(ui, &t!("budget.web_slots"), |ui| {
            changed |= ui
                .add(egui::DragValue::new(limit).range(1..=GROUP_LIMIT_MAX))
                .on_hover_text(t!("budget.tools_hint"))
                .changed();
        });
        theme::row(ui, &t!("agent.web_engine"), |ui| {
            egui::ComboBox::from_id_salt("web-engine")
                .selected_text(engine_label(web.engine))
                .show_ui(ui, |ui| {
                    for engine in [
                        WebEngine::DuckDuckGo,
                        WebEngine::SearXng,
                        WebEngine::Brave,
                        WebEngine::Tavily,
                    ] {
                        if ui
                            .selectable_label(web.engine == engine, engine_label(engine))
                            .clicked()
                        {
                            web.engine = engine;
                            changed = true;
                        }
                    }
                });
        });
        if web.engine == WebEngine::SearXng {
            theme::row(ui, &t!("agent.web_searxng"), |ui| {
                changed |= ui.text_edit_singleline(&mut web.searxng_url).changed();
            });
        }
        theme::row(ui, &t!("agent.proxy"), |ui| {
            changed |= ui.text_edit_singleline(&mut web.proxy).changed();
        });
        if matches!(web.engine, WebEngine::Brave | WebEngine::Tavily) {
            theme::row(ui, &t!("agent.api_key"), |ui| {
                changed |= ui
                    .add(egui::TextEdit::singleline(&mut web.api_key).password(true))
                    .changed();
            });
        }
        if let Some(job) = state.web_check.as_mut() {
            if let Some(result) = job.poll() {
                state.web_check = None;
                state.web_status = Some(result.unwrap_or_else(|error| Err(format!("{error:#}"))));
            }
        }
        if ui.button(t!("agent.web_check")).clicked() {
            let web = web.clone();
            state.web_status = None;
            state.web_check = Job::spawn("web-check", Arc::default(), move || {
                mediaa_agent::web::search(&web, "MediaA", 3)
                    .map(|hits| hits.len().to_string())
                    .map_err(|error| format!("{error:#}"))
            })
            .ok();
        }
        if let Some(result) = &state.web_status {
            match result {
                Ok(count) => theme::hint(ui, t!("agent.provider_ok", n = count)),
                Err(error) => theme::hint(ui, error),
            }
        }
    });
    changed
}
