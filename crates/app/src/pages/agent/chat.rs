//! The studio's chat pane: conversations with the agent about the recording (several chats per
//! recording, kept with its transcript). Answers are Markdown; cited times (`[12:30]`) are links
//! that seek the player; an answer is its timeline (steps, reasoning, text; see `timeline`), and
//! a chat is named once by the titles model after its first question.

use super::composer::{self, Field};
use super::mentions::{self, Roster};
use super::studio::{self, panel, StudioAction};
use super::topics;
use super::Services;
use crate::player::Player;
use crate::tab_strip::{self, Tab, TabAction};
use crate::theme;
use eframe::egui::{self, FontId, Rect, RichText};
use egui_phosphor::regular as icon;
use mediaa_agent::assistant::chat::Live;
use mediaa_agent::assistant::queued::{self, ANSWER_KIND, TITLE_KIND};
use mediaa_agent::assistant::{quote, step_title, title, Event, StepRecord};
#[cfg(test)]
use mediaa_agent::assistant::Turn;
use mediaa_agent::models::local_spec;
use mediaa_agent::tools::parse_time;
use mediaa_core::agent::{AgentSettings, ModelSource};
use mediaa_core::chat::{Author, ChatId, ChatInfo, ChatMessage, Timeline, TimelineEntry};
use mediaa_core::speakers::{self, Popup};
use mediaa_core::transcript::{self, Transcript};
use mediaa_queue::{Resource, Task, WaitReason};
use rust_i18n::t;
use super::timeline;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the chats are re-read (another studio of the same file may write).
const REFRESH: Duration = Duration::from_secs(5);

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
                out.push_str(&format!("[{token}](seek:{t})"));
                i += token.chars().count();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Cited times become `seek:` links of the same size as the text (`[1:05]` → `[1:05](seek:65)`;
/// a range seeks to its start).
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
                out.push_str(&format!("[{inside}](seek:{target})"));
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
#[cfg(test)]
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

/// Where a clicked citation goes.
enum Jump {
    Here(f64),
    Open(PathBuf, f64),
}

/// What a message sent now becomes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sent {
    /// Nothing of this chat runs: a question of its own.
    Question,
    /// This answer of the chat runs: a correction it takes after the current step.
    Correction(i64),
}

