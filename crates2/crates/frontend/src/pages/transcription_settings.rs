use super::Ctx;
use crate::ui::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa2_core::agent::{
    Connection, ConnectionKind, ModelRole, ModelSource, WebEngine, GROUP_LIMIT_MAX,
};
use mediaa2_i18n::t;
use mediaa2_protocol::{ClientCommand, TranscriptionCommand};

fn kind_label(kind: ConnectionKind) -> String {
    match kind {
        ConnectionKind::OpenRouter => t!("transcription.kind_openrouter").into(),
        ConnectionKind::DeepSeek => t!("transcription.kind_deepseek").into(),
        ConnectionKind::HuggingFace => t!("transcription.kind_huggingface").into(),
        ConnectionKind::Ollama => t!("transcription.kind_ollama").into(),
        ConnectionKind::OpenAiCompatible => t!("transcription.kind_openai").into(),
        ConnectionKind::SpeechRecognition => t!("transcription.kind_transcription").into(),
    }
}

const ADD_KINDS: [ConnectionKind; 6] = [
    ConnectionKind::OpenRouter,
    ConnectionKind::DeepSeek,
    ConnectionKind::HuggingFace,
    ConnectionKind::Ollama,
    ConnectionKind::OpenAiCompatible,
    ConnectionKind::SpeechRecognition,
];

pub fn providers(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    ui.heading(t!("transcription.providers_title"));
    ui.add_space(4.0);

    let agent = ctx.view.state.settings.agent.clone();
    let mut remove = None;

    // add row
    ui.horizontal(|ui| {
        let mut picked = ctx.ui.transcription.add_kind;
        theme::combo_box("add-provider")
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
        ctx.ui.transcription.add_kind = picked;
        if ui.button(icon::PLUS.to_string()).clicked() {
            let connection = Connection::new(picked, &kind_label(picked));
            ctx.actions.send(ClientCommand::Transcription(
                TranscriptionCommand::ProviderSave {
                    connection,
                    key: None,
                },
            ));
        }
    });

    // edit each connection from the backend state; edits go back as ProviderSave
    let connections = agent.connections.clone();
    let groups: Vec<String> = agent.groups.iter().map(|g| g.name.clone()).collect();
    for connection in &connections {
        let mut draft = connection.clone();
        let mut key_draft = String::new();
        let id = draft.id.clone();
        ui.add_space(6.0);
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(kind_label(draft.kind)).strong());
                if ui.button(t!("transcription.remove_provider")).clicked() {
                    remove = Some(id.clone());
                }
            });
            theme::row(ui, &t!("transcription.provider_name"), |ui| {
                if ui.text_edit_singleline(&mut draft.name).changed() {
                    save(ctx, &draft, None);
                }
            });
            theme::row(ui, &t!("transcription.provider_pool"), |ui| {
                let selected = draft.group.clone();
                let label = if selected.is_empty() {
                    t!("transcription.provider_no_pool").into_owned()
                } else {
                    selected.clone()
                };
                theme::combo_box(("provider-group", &id))
                    .selected_text(label)
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                selected.is_empty(),
                                t!("transcription.provider_no_pool"),
                            )
                            .clicked()
                        {
                            draft.group.clear();
                            save(ctx, &draft, None);
                        }
                        for group in &groups {
                            if ui.selectable_label(selected == *group, group).clicked() {
                                draft.group = group.clone();
                                save(ctx, &draft, None);
                            }
                        }
                    });
            });
            if !matches!(
                draft.kind,
                ConnectionKind::OpenRouter | ConnectionKind::DeepSeek | ConnectionKind::HuggingFace
            ) {
                theme::row(ui, &t!("transcription.base_url"), |ui| {
                    if ui.text_edit_singleline(&mut draft.base_url).changed() {
                        save(ctx, &draft, None);
                    }
                });
            }
            theme::row(ui, &t!("transcription.api_key"), |ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut key_draft)
                        .password(true)
                        .hint_text(t!("transcription.api_key_kept")),
                );
                if response.lost_focus() && !key_draft.trim().is_empty() {
                    save(ctx, &draft, Some(key_draft.trim().to_string()));
                }
            });
            theme::row(ui, &t!("transcription.proxy"), |ui| {
                if ui.text_edit_singleline(&mut draft.proxy).changed() {
                    save(ctx, &draft, None);
                }
            });
            if draft.kind == ConnectionKind::Ollama {
                ollama(ctx, ui, &draft);
            }
        });
    }

    if let Some(id) = remove {
        ctx.actions.send(ClientCommand::Transcription(
            TranscriptionCommand::ProviderDelete { id },
        ));
    }
    ui.add_space(8.0);
}

