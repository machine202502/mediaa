use super::super::header;
use crate::app::App;
use crate::jobs::Job;
use crate::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_agent::check::Status;
use mediaa_core::agent::{Connection, ModelRole, ModelSource};
use rust_i18n::t;
use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

#[derive(Default)]
pub struct ModelsUi {
    models: HashMap<String, Vec<String>>,
    listing: HashMap<String, Job<Result<Vec<String>, String>>>,
    list_error: HashMap<String, String>,
    pull: HashMap<String, Job<Result<(), String>>>,
    pull_progress: HashMap<String, Arc<Mutex<(u64, u64)>>>,
    check: Option<Job<Vec<(ModelRole, Status)>>>,
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("agent.models_title"), "");
    let listed: Vec<(String, Result<Vec<String>, String>)> = app
        .agent
        .models
        .listing
        .iter_mut()
        .filter_map(|(id, job)| {
            job.poll().map(|result| {
                (
                    id.clone(),
                    result.unwrap_or_else(|error| Err(format!("{error:#}"))),
                )
            })
        })
        .collect();
    for (id, result) in listed {
        app.agent.models.listing.remove(&id);
        match result {
            Ok(models) => {
                app.agent.models.list_error.remove(&id);
                app.agent.models.models.insert(id, models);
            }
            Err(error) => {
                app.agent.models.list_error.insert(id, error);
            }
        }
    }
    if let Some(job) = app.agent.models.check.as_mut() {
        if let Some(result) = job.poll() {
            let _ = result;
            app.agent.models.check = None;
        } else {
            ui.spinner();
        }
    }
    let mut changed = false;
    let connections = app.settings.agent.connections.clone();
    for role in ModelRole::ALL {
        ui.add_space(6.0);
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, icon::CUBE, &role_label(role));
            let fitting: Vec<&Connection> = connections
                .iter()
                .filter(|connection| connection.supports(role.capability()))
                .collect();
            let current = app.settings.agent.models.get(role).clone();
            let mut selected = current
                .as_ref()
                .map(|source| source.connection.clone())
                .unwrap_or_default();
            egui::ComboBox::from_id_salt(("connection", role as u8))
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
                        *app.settings.agent.models.get_mut(role) = None;
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
                            *app.settings.agent.models.get_mut(role) =
                                Some(ModelSource::new(connection.id.clone(), model));
                            changed = true;
                        }
                    }
                });
            if fitting.is_empty() {
                theme::hint(ui, t!("agent.no_connection_for_role"));
            }
            if let Some(source) = app.settings.agent.models.get_mut(role) {
                theme::row(ui, &t!("agent.model_name"), |ui| {
                    changed |= ui.text_edit_singleline(&mut source.model).changed();
                });
                let known = app.agent.models.models.get(&source.connection).cloned();
                if let Some(known) = known {
                    egui::ComboBox::from_id_salt(("model", role as u8))
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
                    let engine = app.agent.engine().clone();
                    let id = connection_id.clone();
                    let job = Job::spawn("list-models", Arc::default(), move || {
                        engine
                            .models(&id)
                            .map(|list| list.into_iter().map(|model| model.id).collect())
                            .map_err(|error| format!("{error:#}"))
                    });
                    if let Ok(job) = job {
                        app.agent.models.listing.insert(connection_id.clone(), job);
                    }
                }
                if let Some(error) = app.agent.models.list_error.get(&connection_id) {
                    theme::hint(ui, error);
                }
                let model_name = source.model.clone();
                let listed = app
                    .agent
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
                    let progress = Arc::new(Mutex::new((0u64, 0u64)));
                    app.agent
                        .models
                        .pull_progress
                        .insert(role_key(role), progress.clone());
                    let connection = connections
                        .iter()
                        .find(|connection| connection.id == connection_id)
                        .cloned();
                    if let Some(connection) = connection {
                        let engine = app.agent.engine().clone();
                        let model = model_name.clone();
                        let job = Job::spawn("ollama-pull", Arc::default(), move || {
                            let cancel = AtomicBool::new(false);
                            engine
                                .pull(
                                    &connection.id,
                                    &model,
                                    &mut |done, total| {
                                        *progress.lock().expect("pull progress") = (done, total);
                                    },
                                    &cancel,
                                )
                                .map_err(|error| format!("{error:#}"))
                        });
                        if let Ok(job) = job {
                            app.agent.models.pull.insert(role_key(role), job);
                        }
                    }
                }
                if let Some(progress) = app.agent.models.pull_progress.get(&role_key(role)) {
                    let (done, total) = *progress.lock().expect("pull progress");
                    if total > 0 {
                        ui.add(egui::ProgressBar::new(done as f32 / total as f32));
                    }
                }
            }
            if let Some(source) = app.settings.agent.models.get_mut(role) {
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
                        egui::ComboBox::from_id_salt(("depth", role as u8))
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
        let settings = app.settings.agent.clone();
        let engine = app.agent.engine().clone();
        app.agent.models.check = Job::spawn("check-models", Arc::default(), move || {
            let mut rows = Vec::new();
            mediaa_agent::check::check_all(&settings, engine.as_ref(), |role, status| {
                rows.push((role, status));
            });
            rows
        })
        .ok();
    }
    if changed {
        app.settings_changed();
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
