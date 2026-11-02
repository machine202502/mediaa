use super::super::header;
use crate::features::agent::requests as request_buffer;
use crate::features::intent::Intent;
use crate::ui::pages::ctx::Ctx;
use crate::ui::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::{Connection, ConnectionKind};
use mediaa_protocol::backend::{AgentRequestWire, AgentResponseStateWire};
use rust_i18n::t;
use std::collections::HashMap;

enum CheckState {
    Running(u64),
    Done(Result<String, String>),
}

pub struct ProvidersUi {
    checks: HashMap<String, CheckState>,
    add_kind: ConnectionKind,
    pub(crate) contract: bool,
}

impl Default for ProvidersUi {
    fn default() -> Self {
        Self {
            checks: HashMap::new(),
            add_kind: ConnectionKind::OpenRouter,
            contract: false,
        }
    }
}

fn kind_label(kind: ConnectionKind) -> String {
    match kind {
        ConnectionKind::OpenRouter => t!("agent.kind_openrouter").into(),
        ConnectionKind::DeepSeek => t!("agent.kind_deepseek").into(),
        ConnectionKind::HuggingFace => t!("agent.kind_huggingface").into(),
        ConnectionKind::Ollama => t!("agent.kind_ollama").into(),
        ConnectionKind::OpenAiCompatible => t!("agent.kind_openai").into(),
        ConnectionKind::Transcription => t!("agent.kind_transcription").into(),
    }
}

const ADD_KINDS: [ConnectionKind; 6] = [
    ConnectionKind::OpenRouter,
    ConnectionKind::DeepSeek,
    ConnectionKind::HuggingFace,
    ConnectionKind::Ollama,
    ConnectionKind::OpenAiCompatible,
    ConnectionKind::Transcription,
];

pub fn ui(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    header(ui, &t!("agent.providers_title"), "");
    let mut requests = Vec::new();
    let running: Vec<(String, u64)> = ctx
        .agent_ui
        .providers
        .checks
        .iter()
        .filter_map(|(id, state)| match state {
            CheckState::Running(request) => Some((id.clone(), *request)),
            CheckState::Done(_) => None,
        })
        .collect();
    for (id, request) in running {
        match request_buffer::take(request) {
            Some(AgentResponseStateWire::Done { payload }) => {
                let version = payload
                    .get("version")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                ctx.agent_ui
                    .providers
                    .checks
                    .insert(id, CheckState::Done(Ok(version)));
            }
            Some(AgentResponseStateWire::Failed { error }) => {
                ctx.agent_ui
                    .providers
                    .checks
                    .insert(id, CheckState::Done(Err(error)));
            }
            _ => {}
        }
    }
    let mut changed = false;
    let mut remove = None;
    let mut open_contract = false;
    ui.horizontal(|ui| {
        let mut picked = ctx.agent_ui.providers.add_kind;
        crate::ui::theme::combo_box("add-provider")
            .selected_text(kind_label(picked))
            .show_ui(ui, |ui| {
                for kind in ADD_KINDS {
                    if kind == ConnectionKind::Transcription {
                        ui.horizontal(|ui| {
                            if ui
                                .selectable_label(picked == kind, kind_label(kind))
                                .clicked()
                            {
                                picked = kind;
                            }
                            if ui
                                .add(egui::Button::new(RichText::new(icon::INFO)).frame(false))
                                .on_hover_text(t!("settings.contract_title"))
                                .clicked()
                            {
                                open_contract = true;
                            }
                        });
                    } else if ui
                        .selectable_label(picked == kind, kind_label(kind))
                        .clicked()
                    {
                        picked = kind;
                    }
                }
            });
        if picked != ctx.agent_ui.providers.add_kind {
            ctx.agent_ui.providers.add_kind = picked;
        }
        if ui.button(icon::PLUS).clicked() {
            let connection = Connection::new(picked, &kind_label(picked));
            ctx.settings.agent.connections.push(connection);
            changed = true;
        }
    });
    if open_contract {
        ctx.agent_ui.providers.contract = true;
    }
    let ids: Vec<String> = ctx
        .settings
        .agent
        .connections
        .iter()
        .map(|connection| connection.id.clone())
        .collect();
    let groups: Vec<String> = ctx
        .settings
        .agent
        .groups
        .iter()
        .map(|group| group.name.clone())
        .collect();
    for id in ids {
        let Some(index) = ctx
            .settings
            .agent
            .connections
            .iter()
            .position(|connection| connection.id == id)
        else {
            continue;
        };
        ui.add_space(6.0);
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let connection = &mut ctx.settings.agent.connections[index];
            ui.horizontal(|ui| {
                ui.label(RichText::new(kind_label(connection.kind)).strong());
                if ui.button(t!("agent.remove_provider")).clicked() {
                    remove = Some(id.clone());
                }
            });
            theme::row(ui, &t!("agent.provider_name"), |ui| {
                changed |= ui.text_edit_singleline(&mut connection.name).changed();
            });
            theme::row(ui, &t!("agent.provider_pool"), |ui| {
                let selected = connection.group.clone();
                let label = if selected.is_empty() {
                    t!("agent.provider_no_pool").into_owned()
                } else {
                    selected.clone()
                };
                crate::ui::theme::combo_box(("provider-group", &id))
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(selected.is_empty(), t!("agent.provider_no_pool"))
                            .clicked()
                        {
                            connection.group.clear();
                            changed = true;
                        }
                        for group in &groups {
                            if ui.selectable_label(selected == *group, group).clicked() {
                                connection.group = group.clone();
                                changed = true;
                            }
                        }
                    });
            });
            changed |= fields(
                ui,
                connection,
                &mut ctx.agent_ui.providers,
                &id,
                &mut requests,
            );
        });
    }
    if let Some(id) = remove {
        ctx.settings
            .agent
            .connections
            .retain(|connection| connection.id != id);
        changed = true;
    }
    if changed {
        ctx.settings_changed();
    }
    ctx.intents.extend(requests);
    crate::ui::pages::about::contract_window(ui.ctx(), &mut ctx.agent_ui.providers.contract);
}

