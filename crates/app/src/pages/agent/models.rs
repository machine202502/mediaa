//! Models: which model does each job (search embeddings, agent, sub-agent, speech recognition
//! for Russian and English, diarization, speech synthesis) — a Hugging Face model of the engine,
//! a local file or folder it can open, or a provider's model — and "Check models".
//!
//! Local models belong to the engine: the repository is inspected before it is downloaded, the
//! download shows its progress, and a downloaded model can be deleted.

use super::super::header;
use super::engine_state::{download_bar, gb};
use crate::app::App;
use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_agent::assistant::hf_token;
use mediaa_agent::check::{Ready, Status};
use mediaa_engine::{Engine, Health, ModelFormat, ModelInspect, ModelInfo, ProgressCallback};
use mediaa_core::agent::{AgentSettings, Device, ModelRole, ModelSource};
use mediaa_core::library::human_size;
use rust_i18n::t;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long the repository field must stay unchanged before the engine is asked about it.
const INSPECT_DEBOUNCE: Duration = Duration::from_secs(1);

/// The answer of an inspect request and the repository it belongs to.
type InspectAnswer = (String, Result<ModelInspect, String>);

/// The repository field of one role: asks the engine what the repository is once typing has
/// stopped (a request per pause, not per keystroke).
#[derive(Default)]
struct Inspect {
    /// The repository currently in the field.
    spec: Option<String>,
    /// When it last changed (the debounce's anchor; `None` while a request runs).
    edited: Option<Instant>,
    answer: Option<Result<ModelInspect, String>>,
    run: Option<Receiver<InspectAnswer>>,
}

impl Inspect {
    /// Feeds the field's current value; starts a request when it has been still for
    /// [`INSPECT_DEBOUNCE`] and the engine is reachable. `start` is given a thread's receiver.
    fn tick(&mut self, spec: &str, now: Instant, online: bool, start: impl FnOnce(String) -> Receiver<InspectAnswer>) {
        if let Some(rx) = &self.run {
            match rx.try_recv() {
                Ok((spec, answer)) => {
                    if self.spec.as_deref() == Some(spec.as_str()) {
                        self.answer = Some(answer);
                    }
                    self.run = None;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {}
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.run = None;
                    self.answer = Some(Err(t!("engine.no_answer").to_string()));
                }
            }
        }
        if self.spec.as_deref() != Some(spec) {
            self.spec = Some(spec.to_string());
            self.edited = Some(now);
            self.answer = None;
            self.run = None;
        }
        if !online {
            // Nothing to ask; a failed answer is retried when the engine is back.
            self.run = None;
            if matches!(self.answer, Some(Err(_))) {
                self.answer = None;
            }
            return;
        }
        if self.run.is_none()
            && self.answer.is_none()
            && self.edited.is_some_and(|edited| now.duration_since(edited) >= INSPECT_DEBOUNCE)
            && usable(spec)
        {
            self.run = Some(start(spec.to_string()));
            self.edited = None;
        }
    }

    fn pending(&self, spec: &str) -> bool {
        self.spec.as_deref() == Some(spec) && self.run.is_some()
    }

    fn answer(&self, spec: &str) -> Option<&Result<ModelInspect, String>> {
        if self.spec.as_deref() == Some(spec) {
            self.answer.as_ref()
        } else {
            None
        }
    }

    fn busy(&self) -> bool {
        self.run.is_some()
    }
}

/// A repository the engine can look at: `owner/name`, optionally with a file after it.
fn usable(spec: &str) -> bool {
    let spec = spec.trim();
    !spec.is_empty() && spec.contains('/') && !spec.starts_with('/')
}

/// Whether the inspected model can serve `role` (the engine's own role names).
fn fits(inspect: &ModelInspect, role: ModelRole) -> bool {
    let name = mediaa_engine::role_name(role);
    inspect.roles.iter().any(|r| r == name)
}

