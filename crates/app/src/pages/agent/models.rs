use super::super::header;
use crate::app::App;
use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_agent::assistant::hf_token;
use mediaa_agent::check::{Ready, Status};
use mediaa_engine::{Engine, ModelFormat, ModelInspect, ModelInfo, ProgressCallback};
use mediaa_core::agent::{AgentSettings, Device, ModelRole, ModelSource};
use mediaa_core::library::human_size;
use rust_i18n::t;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const INSPECT_DEBOUNCE: Duration = Duration::from_secs(1);

type InspectAnswer = (String, Result<ModelInspect, String>);

#[derive(Default)]
struct Inspect {
    spec: Option<String>,
    edited: Option<Instant>,
    answer: Option<Result<ModelInspect, String>>,
    run: Option<Receiver<InspectAnswer>>,
}

impl Inspect {
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
                    self.answer = Some(Err(t!("agent.models_no_answer").to_string()));
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

fn usable(spec: &str) -> bool {
    let spec = spec.trim();
    !spec.is_empty() && spec.contains('/') && !spec.starts_with('/')
}

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

struct Download {
    role: ModelRole,
    progress: Arc<Mutex<(u64, u64)>>,
    result: Receiver<Result<String, String>>,
}

impl Download {
    fn progress(&self) -> (u64, u64) {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner())
    }
}

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
    errors: HashMap<ModelRole, String>,
    fetches: HashMap<ModelRole, Fetch>,
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

    fn stop_download(&mut self, role: ModelRole) {
        self.downloads.retain(|d| d.role != role);
        self.fetches.insert(role, Fetch::default());
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
                    self.note(role, FetchInput::Thread(Ok(())));
                    refresh = true;
                }
                Ok(Err(e)) => {
                    let role = self.downloads[i].role;
                    self.downloads.remove(i);
                    self.statuses.remove(&role);
                    self.note(role, FetchInput::Thread(Err(t!("agent.download_failed", error = format!("{e:#}")).to_string())));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    let role = self.downloads[i].role;
                    let (done, total) = self.downloads[i].progress();
                    self.note(role, FetchInput::Progress(done, total));
                    i += 1;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let role = self.downloads[i].role;
                    self.downloads.remove(i);
                    self.note(role, FetchInput::Thread(Err(t!("agent.models_no_answer").to_string())));
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
                    self.fetches.insert(role, Fetch::default());
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
                    self.errors.insert(role, t!("agent.models_no_answer").to_string());
                }
            }
        }

        if !self.downloads.is_empty() || !self.deletes.is_empty() || self.inspects.values().any(Inspect::busy) || self.run.is_some() || self.fetches.values().any(|f| f.held.is_some()) {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        refresh
    }

    fn note(&mut self, role: ModelRole, input: FetchInput) {
        let fetch = self.fetches.remove(&role).unwrap_or_default();
        self.fetches.insert(role, fetch_next(fetch, input));
    }
}

fn repo_id(repo: &str) -> String {
    repo.trim().trim_matches('/').split('/').take(2).collect::<Vec<_>>().join("/")
}

fn is_downloaded<'a>(repo: &str, downloaded: impl IntoIterator<Item = &'a str>) -> bool {
    let id = repo_id(repo);
    downloaded.into_iter().any(|spec| spec == id)
}