fn save(ctx: &mut Ctx<'_>, connection: &Connection, key: Option<String>) {
    ctx.actions.send(ClientCommand::Transcription(
        TranscriptionCommand::ProviderSave {
            connection: connection.clone(),
            key,
        },
    ));
}

fn ollama(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, connection: &Connection) {
    let id = &connection.id;
    let checking = ctx.ui.transcription.checking.contains(id);
    if checking {
        ui.spinner();
    }
    if ui.button(t!("transcription.check")).clicked() {
        ctx.ui.transcription.checking.insert(id.clone());
        ctx.actions.send(ClientCommand::Transcription(
            TranscriptionCommand::ProviderCheck {
                connection: connection.clone(),
                key: None,
            },
        ));
    }
    match ctx.ui.transcription.checks.get(id) {
        Some(Ok(ok)) => theme::hint(ui, ok.clone()),
        _ => {
            if !checking && ui.button(t!("transcription.ollama_serve")).clicked() {
                ctx.actions.send(ClientCommand::Transcription(
                    TranscriptionCommand::OllamaServe,
                ));
            }
        }
    }
}

pub fn models(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    ui.heading(t!("transcription.models_title"));
    ui.add_space(4.0);

    let mut agent = ctx.view.state.settings.agent.clone();
    let connections = agent.connections.clone();
    for role in ModelRole::ALL {
        ui.add_space(6.0);
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, icon::CUBE, &role_label(role));
            let fitting: Vec<&Connection> = connections
                .iter()
                .filter(|connection| connection.supports(role.capability()))
                .collect();
            let current = agent.models.get(role).clone();
            let mut selected = current
                .as_ref()
                .map(|source| source.connection.clone())
                .unwrap_or_default();
            theme::combo_box(("connection", role as u8))
                .selected_text(if selected.is_empty() {
                    t!("transcription.model_none").into_owned()
                } else {
                    fitting
                        .iter()
                        .find(|connection| connection.id == selected)
                        .map(|connection| connection.name.clone())
                        .unwrap_or_else(|| selected.clone())
                })
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(selected.is_empty(), t!("transcription.model_none"))
                        .clicked()
                    {
                        selected.clear();
                        *agent.models.get_mut(role) = None;
                        set(ctx, &agent);
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
                            *agent.models.get_mut(role) =
                                Some(ModelSource::new(connection.id.clone(), model));
                            set(ctx, &agent);
                        }
                    }
                });
            if fitting.is_empty() {
                theme::hint(ui, t!("transcription.no_connection_for_role"));
            }
            let mut source = agent.models.get(role).clone();
            if let Some(src) = &mut source {
                theme::row(ui, &t!("transcription.model_name"), |ui| {
                    if ui.text_edit_singleline(&mut src.model).changed() {
                        agent.models.set(role, Some(src.clone()));
                        set(ctx, &agent);
                    }
                });
                let known = ctx.ui.transcription.models.get(&src.connection).cloned();
                if let Some(known) = known {
                    theme::combo_box(("model", role as u8))
                        .selected_text(if src.model.is_empty() {
                            t!("transcription.model_name").into_owned()
                        } else {
                            src.model.clone()
                        })
                        .show_ui(ui, |ui| {
                            for id in known {
                                if ui.selectable_label(src.model == id, &id).clicked() {
                                    src.model = id;
                                    agent.models.set(role, Some(src.clone()));
                                    set(ctx, &agent);
                                }
                            }
                        });
                }
                let connection_id = src.connection.clone();
                let model_name = src.model.clone();
                let listed = ctx
                    .ui
                    .transcription
                    .models
                    .get(&connection_id)
                    .map(|ids| ids.iter().any(|id| id == &model_name))
                    .unwrap_or(false);
                let ollama = connections.iter().any(|connection| {
                    connection.id == connection_id && connection.kind == ConnectionKind::Ollama
                });
                if ollama
                    && !model_name.trim().is_empty()
                    && !listed
                    && ui.button(t!("transcription.pull_model")).clicked()
                {
                    // a download is expensive: check the server list first
                    ctx.ui.transcription.pull_confirm =
                        Some((connection_id.clone(), model_name.clone()));
                    ctx.ui.transcription.models_checking = true;
                    ctx.actions.send(ClientCommand::Transcription(
                        TranscriptionCommand::ModelsCheck,
                    ));
                }
                if matches!(role, ModelRole::Agent | ModelRole::Titles) {
                    theme::row(ui, &t!("transcription.context"), |ui| {
                        let mut text = src.context.to_string();
                        if ui.text_edit_singleline(&mut text).changed() {
                            if let Ok(value) = text.trim().parse::<u32>() {
                                src.context = value;
                                agent.models.set(role, Some(src.clone()));
                                set(ctx, &agent);
                            }
                        }
                    });
                    if ui
                        .checkbox(&mut src.thinking, t!("transcription.thinking"))
                        .changed()
                    {
                        agent.models.set(role, Some(src.clone()));
                        set(ctx, &agent);
                    }
                }
                if role == ModelRole::SpeechRecognition {
                    let transcription = connections.iter().any(|connection| {
                        connection.id == src.connection
                            && connection.kind == ConnectionKind::SpeechRecognition
                    });
                    if ui
                        .checkbox(&mut src.diarize, t!("transcription.speech_diarize"))
                        .changed()
                    {
                        agent.models.set(role, Some(src.clone()));
                        set(ctx, &agent);
                    }
                    if ui
                        .checkbox(&mut src.word_times, t!("transcription.speech_words"))
                        .changed()
                    {
                        agent.models.set(role, Some(src.clone()));
                        set(ctx, &agent);
                    }
                    if transcription
                        && ui
                            .checkbox(&mut src.voiceprint, t!("transcription.speech_voice"))
                            .changed()
                    {
                        agent.models.set(role, Some(src.clone()));
                        set(ctx, &agent);
                    }
                }
            }
        });
    }
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        if ctx.ui.transcription.models_checking {
            ui.spinner();
        }
        if ui.button(t!("transcription.check_models")).clicked() {
            ctx.ui.transcription.models_checking = true;
            ctx.actions.send(ClientCommand::Transcription(
                TranscriptionCommand::ModelsCheck,
            ));
        }
    });
    pull_confirm_modal(ctx, ui);
}