fn sent_as(running: Option<i64>) -> Sent {
    running.map_or(Sent::Question, Sent::Correction)
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
    /// Every chat of the recording (or folder), newest first.
    chats: Vec<ChatInfo>,
    /// The chats shown as tabs, in tab order.
    open: Vec<ChatId>,
    /// The chats without a tab, newest first.
    archived: Vec<ChatInfo>,
    /// The chat on screen; `None` is the new-chat tab (no chat stored until the first question).
    current: Option<ChatId>,
    /// The first read has chosen a tab (before that `current` is merely unset).
    loaded: bool,
    messages: Vec<ChatMessage>,
    /// What the user is typing: plain text plus mention and quote chips.
    draft: Field,
    /// Sections of the answer on screen that the pane has already read.
    shown_sections: usize,
    /// Cursor into the library's event feed.
    feed_at: usize,
    /// Chats whose title task was started again in this session (once each).
    retitled: std::collections::HashSet<ChatId>,
    error: Option<String>,
    /// An archived chat waiting for the delete confirmation.
    confirm_delete: Option<ChatInfo>,
    read: Option<Instant>,
    dev_asked: bool,
    /// The `@` completion list. Pure state; the rows are the roster indexes from the last frame.
    mention: Popup,
    mention_open: bool,
    mention_rows: Vec<usize>,
    mention_query: String,
    /// Caret byte to place after an insertion, before the editor draws.
    mention_caret: Option<usize>,
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
            open: Vec::new(),
            archived: Vec::new(),
            current: None,
            loaded: false,
            messages: Vec::new(),
            draft: Field::default(),
            shown_sections: 0,
            feed_at: 0,
            retitled: std::collections::HashSet::new(),
            error: None,
            confirm_delete: None,
            read: None,
            dev_asked: false,
            mention: Popup::default(),
            mention_open: false,
            mention_rows: Vec::new(),
            mention_query: String::new(),
            mention_caret: None,
        }
    }

    /// Speakers the chat can mention, re-read so a rename shows up without a new answer.
    fn roster(&self) -> Roster {
        match &self.scope {
            Scope::File { file_id, .. } => file_id.as_ref().map(|id| Roster::of_file(&self.ai_dir, id)).unwrap_or_default(),
            Scope::Folder { files, .. } => Roster::of_files(&self.ai_dir, files),
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
        self.open = t.open_chats().unwrap_or_default();
        // A chat deleted elsewhere leaves no tab.
        let before = self.open.len();
        self.open.retain(|c| self.chats.iter().any(|x| x.id == *c));
        if self.open.len() != before {
            if let Err(e) = t.set_open_chats(&self.open) {
                log::warn!("chat tabs: {e:#}");
            }
        }
        self.archived = t.archived_chats().unwrap_or_default();
        if !self.loaded {
            self.loaded = true;
            self.current = self.open.first().copied();
        } else if self.current.is_some_and(|c| !self.open.contains(&c)) {
            // The open tab was closed while another studio of the same file was writing.
            self.current = self.open.first().copied();
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

    /// Sends the typed message: while an answer of this chat runs it is a correction of that
    /// answer, otherwise a new question stored at once and answered by a queue task.
    fn send(&mut self, services: &Services, player: Option<&Player>, roster: &Roster) {
        self.draft.name_mentions(roster);
        let question = self.draft.to_message().trim().to_string();
        let Some(t) = self.transcript() else { return };
        if question.is_empty() {
            return;
        }
        if let Sent::Correction(answer) = sent_as(self.running_answer_id(services)) {
            if let Some(chat) = self.current {
                self.steer(&t, chat, answer, &question);
            }
            return;
        }
        let new_chat = self.current.is_none();
        let stored = (|| -> anyhow::Result<ChatId> {
            let chat = match self.current {
                Some(c) => c,
                // The title comes from the titles model after the first question.
                None => t.create_chat("")?,
            };
            t.add_message(chat, Author::User, &question, &serde_json::Value::Null)?;
            // The first question gives the new chat its tab.
            let mut open = t.open_chats()?;
            if !open.contains(&chat) {
                open.push(chat);
                t.set_open_chats(&open)?;
            }
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
        self.draft = Field::default();
        self.error = None;
        self.refresh(true);

        let store = self.store_key();
        if new_chat {
            self.start_title(services, &store, chat, &question);
        }
        let settings = services.settings.read().expect("settings lock").clone();
        let (resource, model) = answer_resource(&settings);
        let input = match &self.scope {
            Scope::File { path, .. } => serde_json::json!({
                "chat": chat,
                "store": store,
                "question": question,
                "locale": rust_i18n::locale().to_string(),
                "path": path,
                "file_id": store,
                "duration": player.map(|p| p.duration()),
                "position": player.map(|p| p.position()),
            }),
            Scope::Folder { files, .. } => serde_json::json!({
                "chat": chat,
                "store": store,
                "question": question,
                "locale": rust_i18n::locale().to_string(),
                "files": files,
            }),
        };
        let task = queued::answer_task(&store, chat, &question, input, resource, model.as_deref());
        if let Err(e) = services.sched.enqueue(task) {
            self.error = Some(format!("{e:#}"));
        }
    }

    /// The key of the store the chats live in.
    fn store_key(&self) -> String {
        match &self.scope {
            Scope::File { file_id, .. } => file_id.clone().unwrap_or_default(),
            Scope::Folder { store, .. } => store.clone(),
        }
    }

    /// A chat whose title task never finished (the program was closed meanwhile) is named again
    /// when it is opened — once. A running answer does not block it.
    fn retitle(&mut self, services: &Services) {
        let Some((chat, question)) = retitle_candidate(&self.chats, self.current, &self.messages, &self.retitled) else { return };
        let store = self.store_key();
        if title_pending(services, &store, chat) {
            return;
        }
        self.retitled.insert(chat);
        self.start_title(services, &store, chat, &question);
    }

    /// Names a chat: one small task for the titles model. The first question (trimmed and
    /// capitalised) is the title when the task cannot be started.
    fn start_title(&mut self, services: &Services, store: &str, chat: ChatId, question: &str) {
        if title_pending(services, store, chat) {
            return;
        }
        let for_model = self.roster().to_model(&quote::expand(question));
        let settings = services.settings.read().expect("settings lock").clone();
        let model = settings.models.titles.as_ref().and_then(|source| local_spec(source).ok());
        let task = queued::title_task(store, chat, question, &for_model, model.as_deref());
        if let Err(e) = services.sched.enqueue(task) {
            log::warn!("the title task was not started: {e:#}");
            if let Some(t) = self.transcript() {
                let _ = t.set_chat_title(chat, &title::fallback_title(question));
            }
        }
    }

    fn poll(&mut self, ctx: &egui::Context, services: Option<&Services>) {
        let Some(services) = services else { return };
        let (next, events, gap) = services.feed.since(self.feed_at);
        let restarted = next < self.feed_at;
        self.feed_at = next;
        let store = self.store_key();
        if gap || restarted || events.iter().any(|event| event.scope.as_deref() == Some(store.as_str())) {
            self.refresh(true);
        }
        let Ok(tasks) = services.sched.store().list() else { return };
        let any = tasks.iter().any(|task| task.kind == ANSWER_KIND && task.scope.as_deref() == Some(store.as_str()) && !task.status.is_finished());
        if any {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let Some(chat) = self.current else { return };
        let Some(task) = queued::running_answer(&tasks, &store, chat) else { return };
        if let Some(live) = services.streams.get(task.id) {
            if live.stored != self.shown_sections {
                self.shown_sections = live.stored;
                self.refresh(true);
            }
        }
    }

    /// Stops this chat's answer. The partial text is stored here; the chunk writes nothing more.
    fn stop(&mut self, services: &Services) {
        let Some(chat) = self.current else { return };
        let Ok(tasks) = services.sched.store().list() else { return };
        let (id, busy) = {
            let Some(task) = queued::running_answer(&tasks, &self.store_key(), chat) else { return };
            (task.id, queued::answer_busy(task))
        };
        if !busy {
            // Still waiting: nothing was written, so cancelling leaves no partial message.
            if let Err(e) = services.sched.cancel(id) {
                self.error = Some(format!("{e:#}"));
            }
            self.refresh(true);
            return;
        }
        services.streams.update(id, |live| {
            live.apply(&Event::Stopped);
            live.close();
        });
        if let Some(t) = self.transcript() {
            if let Some(mut live) = services.streams.get(id) {
                // A phrase that finished before this stop is part of the stored timeline.
                let found = step_title::phrases(&tasks, id);
                apply_phrases(&mut live.timeline.entries, &found);
                for (index, phrase) in &found {
                    if let Some(step) = live.steps.get_mut(*index) {
                        step.title = phrase.clone();
                    }
                }
                let details = serde_json::json!({
                    "steps": live.steps,
                    "forced": false,
                    "timeline": live.timeline.to_value(),
                    "stopped": true,
                    "task": id,
                });
                if let Err(e) = t.add_message(chat, Author::Agent, live.timeline.draft(), &details) {
                    self.error = Some(format!("{e:#}"));
                }
            }
        }
        if let Err(e) = services.sched.cancel(id) {
            self.error = Some(format!("{e:#}"));
        }
        self.refresh(true);
    }

    /// Stores a correction of the running answer: it is kept with the chat, survives a restart
    /// and the answer takes it after the step that runs now.
    fn steer(&mut self, t: &Transcript, chat: ChatId, answer: i64, text: &str) {
        let details = serde_json::json!({ "steer": { "task": answer } });
        match t.add_message(chat, Author::User, text, &details) {
            Ok(()) => {
                self.draft = Field::default();
                self.error = None;
                self.refresh(true);
            }
            Err(e) => self.error = Some(format!("{e:#}")),
        }
    }

    /// The answer of this chat that is not finished yet (running or waiting for a turn).
    fn running_answer_id(&self, services: &Services) -> Option<i64> {
        let chat = self.current?;
        let tasks = services.sched.store().list().ok()?;
        queued::running_answer(&tasks, &self.store_key(), chat).map(|task| task.id)
    }

    fn busy_tabs(&self, services: Option<&Services>) -> Vec<ChatId> {
        let Some(services) = services else { return Vec::new() };
        let Ok(tasks) = services.sched.store().list() else { return Vec::new() };
        let store = self.store_key();
        tasks
            .iter()
            .filter(|task| task.kind == ANSWER_KIND && task.scope.as_deref() == Some(store.as_str()) && !task.status.is_finished())
            .filter_map(|task| task.input.get("chat").and_then(|v| v.as_i64()))
            .collect()
    }

    /// The live answer of this chat with its task id (the corrections sent now belong to it).
    fn live_now(&self, services: Option<&Services>) -> Option<(i64, Live)> {
        let services = services?;
        let chat = self.current?;
        let tasks = services.sched.store().list().ok()?;
        let task = queued::running_answer(&tasks, &self.store_key(), chat)?;
        if !queued::answer_busy(task) {
            return None;
        }
        let id = task.id;
        let live = services.streams.get(id)?;
        (!live.closed()).then(|| (id, with_live_phrases(live, &tasks, id)))
    }

    /// The cursor of an answer that is waiting in a fresh process, so the continuation line can
    /// show before the next chunk is claimed.
    fn parked_answer(&self, services: Option<&Services>) -> Option<Parked> {
        let services = services?;
        let chat = self.current?;
        let tasks = services.sched.store().list().ok()?;
        let task = queued::running_answer(&tasks, &self.store_key(), chat)?;
        let has_live = services.streams.get(task.id).is_some();
        if !continued_cursor(&task.cursor, has_live) {
            return None;
        }
        let mut timeline = Timeline::from_details(&serde_json::json!({ "timeline": task.cursor.get("timeline").cloned().unwrap_or(serde_json::Value::Null) }));
        let mut steps: Vec<StepRecord> = serde_json::from_value(task.cursor.get("steps").cloned().unwrap_or(serde_json::Value::Null)).unwrap_or_default();
        let found = step_title::phrases(&tasks, task.id);
        apply_phrases(&mut timeline.entries, &found);
        for (index, phrase) in &found {
            if let Some(step) = steps.get_mut(*index) {
                step.title = phrase.clone();
            }
        }
        Some(Parked { timeline, steps })
    }

    fn waiting_line(&self, services: Option<&Services>) -> Option<String> {
        let services = services?;
        let chat = self.current?;
        let tasks = services.sched.store().list().ok()?;
        let store = self.store_key();
        let queued = tasks.iter().filter(|task| {
            task.kind == ANSWER_KIND
                && task.scope.as_deref() == Some(store.as_str())
                && task.input.get("chat").and_then(|v| v.as_i64()) == Some(chat)
                && !task.status.is_finished()
                && !queued::answer_busy(task)
        }).max_by_key(|task| task.id)?;
        Some(wait_line(services, queued, &store, &self.chats))
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rect: Rect, player: Option<&mut Player>, services: Option<&Services>, p: theme::Palette) -> Option<StudioAction> {
        self.refresh(false);
        self.poll(ui.ctx(), services);
        if let Some(s) = services {
            self.retitle(s);
        }
        ui.ctx().request_repaint_after(REFRESH);
        let roster = self.roster();
        let body = panel(ui, rect, &t!("studio.chat"), p);
        syntax_hint(ui, rect, &roster);
        let mut seek = None;
        ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(10.0, 8.0))), |ui| {
            if !self.ready() {
                ui.centered_and_justified(|ui| theme::hint(ui, t!("studio.chat_needs_text")));
                return;
            }
            // Dev: `MEDIAA_DEV_ASK=<question>` asks it once (UI checks).
            if let (false, Some(s), Ok(q)) = (self.dev_asked, services, std::env::var("MEDIAA_DEV_ASK")) {
                self.dev_asked = true;
                self.draft.set_plain(&q);
                self.send(s, player.as_deref(), &roster);
            }
            self.chats_bar(ui, &roster, services);
            let input_h = 86.0;
            let bare = self.bare_times();
            let live = self.live_now(services);
            let parked = self.parked_answer(services);
            let tasks = services.and_then(|s| s.sched.store().list().ok());
            // Sent while the answer runs and not taken yet: under «думаю…», not in the list.
            let pending = match (&live, self.current) {
                (Some((answer, live)), Some(_)) => queued::pending_steers(&self.messages, *answer, live.steers_taken),
                _ => Vec::new(),
            };
            let pending_texts: Vec<String> = pending.iter().map(|(_, text)| text.clone()).collect();
            let list = Rect::from_min_max(ui.cursor().min, egui::pos2(ui.max_rect().max.x, ui.max_rect().max.y - input_h));
            let mut typed_seek = None;
            ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).stick_to_bottom(true).show(ui, |ui| {
                    // Long words and wide Markdown never widen the pane.
                    ui.set_max_width(ui.available_width());
                    if self.messages.is_empty() && live.is_none() && parked.is_none() {
                        theme::hint(ui, t!("studio.chat_hint"));
                    }
                    let hidden = timeline::hidden_sections(&self.messages, live.is_some());
                    for (n, m) in self.messages.iter().enumerate() {
                        if hidden[n] || pending.iter().any(|(index, _)| *index == n) {
                            continue;
                        }
                        let overlaid = tasks.as_ref().map(|tasks| with_phrases(m, tasks));
                        let view = overlaid.as_ref().unwrap_or(m);
                        if let Some(t) = message_ui(ui, n, view, bare, &roster, p) {
                            seek = Some(t);
                        }
                    }
                    if let Some(line) = self.waiting_line(services) {
                        theme::hint(ui, line);
                    }
                    if let Some((_, live)) = &live {
                        if live.continuing {
                            theme::hint(ui, t!("studio.continuing"));
                        }
                        if let Some(t) = timeline::live_ui(ui, live, bare, &roster, p, &pending_texts) {
                            seek = Some(t);
                        }
                    } else if let Some(parked) = &parked {
                        // Queued after a crash between chunks: the cursor has steps and this process has no live stream.
                        theme::hint(ui, t!("studio.continuing"));
                        if let Some(t) = timeline::timeline_ui(ui, "parked", &parked.timeline.entries, &parked.steps, false, false, bare, &roster, p) {
                            seek = Some(t);
                        }
                    }
                    if let Some(e) = &self.error {
                        ui.colored_label(p.danger, e);
                    }
                });
            });
            let input = Rect::from_min_max(egui::pos2(ui.max_rect().min.x, ui.max_rect().max.y - input_h + 6.0), ui.max_rect().max);
            ui.scope_builder(egui::UiBuilder::new().max_rect(input), |ui| {
                typed_seek = self.input_ui(ui, player.as_deref(), services, &roster);
            });
            if seek.is_none() {
                if let Some(t) = typed_seek {
                    seek = Some(t.to_string());
                }
            }
        });
        self.delete_dialog(ui.ctx(), &roster);
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

    /// Tabs of the open chats, «+» (a new chat) and «История» (the archive, right of «+»).
    fn chats_bar(&mut self, ui: &mut egui::Ui, roster: &Roster, services: Option<&Services>) {
        let busy = self.busy_tabs(services);
        let tabs = tab_list(&self.chats, &self.open, self.current, &t!("studio.new_chat"), &busy);
        let archived = self.archived.clone();
        let salt = self.salt();
        let mut select = None;
        let mut close = None;
        let mut restore = None;
        let mut confirm = None;

        // Right to left: «История», «+», then the tabs in the room left.
        let row = egui::Layout::right_to_left(egui::Align::Center);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 34.0), row, |ui| {
            let history = theme::icon_button(ui, icon::CLOCK_COUNTER_CLOCKWISE, &t!("studio.history"));
            egui::Popup::menu(&history).show(|ui| {
                ui.set_min_width(320.0);
                ui.set_max_width(380.0);
                if archived.is_empty() {
                    theme::hint(ui, t!("studio.history_empty"));
                    return;
                }
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for (n, c) in archived.iter().enumerate() {
                        if n > 0 {
                            ui.separator();
                        }
                        ui.horizontal(|ui| {
                            // The title and date take the room the buttons leave.
                            let buttons = 200.0;
                            ui.allocate_ui_with_layout(
                                egui::vec2((ui.available_width() - buttons).max(80.0), 36.0),
                                egui::Layout::top_down(egui::Align::LEFT),
                                |ui| {
                                    let p = theme::palette(ui);
                                    let galley = mentions::galley(ui, &shown_title(&c.title), roster, egui::FontId::proportional(13.0), p.text, ui.available_width(), true, false, p);
                                    ui.add(egui::Label::new(galley).truncate());
                                    theme::hint(ui, chat_date(c.created_ms));
                                },
                            );
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.add(theme::secondary(icon::TRASH, &t!("studio.delete_chat"))).clicked() {
                                    confirm = Some(c.clone());
                                    ui.close();
                                }
                                if ui.add(theme::secondary(icon::ARROW_SQUARE_OUT, &t!("studio.open_chat"))).clicked() {
                                    restore = Some(c.id);
                                    ui.close();
                                }
                            });
                        });
                    }
                });
            });
            if theme::icon_button(ui, icon::PLUS, &t!("studio.new_chat")).clicked() {
                // Nothing is stored until the first question.
                select = Some(None);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                match tab_strip::tab_strip(ui, ("chat-tabs", &salt), &tabs, &t!("studio.close_chat"), roster) {
                    Some(TabAction::Selected(chat)) => select = Some(chat),
                    Some(TabAction::Closed(chat)) => close = chat,
                    None => {}
                }
            });
        });
        // An answer belongs to its chat: switching, closing and restoring stay available.
        if let Some(choice) = select {
            self.select(choice);
        }
        if let Some(chat) = close {
            self.close_tab(chat);
        }
        if let Some(chat) = restore {
            self.restore_chat(chat);
        }
        if let Some(info) = confirm {
            self.confirm_delete = Some(info);
        }
        ui.add_space(4.0);
    }

    /// Shows the chosen tab (`None`: the new-chat state).
    fn select(&mut self, choice: Option<ChatId>) {
        if self.current == choice {
            return;
        }
        self.current = choice;
        self.messages.clear();
        self.shown_sections = 0;
        if choice.is_some() {
            self.refresh(true);
        } else {
            // A new chat starts with the next question.
            self.read = Some(Instant::now());
        }
    }

    /// Closes a tab: the chat is archived, never deleted. Closing the current tab shows its
    /// neighbour, or the new-chat state when none is left.
    fn close_tab(&mut self, chat: ChatId) {
        let Some(t) = self.transcript() else { return };
        let Some(at) = self.open.iter().position(|&c| c == chat) else { return };
        let open = without_chat(&self.open, chat);
        if let Err(e) = t.set_open_chats(&open) {
            self.error = Some(format!("{e:#}"));
            return;
        }
        self.open = open;
        if self.current == Some(chat) {
            self.current = neighbour_after_close(&self.open, at);
            self.messages.clear();
            self.shown_sections = 0;
        }
        self.refresh(true);
    }

    /// Gives an archived chat its tab back and makes it current.
    fn restore_chat(&mut self, chat: ChatId) {
        let Some(t) = self.transcript() else { return };
        let open = with_restored(&self.open, chat);
        if let Err(e) = t.set_open_chats(&open) {
            self.error = Some(format!("{e:#}"));
            return;
        }
        self.open = open;
        self.current = Some(chat);
        self.messages.clear();
        self.shown_sections = 0;
        self.refresh(true);
    }

    /// The permanent-delete confirmation of an archived chat.
    fn delete_dialog(&mut self, ctx: &egui::Context, roster: &Roster) {
        let Some(info) = self.confirm_delete.clone() else { return };
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new(("chat-delete", self.salt()))).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(RichText::new(format!("{}  {}", icon::TRASH, t!("studio.delete_chat"))).size(15.0).strong());
            let confirm = t!("studio.delete_chat_confirm", title = shown_title(&info.title)).to_string();
            let p = theme::palette(ui);
            mentions::show(ui, &confirm, roster, egui::FontId::proportional(13.0), p.text, false, p);
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(theme::danger(ui, icon::TRASH, &t!("studio.delete_chat"))).clicked() {
                    decision = Some(true);
                }
                if ui.add(theme::secondary(icon::X, &t!("common.cancel"))).clicked() {
                    decision = Some(false);
                }
            });
        });
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        if let Some(ok) = decision {
            self.confirm_delete = None;
            if ok {
                match self.transcript().map(|t| t.delete_chat(info.id)) {
                    Some(Ok(())) => self.refresh(true),
                    Some(Err(e)) => self.error = Some(format!("{e:#}")),
                    None => {}
                }
            }
        }
    }

    fn input_ui(&mut self, ui: &mut egui::Ui, player: Option<&Player>, services: Option<&Services>, roster: &Roster) -> Option<f64> {
        // Keys of the list are taken before the editor, so arrows do not move the caret and
        // Enter does not send.
        if self.mention_open {
            let (up, down, enter, esc) = ui.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            let n = self.mention_rows.len();
            if up {
                self.mention.up(n);
            }
            if down {
                self.mention.down(n);
            }
            if esc {
                self.mention.dismiss(self.mention.at);
                self.mention_open = false;
            }
            if enter {
                self.insert_mention(roster);
            }
        }
        let id = ui.id().with("ask");
        self.draft.relabel(|label| roster.name_of(label).unwrap_or_default());
        self.take_quote(ui, id);
        if let Some(byte) = self.mention_caret.take() {
            place_cursor(ui, id, &self.draft.text, byte);
        }
        self.nudge_chips(ui, id);
        let chips = self.draft.chips.clone();
        let clocks = self.draft.clocks();
        let p = theme::palette(ui);
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| composer::layout(ui, text.as_str(), &chips, &clocks, wrap, p.text, p.accent, p.accent_soft);
        let saved = self.draft.text.clone();
        let saved_chips = self.draft.chips.clone();
        let editor = ui.horizontal(|ui| {
            let running = services.and_then(|s| self.running_answer_id(s)).is_some();
            let out = {
                let edit = egui::TextEdit::multiline(&mut self.draft.text).id(id).hint_text(t!("studio.ask_hint")).desired_rows(3).desired_width(ui.available_width() - 36.0);
                let edit = edit.layouter(&mut layouter);
                // Enabled while an answer runs too: a message sent then is a correction of it.
                edit.show(ui)
            };
            let resp = &out.response;
            let enter = resp.has_focus() && !self.mention_open && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
            if self.draft.text != saved {
                self.draft.chips = saved_chips;
                let new = std::mem::replace(&mut self.draft.text, saved);
                if let Some(at) = self.draft.reconcile(new) {
                    place_cursor(ui, id, &self.draft.text, at);
                }
            }
            if let Some(byte) = caret_byte(ui, id, &self.draft.text) {
                let snapped = self.draft.snap(byte);
                if snapped != byte {
                    place_cursor(ui, id, &self.draft.text, snapped);
                }
            }
            if enter {
                let cut = self.draft.text.trim_end_matches(['\n', '\r']).to_string();
                if cut.len() != self.draft.text.len() {
                    self.draft.text = cut;
                    let end = self.draft.text.len();
                    self.draft.chips.retain(|c| c.end <= end);
                }
            }
            let seek = resp.hover_pos().or(resp.interact_pointer_pos()).and_then(|pos| {
                let cursor = out.galley.cursor_from_pos(pos - out.galley_pos);
                let byte = chars_to_byte(&self.draft.text, cursor.index.0);
                let hit = self.draft.seek_at(byte);
                if hit.is_some() && resp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                resp.clicked().then_some(hit).flatten()
            });
            let clicked_send = ui.vertical(|ui| {
                if running {
                    theme::icon_button(ui, icon::STOP, &t!("studio.stop")).clicked()
                } else {
                    theme::icon_button(ui, icon::PAPER_PLANE_RIGHT, &t!("studio.send")).clicked()
                }
            }).inner;
            if running && clicked_send {
                if let Some(s) = services {
                    self.stop(s);
                }
            } else if (clicked_send || enter) && !self.draft.to_message().trim().is_empty() {
                match services {
                    Some(s) => self.send(s, player, roster),
                    None => self.error = Some(t!("studio.agent_not_ready").into()),
                }
            }
            (resp.response.rect, out.cursor_range, seek)
        });
        let (rect, cursor, seek) = editor.inner;
        self.sync_mentions(ui, roster, rect, cursor);
        seek
    }

    /// A quote left by «Ответить» on this recording, inserted as one chip.
    fn take_quote(&mut self, ui: &egui::Ui, id: egui::Id) {
        let Scope::File { path, .. } = &self.scope else { return };
        let key = path.to_string_lossy().into_owned();
        let quote_id = composer::quote_id(&key);
        let Some(quote) = ui.ctx().data(|d| d.get_temp::<composer::Quote>(quote_id)) else { return };
        ui.ctx().data_mut(|d| d.remove::<composer::Quote>(quote_id));
        let at = caret_byte(ui, id, &self.draft.text).unwrap_or(self.draft.text.len());
        self.draft.insert_quote(at, &quote);
        if let Some(chip) = self.draft.chips.iter().rev().find(|c| matches!(c.kind, composer::Kind::Quote { .. })) {
            self.mention_caret = Some((chip.end + 1).min(self.draft.text.len()));
        }
    }

    /// Backspace, Delete and the arrows treat a chip as one character.
    fn nudge_chips(&mut self, ui: &mut egui::Ui, id: egui::Id) {
        if !ui.memory(|m| m.has_focus(id)) {
            return;
        }
        let Some(byte) = caret_byte(ui, id, &self.draft.text) else { return };
        if let Some(start) = self.draft.chips.iter().find(|c| c.end == byte).map(|c| c.start) {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
                self.draft.backspace_at(byte);
                place_cursor(ui, id, &self.draft.text, start);
                return;
            }
        }
        if self.draft.chips.iter().any(|c| c.start == byte) && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete)) {
            self.draft.delete_at(byte);
            place_cursor(ui, id, &self.draft.text, byte);
            return;
        }
        if let Some(to) = self.draft.arrow(byte, false) {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowLeft)) {
                place_cursor(ui, id, &self.draft.text, to);
                return;
            }
        }
        if let Some(to) = self.draft.arrow(byte, true) {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowRight)) {
                place_cursor(ui, id, &self.draft.text, to);
            }
        }
    }

    /// Inserts the selected speaker in place of the `@` being typed. Only the list creates a chip.
    fn insert_mention(&mut self, roster: &Roster) {
        let Some(&row) = self.mention_rows.get(self.mention.selected) else { return };
        let Some(who) = roster.speakers.get(row) else { return };
        let label = match who.recording {
            Some(r) => speakers::folder_label(who.index, r),
            None => speakers::label(who.index),
        };
        self.draft.insert_mention(self.mention.at, &self.mention_query, &label, &Roster::row_label(who));
        let caret = self.draft.chips.iter().rev().find(|c| matches!(&c.kind, composer::Kind::Mention { label: l } if l == &label)).map(|c| (c.end + 1).min(self.draft.text.len()));
        self.mention_caret = caret;
        // The inserted `@` sits at the same index; leave it closed until the next `@`.
        self.mention.dismiss(self.mention.at);
        self.mention_open = false;
        self.mention_rows.clear();
        self.mention_query.clear();
    }

    /// Follows the caret: `@` opens the list above the editor (an overlay, so the row stays put).
    fn sync_mentions(&mut self, ui: &mut egui::Ui, roster: &Roster, rect: Rect, cursor: Option<egui::text::CCursorRange>) {
        let byte = cursor.map(|r| chars_to_byte(&self.draft.text, r.primary.index.0)).unwrap_or(self.draft.text.len());
        let query = self.draft.mention_query(byte);
        let rows = query.as_ref().map(|q| speakers::matching(&roster.speakers, &q.query, |i| t!("studio.speaker", n = i + 1).into())).unwrap_or_default();
        self.mention.update(query.as_ref(), rows.len());
        self.mention_open = self.mention.is_open(query.as_ref(), rows.len());
        self.mention_query = query.map(|q| q.query).unwrap_or_default();
        self.mention_rows = rows;
        if !self.mention_open {
            return;
        }
        let p = theme::palette(ui);
        let visible = self.mention_rows.len().min(6);
        let h = visible as f32 * 26.0 + 8.0;
        let pos = egui::pos2(rect.left(), rect.top() - h - 4.0);
        let rows = self.mention_rows.clone();
        let selected = self.mention.selected;
        let mut pick = None;
        egui::Area::new(ui.id().with("speaker-mentions")).order(egui::Order::Foreground).fixed_pos(pos).show(ui.ctx(), |ui| {
            egui::Frame::new().fill(p.card).stroke(egui::Stroke::new(1.0, p.border)).corner_radius(6).inner_margin(4.0).show(ui, |ui| {
                ui.set_min_width(240.0);
                egui::ScrollArea::vertical().max_height(6.0 * 26.0).show(ui, |ui| {
                    for (i, &row) in rows.iter().enumerate() {
                        let Some(who) = roster.speakers.get(row) else { continue };
                        let (r, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width().max(240.0), 26.0), egui::Sense::click());
                        if i == selected || resp.hovered() {
                            ui.painter().rect_filled(r, egui::CornerRadius::same(4), if i == selected { p.accent_soft } else { p.card_hover });
                        }
                        ui.painter().text(egui::pos2(r.left() + 8.0, r.center().y), egui::Align2::LEFT_CENTER, Roster::row_label(who), egui::FontId::proportional(13.0), p.text);
                        if resp.clicked() {
                            pick = Some(i);
                        }
                    }
                });
            });
        });
        if let Some(i) = pick {
            self.mention.selected = i;
            self.insert_mention(roster);
        }
    }
}

