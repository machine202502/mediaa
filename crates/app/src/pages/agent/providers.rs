use super::super::header;
use crate::app::App;
use crate::jobs::Job;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::{Connection, ConnectionKind, WebEngine, WebSettings};
use rust_i18n::t;
use std::collections::HashMap;

enum CheckState {
    Running(Job<Result<String, String>>),
    Done(Result<String, String>),
}

pub struct ProvidersUi {
    checks: HashMap<String, CheckState>,
    web_check: Option<CheckState>,
    add_kind: ConnectionKind,
}

impl Default for ProvidersUi {
    fn default() -> Self {
        Self {
            checks: HashMap::new(),
            web_check: None,
            add_kind: ConnectionKind::OpenRouter,
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
    ui.horizontal(|ui| {
        let mut picked = app.agent.providers.add_kind;
        egui::ComboBox::from_id_salt("add-provider")
            .selected_text(kind_label(picked))
            .show_ui(ui, |ui| {
                for kind in ADD_KINDS {
                    if ui
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
    let ids: Vec<String> = app
        .settings
        .agent
        .connections
        .iter()
        .map(|connection| connection.id.clone())
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
    ui.add_space(8.0);
    changed |= web(ui, &mut app.settings.agent.web, &mut app.agent.providers);
    if changed {
        app.settings_changed();
    }
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

fn web(ui: &mut egui::Ui, web: &mut WebSettings, state: &mut ProvidersUi) -> bool {
    let mut changed = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::GLOBE, &t!("agent.web_search"));
        changed |= ui
            .checkbox(&mut web.enabled, t!("agent.web_enabled"))
            .changed();
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
        if let Some(CheckState::Running(job)) = state.web_check.as_mut() {
            if let Some(result) = job.poll() {
                state.web_check = Some(CheckState::Done(
                    result.unwrap_or_else(|error| Err(format!("{error:#}"))),
                ));
            }
        }
        if ui.button(t!("agent.check")).clicked() {
            let web = web.clone();
            state.web_check = Some(
                Job::spawn("web-check", std::sync::Arc::default(), move || {
                    mediaa_agent::web::search(&web, "MediaA", 3)
                        .map(|hits| hits.len().to_string())
                        .map_err(|error| format!("{error:#}"))
                })
                .map_or_else(
                    |error| CheckState::Done(Err(format!("{error:#}"))),
                    CheckState::Running,
                ),
            );
        }
        if let Some(CheckState::Done(result)) = &state.web_check {
            match result {
                Ok(count) => theme::hint(ui, t!("agent.provider_ok", n = count)),
                Err(error) => theme::hint(ui, error),
            }
        }
    });
    changed
}
