use super::super::header;
use crate::app::App;
use crate::jobs::Job;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::{Connection, ConnectionKind, OutputFormat, WebEngine, WebSettings};
use rust_i18n::t;
use std::collections::HashMap;
use std::path::PathBuf;

enum CheckState {
    Running(Job<Result<String, String>>),
    Done(Result<String, String>),
}

#[derive(Default)]
pub struct ProvidersUi {
    checks: HashMap<String, CheckState>,
    shown_keys: Vec<String>,
    web_check: Option<CheckState>,
}

fn kind_label(kind: ConnectionKind) -> String {
    match kind {
        ConnectionKind::OpenRouter => t!("agent.kind_openrouter").into(),
        ConnectionKind::OpenAi => t!("agent.kind_openai_api").into(),
        ConnectionKind::DeepSeek => t!("agent.kind_deepseek").into(),
        ConnectionKind::HuggingFace => t!("agent.kind_huggingface").into(),
        ConnectionKind::Ollama => t!("agent.kind_ollama").into(),
        ConnectionKind::OpenAiCompatible => t!("agent.kind_openai").into(),
        ConnectionKind::Command => t!("agent.kind_command").into(),
    }
}

fn format_label(format: OutputFormat) -> String {
    match format {
        OutputFormat::WhisperJson => "Whisper JSON".into(),
        OutputFormat::OpenAiJson => "OpenAI JSON".into(),
        OutputFormat::Srt => "SRT".into(),
        OutputFormat::Vtt => "VTT".into(),
        OutputFormat::Rttm => "RTTM".into(),
        OutputFormat::Vectors => t!("agent.output_vectors").into(),
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("agent.providers_title"), "");
    let mut changed = false;
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for kind in [
            ConnectionKind::OpenRouter,
            ConnectionKind::OpenAi,
            ConnectionKind::DeepSeek,
            ConnectionKind::HuggingFace,
            ConnectionKind::Ollama,
            ConnectionKind::OpenAiCompatible,
            ConnectionKind::Command,
        ] {
            if ui.button(kind_label(kind)).clicked() {
                let mut connection = Connection::new(kind, &kind_label(kind));
                if kind == ConnectionKind::Command {
                    connection.command = Some(mediaa_core::agent::CommandSpec {
                        program: PathBuf::from("whisper-cli"),
                        args: vec!["-m".into(), "{model}".into(), "-f".into(), "{input}".into()],
                        output: OutputFormat::WhisperJson,
                        whole_file: false,
                    });
                }
                app.settings.agent.connections.push(connection);
                changed = true;
            }
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
    if connection.kind != ConnectionKind::Command {
        if matches!(
            connection.kind,
            ConnectionKind::Ollama | ConnectionKind::OpenAiCompatible
        ) {
            theme::row(ui, &t!("agent.base_url"), |ui| {
                changed |= ui.text_edit_singleline(&mut connection.base_url).changed();
            });
        }
        theme::row(ui, &t!("agent.api_key"), |ui| {
            let shown = state.shown_keys.iter().any(|item| item == id);
            let mut edit = egui::TextEdit::singleline(&mut connection.api_key);
            if !shown {
                edit = edit.password(true);
            }
            changed |= ui.add(edit).changed();
            if ui
                .button(if shown {
                    t!("agent.hide_key")
                } else {
                    t!("agent.show_key")
                })
                .clicked()
            {
                if shown {
                    state.shown_keys.retain(|item| item != id);
                } else {
                    state.shown_keys.push(id.to_string());
                }
            }
        });
        theme::row(ui, &t!("agent.proxy"), |ui| {
            changed |= ui.text_edit_singleline(&mut connection.proxy).changed();
        });
        if connection.kind == ConnectionKind::Ollama {
            ollama(ui, connection, state, id);
        }
    } else if let Some(command) = connection.command.as_mut() {
        theme::row(ui, &t!("agent.command_program"), |ui| {
            let mut program = command.program.display().to_string();
            if ui.text_edit_singleline(&mut program).changed() {
                command.program = PathBuf::from(program);
                changed = true;
            }
        });
        theme::row(ui, &t!("agent.command_args"), |ui| {
            let mut args = command.args.join("\n");
            if ui.text_edit_multiline(&mut args).changed() {
                command.args = args.lines().map(str::to_string).collect();
                changed = true;
            }
        });
        theme::row(ui, &t!("agent.command_output"), |ui| {
            for format in [
                OutputFormat::WhisperJson,
                OutputFormat::OpenAiJson,
                OutputFormat::Srt,
                OutputFormat::Vtt,
                OutputFormat::Rttm,
                OutputFormat::Vectors,
            ] {
                if ui
                    .selectable_label(command.output == format, format_label(format))
                    .clicked()
                {
                    command.output = format;
                    changed = true;
                }
            }
        });
        changed |= ui
            .checkbox(&mut command.whole_file, t!("agent.command_whole"))
            .changed();
        theme::hint(ui, t!("agent.command_placeholders"));
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
                changed |= ui.text_edit_singleline(&mut web.api_key).changed();
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