fn fields(
    ui: &mut egui::Ui,
    connection: &mut Connection,
    state: &mut ProvidersUi,
    id: &str,
    requests: &mut Vec<Intent>,
) -> bool {
    let mut changed = false;
    if connection.kind != ConnectionKind::OpenRouter
        && connection.kind != ConnectionKind::DeepSeek
        && connection.kind != ConnectionKind::HuggingFace
    {
        theme::row(ui, &t!("agent.base_url"), |ui| {
            changed |= ui.text_edit_singleline(&mut connection.base_url).changed();
        });
    }
    theme::row(ui, &t!("agent.api_key"), |ui| {
        changed |= ui
            .add(egui::TextEdit::singleline(&mut connection.api_key).password(true))
            .changed();
    });
    theme::row(ui, &t!("agent.proxy"), |ui| {
        changed |= ui.text_edit_singleline(&mut connection.proxy).changed();
    });
    if connection.kind == ConnectionKind::Ollama {
        ollama(ui, connection, state, id, requests);
    }
    changed
}

fn ollama(
    ui: &mut egui::Ui,
    connection: &Connection,
    state: &mut ProvidersUi,
    id: &str,
    requests: &mut Vec<Intent>,
) {
    if let Some(CheckState::Running(request)) = state.checks.get(id) {
        if request_buffer::take(*request).is_none() {
            ui.spinner();
        }
    }
    if ui.button(t!("agent.check")).clicked() {
        let request = request_buffer::next_id();
        state
            .checks
            .insert(id.to_string(), CheckState::Running(request));
        requests.push(Intent::AgentRequest(AgentRequestWire {
            request,
            kind: "check_connection".into(),
            payload: serde_json::json!({ "connection": connection.id }),
        }));
    }
    if let Some(CheckState::Done(Err(error))) = state.checks.get(id) {
        theme::hint(ui, error);
        if ui.button(t!("agent.ollama_serve")).clicked() {
            let request = request_buffer::next_id();
            requests.push(Intent::AgentRequest(AgentRequestWire {
                request,
                kind: "ollama_serve".into(),
                payload: serde_json::json!({ "connection": connection.id }),
            }));
        }
    }
    if let Some(CheckState::Done(Ok(version))) = state.checks.get(id) {
        theme::hint(ui, version);
    }
}