/// Puts the caret at `byte` in the chat editor (character index, as egui stores it).
fn place_cursor(ui: &egui::Ui, id: egui::Id, text: &str, byte: usize) {
    use egui::text::{CCursor, CCursorRange};
    let chars = text.get(..byte.min(text.len())).map(|s| s.chars().count()).unwrap_or(0);
    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::one(CCursor::new(chars))));
    state.store(ui.ctx(), id);
}

fn chars_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices().nth(chars).map(|(i, _)| i).unwrap_or(text.len())
}

/// The caret as a byte, when it is a single point (a selection is left to the editor).
fn caret_byte(ui: &egui::Ui, id: egui::Id, text: &str) -> Option<usize> {
    let state = egui::text_edit::TextEditState::load(ui.ctx(), id)?;
    let range = state.cursor.char_range()?;
    if range.primary.index != range.secondary.index {
        return None;
    }
    Some(chars_to_byte(text, range.primary.index.0))
}

/// «i» just after the chat title. The right edge of the header belongs to the pane button.
fn syntax_hint(ui: &mut egui::Ui, pane: Rect, roster: &Roster) {
    let title = t!("studio.chat").to_string();
    let title_w = ui.fonts_mut(|f| f.layout_no_wrap(title, FontId::proportional(12.0), egui::Color32::PLACEHOLDER)).size().x;
    let side = theme::ICON_BUTTON;
    let x = pane.left() + 12.0 + title_w + 4.0;
    if x + side > pane.right() - side - 8.0 {
        return;
    }
    let y = pane.top() + (studio::PANE_HEAD - side) / 2.0;
    let rect = Rect::from_min_size(egui::pos2(x, y), egui::vec2(side, side));
    let resp = ui.interact(rect, ui.id().with("chat-syntax"), egui::Sense::hover());
    if ui.is_rect_visible(rect) {
        let p = theme::palette(ui);
        let color = if resp.hovered() { p.text } else { p.text_weak };
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::INFO, FontId::proportional(14.0), color);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    topics::text_tip(&resp, &t!("chat.syntax"), roster);
}

