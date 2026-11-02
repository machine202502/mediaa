use super::super::header;
use crate::features::agent::requests as request_buffer;
use crate::features::intent::Intent;
use crate::ui::pages::ctx::Ctx;
use crate::ui::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::{WebEngine, WebSettings, GROUP_LIMIT_MAX};
use mediaa_protocol::backend::{AgentRequestWire, AgentResponseStateWire};
use rust_i18n::t;

pub struct BudgetUi {
    name: String,
    limit: u32,
    error: Option<String>,
    web_check: Option<u64>,
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

pub fn ui(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    header(ui, &t!("budget.title"), "");
    if let Some(request) = ctx.agent_ui.budget.web_check {
        match request_buffer::take(request) {
            Some(AgentResponseStateWire::Done { payload }) => {
                let count = payload
                    .get("count")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(0);
                ctx.agent_ui.budget.web_check = None;
                ctx.agent_ui.budget.web_status = Some(Ok(count.to_string()));
            }
            Some(AgentResponseStateWire::Failed { error }) => {
                ctx.agent_ui.budget.web_check = None;
                ctx.agent_ui.budget.web_status = Some(Err(error));
            }
            _ => {}
        }
    }
    let mut changed = false;
    let agent = &mut ctx.settings.agent;
    let state = &mut ctx.agent_ui.budget;
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
        &mut ctx.settings.agent.web,
        &mut ctx.settings.agent.web_limit,
        state,
        &mut ctx.intents,
    );
    if changed {
        ctx.settings_changed();
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

fn web(
    ui: &mut egui::Ui,
    web: &mut WebSettings,
    limit: &mut u32,
    state: &mut BudgetUi,
    intents: &mut Vec<Intent>,
) -> bool {
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
            crate::ui::theme::combo_box("web-engine")
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
        if let Some(request) = state.web_check {
            let _ = request;
            ui.spinner();
        }
        if ui.button(t!("agent.web_check")).clicked() {
            let request = request_buffer::next_id();
            state.web_status = None;
            state.web_check = Some(request);
            intents.push(Intent::AgentRequest(AgentRequestWire {
                request,
                kind: "web_check".into(),
                payload: serde_json::Value::Null,
            }));
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