fn pull_confirm_modal(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let Some((connection_id, model)) = ctx.ui.transcription.pull_confirm.clone() else {
        return;
    };
    let known = ctx.ui.transcription.models.get(&connection_id).cloned();
    egui::Modal::new(egui::Id::new("pull-confirm")).show(ui.ctx(), |ui| {
        ui.heading(t!("transcription.pull_confirm_title"));
        ui.add_space(4.0);
        ui.label(t!("transcription.pull_confirm_text", model = model));
        if let Some(known) = &known {
            if known.is_empty() {
                theme::hint(ui, t!("transcription.pull_confirm_empty"));
            } else {
                ui.add_space(4.0);
                ui.strong(t!("transcription.pull_confirm_available"));
                for id in known.iter().take(8) {
                    ui.label(format!("  • {id}"));
                }
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button(t!("transcription.pull_confirm_yes")).clicked() {
                ctx.ui.transcription.pull_confirm = None;
                ctx.actions.send(ClientCommand::Transcription(
                    TranscriptionCommand::OllamaPull { model },
                ));
            }
            if ui.button(t!("transcription.pull_confirm_no")).clicked() {
                ctx.ui.transcription.pull_confirm = None;
            }
        });
    });
}

fn set(ctx: &mut Ctx<'_>, agent: &mediaa2_core::agent::AgentSettings) {
    ctx.actions.send(ClientCommand::Transcription(
        TranscriptionCommand::ModelsSet {
            settings: agent.clone(),
        },
    ));
}

fn role_label(role: ModelRole) -> String {
    match role {
        ModelRole::Embedding => t!("transcription.role_embedding"),
        ModelRole::Reranker => t!("transcription.role_reranker"),
        ModelRole::Agent => t!("transcription.role_agent"),
        ModelRole::Titles => t!("transcription.role_titles"),
        ModelRole::SpeechRecognition => t!("transcription.role_speech_rec"),
    }
    .into()
}

pub fn budget(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let mut agent = ctx.view.state.settings.agent.clone();
    let mut remove = None;

    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.scope(|ui| theme::section(ui, icon::STACK, &t!("transcription.budget_pools")))
            .response
            .on_hover_text(t!("transcription.budget_pools_hint"));
        for index in 0..agent.groups.len() {
            ui.horizontal(|ui| {
                ui.label(RichText::new(&agent.groups[index].name).strong());
                if ui
                    .add(
                        egui::DragValue::new(&mut agent.groups[index].limit)
                            .range(1..=GROUP_LIMIT_MAX),
                    )
                    .on_hover_text(t!("transcription.budget_pools_hint"))
                    .changed()
                {
                    set(ctx, &agent);
                }
                if ui.button(icon::TRASH.to_string()).clicked() {
                    remove = Some(agent.groups[index].name.clone());
                }
            });
        }
        if let Some(name) = remove {
            agent.remove_group(&name);
            set(ctx, &agent);
        }
        ui.add_space(4.0);
        let mut name = ctx.ui.transcription.pool_name.clone();
        let mut limit = ctx.ui.transcription.pool_limit;
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut name)
                    .hint_text(t!("transcription.budget_pool_name"))
                    .desired_width(140.0),
            );
            ui.add(egui::DragValue::new(&mut limit).range(1..=GROUP_LIMIT_MAX))
                .on_hover_text(t!("transcription.budget_pools_hint"));
            if ui.button(t!("transcription.budget_add")).clicked() {
                if agent.add_group(name.trim(), limit) {
                    set(ctx, &agent);
                    name.clear();
                    ctx.ui.transcription.pool_error = None;
                } else {
                    ctx.ui.transcription.pool_error =
                        Some(t!("transcription.budget_pool_error").into_owned());
                }
            }
        });
        ctx.ui.transcription.pool_name = name;
        ctx.ui.transcription.pool_limit = limit;
        if let Some(error) = &ctx.ui.transcription.pool_error {
            theme::hint(ui, error.clone());
        }
    });
    ui.add_space(6.0);
    web(ctx, ui, &mut agent);
}

