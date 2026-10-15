//! The studio's chat pane: conversations with the agent about the recording (several chats per
//! recording, kept with its transcript). Answers are Markdown; cited times (`[12:30]`) are links
//! that seek the player; while the agent works its steps appear under «Thinking…», and each
//! answer keeps them in its details.

use super::studio::{panel, StudioAction};
use super::Services;
use crate::player::Player;
use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, FontId, Rect, RichText};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};
use egui_phosphor::regular as icon;
use mediaa_agent::assistant::folder::Item;
use mediaa_agent::assistant::{self, Event, Memory, Recording, StepRecord, StepStatus, Turn};
use mediaa_agent::tools::parse_time;
use mediaa_core::chat::{Author, ChatId, ChatInfo, ChatMessage};
use mediaa_core::transcript::{self, Transcript};
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the chats are re-read (another studio of the same file may write).
const REFRESH: Duration = Duration::from_secs(5);
/// Characters of a step's result shown in the details.
const RESULT_CHARS: usize = 400;
/// Characters of the first question that name a new chat.
const TITLE_CHARS: usize = 48;

/// Times written without brackets (`в 1:05`, `0:00–0:11`) as links too.
fn link_bare(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let starts = chars[i].is_ascii_digit() && (i == 0 || !(chars[i - 1].is_alphanumeric() || matches!(chars[i - 1], ':' | '.' | ',')));
        if starts {
            // The longest `d{1,2}:dd(:dd)?` here.
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ':') {
                j += 1;
            }
            let token: String = chars[i..j].iter().collect::<String>().trim_end_matches(':').to_string();
            let shape = token.split(':').collect::<Vec<_>>();
            let ok = (2..=3).contains(&shape.len()) && shape[0].len() <= 2 && shape[1..].iter().all(|p| p.len() == 2);
            if let (true, Some(t)) = (ok, parse_time(&token).filter(|_| ok)) {
                out.push_str(&format!("[`{token}`](seek:{t})"));
                i += token.chars().count();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Cited times become `seek:` links shown as time chips (`[1:05]` → ``[`1:05`](seek:65)``; a
/// range seeks to its start).
pub fn linkify(text: &str, bare_times: bool) -> String {
    let bare = |s: &str| if bare_times { link_bare(s) } else { s.to_string() };
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&bare(&rest[..open]));
        let after = &rest[open + 1..];
        let close = after.find(']');
        // A Markdown link already: kept as it is.
        if let Some(end) = close.filter(|&c| after[c + 1..].starts_with('(')).and_then(|c| after[c + 1..].find(')').map(|e| c + 1 + e)) {
            out.push('[');
            out.push_str(&after[..=end]);
            rest = &after[end + 1..];
            continue;
        }
        let link = close.and_then(|c| {
            let inside = &after[..c];
            // `#2 12:30` in a folder's chat: the recording's number before the time.
            let (recording, time) = match inside.strip_prefix('#').and_then(|r| r.split_once(' ')) {
                Some((n, time)) => (Some(n.parse::<usize>().ok()?), time),
                None => (None, inside),
            };
            let start = time.split(['–', '-']).next().unwrap_or_default();
            let t = start.contains(':').then(|| parse_time(start)).flatten()?;
            Some((c, inside, recording.map_or_else(|| t.to_string(), |n| format!("{n}:{t}"))))
        });
        match link {
            Some((c, inside, target)) => {
                out.push_str(&format!("[`{inside}`](seek:{target})"));
                rest = &after[c + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(&bare(rest));
    out
}

/// A chat's earlier questions with their answers.
pub fn history(messages: &[ChatMessage]) -> Vec<Turn> {
    // An answer may come as several messages (sections sent first).
    let mut turns: Vec<Turn> = Vec::new();
    let mut question: Option<&str> = None;
    for m in messages {
        match m.author {
            Author::User => question = Some(&m.text),
            Author::Agent => match (question.take(), turns.last_mut()) {
                (Some(q), _) => turns.push(Turn { question: q.to_string(), answer: m.text.clone() }),
                (None, Some(t)) => {
                    t.answer.push_str("\n\n");
                    t.answer.push_str(&m.text);
                }
                (None, None) => {}
            },
        }
    }
    turns
}

/// The name of a chat from its first question.
pub fn chat_title(question: &str) -> String {
    let q = question.split_whitespace().collect::<Vec<_>>().join(" ");
    if q.chars().count() <= TITLE_CHARS {
        return q;
    }
    let cut: String = q.chars().take(TITLE_CHARS).collect();
    format!("{}…", cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head))
}

fn status_icon(s: StepStatus) -> &'static str {
    match s {
        StepStatus::Found => icon::CHECK_CIRCLE,
        StepStatus::Nothing => icon::CIRCLE_DASHED,
        StepStatus::Repeated => icon::ARROW_COUNTER_CLOCKWISE,
        StepStatus::Failed => icon::WARNING_CIRCLE,
    }
}

enum Progress {
    Step(StepRecord),
    Draft(String),
    DraftDiscarded,
    Scanning(usize, usize),
    /// A section was stored as a message.
    Said,
    Done(Result<(), String>),
}

/// An answer being written.
struct Running {
    rx: Receiver<Progress>,
    cancel: Arc<AtomicBool>,
    steps: Vec<StepRecord>,
    /// The answer as it is being written.
    draft: String,
    /// Parts of the recording read by a running scan, of all.
    scanning: Option<(usize, usize)>,
    started: Instant,
}

/// Where a clicked citation goes.
enum Jump {
    Here(f64),
    Open(PathBuf, f64),
}

/// What an answer is about, for the thread writing it.
enum Target {
    /// A recording, with the player's length and position when it plays here.
    File(PathBuf, Option<(f64, f64)>),
    Folder(Vec<PathBuf>),
}

/// What the agent knows of a transcribed recording; `timing`: the player's length and position.
fn recording_of(ai: &Path, id: &str, path: &Path, timing: Option<(f64, f64)>) -> anyhow::Result<Recording> {
    let t = Transcript::open(ai, id)?;
    let replies = t.dialog()?;
    let duration = timing.map_or_else(|| replies.last().map_or(0.0, |r| r.end), |(d, _)| d);
    Ok(Recording {
        title: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        duration,
        diarized: replies.iter().any(|r| r.speaker.is_some()),
        names: t.speaker_names()?,
        replies,
        chapters: t.chapters()?,
        position: timing.map(|(_, p)| p),
    })
}

/// The recordings of a folder; those not transcribed yet are listed with their titles only.
fn items_of(ai: &Path, files: &[PathBuf]) -> Vec<Item> {
    files
        .iter()
        .filter_map(|path| {
            let id = transcript::file_id(path).ok()?;
            let recording = if Transcript::exists(ai, &id) {
                recording_of(ai, &id, path, None).map_err(|e| log::warn!("{}: {e:#}", path.display())).ok()?
            } else {
                Recording { title: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(), ..Recording::default() }
            };
            Some(Item { recording, file_id: id, path: path.clone() })
        })
        .collect()
}

/// What a chat talks about.
enum Scope {
    /// One recording; its chats are kept with its transcript.
    File { path: PathBuf, file_id: Option<String> },
    /// The recordings of a catalog folder; its chats are kept in a store of their own.
    Folder { store: String, files: Vec<PathBuf> },
}

pub struct Chat {
    scope: Scope,
    ai_dir: PathBuf,
    chats: Vec<ChatInfo>,
    current: Option<ChatId>,
    messages: Vec<ChatMessage>,
    input: String,
    running: Option<Running>,
    error: Option<String>,
    read: Option<Instant>,
    cache: CommonMarkCache,
    dev_asked: bool,
}

impl Chat {
    /// The chat of one recording.
    pub fn new(path: &Path, ai_dir: &Path) -> Chat {
        Chat::with_scope(Scope::File { path: path.to_path_buf(), file_id: transcript::file_id(path).ok() }, ai_dir)
    }

    /// The chat of a catalog folder (`node`) over its recordings.
    pub fn folder(node: i64, files: Vec<PathBuf>, ai_dir: &Path) -> Chat {
        Chat::with_scope(Scope::Folder { store: format!("folder-{node}"), files }, ai_dir)
    }

    fn with_scope(scope: Scope, ai_dir: &Path) -> Chat {
        Chat {
            scope,
            ai_dir: ai_dir.to_path_buf(),
            chats: Vec::new(),
            current: None,
            messages: Vec::new(),
            input: String::new(),
            running: None,
            error: None,
            read: None,
            cache: CommonMarkCache::default(),
            dev_asked: false,
        }
    }

    /// Where the chats are kept: a recording's transcript, or a folder's store (made on use).
    fn transcript(&self) -> Option<Transcript> {
        match &self.scope {
            Scope::File { file_id, .. } => {
                let id = file_id.as_ref()?;
                Transcript::exists(&self.ai_dir, id).then(|| Transcript::open(&self.ai_dir, id).ok()).flatten()
            }
            Scope::Folder { store, .. } => Transcript::open(&self.ai_dir, store).ok(),
        }
    }

    /// Times without brackets seek this recording's player; in a folder they mean nothing.
    fn bare_times(&self) -> bool {
        matches!(self.scope, Scope::File { .. })
    }

    fn salt(&self) -> String {
        match &self.scope {
            Scope::File { path, .. } => path.to_string_lossy().into_owned(),
            Scope::Folder { store, .. } => store.clone(),
        }
    }

    fn refresh(&mut self, force: bool) {
        if !force && self.read.is_some_and(|r| r.elapsed() < REFRESH) {
            return;
        }
        self.read = Some(Instant::now());
        let Some(t) = self.transcript() else { return };
        self.chats = t.chats().unwrap_or_default();
        if self.current.is_none_or(|c| !self.chats.iter().any(|x| x.id == c)) {
            self.current = self.chats.first().map(|c| c.id);
        }
        self.messages = self.current.and_then(|c| t.messages(c).ok()).unwrap_or_default();
    }

    /// Whether there is something to talk about (a folder: always; its list tells what is not
    /// transcribed yet).
    fn ready(&self) -> bool {
        match &self.scope {
            Scope::File { file_id, .. } => file_id.as_ref().is_some_and(|id| Transcript::exists(&self.ai_dir, id)),
            Scope::Folder { .. } => true,
        }
    }

    /// Sends the typed question: stored at once, answered in the background.
    fn send(&mut self, services: &Services, player: Option<&Player>) {
        let question = self.input.trim().to_string();
        let Some(t) = self.transcript() else { return };
        if question.is_empty() || self.running.is_some() {
            return;
        }
        let stored = (|| -> anyhow::Result<ChatId> {
            let chat = match self.current {
                Some(c) => c,
                None => t.create_chat(&chat_title(&question))?,
            };
            t.add_message(chat, Author::User, &question, &serde_json::Value::Null)?;
            Ok(chat)
        })();
        let chat = match stored {
            Ok(c) => c,
            Err(e) => {
                self.error = Some(format!("{e:#}"));
                return;
            }
        };
        self.current = Some(chat);
        let history = history(&self.messages);
        self.input.clear();
        self.error = None;
        self.refresh(true);

        let (tx, rx) = crossbeam_channel::unbounded();
        let cancel = Arc::new(AtomicBool::new(false));
        let (s, c, ai) = (services.clone(), cancel.clone(), self.ai_dir.clone());
        let (store, target) = match &self.scope {
            Scope::File { path, file_id } => (file_id.clone().unwrap_or_default(), Target::File(path.clone(), player.map(|p| (p.duration(), p.position())))),
            Scope::Folder { store, files } => (store.clone(), Target::Folder(files.clone())),
        };
        let _ = std::thread::Builder::new().name("agent-answer".into()).spawn(move || {
            let run = || -> anyhow::Result<()> {
                let t = Transcript::open(&ai, &store)?;
                let settings = s.settings.read().expect("settings lock").clone();
                let mut on_event = |e: &Event| {
                    let p = match e {
                        Event::Step(step) => Progress::Step(step.clone()),
                        Event::Draft(text) => Progress::Draft(text.clone()),
                        Event::DraftDiscarded => Progress::DraftDiscarded,
                        Event::Scanning { done, total } => Progress::Scanning(*done, *total),
                        // A section of a long answer: a message of its own, shown at once.
                        Event::Say(text) => match t.add_message(chat, Author::Agent, text, &serde_json::json!({ "section": true })) {
                            Ok(()) => Progress::Said,
                            Err(e) => return log::warn!("chat: {e:#}"),
                        },
                        Event::Answer(_) => return,
                    };
                    let _ = tx.send(p);
                };
                let mut memory: Memory = t.chat_memory(chat)?.and_then(|m| serde_json::from_str(&m).ok()).unwrap_or_default();
                let answered = match &target {
                    Target::File(path, timing) => {
                        let recording = recording_of(&ai, &store, path, *timing)?;
                        assistant::ask(&s.runtime, &settings, &ai, &store, &recording, &history, &mut memory, &question, &c, &mut on_event)
                    }
                    Target::Folder(files) => {
                        let items = items_of(&ai, files);
                        assistant::ask_folder(&s.runtime, &settings, &ai, &items, &history, &mut memory, &question, &c, &mut on_event)
                    }
                };
                // What the chat learned is kept even when the answer failed.
                t.set_chat_memory(chat, &serde_json::to_string(&memory)?)?;
                let a = answered?;
                let details = serde_json::json!({ "steps": a.steps, "forced": a.forced });
                t.add_message(chat, Author::Agent, &a.text, &details)?;
                Ok(())
            };
            let _ = tx.send(Progress::Done(run().map_err(|e| format!("{e:#}"))));
        });
        self.running = Some(Running { rx, cancel, steps: Vec::new(), draft: String::new(), scanning: None, started: Instant::now() });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(r) = &mut self.running else { return };
        ctx.request_repaint_after(Duration::from_millis(200));
        let mut done = None;
        let mut said = false;
        for p in r.rx.try_iter() {
            match p {
                Progress::Step(s) => {
                    r.steps.push(s);
                    r.scanning = None;
                }
                Progress::Scanning(done, total) => r.scanning = Some((done, total)),
                Progress::Draft(t) => r.draft.push_str(&t),
                Progress::DraftDiscarded => r.draft.clear(),
                // The section is a message now (the draft streamed it).
                Progress::Said => {
                    r.draft.clear();
                    said = true;
                }
                Progress::Done(result) => done = Some(result),
            }
        }
        if said {
            self.refresh(true);
        }
        let Some(r) = &mut self.running else { return };
        if let Some(result) = done {
            if let Err(e) = result {
                // Stopped on purpose is not an error.
                if !r.cancel.load(Ordering::Relaxed) {
                    self.error = Some(e);
                }
            }
            self.running = None;
            self.refresh(true);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rect: Rect, player: Option<&mut Player>, services: Option<&Services>, p: theme::Palette) -> Option<StudioAction> {
        self.refresh(false);
        self.poll(ui.ctx());
        ui.ctx().request_repaint_after(REFRESH);
        let body = panel(ui, rect, &t!("studio.chat"), p);
        let mut seek = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(10.0, 8.0))), |ui| {
            if !self.ready() {
                ui.centered_and_justified(|ui| theme::hint(ui, t!("studio.chat_needs_text")));
                return;
            }
            // Dev: `MEDIAA_DEV_ASK=<question>` asks it once (UI checks).
            if let (false, Some(s), Ok(q)) = (self.dev_asked, services, std::env::var("MEDIAA_DEV_ASK")) {
                self.dev_asked = true;
                self.input = q;
                self.send(s, player.as_deref());
            }
            self.chats_bar(ui);
            let input_h = 86.0;
            let bare = self.bare_times();
            let list = Rect::from_min_max(ui.cursor().min, egui::pos2(ui.max_rect().max.x, ui.max_rect().max.y - input_h));
            ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    // Long words and wide Markdown never widen the pane.
                    ui.set_max_width(ui.available_width());
                    if self.messages.is_empty() && self.running.is_none() {
                        theme::hint(ui, t!("studio.chat_hint"));
                    }
                    for m in &self.messages {
                        if let Some(t) = message_ui(ui, m, &mut self.cache, bare, p) {
                            seek = Some(t);
                        }
                    }
                    if let Some(r) = &self.running {
                        if let Some(t) = thinking_ui(ui, r, &mut self.cache, bare, p) {
                            seek = Some(t);
                        }
                    }
                    if let Some(e) = &self.error {
                        ui.colored_label(p.danger, e);
                    }
                });
            });
            let input = Rect::from_min_max(egui::pos2(ui.max_rect().min.x, ui.max_rect().max.y - input_h + 6.0), ui.max_rect().max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(input), |ui| self.input_ui(ui, player.as_deref(), services));
        });
        // A citation: this recording's player, or (in a folder) the recording it names.
        match (seek.as_deref().and_then(|s| self.jump(s)), player) {
            (Some(Jump::Here(t)), Some(p)) => {
                p.seek(t);
                None
            }
            (Some(Jump::Open(path, t)), _) => Some(StudioAction::OpenAt(path, t)),
            _ => None,
        }
    }

    /// Where a `seek:` link goes: `750` (this recording) or `2:750` (the folder's 2nd).
    fn jump(&self, target: &str) -> Option<Jump> {
        match (target.split_once(':'), &self.scope) {
            (None, _) => target.parse().ok().map(Jump::Here),
            (Some((n, t)), Scope::Folder { files, .. }) => {
                let path = files.get(n.parse::<usize>().ok()?.checked_sub(1)?)?;
                Some(Jump::Open(path.clone(), t.parse().ok()?))
            }
            (Some(_), Scope::File { .. }) => None,
        }
    }

    /// Chat choice, a new chat, removing the current one.
    fn chats_bar(&mut self, ui: &mut egui::Ui) {
        // Right to left: the buttons, then the chat list in what is left (never wider).
        let row = egui::Layout::right_to_left(egui::Align::Center);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), theme::ICON_BUTTON), row, |ui| {
            let current = self.current.and_then(|c| self.chats.iter().find(|x| x.id == c)).map_or_else(|| t!("studio.new_chat").to_string(), |c| c.title.clone());
            let mut pick = None;
            if let Some(c) = self.current {
                if theme::icon_button(ui, icon::TRASH, &t!("studio.delete_chat")).clicked() {
                    if let Some(t) = self.transcript() {
                        if let Err(e) = t.delete_chat(c) {
                            self.error = Some(format!("{e:#}"));
                        }
                    }
                    pick = Some(None);
                }
            }
            if theme::icon_button(ui, icon::PLUS, &t!("studio.new_chat")).clicked() {
                pick = Some(None);
            }
            // The width given to a combo box leaves out its arrow and padding.
            egui::ComboBox::from_id_salt(("chat-pick", self.salt())).selected_text(current).truncate().width((ui.available_width() - 32.0).max(60.0)).show_ui(ui, |ui| {
                for c in &self.chats {
                    if ui.selectable_label(Some(c.id) == self.current, &c.title).clicked() {
                        pick = Some(Some(c.id));
                    }
                }
            });
            if let (Some(choice), None) = (pick, &self.running) {
                self.current = choice;
                self.messages.clear();
                if choice.is_some() {
                    self.refresh(true);
                } else {
                    // A new chat starts with the next question.
                    self.read = Some(Instant::now());
                }
            }
        });
        ui.add_space(4.0);
    }

    fn input_ui(&mut self, ui: &mut egui::Ui, player: Option<&Player>, services: Option<&Services>) {
        ui.horizontal(|ui| {
            let running = self.running.is_some();
            let edit = egui::TextEdit::multiline(&mut self.input).hint_text(t!("studio.ask_hint")).desired_rows(3).desired_width(ui.available_width() - 36.0);
            let resp = ui.add_enabled(!running, edit);
            let enter = resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if enter {
                // Enter sends; the newline it typed is not part of the question.
                self.input = self.input.trim_end_matches(['\n', '\r']).to_string();
            }
            ui.vertical(|ui| {
                if running {
                    if theme::icon_button(ui, icon::STOP, &t!("studio.stop")).clicked() {
                        if let Some(r) = &self.running {
                            r.cancel.store(true, Ordering::Relaxed);
                        }
                    }
                } else if (theme::icon_button(ui, icon::PAPER_PLANE_RIGHT, &t!("studio.send")).clicked() || enter) && !self.input.trim().is_empty() {
                    match services {
                        Some(s) => self.send(s, player),
                        None => self.error = Some(t!("studio.agent_not_ready").into()),
                    }
                }
            });
        });
    }
}