const FETCH_SHOW_MS: u128 = 400;
const FETCH_GAP: f32 = 8.0;
const FETCH_TEXT_H: f32 = 18.0;
const FETCH_BAR_H: f32 = 6.0;
const FETCH_ROW_H: f32 = FETCH_TEXT_H + 4.0 + FETCH_BAR_H;
const FETCH_STATUS_W: f32 = 280.0;
const FETCH_BTN_W: f32 = 180.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchState {
    Idle,
    Fetching { done: u64, total: u64 },
    Saved,
    Already,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetch {
    pub state: FetchState,
    pub held: Option<Result<(), String>>,
    pub started_ms: Option<u128>,
}

impl Default for Fetch {
    fn default() -> Fetch {
        Fetch { state: FetchState::Idle, held: None, started_ms: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchInput {
    Press { on_disk: bool, online: bool, offline: String },
    Progress(u64, u64),
    Thread(Result<(), String>),
    Listed,
    Frame { now_ms: u128 },
}

pub fn fetch_next(mut fetch: Fetch, input: FetchInput) -> Fetch {
    match input {
        FetchInput::Press { online: false, offline, .. } => {
            fetch.held = None;
            fetch.started_ms = None;
            fetch.state = FetchState::Failed(offline);
        }
        FetchInput::Press { on_disk: true, .. } => {
            fetch.held = None;
            fetch.started_ms = None;
            fetch.state = FetchState::Already;
        }
        FetchInput::Press { .. } => {
            fetch.held = None;
            fetch.started_ms = None;
            fetch.state = FetchState::Fetching { done: 0, total: 0 };
        }
        FetchInput::Progress(done, total) => {
            if matches!(fetch.state, FetchState::Fetching { .. }) {
                fetch.state = FetchState::Fetching { done, total };
            }
        }
        FetchInput::Thread(result) => {
            if matches!(fetch.state, FetchState::Fetching { .. }) {
                fetch.held = Some(result);
            } else {
                fetch.state = match result {
                    Ok(()) => FetchState::Saved,
                    Err(e) => FetchState::Failed(e),
                };
            }
        }
        FetchInput::Listed => {
            if matches!(fetch.state, FetchState::Idle) {
                fetch.state = FetchState::Already;
            }
        }
        FetchInput::Frame { now_ms } => {
            if matches!(fetch.state, FetchState::Fetching { .. }) && fetch.started_ms.is_none() {
                fetch.started_ms = Some(now_ms);
            }
            if let Some(result) = fetch.held.clone() {
                let shown = fetch.started_ms.is_some_and(|start| now_ms.saturating_sub(start) >= FETCH_SHOW_MS);
                if shown {
                    fetch.held = None;
                    fetch.started_ms = None;
                    fetch.state = match result {
                        Ok(()) => FetchState::Saved,
                        Err(e) => FetchState::Failed(e),
                    };
                }
            }
        }
    }
    fetch
}

fn mono_ms() -> u128 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Choice {
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
        ModelRole::Titles => t!("agent.hint_titles"),
        ModelRole::SpeechRecognition => t!("agent.hint_speech_rec"),
        ModelRole::Diarization => t!("agent.hint_diarization"),
        ModelRole::Speech => t!("agent.hint_speech"),
    }
    .into()
}

fn role_fit_hint(r: ModelRole) -> String {
    match r {
        ModelRole::Embedding => t!("agent.fit_embedding"),
        ModelRole::Reranker => t!("agent.fit_reranker"),
        ModelRole::Agent => t!("agent.fit_agent"),
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
            };
            ui.label(RichText::new(format!("{}  {text}", icon::CHECK_CIRCLE)).color(p.success));
        }
        Status::Failed(e) => {
            ui.add(egui::Label::new(RichText::new(format!("{}  {e}", icon::WARNING)).color(p.danger)).wrap());
        }
    }
}

fn models_folder(app: &mut App, ui: &mut egui::Ui) {
    let dir = mediaa_core::paths::models_dir();
    theme::row(ui, &t!("agent.models_folder"), |ui| {
        theme::hint(ui, dir.display().to_string());
        if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("lib.reveal")).clicked() {
            if let Err(e) = mediaa_platform::files::reveal(&dir) {
                app.error(format!("{e:#}"));
            }
        }
    });
}

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
            let unset = t!("agent.not_used");
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

struct RoleRow<'a> {
    role: ModelRole,
    repo: &'a str,
    downloaded: bool,
    online: bool,
}