/// A crash between chunks: the task is still queued, its cursor already has steps, and this
/// process has no live stream. A yield in the same process still has the stream.
fn continued_cursor(cursor: &serde_json::Value, has_live: bool) -> bool {
    if has_live || cursor.is_null() {
        return false;
    }
    cursor.get("steps").and_then(|v| v.as_array()).is_some_and(|steps| !steps.is_empty())
}

struct Parked {
    timeline: Timeline,
    steps: Vec<StepRecord>,
}

fn apply_phrases(entries: &mut [TimelineEntry], phrases: &[(usize, String)]) {
    for (index, phrase) in phrases {
        let mut n = 0;
        for entry in entries.iter_mut() {
            if let TimelineEntry::Step { title, .. } = entry {
                if n == *index {
                    *title = phrase.clone();
                    break;
                }
                n += 1;
            }
        }
    }
}

fn with_live_phrases(mut live: Live, tasks: &[Task], answer: i64) -> Live {
    let found = step_title::phrases(tasks, answer);
    if found.is_empty() {
        return live;
    }
    apply_phrases(&mut live.timeline.entries, &found);
    for (index, phrase) in found {
        if let Some(step) = live.steps.get_mut(index) {
            step.title = phrase;
        }
    }
    live
}

fn with_phrases(message: &ChatMessage, tasks: &[Task]) -> ChatMessage {
    let Some(id) = message.details.get("task").and_then(|v| v.as_i64()) else {
        return message.clone();
    };
    let found = step_title::phrases(tasks, id);
    if found.is_empty() {
        return message.clone();
    }
    let mut message = message.clone();
    let mut timeline = message.timeline();
    apply_phrases(&mut timeline.entries, &found);
    message.details["timeline"] = timeline.to_value();
    if let Some(steps) = message.details.get_mut("steps").and_then(|v| v.as_array_mut()) {
        for (index, phrase) in &found {
            if let Some(step) = steps.get_mut(*index).and_then(|v| v.as_object_mut()) {
                step.insert("title".to_string(), serde_json::json!(phrase));
            }
        }
    }
    message
}