/// A message; returns the time of a clicked citation.
fn message_ui(ui: &mut egui::Ui, m: &ChatMessage, cache: &mut CommonMarkCache, bare_times: bool, p: theme::Palette) -> Option<String> {
    match m.author {
        Author::User => {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                egui::Frame::new().fill(p.accent_soft).corner_radius(10).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                    ui.set_max_width(ui.available_width() * 0.85);
                    ui.add(egui::Label::new(RichText::new(&m.text).size(13.5)).wrap());
                });
            });
            ui.add_space(6.0);
            None
        }
        Author::Agent => {
            CommonMarkViewer::new().show(ui, cache, &linkify(&m.text, bare_times));
            let seek = take_seek(ui);
            let steps: Vec<StepRecord> = serde_json::from_value(m.details["steps"].clone()).unwrap_or_default();
            if !steps.is_empty() {
                egui::CollapsingHeader::new(RichText::new(t!("studio.steps", n = steps.len())).size(11.5).color(p.text_weak))
                    .id_salt(("steps", m.text.len(), steps.len(), &steps[0].title))
                    .default_open(false)
                    .show(ui, |ui| {
                        for s in &steps {
                            step_ui(ui, s, true, p);
                        }
                        if m.details["forced"].as_bool() == Some(true) {
                            theme::hint(ui, t!("studio.forced"));
                        }
                    });
            }
            ui.add_space(10.0);
            seek
        }
    }
}

