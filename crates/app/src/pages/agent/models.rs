//! Models: which model does each job (search embeddings, agent, sub-agent, speech recognition
//! for Russian and English, diarization, speech synthesis) — a Hugging Face model downloaded
//! into `.ai/models`, a local file or folder, or a provider's model — and "Check models".

use super::super::header;
use crate::app::App;
use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_agent::check::{Ready, Status};
use mediaa_core::agent::{Device, ModelRole, ModelSource};
use mediaa_core::catalogs::ai_dir;
use mediaa_core::library::human_size;
use rust_i18n::t;
use std::collections::HashMap;

#[derive(Default)]
pub struct ModelsUi {
    run: Option<Receiver<(ModelRole, Status)>>,
    statuses: HashMap<ModelRole, Status>,
}

impl ModelsUi {
    fn running(&self) -> bool {
        self.run.is_some()
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.run else { return };
        loop {
            match rx.try_recv() {
                Ok((role, s)) => {
                    self.statuses.insert(role, s);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.run = None;
                    break;
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
    /// Optional roles: unset (the sub-agent then uses the main agent's model).
    Unset,
    HuggingFace,
    Local,
    Provider,
}

fn role_title(r: ModelRole) -> String {
    match r {
        ModelRole::Embedding => t!("agent.role_embedding"),
        ModelRole::Reranker => t!("agent.role_reranker"),
        ModelRole::Agent => t!("agent.role_agent"),
        ModelRole::Subagent => t!("agent.role_subagent"),
        ModelRole::Titles => t!("agent.role_titles"),
        ModelRole::SpeechRecognition => t!("agent.role_speech_rec"),
        ModelRole::Diarization => t!("agent.role_diarization"),
        ModelRole::Speech => t!("agent.role_speech"),
    }
    .into()
}

fn role_hint(r: ModelRole) -> String {
    match r {
        ModelRole::Embedding => t!("agent.hint_embedding"),
        ModelRole::Reranker => t!("agent.hint_reranker"),
        ModelRole::Agent => t!("agent.hint_agent"),
        ModelRole::Subagent => t!("agent.hint_subagent"),
        ModelRole::Titles => t!("agent.hint_titles"),
        ModelRole::SpeechRecognition => t!("agent.hint_speech_rec"),
        ModelRole::Diarization => t!("agent.hint_diarization"),
        ModelRole::Speech => t!("agent.hint_speech"),
    }
    .into()
}

fn role_icon(r: ModelRole) -> &'static str {
    match r {
        ModelRole::Embedding => icon::MAGNIFYING_GLASS,
        ModelRole::Reranker => icon::SORT_DESCENDING,
        ModelRole::Agent => icon::SPARKLE,
        ModelRole::Subagent => icon::USERS_THREE,
        ModelRole::Titles => icon::TEXT_T,
        ModelRole::SpeechRecognition => icon::MICROPHONE,
        ModelRole::Diarization => icon::USER_SWITCH,
        ModelRole::Speech => icon::SPEAKER_HIGH,
    }
}

fn status_line(ui: &mut egui::Ui, s: &Status) {
    let p = theme::palette(ui);
    match s {
        Status::Checking => {
            ui.spinner();
            theme::hint(ui, t!("agent.checking"));
        }
        Status::Downloading { done, total } => {
            let frac = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
            ui.add(egui::ProgressBar::new(frac).desired_width(220.0).text(format!("{} / {}", human_size(*done), human_size(*total))));
        }
        Status::Ready(r) => {
            let text = match r {
                Ready::Files { files, bytes } => t!("agent.ready_files", n = files, size = human_size(*bytes)).to_string(),
                Ready::Ran { bytes } => t!("agent.ready_ran", size = human_size(*bytes)).to_string(),
                Ready::Provider => t!("agent.ready_provider").into(),
                Ready::NotUsed => t!("agent.ready_unused").into(),
                Ready::SameAsAgent => t!("agent.ready_same").into(),
            };
            ui.label(RichText::new(format!("{}  {text}", icon::CHECK_CIRCLE)).color(p.success));
        }
        Status::Failed(e) => {
            ui.add(egui::Label::new(RichText::new(format!("{}  {e}", icon::WARNING)).color(p.danger)).wrap());
        }
    }
}

/// The source editor of one role; returns whether it changed.
fn source_editor(app: &mut App, ui: &mut egui::Ui, role: ModelRole) -> bool {
    let mut changed = false;
    let providers = app.settings.agent.providers.clone();
    let current = app.settings.agent.models.get(role).clone();
    let mut choice = match &current {
        None => Choice::Unset,
        Some(ModelSource::HuggingFace { .. }) => Choice::HuggingFace,
        Some(ModelSource::Local { .. }) => Choice::Local,
        Some(ModelSource::Provider { .. }) => Choice::Provider,
    };
    let before = choice;
    theme::row(ui, &t!("agent.source"), |ui| {
        let mut opts = vec![
            (Choice::HuggingFace, format!("{}  Hugging Face", icon::CLOUD_ARROW_DOWN)),
            (Choice::Local, format!("{}  {}", icon::HARD_DRIVES, t!("agent.source_local"))),
            (Choice::Provider, format!("{}  {}", icon::PLUGS_CONNECTED, t!("agent.source_provider"))),
        ];
        if !role.required() {
            let unset = if role == ModelRole::Subagent { t!("agent.same_as_agent") } else { t!("agent.not_used") };
            opts.insert(0, (Choice::Unset, unset.into()));
        }
        theme::segmented(ui, &mut choice, &opts);
    });
    if choice != before {
        *app.settings.agent.models.get_mut(role) = match choice {
            Choice::Unset => None,
            Choice::HuggingFace => Some(role.default_source().unwrap_or(ModelSource::HuggingFace { repo: String::new() })),
            Choice::Local => Some(ModelSource::Local { path: Default::default() }),
            Choice::Provider => Some(ModelSource::Provider { provider: providers.first().map(|p| p.id.clone()).unwrap_or_default(), model: String::new() }),
        };
        changed = true;
    }

    let ai = ai_dir(&super::super::library::library_dir(&app.settings));
    let known: HashMap<String, Vec<String>> =
        providers.iter().filter_map(|p| app.agent.providers.known_models(&p.id).map(|m| (p.id.clone(), m.to_vec()))).collect();
    match app.settings.agent.models.get_mut(role) {
        None => {}
        Some(ModelSource::HuggingFace { repo }) => {
            theme::row(ui, &t!("agent.repo"), |ui| {
                changed |= ui.add(egui::TextEdit::singleline(repo).hint_text("owner/name").desired_width(320.0)).changed();
                if let Some(ModelSource::HuggingFace { repo: def }) = role.default_source() {
                    if *repo != def && theme::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, &t!("agent.default_model", name = def)).clicked() {
                        *repo = def;
                        changed = true;
                    }
                }
                if mediaa_agent::hub::is_downloaded(&ai, repo) {
                    theme::hint(ui, t!("agent.downloaded"));
                }
            });
        }
        Some(ModelSource::Local { path }) => {
            theme::row(ui, &t!("agent.path"), |ui| {
                let mut text = path.to_string_lossy().into_owned();
                if ui.add(egui::TextEdit::singleline(&mut text).hint_text(t!("agent.path_hint")).desired_width(360.0)).changed() {
                    *path = text.into();
                    changed = true;
                }
                // One button: a model is a weights file or a folder of them.
                let browse = theme::icon_button(ui, icon::FOLDER_OPEN, &t!("agent.browse"));
                let picked = egui::Popup::menu(&browse)
                    .show(|ui| {
                        let mut picked = None;
                        if theme::menu_item(ui, icon::FILE, &t!("agent.pick_file"), false).clicked() {
                            picked = rfd::FileDialog::new().add_filter(t!("agent.model_files"), &["safetensors", "gguf", "onnx", "bin", "pt", "pth"]).pick_file();
                            ui.close();
                        }
                        if theme::menu_item(ui, icon::FOLDER, &t!("agent.pick_folder"), false).clicked() {
                            picked = rfd::FileDialog::new().pick_folder();
                            ui.close();
                        }
                        picked
                    })
                    .and_then(|r| r.inner);
                if let Some(p) = picked {
                    *path = p;
                    changed = true;
                }
            });
        }
        Some(ModelSource::Provider { provider, model }) => {
            if providers.is_empty() {
                theme::row(ui, "", |ui| theme::hint(ui, t!("agent.add_provider_first")));
            } else {
                theme::row(ui, &t!("agent.source_provider"), |ui| {
                    let name = providers.iter().find(|p| p.id == *provider).map_or_else(|| t!("agent.choose").to_string(), |p| p.name.clone());
                    egui::ComboBox::from_id_salt(("model-provider", role as u8)).selected_text(name).show_ui(ui, |ui| {
                        for p in &providers {
                            changed |= ui.selectable_value(provider, p.id.clone(), &p.name).changed();
                        }
                    });
                });
                theme::row(ui, &t!("agent.model"), |ui| {
                    changed |= ui.add(egui::TextEdit::singleline(model).hint_text("gpt-4o-mini").desired_width(280.0)).changed();
                    // Models the provider listed at its last check.
                    if let Some(list) = known.get(provider.as_str()) {
                        egui::ComboBox::from_id_salt(("model-pick", role as u8)).selected_text(t!("agent.pick_model")).width(160.0).show_ui(ui, |ui| {
                            for m in list {
                                if ui.selectable_label(m == model, m).clicked() {
                                    *model = m.clone();
                                    changed = true;
                                }
                            }
                        });
                    }
                });
            }
        }
    }
    changed
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    app.agent.models.poll(ui.ctx());
    header(ui, &t!("agent.models_title"), "");
    let running = app.agent.models.running();
    let mut changed = false;
    for role in ModelRole::ALL {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                // What the role does: on hover over its title.
                ui.scope(|ui| theme::section(ui, role_icon(role), &role_title(role))).response.on_hover_text(role_hint(role));
                if role.required() {
                    ui.label(RichText::new(t!("agent.required")).size(11.5).color(theme::palette(ui).accent));
                }
            });
            ui.add_enabled_ui(!running, |ui| {
                changed |= source_editor(app, ui, role);
                // Local models: where they run (the runtime falls back to the CPU).
                if matches!(app.settings.agent.models.get(role), Some(ModelSource::HuggingFace { .. } | ModelSource::Local { .. })) {
                    theme::row(ui, &t!("agent.device"), |ui| {
                        let opts = [
                            (Device::Auto, t!("agent.device_auto").to_string()),
                            (Device::Gpu, format!("{}  GPU", icon::GRAPHICS_CARD)),
                            (Device::Cpu, format!("{}  CPU", icon::CPU)),
                        ];
                        ui.scope(|ui| changed |= theme::segmented(ui, app.settings.agent.devices.get_mut(role), &opts))
                            .response
                            .on_hover_text(t!("agent.device_hint"));
                    });
                    if role == ModelRole::Agent {
                        theme::row(ui, &t!("agent.context"), |ui| {
                            changed |= ui
                                .add(egui::DragValue::new(&mut app.settings.agent.agent_context).range(2048..=262_144).speed(256).suffix(t!("agent.tokens")))
                                .on_hover_text(t!("agent.context_hint"))
                                .changed();
                        });
                    }
                }
            });
            if let Some(s) = app.agent.models.statuses.get(&role) {
                theme::row(ui, &t!("agent.status"), |ui| status_line(ui, s));
            }
        });
        ui.add_space(6.0);
    }
    if changed {
        // Results of an earlier check no longer apply to the changed choices.
        app.agent.models.statuses.clear();
        app.settings_changed();
    }
    ui.add_space(8.0);
    ui.vertical_centered(|ui| {
        let btn = theme::primary(ui, icon::CHECK_CIRCLE, &t!("agent.check_models")).min_size(egui::vec2(240.0, 38.0));
        if ui.add_enabled(!running, btn).on_hover_text(t!("agent.check_models_hint")).clicked() {
            let agent = app.settings.agent.clone();
            let ai = ai_dir(&super::super::library::library_dir(&app.settings));
            let (tx, rx) = crossbeam_channel::unbounded();
            let ctx = ui.ctx().clone();
            let _ = std::thread::Builder::new().name("check-models".into()).spawn(move || {
                mediaa_agent::check::check_all(&agent, &ai, move |role, s| {
                    let _ = tx.send((role, s));
                    ctx.request_repaint();
                });
            });
            app.agent.models.statuses.clear();
            app.agent.models.run = Some(rx);
        }
        if running {
            ui.spinner();
        }
    });
}