/// What a chat is called on screen: its title, or «Новый чат» until the titles model has
/// written one.
fn shown_title(title: &str) -> String {
    if title.is_empty() {
        t!("studio.new_chat").to_string()
    } else {
        title.to_string()
    }
}

/// The chat to name again and the question to name it by: the open chat has no title yet, has a
/// question, and was not tried in this session.
fn retitle_candidate(chats: &[ChatInfo], current: Option<ChatId>, messages: &[ChatMessage], tried: &std::collections::HashSet<ChatId>) -> Option<(ChatId, String)> {
    let chat = current?;
    if tried.contains(&chat) || chats.iter().find(|c| c.id == chat).is_none_or(|c| !c.title.is_empty()) {
        return None;
    }
    let question = messages.iter().find(|m| m.author == Author::User)?.text.trim().to_string();
    (!question.is_empty()).then_some((chat, question))
}

/// The tabs to draw: the open chats in their tab order, and «Новый чат» while no chat is
/// current (a new chat is stored only with its first question); a chat whose title is not
/// written yet is called the same.
fn tab_list(chats: &[ChatInfo], open: &[ChatId], current: Option<ChatId>, new_title: &str, busy: &[ChatId]) -> Vec<Tab<Option<ChatId>>> {
    let mut tabs: Vec<Tab<Option<ChatId>>> = open
        .iter()
        .filter_map(|&id| chats.iter().find(|c| c.id == id))
        .map(|c| {
            let title = if c.title.is_empty() { new_title.to_string() } else { c.title.clone() };
            let title = if busy.contains(&c.id) { format!("● {title}") } else { title };
            Tab::new(Some(c.id), title, current == Some(c.id))
        })
        .collect();
    if current.is_none() {
        tabs.push(Tab::new(None, new_title, true).fixed());
    }
    tabs
}

