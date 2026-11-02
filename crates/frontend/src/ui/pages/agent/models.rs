use super::super::header;
use crate::features::agent::requests as request_buffer;
use crate::features::intent::Intent;
use crate::ui::pages::ctx::Ctx;
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_core::agent::{Connection, ModelRole, ModelSource};
use mediaa_protocol::backend::{AgentRequestWire, AgentResponseStateWire};
use rust_i18n::t;
use std::collections::HashMap;

#[derive(Default)]
pub struct ModelsUi {
    models: HashMap<String, Vec<String>>,
    listing: HashMap<String, u64>,
    list_error: HashMap<String, String>,
    pull: HashMap<String, u64>,
    pull_progress: HashMap<String, (u64, u64)>,
    check: Option<u64>,
}

pub fn ui(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    header(ui, &t!("agent.models_title"), "");
    let listing: Vec<(String, u64)> = ctx
        .agent_ui
        .models
        .listing
        .iter()
        .map(|(id, request)| (id.clone(), *request))
        .collect();
    for (id, request) in listing {
        match request_buffer::take(request) {
            Some(AgentResponseStateWire::Done { payload }) => {
                ctx.agent_ui.models.listing.remove(&id);
                ctx.agent_ui.models.list_error.remove(&id);
                let models: Vec<String> = serde_json::from_value(payload).unwrap_or_default();
                ctx.agent_ui.models.models.insert(id, models);
            }
            Some(AgentResponseStateWire::Failed { error }) => {
                ctx.agent_ui.models.listing.remove(&id);
                ctx.agent_ui.models.list_error.insert(id, error);
            }
            _ => {}
        }
    }
    let pulls: Vec<(String, u64)> = ctx
        .agent_ui
        .models
        .pull
        .iter()
        .map(|(key, request)| (key.clone(), *request))
        .collect();
    for (key, request) in pulls {
        match request_buffer::take(request) {
            Some(AgentResponseStateWire::Progress { done, total }) => {
                ctx.agent_ui.models.pull_progress.insert(key, (done, total));
            }
            Some(AgentResponseStateWire::Done { .. }) => {
                ctx.agent_ui.models.pull.remove(&key);
                ctx.agent_ui.models.pull_progress.remove(&key);
            }
            Some(AgentResponseStateWire::Failed { .. }) => {
                ctx.agent_ui.models.pull.remove(&key);
                ctx.agent_ui.models.pull_progress.remove(&key);
            }
            _ => {}
        }
    }
    if let Some(request) = ctx.agent_ui.models.check {
        match request_buffer::take(request) {
            Some(AgentResponseStateWire::Done { .. })
            | Some(AgentResponseStateWire::Failed { .. }) => {
                ctx.agent_ui.models.check = None;
            }
            _ => {
                ui.spinner();
            }
        }
    }
    let mut changed = false;
    let connections = ctx.settings.agent.connections.clone();
    for role in ModelRole::ALL {
        ui.add_space(6.0);
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, icon::CUBE, &role_label(role));
            let fitting: Vec<&Connection> = connections
                .iter()
                .filter(|connection| connection.supports(role.capability()))
                .collect();
            let current = ctx.settings.agent.models.get(role).clone();
            let mut selected = current
                .as_ref()
                .map(|source| source.connection.clone())
                .unwrap_or_default();
            crate::ui::theme::combo_box(("connection", role as u8))
                .selected_text(if selected.is_empty() {
                    t!("agent.model_connection").into_owned()
                } else {
                    fitting
                        .iter()
                        .find(|connection| connection.id == selected)
                        .map(|connection| connection.name.clone())
                        .unwrap_or(selected.clone())
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(selected.is_empty(), t!("agent.model_none"))
                        .clicked()
                    {
                        selected.clear();
                        *ctx.settings.agent.models.get_mut(role) = None;
                        changed = true;
                    }
                    for connection in &fitting {
                        if ui
                            .selectable_label(selected == connection.id, &connection.name)
                            .clicked()
                        {
                            selected = connection.id.clone();
                            let model = current
                                .as_ref()
                                .map(|source| source.model.clone())
                                .unwrap_or_default();
                            *ctx.settings.agent.models.get_mut(role) =
                                Some(ModelSource::new(connection.id.clone(), model));
                            changed = true;
                        }
                    }
                });
            if fitting.is_empty() {
                theme::hint(ui, t!("agent.no_connection_for_role"));
            }
            if let Some(source) = ctx.settings.agent.models.get_mut(role) {
                theme::row(ui, &t!("agent.model_name"), |ui| {
                    changed |= ui.text_edit_singleline(&mut source.model).changed();
                });
                let known = ctx.agent_ui.models.models.get(&source.connection).cloned();
                if let Some(known) = known {
                    crate::ui::theme::combo_box(("model", role as u8))
                        .selected_text(if source.model.is_empty() {
                            t!("agent.model_name").into_owned()
                        } else {
                            source.model.clone()
                        })
                        .show_ui(ui, |ui| {
                            for id in known {
                                if ui.selectable_label(source.model == id, &id).clicked() {
                                    source.model = id;
                                    changed = true;
                                }
                            }
                        });
                }
                let connection_id = source.connection.clone();
                if ui.button(t!("agent.list_models")).clicked() {
                    let request = request_buffer::next_id();
                    ctx.agent_ui
                        .models
                        .listing
                        .insert(connection_id.clone(), request);
                    ctx.intents.push(Intent::AgentRequest(AgentRequestWire {
                        request,
                        kind: "models".into(),
                        payload: serde_json::json!({ "connection": connection_id }),
                    }));
                }
                if let Some(error) = ctx.agent_ui.models.list_error.get(&connection_id) {
                    theme::hint(ui, error);
                }
                let model_name = source.model.clone();
                let listed = ctx
                    .agent_ui
                    .models
                    .models
                    .get(&connection_id)
                    .map(|ids| ids.iter().any(|id| id == &model_name))
                    .unwrap_or(false);
                let ollama = connections.iter().any(|connection| {
                    connection.id == connection_id
                        && connection.kind == mediaa_core::agent::ConnectionKind::Ollama
                });
                if ollama
                    && !model_name.trim().is_empty()
                    && !listed
                    && ui.button(t!("agent.pull_model")).clicked()
                {
                    let request = request_buffer::next_id();
                    ctx.agent_ui.models.pull.insert(role_key(role), request);
                    ctx.agent_ui
                        .models
                        .pull_progress
                        .insert(role_key(role), (0, 0));
                    ctx.intents.push(Intent::AgentRequest(AgentRequestWire {
                        request,
                        kind: "pull".into(),
                        payload: serde_json::json!({
                            "connection": connection_id,
                            "model": model_name,
                        }),
                    }));
                }
                if let Some((done, total)) = ctx.agent_ui.models.pull_progress.get(&role_key(role))
                {
                    if *total > 0 {
                        ui.add(egui::ProgressBar::new(*done as f32 / *total as f32));
                    }
                }
            }
            if let Some(source) = ctx.settings.agent.models.get_mut(role) {
                if matches!(role, ModelRole::Agent | ModelRole::Titles) {
                    theme::row(ui, &t!("agent.context"), |ui| {
                        let mut text = source.context.to_string();
                        if ui.text_edit_singleline(&mut text).changed() {
                            if let Ok(value) = text.trim().parse::<u32>() {
                                source.context = value;
                                changed = true;
                            }
                        }
                    });
                    if ui
                        .checkbox(&mut source.thinking, t!("agent.thinking"))
                        .changed()
                    {
                        changed = true;
                    }
                    if source.thinking {
                        let mut depth = source.thinking_depth;
                        crate::ui::theme::combo_box(("depth", role as u8))
                            .selected_text(depth_label(depth))
                            .show_ui(ui, |ui| {
                                for item in [
                                    mediaa_core::agent::ReasoningDepth::Low,
                                    mediaa_core::agent::ReasoningDepth::Medium,
                                    mediaa_core::agent::ReasoningDepth::High,
                                    mediaa_core::agent::ReasoningDepth::XHigh,
                                ] {
                                    if ui
                                        .selectable_label(depth == item, depth_label(item))
                                        .clicked()
                                    {
                                        depth = item;
                                    }
                                }
                            });
                        if depth != source.thinking_depth {
                            source.thinking_depth = depth;
                            changed = true;
                        }
                    }
                }
                if role == ModelRole::SpeechRecognition {
                    let transcription = connections.iter().any(|connection| {
                        connection.id == source.connection
                            && connection.kind == mediaa_core::agent::ConnectionKind::Transcription
                    });
                    changed |= ui
                        .checkbox(&mut source.diarize, t!("agent.speech_diarize"))
                        .changed();
                    changed |= ui
                        .checkbox(&mut source.word_times, t!("agent.speech_words"))
                        .changed();
                    if transcription {
                        changed |= ui
                            .checkbox(&mut source.voiceprint, t!("agent.speech_voice"))
                            .changed();
                    }
                }
            }
        });
    }
    if ui.button(t!("agent.check_models")).clicked() {
        let request = request_buffer::next_id();
        ctx.agent_ui.models.check = Some(request);
        ctx.intents.push(Intent::AgentRequest(AgentRequestWire {
            request,
            kind: "check_models".into(),
            payload: serde_json::Value::Null,
        }));
    }
    if changed {
        ctx.settings_changed();
    }
}

fn role_key(role: ModelRole) -> String {
    format!("{role:?}")
}

fn role_label(role: ModelRole) -> String {
    match role {
        ModelRole::Embedding => t!("agent.role_embedding"),
        ModelRole::Reranker => t!("agent.role_reranker"),
        ModelRole::Agent => t!("agent.role_agent"),
        ModelRole::Titles => t!("agent.role_titles"),
        ModelRole::SpeechRecognition => t!("agent.role_speech_rec"),
    }
    .into()
}

fn depth_label(depth: mediaa_core::agent::ReasoningDepth) -> &'static str {
    match depth {
        mediaa_core::agent::ReasoningDepth::Low => "low",
        mediaa_core::agent::ReasoningDepth::Medium => "medium",
        mediaa_core::agent::ReasoningDepth::High => "high",
        mediaa_core::agent::ReasoningDepth::XHigh => "xhigh",
    }
}
