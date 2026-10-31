use super::super::header;
use crate::app::App;
use crate::jobs::Job;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::{Connection, ConnectionKind};
use rust_i18n::t;
use std::collections::HashMap;

enum CheckState {
    Running(Job<Result<String, String>>),
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

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("agent.providers_title"), "");
    let mut changed = false;
    let mut remove = None;
    let mut open_contract = false;
    ui.horizontal(|ui| {
        let mut picked = app.agent.providers.add_kind;
        egui::ComboBox::from_id_salt("add-provider")
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
        if picked != app.agent.providers.add_kind {
            app.agent.providers.add_kind = picked;
        }
        if ui.button(icon::PLUS).clicked() {
            let connection = Connection::new(picked, &kind_label(picked));
            app.settings.agent.connections.push(connection);
            changed = true;
        }
    });
    if open_contract {
        app.agent.providers.contract = true;
    }
    let ids: Vec<String> = app
        .settings
        .agent
        .connections
        .iter()
        .map(|connection| connection.id.clone())
        .collect();
    let groups: Vec<String> = app
        .settings
        .agent
        .groups
        .iter()
        .map(|group| group.name.clone())
        .collect();
    for id in ids {
        let Some(index) = app
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
            let connection = &mut app.settings.agent.connections[index];
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
                egui::ComboBox::from_id_salt(("provider-group", &id))
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
            changed |= fields(ui, connection, &mut app.agent.providers, &id);
        });
    }
    if let Some(id) = remove {
        app.settings
            .agent
            .connections
            .retain(|connection| connection.id != id);
        changed = true;
    }
    if changed {
        app.settings_changed();
    }
    crate::pages::about::contract_window(ui.ctx(), &mut app.agent.providers.contract);
}

fn fields(
    ui: &mut egui::Ui,
    connection: &mut Connection,
    state: &mut ProvidersUi,
    id: &str,
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
        ollama(ui, connection, state, id);
    }
    changed
}

fn ollama(ui: &mut egui::Ui, connection: &Connection, state: &mut ProvidersUi, id: &str) {
    if let Some(CheckState::Running(job)) = state.checks.get_mut(id) {
        if let Some(result) = job.poll() {
            state.checks.insert(
                id.to_string(),
                CheckState::Done(result.unwrap_or_else(|error| Err(format!("{error:#}")))),
            );
        } else {
            ui.spinner();
        }
    }
    if ui.button(t!("agent.check")).clicked() {
        let id_owned = connection.id.clone();
        let connections =
            std::sync::Arc::new(mediaa_engine::Connections::new(vec![connection.clone()]));
        let started = Job::spawn("ollama-check", std::sync::Arc::default(), move || {
            connections
                .ollama_version(&id_owned)
                .map_err(|error| format!("{error:#}"))
        });
        state.checks.insert(
            id.to_string(),
            started.map_or_else(
                |error| CheckState::Done(Err(format!("{error:#}"))),
                CheckState::Running,
            ),
        );
    }
    if let Some(CheckState::Done(Err(error))) = state.checks.get(id) {
        theme::hint(ui, error);
        if ui.button(t!("agent.ollama_serve")).clicked() {
            let id_owned = connection.id.clone();
            let connections =
                std::sync::Arc::new(mediaa_engine::Connections::new(vec![connection.clone()]));
            let _ = Job::spawn("ollama-serve", std::sync::Arc::default(), move || {
                connections.ollama_serve(&id_owned)
            });
        }
    }
    if let Some(CheckState::Done(Ok(version))) = state.checks.get(id) {
        theme::hint(ui, version);
    }
}