fn step_ui(ui: &mut egui::Ui, s: &StepRecord, full: bool, p: theme::Palette) {
    let color = match s.status {
        StepStatus::Found => p.success,
        StepStatus::Failed => p.danger,
        _ => p.text_weak,
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(status_icon(s.status)).color(color));
        ui.add(egui::Label::new(RichText::new(&s.title).size(12.5)).truncate());
    });
    if full {
        ui.label(RichText::new(format!("{} {}", s.tool, s.args)).font(FontId::monospace(10.5)).color(p.text_weak));
        let result: String = s.result.chars().take(RESULT_CHARS).collect();
        ui.add(egui::Label::new(RichText::new(result).size(11.5).color(p.text_weak)).wrap());
        ui.add_space(4.0);
    }
}

/// The answer being written: its steps so far and the text as it comes; returns the time of a
/// clicked citation.
fn thinking_ui(ui: &mut egui::Ui, r: &Running, cache: &mut CommonMarkCache, bare_times: bool, p: theme::Palette) -> Option<String> {
    for s in &r.steps {
        step_ui(ui, s, false, p);
    }
    if let Some((done, total)) = r.scanning {
        ui.label(RichText::new(t!("studio.scanning", done = done, total = total)).size(12.5).color(p.text_weak));
        ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_height(5.0));
    }
    if !r.draft.is_empty() {
        CommonMarkViewer::new().show(ui, cache, &linkify(&r.draft, bare_times));
    }
    ui.horizontal(|ui| {
        ui.spinner();
        let label = if r.draft.is_empty() { t!("studio.thinking", s = r.started.elapsed().as_secs()) } else { t!("studio.writing") };
        ui.label(RichText::new(label).color(p.text_weak));
    });
    take_seek(ui)
}