/// The tabs without `chat` (closing archives it; the others keep their order).
fn without_chat(open: &[ChatId], chat: ChatId) -> Vec<ChatId> {
    open.iter().copied().filter(|&c| c != chat).collect()
}

/// The chat shown after the tab at `at` (its index before closing) is gone: the right
/// neighbour that took its place, else the left one, else the new-chat state.
fn neighbour_after_close(open: &[ChatId], at: usize) -> Option<ChatId> {
    open.get(at).or_else(|| at.checked_sub(1).and_then(|i| open.get(i))).copied()
}

/// The tabs with `chat` restored: appended unless it is open already.
fn with_restored(open: &[ChatId], chat: ChatId) -> Vec<ChatId> {
    let mut out = open.to_vec();
    if !out.contains(&chat) {
        out.push(chat);
    }
    out
}

/// Local model uses the `llm` pool; a provider model uses the provider pool.
fn answer_resource(settings: &AgentSettings) -> (Resource, Option<String>) {
    match settings.models.agent.as_ref() {
        Some(ModelSource::Provider { model, .. }) => (Resource::Provider, Some(model.clone())),
        Some(source) => (Resource::Llm, local_spec(source).ok()),
        None => (Resource::Llm, None),
    }
}

/// A title task for this chat that has not finished.
fn title_pending(services: &Services, store: &str, chat: ChatId) -> bool {
    let Ok(tasks) = services.sched.store().list() else { return false };
    tasks.iter().any(|task| {
        task.kind == TITLE_KIND && task.scope.as_deref() == Some(store) && task.input.get("chat").and_then(|v| v.as_i64()) == Some(chat) && !task.status.is_finished()
    })
}