fn format_name(format: ModelFormat) -> String {
    match format {
        ModelFormat::Gguf => "GGUF".to_string(),
        ModelFormat::SherpaOnnx => "sherpa-onnx".to_string(),
        ModelFormat::Onnx => "ONNX".to_string(),
        ModelFormat::Other => t!("agent.format_other").to_string(),
    }
}

/// A download running in the background: the engine reports its bytes, the thread its result.
struct Download {
    role: ModelRole,
    /// `(done, total)` bytes, written by the pull's progress callback.
    progress: Arc<Mutex<(u64, u64)>>,
    /// The model's path, or why the download failed.
    result: Receiver<Result<String, String>>,
}

impl Download {
    fn progress(&self) -> (u64, u64) {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A delete running in the background.
struct Delete {
    role: ModelRole,
    result: Receiver<Result<(), String>>,
}

#[derive(Default)]
pub struct ModelsUi {
    run: Option<Receiver<(ModelRole, Status)>>,
    statuses: HashMap<ModelRole, Status>,
    inspects: HashMap<ModelRole, Inspect>,
    downloads: Vec<Download>,
    deletes: Vec<Delete>,
    /// The last download or delete error of a role.
    errors: HashMap<ModelRole, String>,
    /// The model waiting for a delete confirmation: `(role, repository)`.
    confirm_delete: Option<(ModelRole, String)>,
}

impl ModelsUi {
    fn running(&self) -> bool {
        self.run.is_some()
    }

    fn downloading(&self, role: ModelRole) -> Option<&Download> {
        self.downloads.iter().find(|d| d.role == role)
    }

    fn deleting(&self, role: ModelRole) -> bool {
        self.deletes.iter().any(|d| d.role == role)
    }

    fn start_download(&mut self, role: ModelRole, spec: String, engine: Arc<dyn Engine>, hf_token: Option<String>, ctx: egui::Context) {
        let progress = Arc::new(Mutex::new((0, 0)));
        let shared = progress.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _ = std::thread::Builder::new().name("engine-pull".into()).spawn(move || {
            let report: ProgressCallback = Arc::new(move |done: u64, total: u64| {
                if let Ok(mut value) = shared.lock() {
                    *value = (done, total);
                }
                ctx.request_repaint();
            });
            let answer = engine.pull(&spec, hf_token.as_deref(), report).map(|path| path.display().to_string()).map_err(|e| format!("{e:#}"));
            let _ = tx.send(answer);
        });
        self.statuses.remove(&role);
        self.errors.remove(&role);
        self.downloads.push(Download { role, progress, result: rx });
    }

    fn start_delete(&mut self, role: ModelRole, spec: String, engine: Arc<dyn Engine>) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let _ = std::thread::Builder::new().name("engine-delete".into()).spawn(move || {
            let _ = tx.send(engine.delete_model(&spec).map_err(|e| format!("{e:#}")));
        });
        self.statuses.remove(&role);
        self.errors.remove(&role);
        self.deletes.push(Delete { role, result: rx });
    }

    /// Collects finished checks, inspects, downloads and deletes, and starts the debounced
    /// inspects. Returns whether the engine's model list should be re-read.
    fn poll(&mut self, agent: &AgentSettings, engine: &Arc<dyn Engine>, hf_token: Option<String>, online: bool, ctx: &egui::Context) -> bool {
        let mut refresh = false;
        if let Some(rx) = &self.run {
            loop {
                match rx.try_recv() {
                    Ok((role, s)) => {
                        self.statuses.insert(role, s);
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(100));
                        break;
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        self.run = None;
                        break;
                    }
                }
            }
        }

        let now = Instant::now();
        for role in ModelRole::ALL {
            let spec = match agent.models.get(role) {
                Some(ModelSource::HuggingFace { repo }) => repo.trim().to_string(),
                _ => {
                    self.inspects.remove(&role);
                    continue;
                }
            };
            let engine = engine.clone();
            let token = hf_token.clone();
            let entry = self.inspects.entry(role).or_default();
            entry.tick(&spec, now, online, move |spec| {
                let (tx, rx) = crossbeam_channel::bounded(1);
                let _ = std::thread::Builder::new().name("engine-inspect".into()).spawn(move || {
                    let answer = engine.inspect(&spec, token.as_deref()).map_err(|e| format!("{e:#}"));
                    let _ = tx.send((spec, answer));
                });
                rx
            });
        }

        let mut i = 0;
        while i < self.downloads.len() {
            match self.downloads[i].result.try_recv() {
                Ok(Ok(_path)) => {
                    let role = self.downloads[i].role;
                    self.downloads.remove(i);
                    self.statuses.remove(&role);
                    self.errors.remove(&role);
                    refresh = true;
                }
                Ok(Err(e)) => {
                    let role = self.downloads[i].role;
                    self.downloads.remove(i);
                    self.statuses.remove(&role);
                    self.errors.insert(role, t!("agent.download_failed", error = format!("{e:#}")).to_string());
                }
                Err(crossbeam_channel::TryRecvError::Empty) => i += 1,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let role = self.downloads[i].role;
                    self.downloads.remove(i);
                    self.errors.insert(role, t!("engine.no_answer").to_string());
                }
            }
        }
        let mut i = 0;
        while i < self.deletes.len() {
            match self.deletes[i].result.try_recv() {
                Ok(Ok(())) => {
                    let role = self.deletes[i].role;
                    self.deletes.remove(i);
                    self.statuses.remove(&role);
                    self.errors.remove(&role);
                    refresh = true;
                }
                Ok(Err(e)) => {
                    let role = self.deletes[i].role;
                    self.deletes.remove(i);
                    self.errors.insert(role, t!("agent.delete_failed", error = format!("{e:#}")).to_string());
                }
                Err(crossbeam_channel::TryRecvError::Empty) => i += 1,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let role = self.deletes[i].role;
                    self.deletes.remove(i);
                    self.errors.insert(role, t!("engine.no_answer").to_string());
                }
            }
        }

        if !self.downloads.is_empty() || !self.deletes.is_empty() || self.inspects.values().any(Inspect::busy) || self.run.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        refresh
    }
}