/// A clicked citation link (what follows `seek:`): it seeks instead of opening a browser.
fn take_seek(ui: &egui::Ui) -> Option<String> {
    let mut seek = None;
    ui.ctx().output_mut(|o| {
        o.commands.retain(|c| match c {
            egui::OutputCommand::OpenUrl(u) if u.url.starts_with("seek:") => {
                seek = Some(u.url["seek:".len()..].to_string());
                false
            }
            _ => true,
        })
    });
    seek
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citations_become_seek_links() {
        assert_eq!(linkify("В пятницу [1:05].", true), "В пятницу [`1:05`](seek:65).");
        assert_eq!(linkify("[0:00–0:11] и [1:02:30]", true), "[`0:00–0:11`](seek:0) и [`1:02:30`](seek:3750)");
        assert_eq!(linkify("уже [1:05](seek:65), [см.] и [5]", true), "уже [1:05](seek:65), [см.] и [5]");
        assert_eq!(linkify("[незакрытая", true), "[незакрытая");
        assert_eq!(linkify("в [#2 1:05] и [#10 0:00–0:30]", true), "в [`#2 1:05`](seek:2:65) и [`#10 0:00–0:30`](seek:10:0)");
        assert_eq!(linkify("в моменте 0:00–0:11.", true), "в моменте [`0:00`](seek:0)–[`0:11`](seek:11).");
        assert_eq!(linkify("в 1:02:30, не 3.14:15, не 123:45, не 12:5", true), "в [`1:02:30`](seek:3750), не 3.14:15, не 123:45, не 12:5");
        assert_eq!(linkify("длится 26:39, см. [#2 1:05]", false), "длится 26:39, см. [`#2 1:05`](seek:2:65)", "a folder: no bare times");
    }

    #[test]
    fn history_pairs_questions_with_answers() {
        let m = |author, text: &str| ChatMessage { author, text: text.into(), details: serde_json::Value::Null };
        let h = history(&[m(Author::User, "a"), m(Author::Agent, "b"), m(Author::User, "c")]);
        assert_eq!(h, vec![Turn { question: "a".into(), answer: "b".into() }], "an unanswered question is not history");
        // Sections and the final message make one answer.
        let h = history(&[m(Author::User, "a"), m(Author::Agent, "часть 1"), m(Author::Agent, "часть 2"), m(Author::User, "c"), m(Author::Agent, "d")]);
        assert_eq!(h.iter().map(|t| t.answer.as_str()).collect::<Vec<_>>(), ["часть 1\n\nчасть 2", "d"]);
    }

    #[test]
    fn chats_are_named_by_their_first_question() {
        assert_eq!(chat_title("  Когда   отчёт? "), "Когда отчёт?");
        let long = chat_title("Расскажи подробно, что решили про бюджет на следующий квартал и кто отвечает");
        assert!(long.ends_with('…') && long.chars().count() <= TITLE_CHARS + 1, "{long}");
    }
}