/// Why this chat's answer has not started. A busy local model names the chat that holds it.
fn wait_line(services: &Services, task: &Task, store: &str, chats: &[ChatInfo]) -> String {
    let reason = services.sched.snapshot().ok().and_then(|snap| snap.tasks.into_iter().find(|view| view.task.id == task.id).and_then(|view| view.wait));
    let holder = services.sched.store().list().ok().and_then(|tasks| {
        tasks.into_iter().find(|other| {
            other.kind == ANSWER_KIND
                && other.scope.as_deref() == Some(store)
                && queued::answer_busy(other)
                && other.resource == Some(Resource::Llm)
                && other.input.get("chat").and_then(|v| v.as_i64()) != task.input.get("chat").and_then(|v| v.as_i64())
        })
    });
    let model_wait = matches!(reason.as_ref(), Some(WaitReason::Turn | WaitReason::Resource(Resource::Llm)));
    if model_wait {
        if let Some(other) = holder {
            let id = other.input.get("chat").and_then(|v| v.as_i64());
            let title = id.and_then(|id| chats.iter().find(|c| c.id == id)).map(|c| shown_title(&c.title)).unwrap_or_else(|| t!("studio.new_chat").into());
            return t!("studio.wait_model_chat", title = title).into();
        }
    }
    match reason {
        Some(WaitReason::Dependency) => t!("queue.wait_dependency"),
        Some(WaitReason::Resource(Resource::Speech)) => t!("queue.wait_speech"),
        Some(WaitReason::Resource(Resource::Embed)) => t!("queue.wait_embed"),
        Some(WaitReason::Resource(Resource::Provider)) => t!("queue.wait_provider"),
        Some(WaitReason::Memory) => t!("queue.wait_memory_pool"),
        Some(WaitReason::Paused) => t!("queue.wait_paused"),
        _ => t!("studio.wait_turn"),
    }
    .into()
}

/// A chat's date for the history list.
fn chat_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms).map(|d| d.with_timezone(&chrono::Local).format("%d.%m.%Y").to_string()).unwrap_or_default()
}