/// `owner/name` of a repository spec (a file after it is not part of the repository name).
fn repo_id(repo: &str) -> String {
    repo.trim().trim_matches('/').split('/').take(2).collect::<Vec<_>>().join("/")
}

/// Whether the engine has the Hugging Face repository `repo`.
fn is_downloaded<'a>(repo: &str, downloaded: impl IntoIterator<Item = &'a str>) -> bool {
    let id = repo_id(repo);
    downloaded.into_iter().any(|spec| spec == id)
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

/// What kind of repository fits the role (shown under the repository field).
fn role_fit_hint(r: ModelRole) -> String {
    match r {
        ModelRole::Embedding => t!("agent.fit_embedding"),
        ModelRole::Reranker => t!("agent.fit_reranker"),
        ModelRole::Agent => t!("agent.fit_agent"),
        ModelRole::Subagent => t!("agent.fit_agent"),
        ModelRole::Titles => t!("agent.fit_titles"),
        ModelRole::SpeechRecognition => t!("agent.fit_speech_rec"),
        ModelRole::Diarization => t!("agent.fit_diarization"),
        ModelRole::Speech => t!("agent.fit_speech"),
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
        Status::Ready(r) => {
            let text = match r {
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

/// The engine's state: it is this program, so there is nothing to set up here.
fn engine_card(ui: &mut egui::Ui, health: &Option<Health>, loading: bool, error: Option<&str>) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::ENGINE, &t!("engine.title"));
        theme::row(ui, &t!("engine.status"), |ui| {
            let p = theme::palette(ui);
            if loading {
                ui.spinner();
                theme::hint(ui, t!("engine.checking"));
            } else if let Some(h) = health {
                ui.label(RichText::new(format!("{}  {}", icon::CHECK_CIRCLE, t!("engine.running"))).color(p.success));
                theme::hint(ui, format!("{} {}", h.name, h.version));
            } else {
                let text = t!("engine.failed", error = error.unwrap_or_default());
                ui.add(egui::Label::new(RichText::new(format!("{}  {text}", icon::WARNING)).color(p.danger)).wrap());
            }
        });
        if let Some(h) = health {
            if !h.models_dir.is_empty() {
                theme::row(ui, &t!("engine.models_dir"), |ui| theme::hint(ui, &h.models_dir));
            }
            if h.gpus.is_empty() {
                theme::row(ui, &t!("engine.gpus"), |ui| theme::hint(ui, t!("engine.no_gpus")));
            } else {
                for g in &h.gpus {
                    theme::row(ui, &t!("engine.gpus"), |ui| {
                        ui.label(RichText::new(&g.name).strong());
                        theme::hint(
                            ui,
                            t!("engine.gpu_line", backend = g.backend, total = gb(g.memory_total), free = gb(g.memory_free)),
                        );
                    });
                }
            }
        }
    });
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

/// What a role's Hugging Face row shows: its repository, whether the engine already has it, and
/// whether the engine is reachable and not busy with a check.
struct RoleRow<'a> {
    role: ModelRole,
    repo: &'a str,
    downloaded: bool,
    online: bool,
    checking: bool,
}

/// The Hugging Face part of a role's card: what fits, what the engine found in the repository,
/// and Download or Delete.
fn repository_rows(app: &mut App, ui: &mut egui::Ui, row: RoleRow, engine: &Arc<dyn Engine>) {
    let RoleRow { role, repo, downloaded, online, checking } = row;
    let spec = repo.trim().to_string();
    theme::row(ui, "", |ui| theme::hint(ui, role_fit_hint(role)));
    let inspect = app.agent.models.inspects.get(&role);
    if let Some(answer) = inspect.and_then(|i| i.answer(&spec)) {
        let p = theme::palette(ui);
        match answer {
            Ok(found) => {
                theme::row(ui, "", |ui| {
                    theme::hint(ui, t!("agent.inspect_info", format = format_name(found.format), size = human_size(found.download_bytes)));
                });
                theme::row(ui, "", |ui| {
                    if fits(found, role) {
                        ui.label(RichText::new(format!("{}  {}", icon::CHECK, t!("agent.inspect_fits"))).color(p.success));
                    } else {
                        let reason = if found.problems.is_empty() {
                            t!("agent.inspect_no_problems").to_string()
                        } else {
                            found.problems.join("; ")
                        };
                        ui.add(egui::Label::new(RichText::new(format!("{}  {}", icon::WARNING, t!("agent.inspect_unfit", reason = reason))).color(p.danger)).wrap());
                    }
                });
            }
            Err(e) => {
                theme::row(ui, "", |ui| {
                    ui.add(egui::Label::new(RichText::new(format!("{}  {}", icon::WARNING, t!("agent.inspect_failed", error = e))).color(p.danger)).wrap());
                });
            }
        }
    } else if inspect.is_some_and(|i| i.pending(&spec)) {
        theme::row(ui, "", |ui| {
            ui.spinner();
            theme::hint(ui, t!("agent.inspecting"));
        });
    }

    theme::row(ui, &t!("agent.model"), |ui| {
        if let Some(download) = app.agent.models.downloading(role) {
            let (fill, text) = download_bar(download.progress().0, download.progress().1);
            ui.add(egui::ProgressBar::new(fill).text(text).desired_width(300.0));
        } else if app.agent.models.deleting(role) {
            ui.spinner();
            theme::hint(ui, t!("agent.deleting"));
        } else if downloaded {
            theme::hint(ui, t!("agent.downloaded"));
            if ui.add(theme::danger(ui, icon::TRASH, &t!("agent.delete_model"))).clicked() {
                app.agent.models.confirm_delete = Some((role, repo_id(&spec)));
            }
        } else {
            let button = ui
                .add_enabled(online && !checking, theme::secondary(icon::DOWNLOAD_SIMPLE, &t!("agent.download")))
                .on_disabled_hover_text(t!("engine.not_running"));
            if button.clicked() {
                let token = hf_token(&app.settings.agent);
                app.agent.models.start_download(role, spec.clone(), engine.clone(), token, ui.ctx().clone());
            }
        }
    });
    if let Some(e) = app.agent.models.errors.get(&role) {
        let p = theme::palette(ui);
        ui.add(egui::Label::new(RichText::new(format!("{}  {e}", icon::WARNING)).color(p.danger)).wrap());
    }
}

fn delete_dialog(app: &mut App, ctx: &egui::Context) {
    let Some((role, spec)) = app.agent.models.confirm_delete.clone() else { return };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("model-delete")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(RichText::new(format!("{}  {}", icon::TRASH, t!("agent.model_delete_title"))).size(15.0).strong());
        ui.label(t!("agent.model_delete_text", name = spec));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add(theme::danger(ui, icon::TRASH, &t!("agent.model_delete_confirm"))).clicked() {
                decision = Some(true);
            }
            if ui.add(theme::secondary(icon::X, &t!("agent.model_delete_keep"))).clicked() {
                decision = Some(false);
            }
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => {
            app.agent.models.confirm_delete = None;
            let engine = app.agent.engine().clone();
            app.agent.models.start_delete(role, spec, engine);
        }
        Some(false) => app.agent.models.confirm_delete = None,
        None => {}
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let engine = app.agent.engine().clone();
    let online = app.agent.engine_state.online();
    let token = hf_token(&app.settings.agent);
    if app.agent.models.poll(&app.settings.agent, &engine, token.clone(), online, ui.ctx()) {
        app.agent.engine_state.refresh_now();
    }
    // The engine state of this frame (the poller's last answer).
    let health = app.agent.engine_state.health().cloned();
    let loading = app.agent.engine_state.loading();
    let engine_error = app.agent.engine_state.error().map(str::to_string);
    let downloaded: HashSet<String> = app.agent.engine_state.models().iter().map(|m: &ModelInfo| m.spec.clone()).collect();

    header(ui, &t!("agent.models_title"), "");
    engine_card(ui, &health, loading, engine_error.as_deref());
    ui.add_space(6.0);
    let mut changed = false;

    let running = app.agent.models.running();
    for role in ModelRole::ALL {
        let source = app.settings.agent.models.get(role).clone();
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
            if let Some(ModelSource::HuggingFace { repo }) = source {
                let downloaded = is_downloaded(&repo, downloaded.iter().map(String::as_str));
                repository_rows(app, ui, RoleRow { role, repo: &repo, downloaded, online, checking: running }, &engine);
            }
            // The check's answer; a download of this role hides it (the download is what the
            // role is doing then).
            let busy = app.agent.models.downloading(role).is_some() || app.agent.models.deleting(role);
            if !busy {
                if let Some(s) = app.agent.models.statuses.get(&role) {
                    theme::row(ui, &t!("agent.status"), |ui| status_line(ui, s));
                }
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
        let busy = running || !app.agent.models.downloads.is_empty();
        let btn = theme::primary(ui, icon::CHECK_CIRCLE, &t!("agent.check_models")).min_size(egui::vec2(240.0, 38.0));
        if ui.add_enabled(!busy, btn).on_hover_text(t!("agent.check_models_hint")).clicked() {
            let agent = app.settings.agent.clone();
            let engine = engine.clone();
            let (tx, rx) = crossbeam_channel::unbounded();
            let ctx = ui.ctx().clone();
            let _ = std::thread::Builder::new().name("check-models".into()).spawn(move || {
                mediaa_agent::check::check_all(&agent, engine.as_ref(), move |role, s| {
                    let _ = tx.send((role, s));
                    ctx.request_repaint();
                });
            });
            app.agent.models.statuses.clear();
            app.agent.models.run = Some(rx);
        }
        if busy {
            ui.spinner();
        }
    });
    delete_dialog(app, ui.ctx());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::Sender;

    fn found(roles: &[&str], problems: &[&str]) -> ModelInspect {
        ModelInspect {
            spec: "owner/repo".into(),
            files: Vec::new(),
            format: ModelFormat::Gguf,
            download_bytes: 1 << 30,
            roles: roles.iter().map(|r| (*r).to_string()).collect(),
            problems: problems.iter().map(|p| (*p).to_string()).collect(),
        }
    }

    fn channel_for(spec: String, out: &mut Vec<Sender<InspectAnswer>>) -> Receiver<InspectAnswer> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        out.push(tx);
        let _ = spec;
        rx
    }

    #[test]
    fn a_repository_fits_the_roles_the_engine_names() {
        let inspect = found(&["agent", "embedding"], &[]);
        assert!(fits(&inspect, ModelRole::Agent));
        assert!(fits(&inspect, ModelRole::Embedding));
        assert!(!fits(&inspect, ModelRole::SpeechRecognition));
        assert!(usable("owner/repo"));
        assert!(usable(" owner/repo/file.gguf "));
        assert!(!usable(""));
        assert!(!usable("owner"));
        assert_eq!(repo_id(" owner/repo/path/file "), "owner/repo");
        assert!(is_downloaded("owner/repo/path/file.gguf", ["owner/repo"]));
        assert!(!is_downloaded("owner/other", ["owner/repo"]));
    }

    #[test]
    fn inspect_waits_for_typing_to_stop() {
        let mut inspect = Inspect::default();
        let t0 = Instant::now();
        let mut requests = Vec::new();
        inspect.tick("owner/repo", t0, true, |spec| channel_for(spec, &mut requests));
        inspect.tick("owner/repo", t0 + Duration::from_millis(999), true, |spec| channel_for(spec, &mut requests));
        assert!(requests.is_empty(), "not while the user is still typing");
        inspect.tick("owner/repo", t0 + INSPECT_DEBOUNCE, true, |spec| channel_for(spec, &mut requests));
        assert_eq!(requests.len(), 1);
        assert!(inspect.pending("owner/repo"));

        requests[0].send(("owner/repo".into(), Ok(found(&["agent"], &[])))).unwrap();
        inspect.tick("owner/repo", t0 + Duration::from_secs(2), true, |spec| channel_for(spec, &mut requests));
        assert!(!inspect.pending("owner/repo"));
        assert!(inspect.answer("owner/repo").is_some_and(Result::is_ok));

        // Another repository: the old answer is dropped and the debounce starts over.
        inspect.tick("owner/other", t0 + Duration::from_secs(3), true, |spec| channel_for(spec, &mut requests));
        assert!(inspect.answer("owner/other").is_none());
        inspect.tick("owner/other", t0 + Duration::from_millis(3500), true, |spec| channel_for(spec, &mut requests));
        assert_eq!(requests.len(), 1, "the debounce starts from the edit");
        inspect.tick("owner/other", t0 + Duration::from_secs(4), true, |spec| channel_for(spec, &mut requests));
        assert_eq!(requests.len(), 2);
    }

    #[test]
    fn inspect_waits_for_the_engine() {
        let mut inspect = Inspect::default();
        let t0 = Instant::now();
        let mut requests = Vec::new();
        inspect.tick("owner/repo", t0, false, |spec| channel_for(spec, &mut requests));
        inspect.tick("owner/repo", t0 + Duration::from_secs(5), false, |spec| channel_for(spec, &mut requests));
        assert!(requests.is_empty(), "nothing is asked while the engine does not answer");
        inspect.tick("owner/repo", t0 + Duration::from_secs(6), true, |spec| channel_for(spec, &mut requests));
        assert_eq!(requests.len(), 1, "asked at once when the engine is back");
    }
}