fn repository_rows(app: &mut App, ui: &mut egui::Ui, row: RoleRow, engine: &Arc<dyn Engine>) {
    let RoleRow { role, repo, downloaded, online } = row;
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

    let deleting = app.agent.models.deleting(role);
    let mut fetch = app.agent.models.fetches.get(&role).cloned().unwrap_or_default();
    fetch = fetch_next(fetch, FetchInput::Frame { now_ms: mono_ms() });
    if downloaded && !deleting {
        fetch = fetch_next(fetch, FetchInput::Listed);
    }
    let delete_error = app.agent.models.errors.get(&role).cloned();
    let line = delete_error.clone().unwrap_or_else(|| fetch_line(&fetch.state, deleting));
    let click = theme::row(ui, &t!("agent.model"), |ui| fetch_controls(ui, role, &line, &fetch.state, deleting, delete_error.is_some()));
    let mut stopped = false;
    match click {
        FetchClick::Stop => stopped = true,
        FetchClick::Delete => app.agent.models.confirm_delete = Some((role, repo_id(&spec))),
        FetchClick::Download => {
            let offline = app.agent.engine_state.error().unwrap_or(&t!("agent.models_offline")).to_string();
            fetch = fetch_next(fetch, FetchInput::Press { on_disk: downloaded, online, offline });
            if matches!(fetch.state, FetchState::Fetching { .. }) {
                let token = hf_token(&app.settings.agent);
                app.agent.models.start_download(role, spec.clone(), engine.clone(), token, ui.ctx().clone());
                ui.ctx().request_repaint();
            }
        }
        FetchClick::None => {}
    }
    if stopped {
        app.agent.models.stop_download(role);
    } else {
        app.agent.models.fetches.insert(role, fetch);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FetchClick {
    None,
    Download,
    Stop,
    Delete,
}

fn fetch_controls(ui: &mut egui::Ui, role: ModelRole, line: &str, state: &FetchState, deleting: bool, delete_error: bool) -> FetchClick {
    let p = theme::palette(ui);
    let width = FETCH_STATUS_W + FETCH_GAP + FETCH_BTN_W;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, FETCH_ROW_H), egui::Sense::hover());
    let status = egui::Rect::from_min_size(rect.min, egui::vec2(FETCH_STATUS_W, FETCH_ROW_H));
    let text_rect = egui::Rect::from_min_size(status.min, egui::vec2(FETCH_STATUS_W, FETCH_TEXT_H));
    let color = if delete_error || matches!(state, FetchState::Failed(_)) {
        p.danger
    } else if matches!(state, FetchState::Saved | FetchState::Already) {
        p.success
    } else if deleting {
        p.text_weak
    } else {
        p.text
    };
    let galley = egui::WidgetText::from(RichText::new(line).color(color)).into_galley(ui, Some(egui::TextWrapMode::Truncate), FETCH_STATUS_W, egui::TextStyle::Body);
    ui.painter_at(text_rect).galley(egui::pos2(text_rect.left(), text_rect.center().y - galley.size().y / 2.0), galley, color);
    if let FetchState::Fetching { done, total } = state {
        let bar = egui::Rect::from_min_size(egui::pos2(status.left(), status.bottom() - FETCH_BAR_H), egui::vec2(FETCH_STATUS_W, FETCH_BAR_H));
        let painter = ui.painter_at(bar);
        painter.rect_filled(bar, egui::CornerRadius::same(3), p.border);
        let fill = if *total == 0 { 0.0 } else { (*done as f64 / *total as f64).clamp(0.0, 1.0) as f32 };
        if fill > 0.0 {
            painter.rect_filled(egui::Rect::from_min_size(bar.min, egui::vec2(bar.width() * fill, bar.height())), egui::CornerRadius::same(3), p.accent);
        }
    }
    let full = ui.fonts_mut(|f| f.layout_no_wrap(line.to_string(), egui::FontId::proportional(13.0), color).size().x);
    if full > FETCH_STATUS_W || delete_error || matches!(state, FetchState::Failed(_)) {
        let resp = ui.interact(status, ui.id().with(("model-status", role as u8)), egui::Sense::hover());
        super::topics::text_tip(&resp, line, &super::mentions::Roster::default());
    }
    let fetching = matches!(state, FetchState::Fetching { .. });
    let on_disk = !fetching && !deleting && matches!(state, FetchState::Saved | FetchState::Already);
    let (glyph, label, kind) = if fetching {
        (icon::STOP, t!("agent.stop").to_string(), FetchClick::Stop)
    } else if on_disk || deleting {
        (icon::TRASH, t!("agent.delete_model").to_string(), FetchClick::Delete)
    } else {
        (icon::DOWNLOAD_SIMPLE, t!("agent.download").to_string(), FetchClick::Download)
    };
    let button_rect = egui::Rect::from_min_size(egui::pos2(rect.right() - FETCH_BTN_W, rect.top()), egui::vec2(FETCH_BTN_W, FETCH_ROW_H));
    let enabled = !deleting;
    let clicked = ui
        .scope_builder(egui::UiBuilder::new().max_rect(button_rect), |ui| {
            let button = if kind == FetchClick::Delete {
                theme::danger(ui, glyph, &label)
            } else {
                theme::secondary(glyph, &label)
            };
            ui.add_enabled(enabled, button.min_size(egui::vec2(FETCH_BTN_W, FETCH_ROW_H))).clicked()
        })
        .inner;
    if clicked { kind } else { FetchClick::None }
}