/// A message (the `n`-th of the chat); returns the time of a clicked citation. An answer is its
/// timeline: the final one folds its steps and reasoning, a stopped one shows them as they were.
fn message_ui(ui: &mut egui::Ui, n: usize, m: &ChatMessage, bare_times: bool, roster: &Roster, p: theme::Palette) -> Option<String> {
    match m.author {
        Author::User => {
            let mut seek = None;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                egui::Frame::new().fill(p.accent_soft).corner_radius(10).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                    ui.set_max_width(ui.available_width() * 0.85);
                    seek = composer::show_user(ui, &m.text, |label| roster.name_of(label), FontId::proportional(13.5), p.text, p.accent, p.accent_soft).map(|t| t.to_string());
                });
            });
            ui.add_space(timeline::GAP);
            seek
        }
        Author::Agent => {
            let timeline = m.timeline();
            let mut seek = None;
            if timeline.is_empty() {
                // A section, or an answer from before timelines: just the text.
                if !m.text.is_empty() {
                    seek = super::markdown::show(ui, &linkify(&m.text, bare_times), roster, p);
                }
            } else {
                let forced = m.details["forced"].as_bool() == Some(true);
                let steps: Vec<mediaa_agent::assistant::StepRecord> = serde_json::from_value(m.details["steps"].clone()).unwrap_or_default();
                seek = timeline::timeline_ui(ui, ("message", n), &timeline.entries, &steps, true, forced, bare_times, roster, p);
                // The numbers of the finished answer stay in its details.
                let metrics: Option<mediaa_agent::assistant::Metrics> = serde_json::from_value(m.details["metrics"].clone()).ok();
                if let Some(metrics) = metrics {
                    timeline::metrics_ui(ui, Some(&metrics), p);
                }
            }
            if m.details["continuing"].as_bool() == Some(true) {
                theme::hint(ui, t!("studio.continuing"));
            }
            if m.stopped() {
                theme::hint(ui, t!("studio.stopped"));
            }
            ui.add_space(timeline::GAP);
            seek
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_queued_cursor_with_steps_continues_only_when_there_is_no_live_stream() {
        let cursor = serde_json::json!({ "steps": [{ "title": "Поиск" }] });
        assert!(continued_cursor(&cursor, false));
        assert!(!continued_cursor(&cursor, true), "a yield in this process still has the stream");
        assert!(!continued_cursor(&serde_json::Value::Null, false));
        assert!(!continued_cursor(&serde_json::json!({ "steps": [] }), false));
    }

    #[test]
    fn a_chat_without_a_title_is_named_again_once() {
        let info = |id: ChatId, title: &str| ChatInfo { id, title: title.into(), created_ms: 1, messages: 1 };
        let user = |text: &str| ChatMessage { author: Author::User, text: text.into(), details: serde_json::Value::Null };
        let chats = [info(5, ""), info(7, "Семь")];
        let messages = [user("  Когда отчёт? "), user("и что?")];
        let mut tried = std::collections::HashSet::new();
        assert_eq!(retitle_candidate(&chats, Some(5), &messages, &tried), Some((5, "Когда отчёт?".to_string())), "the first question names it");
        assert_eq!(retitle_candidate(&chats, Some(7), &messages, &tried), None, "it has a title");
        assert_eq!(retitle_candidate(&chats, None, &messages, &tried), None, "the new-chat tab");
        assert_eq!(retitle_candidate(&chats, Some(5), &[], &tried), None, "nothing to name it by");
        tried.insert(5);
        assert_eq!(retitle_candidate(&chats, Some(5), &messages, &tried), None, "only once");
    }

    #[test]
    fn citations_become_seek_links() {
        assert_eq!(linkify("В пятницу [1:05].", true), "В пятницу [1:05](seek:65).");
        assert_eq!(linkify("[0:00–0:11] и [1:02:30]", true), "[0:00–0:11](seek:0) и [1:02:30](seek:3750)");
        assert_eq!(linkify("уже [1:05](seek:65), [см.] и [5]", true), "уже [1:05](seek:65), [см.] и [5]");
        assert_eq!(linkify("[незакрытая", true), "[незакрытая");
        assert_eq!(linkify("в [#2 1:05] и [#10 0:00–0:30]", true), "в [#2 1:05](seek:2:65) и [#10 0:00–0:30](seek:10:0)");
        assert_eq!(linkify("в моменте 0:00–0:11.", true), "в моменте [0:00](seek:0)–[0:11](seek:11).");
        assert_eq!(linkify("в 1:02:30, не 3.14:15, не 123:45, не 12:5", true), "в [1:02:30](seek:3750), не 3.14:15, не 123:45, не 12:5");
        assert_eq!(linkify("длится 26:39, см. [#2 1:05]", false), "длится 26:39, см. [#2 1:05](seek:2:65)", "a folder: no bare times");
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
    fn tabs_close_to_a_neighbour_restore_and_start_a_new_chat() {
        let info = |id: ChatId, title: &str| ChatInfo { id, title: title.into(), created_ms: 1, messages: 0 };
        let chats = [info(5, "Пять"), info(7, "Семь"), info(9, "Девять")];
        let open = [5, 7, 9];
        // Closing the current tab shows its right neighbour; the last one shows its left.
        assert_eq!(neighbour_after_close(&without_chat(&open, 7), 1), Some(9));
        assert_eq!(neighbour_after_close(&without_chat(&open, 9), 2), Some(7));
        assert_eq!(neighbour_after_close(&without_chat(&open, 5), 0), Some(7));
        // The only tab closed: the new-chat state.
        assert_eq!(neighbour_after_close(&without_chat(&[5], 5), 0), None);
        let tabs = tab_list(&chats, &[], None, "Новый чат", &[]);
        assert_eq!(tabs.len(), 1, "only «Новый чат»");
        assert_eq!(tabs[0].id, None);
        assert!(tabs[0].active && !tabs[0].closable, "there is no stored chat to close");

        // «+»: the chat tabs stay, the new-chat tab is added last and is current.
        let tabs = tab_list(&chats, &open, None, "Новый чат", &[]);
        assert_eq!(tabs.iter().map(|t| t.id).collect::<Vec<_>>(), [Some(5), Some(7), Some(9), None]);
        assert!(tabs.last().is_some_and(|t| t.active) && tabs[..3].iter().all(|t| !t.active));

        // Restoring appends the chat and makes it current.
        let mut open = without_chat(&open, 7);
        assert_eq!(open, [5, 9]);
        open = with_restored(&open, 7);
        assert_eq!(open, [5, 9, 7], "a closed tab does not keep its place");
        assert_eq!(with_restored(&open, 7), open, "already open");
        let tabs = tab_list(&chats, &open, Some(7), "Новый чат", &[]);
        assert_eq!(tabs.iter().map(|t| t.id).collect::<Vec<_>>(), [Some(5), Some(9), Some(7)]);
        assert!(tabs[2].active && tabs[..2].iter().all(|t| !t.active));
    }

    #[test]
    fn a_message_sent_while_the_answer_runs_is_a_correction_of_it() {
        // The input stays enabled while an answer runs; what is sent then corrects that answer.
        assert_eq!(sent_as(None), Sent::Question);
        assert_eq!(sent_as(Some(7)), Sent::Correction(7));
    }

    #[test]
    fn corrections_wait_under_the_answer_until_taken() {
        let steer = |task: i64, text: &str| ChatMessage { author: Author::User, text: text.into(), details: serde_json::json!({ "steer": { "task": task } }) };
        let plain = ChatMessage { author: Author::User, text: "Когда отчёт?".into(), details: serde_json::Value::Null };
        let messages = [plain, steer(7, "Ещё про бюджет."), steer(7, "И про сроки."), steer(9, "Другое.")];
        let pending = |answer, taken| queued::pending_steers(&messages, answer, taken);
        assert_eq!(pending(7, 0), [(1, "Ещё про бюджет.".to_string()), (2, "И про сроки.".to_string())], "both under «думаю…», in order");
        // Taken ones leave the list and the hint; a correction of another answer is not this one's.
        assert_eq!(pending(7, 1).iter().map(|(index, _)| *index).collect::<Vec<_>>(), [2]);
        assert!(pending(7, 2).is_empty());
        assert_eq!(pending(9, 0).len(), 1);
    }

    #[test]
    fn a_chat_without_a_title_is_a_new_chat_until_it_is_named() {
        let info = |id: ChatId, title: &str| ChatInfo { id, title: title.into(), created_ms: 1, messages: 2 };
        let chats = [info(5, ""), info(7, "Приветствие")];
        let tabs = tab_list(&chats, &[5, 7], Some(5), "Новый чат", &[]);
        assert_eq!(tabs.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Новый чат", "Приветствие"]);
        let marked = tab_list(&chats, &[5, 7], Some(5), "Новый чат", &[7]);
        assert_eq!(marked.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Новый чат", "● Приветствие"]);
        assert_eq!(shown_title(""), t!("studio.new_chat"));
        assert_eq!(shown_title("Сроки"), "Сроки");
    }
}