fn web(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, agent: &mut mediaa2_core::agent::AgentSettings) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.scope(|ui| theme::section(ui, icon::GLOBE, &t!("transcription.budget_tools")))
            .response
            .on_hover_text(t!("transcription.budget_tools_hint"));
        if ui
            .checkbox(&mut agent.web.enabled, t!("transcription.web_enabled"))
            .changed()
        {
            let enabled = agent.web.enabled;
            ctx.actions.send(ClientCommand::Transcription(
                TranscriptionCommand::WebSearchSet { enabled, key: None },
            ));
        }
        theme::row(ui, &t!("transcription.retries"), |ui| {
            if ui
                .add(egui::DragValue::new(&mut agent.retries).range(1..=8))
                .on_hover_text(t!("transcription.retries_hint"))
                .changed()
            {
                set(ctx, agent);
            }
        });
        theme::row(ui, &t!("transcription.budget_web_slots"), |ui| {
            if ui
                .add(egui::DragValue::new(&mut agent.web_limit).range(1..=GROUP_LIMIT_MAX))
                .on_hover_text(t!("transcription.budget_tools_hint"))
                .changed()
            {
                set(ctx, agent);
            }
        });
        theme::row(ui, &t!("transcription.web_engine"), |ui| {
            let mut engine = agent.web.engine;
            theme::combo_box("web-engine")
                .selected_text(engine_label(engine))
                .show_ui(ui, |ui| {
                    for item in [
                        WebEngine::DuckDuckGo,
                        WebEngine::SearXng,
                        WebEngine::Brave,
                        WebEngine::Tavily,
                    ] {
                        if ui
                            .selectable_label(engine == item, engine_label(item))
                            .clicked()
                        {
                            engine = item;
                        }
                    }
                });
            if engine != agent.web.engine {
                agent.web.engine = engine;
                set(ctx, agent);
            }
        });
        if agent.web.engine == WebEngine::SearXng {
            theme::row(ui, &t!("transcription.web_searxng"), |ui| {
                if ui
                    .text_edit_singleline(&mut agent.web.searxng_url)
                    .changed()
                {
                    set(ctx, agent);
                }
            });
        }
        theme::row(ui, &t!("transcription.proxy"), |ui| {
            if ui.text_edit_singleline(&mut agent.web.proxy).changed() {
                set(ctx, agent);
            }
        });
        if matches!(agent.web.engine, WebEngine::Brave | WebEngine::Tavily) {
            theme::row(ui, &t!("transcription.api_key"), |ui| {
                let mut key = String::new();
                let response = ui.add(
                    egui::TextEdit::singleline(&mut key)
                        .password(true)
                        .hint_text(t!("transcription.api_key_kept")),
                );
                if response.lost_focus() && !key.trim().is_empty() {
                    let enabled = agent.web.enabled;
                    ctx.actions.send(ClientCommand::Transcription(
                        TranscriptionCommand::WebSearchSet {
                            enabled,
                            key: Some(key.trim().to_string()),
                        },
                    ));
                }
            });
        }
        if ui.button(t!("transcription.web_check")).clicked() {
            ctx.ui.transcription.web_status = None;
            ctx.actions
                .send(ClientCommand::Transcription(TranscriptionCommand::WebCheck));
        }
        if let Some(status) = &ctx.ui.transcription.web_status {
            match status {
                Ok(text) => theme::hint(ui, text.clone()),
                Err(error) => theme::hint(ui, error.clone()),
            }
        }
    });
}

fn engine_label(engine: WebEngine) -> &'static str {
    match engine {
        WebEngine::DuckDuckGo => "DuckDuckGo",
        WebEngine::SearXng => "SearXNG",
        WebEngine::Brave => "Brave",
        WebEngine::Tavily => "Tavily",
    }
}