pub fn fetch_line(state: &FetchState, deleting: bool) -> String {
    if deleting {
        return t!("agent.deleting").into();
    }
    match state {
        FetchState::Fetching { done, total } => fetching_line(*done, *total),
        FetchState::Saved => t!("agent.saved").into(),
        FetchState::Already => t!("agent.already").into(),
        FetchState::Failed(e) => e.clone(),
        FetchState::Idle => t!("agent.not_downloaded").into(),
    }
}

fn fetching_line(done: u64, total: u64) -> String {
    if total == 0 {
        return t!("agent.fetching").into();
    }
    let percent = ((done as f64 / total as f64) * 100.0).clamp(0.0, 100.0).round() as u64;
    t!("agent.fetching_progress", percent = percent, done = mb(done), total = mb(total)).into()
}

fn mb(bytes: u64) -> u64 {
    (bytes as f64 / 1_048_576.0).round() as u64
}

fn delete_dialog(app: &mut App, ctx: &egui::Context) {
    let Some((role, spec)) = app.agent.models.confirm_delete.clone() else { return };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("model-delete")).show(ctx, |ui| {
        ui.set_width(340.0);
        theme::popup_title(ui, &t!("agent.model_delete_title"));
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
    let downloaded: HashSet<String> = app.agent.engine_state.models().iter().map(|m: &ModelInfo| m.spec.clone()).collect();

    header(ui, &t!("agent.models_title"), "");
    models_folder(app, ui);
    app.agent.engine_state.offline_hint(ui);
    ui.add_space(6.0);
    let mut changed = false;

    let running = app.agent.models.running();
    for role in ModelRole::ALL {
        let source = app.settings.agent.models.get(role).clone();
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.scope(|ui| theme::section(ui, role_icon(role), &role_title(role))).response.on_hover_text(role_hint(role));
                if role.required() {
                    ui.label(RichText::new(t!("agent.required")).size(11.5).color(theme::palette(ui).accent));
                }
            });
            ui.add_enabled_ui(!running, |ui| {
                changed |= source_editor(app, ui, role);
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
                if role == ModelRole::Agent {
                    if let Some(model) = app.settings.agent.models.agent.clone().filter(ModelSource::supports_thinking) {
                        let mut on = app.settings.agent.thinking_on(&model);
                        if ui.checkbox(&mut on, t!("agent.thinking")).on_hover_text(t!("agent.thinking_hint")).changed() {
                            app.settings.agent.set_thinking(&model, on);
                            changed = true;
                        }
                    }
                    let mut verify = app.settings.agent.verify_answers;
                    let resp = ui.checkbox(&mut verify, t!("agent.verify_answers"));
                    if resp.changed() {
                        app.settings.agent.verify_answers = verify;
                        changed = true;
                    }
                    if resp.hovered() {
                        super::topics::text_tip(&resp, &t!("agent.verify_answers_hint"), &super::mentions::Roster::default());
                    }
                }
            });
            if let Some(ModelSource::HuggingFace { repo }) = source {
                let downloaded = is_downloaded(&repo, downloaded.iter().map(String::as_str));
                repository_rows(app, ui, RoleRow { role, repo: &repo, downloaded, online }, &engine);
            }
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
        app.agent.models.statuses.clear();
        app.agent.models.fetches.clear();
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

    fn press(on_disk: bool, online: bool) -> FetchInput {
        FetchInput::Press { on_disk, online, offline: "engine down".into() }
    }

    #[test]
    fn a_download_says_what_it_is_doing() {
        let idle = Fetch::default();
        let fetching = fetch_next(idle.clone(), press(false, true));
        assert_eq!(fetching.state, FetchState::Fetching { done: 0, total: 0 }, "a click shows the download at once");

        let started = fetch_next(fetching, FetchInput::Frame { now_ms: 1_000 });
        assert!(started.started_ms.is_some());
        let moving = fetch_next(started.clone(), FetchInput::Progress(10, 100));
        assert_eq!(moving.state, FetchState::Fetching { done: 10, total: 100 });

        let held = fetch_next(started.clone(), FetchInput::Thread(Ok(())));
        assert!(matches!(held.state, FetchState::Fetching { .. }), "the result waits");
        assert!(held.held.is_some());
        let early = fetch_next(held.clone(), FetchInput::Frame { now_ms: 1_000 + FETCH_SHOW_MS - 1 });
        assert!(matches!(early.state, FetchState::Fetching { .. }));
        let saved = fetch_next(held, FetchInput::Frame { now_ms: 1_000 + FETCH_SHOW_MS });
        assert_eq!(saved.state, FetchState::Saved);

        let failed = fetch_next(started, FetchInput::Thread(Err("disk full".into())));
        let failed = fetch_next(failed, FetchInput::Frame { now_ms: 1_000 + FETCH_SHOW_MS });
        assert_eq!(failed.state, FetchState::Failed("disk full".into()));
    }

    #[test]
    fn a_click_on_a_downloaded_or_unreachable_model_is_not_silent() {
        assert_eq!(fetch_next(Fetch::default(), press(true, true)).state, FetchState::Already);
        assert_eq!(fetch_next(Fetch::default(), press(false, false)).state, FetchState::Failed("engine down".into()));
        assert_eq!(fetch_next(Fetch::default(), FetchInput::Listed).state, FetchState::Already);
        let failed = Fetch { state: FetchState::Failed("x".into()), ..Fetch::default() };
        assert_eq!(fetch_next(failed, FetchInput::Listed).state, FetchState::Failed("x".into()));
    }

    #[test]
    fn the_status_line_names_every_state_in_one_line() {
        assert_eq!(fetch_line(&FetchState::Idle, false), t!("agent.not_downloaded").to_string());
        assert_eq!(fetch_line(&FetchState::Saved, false), t!("agent.saved").to_string());
        assert_eq!(fetch_line(&FetchState::Already, false), t!("agent.already").to_string());
        assert_eq!(fetch_line(&FetchState::Idle, true), t!("agent.deleting").to_string());
        assert_eq!(fetch_line(&FetchState::Failed("disk full".into()), false), "disk full");
        assert_eq!(fetch_line(&FetchState::Fetching { done: 0, total: 0 }, false), t!("agent.fetching").to_string());
        let total = 250 * 1024 * 1024;
        let done = 112 * 1024 * 1024;
        let line = fetch_line(&FetchState::Fetching { done, total }, false);
        assert_eq!(line, t!("agent.fetching_progress", percent = 45, done = 112, total = 250).to_string());
        assert!(!line.contains('\n'), "{line}");
    }
}
