use super::composer::{self, Field};
use super::mentions::{self, Roster};
use super::studio::{self, panel, StudioAction};
use super::tab_strip::{self, Tab, TabAction};
use super::timeline;
use super::topics;
use crate::features::agent::view as queue;
use crate::features::agent::view::{
    answer_busy, parse_time, running_answer, Resource, Task, WaitReason, ANSWER_KIND,
};
use crate::features::library::player::Player;
use crate::ui::theme;
use eframe::egui::{self, FontId, Rect, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::assistant::{Live, StepRecord};
use mediaa_core::chat::{Author, ChatId, ChatInfo, ChatMessage, Timeline, TimelineEntry};
use mediaa_core::speakers::{self, Popup};
use mediaa_core::transcript::{self, Transcript};
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub enum AgentCommand {
    Send(mediaa_protocol::backend::AgentSendWire),
    Stop(i64),
    Resume(i64),
    Steer(mediaa_protocol::backend::AgentSteerWire),
}

static AGENT_COMMANDS: OnceLock<crossbeam_channel::Sender<AgentCommand>> = OnceLock::new();
static AGENT_CHAT: AtomicI64 = AtomicI64::new(-1);

pub fn set_agent_chat(chat: i64) {
    AGENT_CHAT.store(chat, Ordering::Relaxed);
}

fn take_agent_chat() -> Option<i64> {
    let chat = AGENT_CHAT.swap(-1, Ordering::Relaxed);
    (chat >= 0).then_some(chat)
}

pub fn install_agent_commands(sender: crossbeam_channel::Sender<AgentCommand>) {
    let _ = AGENT_COMMANDS.set(sender);
}

pub fn send_agent_stop(id: i64) -> bool {
    send_agent_command(AgentCommand::Stop(id))
}

pub fn send_agent_resume(id: i64) -> bool {
    send_agent_command(AgentCommand::Resume(id))
}

fn send_agent_command(command: AgentCommand) -> bool {
    AGENT_COMMANDS
        .get()
        .is_some_and(|sender| sender.try_send(command).is_ok())
}
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(5);

fn link_bare(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let starts = chars[i].is_ascii_digit()
            && (i == 0
                || !(chars[i - 1].is_alphanumeric() || matches!(chars[i - 1], ':' | '.' | ',')));
        if starts {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ':') {
                j += 1;
            }
            let token: String = chars[i..j]
                .iter()
                .collect::<String>()
                .trim_end_matches(':')
                .to_string();
            let shape = token.split(':').collect::<Vec<_>>();
            let ok = (2..=3).contains(&shape.len())
                && shape[0].len() <= 2
                && shape[1..].iter().all(|p| p.len() == 2);
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

pub fn linkify(text: &str, bare_times: bool) -> String {
    let bare = |s: &str| {
        if bare_times {
            link_bare(s)
        } else {
            s.to_string()
        }
    };
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&bare(&rest[..open]));
        let after = &rest[open + 1..];
        let close = after.find(']');
        if let Some(end) = close
            .filter(|&c| after[c + 1..].starts_with('('))
            .and_then(|c| after[c + 1..].find(')').map(|e| c + 1 + e))
        {
            out.push('[');
            out.push_str(&after[..=end]);
            rest = &after[end + 1..];
            continue;
        }
        let link = close.and_then(|c| {
            let inside = &after[..c];
            let (recording, time) = match inside.strip_prefix('#').and_then(|r| r.split_once(' ')) {
                Some((n, time)) => (Some(n.parse::<usize>().ok()?), time),
                None => (None, inside),
            };
            let start = time.split(['–', '-']).next().unwrap_or_default();
            let t = start.contains(':').then(|| parse_time(start)).flatten()?;
            Some((
                c,
                inside,
                recording.map_or_else(|| t.to_string(), |n| format!("{n}:{t}")),
            ))
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

enum Jump {
    Here(f64),
    Open(PathBuf, f64),
}

enum Scope {
    File {
        path: PathBuf,
        file_id: Option<String>,
    },
    Folder {
        store: String,
        files: Vec<PathBuf>,
    },
}

pub struct Chat {
    scope: Scope,
    ai_dir: PathBuf,
    chats: Vec<ChatInfo>,
    open: Vec<ChatId>,
    archived: Vec<ChatInfo>,
    current: Option<ChatId>,
    loaded: bool,
    messages: Vec<ChatMessage>,
    draft: Field,
    shown_sections: usize,
    error: Option<String>,
    confirm_delete: Option<ChatInfo>,
    read: Option<Instant>,
    dev_asked: bool,
    mention: Popup,
    mention_open: bool,
    mention_rows: Vec<usize>,
    mention_query: String,
    mention_caret: Option<usize>,
}

impl Chat {
    pub fn new(path: &Path, ai_dir: &Path) -> Chat {
        Chat::with_scope(
            Scope::File {
                path: path.to_path_buf(),
                file_id: transcript::file_id(path).ok(),
            },
            ai_dir,
        )
    }

    pub fn folder(node: i64, files: Vec<PathBuf>, ai_dir: &Path) -> Chat {
        Chat::with_scope(
            Scope::Folder {
                store: format!("folder-{node}"),
                files,
            },
            ai_dir,
        )
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

    fn roster(&self) -> Roster {
        match &self.scope {
            Scope::File { file_id, .. } => file_id
                .as_ref()
                .map(|id| Roster::of_file(&self.ai_dir, id))
                .unwrap_or_default(),
            Scope::Folder { files, .. } => Roster::of_files(&self.ai_dir, files),
        }
    }

    fn transcript(&self) -> Option<Transcript> {
        match &self.scope {
            Scope::File { file_id, .. } => {
                let id = file_id.as_ref()?;
                Transcript::exists(&self.ai_dir, id)
                    .then(|| Transcript::open(&self.ai_dir, id).ok())
                    .flatten()
            }
            Scope::Folder { store, .. } => Transcript::open(&self.ai_dir, store).ok(),
        }
    }

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
            self.current = self.open.first().copied();
        }
        self.messages = self
            .current
            .and_then(|c| t.messages(c).ok())
            .unwrap_or_default();
    }

    fn ready(&self) -> bool {
        match &self.scope {
            Scope::File { file_id, .. } => file_id
                .as_ref()
                .is_some_and(|id| Transcript::exists(&self.ai_dir, id)),
            Scope::Folder { .. } => true,
        }
    }

    fn send(&mut self, player: Option<&Player>, roster: &Roster) {
        self.draft.name_mentions(roster);
        let question = self.draft.to_message().trim().to_string();
        let Some(t) = self.transcript() else { return };
        if question.is_empty() {
            return;
        }
        if let Some(answer) = self.running_answer_id() {
            if send_agent_command(AgentCommand::Steer(
                mediaa_protocol::backend::AgentSteerWire {
                    task: answer,
                    text: question.clone(),
                },
            )) {
                self.draft = Field::default();
                self.refresh(true);
                return;
            }
        }
        let chat = match self.current {
            Some(chat) => chat,
            None => match t.create_chat("") {
                Ok(chat) => chat,
                Err(error) => {
                    self.error = Some(format!("{error:#}"));
                    return;
                }
            },
        };
        if let Err(error) = t.add_message(chat, Author::User, &question, &serde_json::Value::Null) {
            self.error = Some(format!("{error:#}"));
            return;
        }
        let mut open = t.open_chats().unwrap_or_default();
        if !open.contains(&chat) {
            open.push(chat);
            let _ = t.set_open_chats(&open);
        }
        self.current = Some(chat);
        self.draft = Field::default();
        self.error = None;
        self.refresh(true);
        let locale = rust_i18n::locale().to_string();
        let wire = match &self.scope {
            Scope::File { path, .. } => mediaa_protocol::backend::AgentSendWire {
                store: self.store_key(),
                chat: Some(chat),
                question,
                locale,
                path: Some(path.to_string_lossy().into_owned()),
                files: Vec::new(),
                duration: player.map(|player| player.duration()),
                position: player.map(|player| player.position()),
                stored: true,
            },
            Scope::Folder { files, .. } => mediaa_protocol::backend::AgentSendWire {
                store: self.store_key(),
                chat: Some(chat),
                question,
                locale,
                path: None,
                files: files
                    .iter()
                    .map(|path| path.to_string_lossy().into_owned())
                    .collect(),
                duration: None,
                position: None,
                stored: true,
            },
        };
        if !send_agent_command(AgentCommand::Send(wire)) {
            self.error = Some(t!("studio.agent_not_ready").into());
        }
    }

    fn store_key(&self) -> String {
        match &self.scope {
            Scope::File { file_id, .. } => file_id.clone().unwrap_or_default(),
            Scope::Folder { store, .. } => store.clone(),
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(snapshot) = super::live_buffer::snapshot() else {
            return;
        };
        let tasks = queue::tasks(&snapshot);
        let store = self.store_key();
        let running = tasks.iter().any(|task| {
            task.kind == ANSWER_KIND
                && task.scope.as_deref() == Some(store.as_str())
                && !task.status.is_finished()
        });
        if running {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
        let Some(chat) = self.current else {
            return;
        };
        let Some(task) = running_answer(&tasks, &store, chat) else {
            return;
        };
        if let Some(live) = super::live_buffer::live(task.id) {
            if live.stored != self.shown_sections {
                self.shown_sections = live.stored;
                self.refresh(true);
            }
        }
    }

    fn stop(&mut self) {
        let Some(chat) = self.current else { return };
        let tasks = snapshot_tasks();
        let Some(task) = running_answer(&tasks, &self.store_key(), chat) else {
            return;
        };
        if send_agent_command(AgentCommand::Stop(task.id)) {
            self.refresh(true);
        }
    }

    fn running_answer_id(&self) -> Option<i64> {
        let chat = self.current?;
        let tasks = snapshot_tasks();
        running_answer(&tasks, &self.store_key(), chat).map(|task| task.id)
    }

    fn busy_tabs(&self) -> Vec<ChatId> {
        let tasks = snapshot_tasks();
        let store = self.store_key();
        tasks
            .iter()
            .filter(|task| {
                task.kind == ANSWER_KIND
                    && task.scope.as_deref() == Some(store.as_str())
                    && !task.status.is_finished()
            })
            .filter_map(|task| task.input.get("chat").and_then(|value| value.as_i64()))
            .collect()
    }

    fn live_now(&self) -> Option<(i64, Live)> {
        let chat = self.current?;
        let tasks = snapshot_tasks();
        let task = running_answer(&tasks, &self.store_key(), chat)?;
        if !answer_busy(task) {
            return None;
        }
        let id = task.id;
        let live = super::live_buffer::live(id)?;
        (!live.closed()).then(|| (id, with_live_phrases(live, &tasks, id)))
    }

    fn parked_answer(&self) -> Option<Parked> {
        let chat = self.current?;
        let tasks = snapshot_tasks();
        let task = running_answer(&tasks, &self.store_key(), chat)?;
        let has_live = super::live_buffer::live(task.id).is_some();
        if !continued_cursor(&task.cursor, has_live) {
            return None;
        }
        let mut timeline = Timeline::from_details(
            &serde_json::json!({ "timeline": task.cursor.get("timeline").cloned().unwrap_or(serde_json::Value::Null) }),
        );
        let mut steps: Vec<StepRecord> = serde_json::from_value(
            task.cursor
                .get("steps")
                .cloned()
                .unwrap_or(serde_json::Value::Null),
        )
        .unwrap_or_default();
        let found = queue::phrases(&tasks, task.id);
        apply_phrases(&mut timeline.entries, &found);
        for (index, phrase) in &found {
            if let Some(step) = steps.get_mut(*index) {
                step.title = phrase.clone();
            }
        }
        Some(Parked { timeline, steps })
    }

    fn waiting_line(&self) -> Option<String> {
        let chat = self.current?;
        let tasks = snapshot_tasks();
        let store = self.store_key();
        let queued_task = tasks
            .iter()
            .filter(|task| {
                task.kind == ANSWER_KIND
                    && task.scope.as_deref() == Some(store.as_str())
                    && task.input.get("chat").and_then(|value| value.as_i64()) == Some(chat)
                    && !task.status.is_finished()
                    && !answer_busy(task)
            })
            .max_by_key(|task| task.id)?;
        Some(wait_line(queued_task, &tasks, &store, &self.chats))
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        player: Option<&mut Player>,
        p: theme::Palette,
    ) -> Option<StudioAction> {
        self.refresh(false);
        self.poll(ui.ctx());
        ui.ctx().request_repaint_after(REFRESH);
        if let Some(chat) = take_agent_chat() {
            self.current = Some(chat);
            self.refresh(true);
        }
        let roster = self.roster();
        let body = panel(ui, rect, &t!("studio.chat"), p);
        syntax_hint(ui, rect, &roster);
        let mut seek = None;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(10.0, 8.0))),
            |ui| {
                if !self.ready() {
                    ui.centered_and_justified(|ui| theme::hint(ui, t!("studio.chat_needs_text")));
                    return;
                }
                if let (false, Ok(q)) = (self.dev_asked, std::env::var("MEDIAA_DEV_ASK")) {
                    self.dev_asked = true;
                    self.draft.set_plain(&q);
                    self.send(player.as_deref(), &roster);
                }
                self.chats_bar(ui, &roster);
                let input_h = 86.0;
                let bare = self.bare_times();
                let live = self.live_now();
                let parked = self.parked_answer();
                let tasks = snapshot_tasks();
                let pending = match (&live, self.current) {
                    (Some((answer, live)), Some(_)) => {
                        pending_steers(&self.messages, *answer, live.steers_taken)
                    }
                    _ => Vec::new(),
                };
                let pending_texts: Vec<String> =
                    pending.iter().map(|(_, text)| text.clone()).collect();
                let list = Rect::from_min_max(
                    ui.cursor().min,
                    egui::pos2(ui.max_rect().max.x, ui.max_rect().max.y - input_h),
                );
                let mut typed_seek = None;
                ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width());
                            if self.messages.is_empty() && live.is_none() && parked.is_none() {
                                theme::hint(ui, t!("studio.chat_hint"));
                            }
                            let hidden = timeline::hidden_sections(&self.messages, live.is_some());
                            for (n, m) in self.messages.iter().enumerate() {
                                if hidden[n] || pending.iter().any(|(index, _)| *index == n) {
                                    continue;
                                }
                                let overlaid = with_phrases(m, &tasks);
                                let view = &overlaid;
                                if let Some(t) = message_ui(ui, n, view, bare, &roster, p) {
                                    seek = Some(t);
                                }
                            }
                            if let Some(line) = self.waiting_line() {
                                theme::hint(ui, line);
                            }
                            if let Some((_, live)) = &live {
                                if live.continuing {
                                    theme::hint(ui, t!("studio.continuing"));
                                }
                                if let Some(t) =
                                    timeline::live_ui(ui, live, bare, &roster, p, &pending_texts)
                                {
                                    seek = Some(t);
                                }
                            } else if let Some(parked) = &parked {
                                theme::hint(ui, t!("studio.continuing"));
                                if let Some(t) = timeline::timeline_ui(
                                    ui,
                                    "parked",
                                    &parked.timeline.entries,
                                    &parked.steps,
                                    false,
                                    false,
                                    bare,
                                    &roster,
                                    p,
                                ) {
                                    seek = Some(t);
                                }
                            }
                            if let Some(e) = &self.error {
                                ui.colored_label(p.danger, e);
                            }
                        });
                });
                let input = Rect::from_min_max(
                    egui::pos2(ui.max_rect().min.x, ui.max_rect().max.y - input_h + 6.0),
                    ui.max_rect().max,
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(input), |ui| {
                    typed_seek = self.input_ui(ui, player.as_deref(), &roster);
                });
                if seek.is_none() {
                    if let Some(t) = typed_seek {
                        seek = Some(t.to_string());
                    }
                }
            },
        );
        self.delete_dialog(ui.ctx(), &roster);
        match (seek.as_deref().and_then(|s| self.jump(s)), player) {
            (Some(Jump::Here(t)), Some(p)) => {
                p.seek(t);
                None
            }
            (Some(Jump::Open(path, t)), _) => Some(StudioAction::OpenAt(path, t)),
            _ => None,
        }
    }

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

    fn chats_bar(&mut self, ui: &mut egui::Ui, roster: &Roster) {
        let busy = self.busy_tabs();
        let tabs = tab_list(
            &self.chats,
            &self.open,
            self.current,
            &t!("studio.new_chat"),
            &busy,
        );
        let archived = self.archived.clone();
        let salt = self.salt();
        let mut select = None;
        let mut close = None;
        let mut restore = None;
        let mut confirm = None;

        let row = egui::Layout::right_to_left(egui::Align::Center);
        ui.allocate_ui_with_layout(egui::vec2(ui.available_width(), 34.0), row, |ui| {
            let history =
                theme::icon_button(ui, icon::CLOCK_COUNTER_CLOCKWISE, &t!("studio.history"));
            egui::Popup::menu(&history).show(|ui| {
                ui.set_min_width(320.0);
                ui.set_max_width(380.0);
                if archived.is_empty() {
                    theme::hint(ui, t!("studio.history_empty"));
                    return;
                }
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .show(ui, |ui| {
                        for (n, c) in archived.iter().enumerate() {
                            if n > 0 {
                                ui.separator();
                            }
                            ui.horizontal(|ui| {
                                let buttons = 200.0;
                                ui.allocate_ui_with_layout(
                                    egui::vec2((ui.available_width() - buttons).max(80.0), 36.0),
                                    egui::Layout::top_down(egui::Align::LEFT),
                                    |ui| {
                                        let p = theme::palette(ui);
                                        let galley = mentions::galley(
                                            ui,
                                            &shown_title(&c.title),
                                            roster,
                                            egui::FontId::proportional(13.0),
                                            p.text,
                                            ui.available_width(),
                                            true,
                                            false,
                                            p,
                                        );
                                        ui.add(egui::Label::new(galley).truncate());
                                        theme::hint(ui, chat_date(c.created_ms));
                                    },
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add(theme::secondary(
                                                icon::TRASH,
                                                &t!("studio.delete_chat"),
                                            ))
                                            .clicked()
                                        {
                                            confirm = Some(c.clone());
                                            ui.close();
                                        }
                                        if ui
                                            .add(theme::secondary(
                                                icon::ARROW_SQUARE_OUT,
                                                &t!("studio.open_chat"),
                                            ))
                                            .clicked()
                                        {
                                            restore = Some(c.id);
                                            ui.close();
                                        }
                                    },
                                );
                            });
                        }
                    });
            });
            if theme::icon_button(ui, icon::PLUS, &t!("studio.new_chat")).clicked() {
                select = Some(None);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                match tab_strip::tab_strip(
                    ui,
                    ("chat-tabs", &salt),
                    &tabs,
                    &t!("studio.close_chat"),
                    roster,
                ) {
                    Some(TabAction::Selected(chat)) => select = Some(chat),
                    Some(TabAction::Closed(chat)) => close = chat,
                    None => {}
                }
            });
        });
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
            self.read = Some(Instant::now());
        }
    }

    fn close_tab(&mut self, chat: ChatId) {
        let Some(t) = self.transcript() else { return };
        let Some(at) = self.open.iter().position(|&c| c == chat) else {
            return;
        };
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

    fn delete_dialog(&mut self, ctx: &egui::Context, roster: &Roster) {
        let Some(info) = self.confirm_delete.clone() else {
            return;
        };
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new(("chat-delete", self.salt()))).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(
                RichText::new(format!("{}  {}", icon::TRASH, t!("studio.delete_chat")))
                    .size(15.0)
                    .strong(),
            );
            let confirm = t!(
                "studio.delete_chat_confirm",
                title = shown_title(&info.title)
            )
            .to_string();
            let p = theme::palette(ui);
            mentions::show(
                ui,
                &confirm,
                roster,
                egui::FontId::proportional(13.0),
                p.text,
                false,
                p,
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui
                    .add(theme::danger(ui, icon::TRASH, &t!("studio.delete_chat")))
                    .clicked()
                {
                    decision = Some(true);
                }
                if ui
                    .add(theme::secondary(icon::X, &t!("common.cancel")))
                    .clicked()
                {
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

    fn input_ui(
        &mut self,
        ui: &mut egui::Ui,
        player: Option<&Player>,
        roster: &Roster,
    ) -> Option<f64> {
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
        self.draft
            .relabel(|label| roster.name_of(label).unwrap_or_default());
        self.take_quote(ui, id);
        if let Some(byte) = self.mention_caret.take() {
            place_cursor(ui, id, &self.draft.text, byte);
        }
        self.nudge_chips(ui, id);
        let chips = self.draft.chips.clone();
        let clocks = self.draft.clocks();
        let p = theme::palette(ui);
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
            composer::layout(
                ui,
                text.as_str(),
                &chips,
                &clocks,
                wrap,
                p.text,
                p.accent,
                p.accent_soft,
            )
        };
        let saved = self.draft.text.clone();
        let saved_chips = self.draft.chips.clone();
        let editor = ui.horizontal(|ui| {
            let running = self.running_answer_id().is_some();
            let out = {
                let edit = egui::TextEdit::multiline(&mut self.draft.text)
                    .id(id)
                    .hint_text(t!("studio.ask_hint"))
                    .desired_rows(3)
                    .desired_width(ui.available_width() - 36.0);
                let edit = edit.layouter(&mut layouter);
                edit.show(ui)
            };
            let resp = &out.response;
            let enter = resp.has_focus()
                && !self.mention_open
                && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift);
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
            let seek = resp
                .hover_pos()
                .or(resp.interact_pointer_pos())
                .and_then(|pos| {
                    let cursor = out.galley.cursor_from_pos(pos - out.galley_pos);
                    let byte = chars_to_byte(&self.draft.text, cursor.index.0);
                    let hit = self.draft.seek_at(byte);
                    if hit.is_some() && resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    resp.clicked().then_some(hit).flatten()
                });
            let clicked_send = ui
                .vertical(|ui| {
                    if running {
                        theme::icon_button(ui, icon::STOP, &t!("studio.stop")).clicked()
                    } else {
                        theme::icon_button(ui, icon::PAPER_PLANE_RIGHT, &t!("studio.send"))
                            .clicked()
                    }
                })
                .inner;
            if running && clicked_send {
                self.stop();
            } else if (clicked_send || enter) && !self.draft.to_message().trim().is_empty() {
                self.send(player, roster);
            }
            (resp.response.rect, out.cursor_range, seek)
        });
        let (rect, cursor, seek) = editor.inner;
        self.sync_mentions(ui, roster, rect, cursor);
        seek
    }

    fn take_quote(&mut self, ui: &egui::Ui, id: egui::Id) {
        let Scope::File { path, .. } = &self.scope else {
            return;
        };
        let key = path.to_string_lossy().into_owned();
        let quote_id = composer::quote_id(&key);
        let Some(quote) = ui.ctx().data(|d| d.get_temp::<composer::Quote>(quote_id)) else {
            return;
        };
        ui.ctx().data_mut(|d| d.remove::<composer::Quote>(quote_id));
        let at = caret_byte(ui, id, &self.draft.text).unwrap_or(self.draft.text.len());
        self.draft.insert_quote(at, &quote);
        if let Some(chip) = self
            .draft
            .chips
            .iter()
            .rev()
            .find(|c| matches!(c.kind, composer::Kind::Quote { .. }))
        {
            self.mention_caret = Some((chip.end + 1).min(self.draft.text.len()));
        }
    }

    fn nudge_chips(&mut self, ui: &mut egui::Ui, id: egui::Id) {
        if !ui.memory(|m| m.has_focus(id)) {
            return;
        }
        let Some(byte) = caret_byte(ui, id, &self.draft.text) else {
            return;
        };
        if let Some(start) = self
            .draft
            .chips
            .iter()
            .find(|c| c.end == byte)
            .map(|c| c.start)
        {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
                self.draft.backspace_at(byte);
                place_cursor(ui, id, &self.draft.text, start);
                return;
            }
        }
        if self.draft.chips.iter().any(|c| c.start == byte)
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Delete))
        {
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

    fn insert_mention(&mut self, roster: &Roster) {
        let Some(&row) = self.mention_rows.get(self.mention.selected) else {
            return;
        };
        let Some(who) = roster.speakers.get(row) else {
            return;
        };
        let label = match who.recording {
            Some(r) => speakers::folder_label(who.index, r),
            None => speakers::label(who.index),
        };
        self.draft.insert_mention(
            self.mention.at,
            &self.mention_query,
            &label,
            &Roster::row_label(who),
        );
        let caret = self
            .draft
            .chips
            .iter()
            .rev()
            .find(|c| matches!(&c.kind, composer::Kind::Mention { label: l } if l == &label))
            .map(|c| (c.end + 1).min(self.draft.text.len()));
        self.mention_caret = caret;
        self.mention.dismiss(self.mention.at);
        self.mention_open = false;
        self.mention_rows.clear();
        self.mention_query.clear();
    }

    fn sync_mentions(
        &mut self,
        ui: &mut egui::Ui,
        roster: &Roster,
        rect: Rect,
        cursor: Option<egui::text::CCursorRange>,
    ) {
        let byte = cursor
            .map(|r| chars_to_byte(&self.draft.text, r.primary.index.0))
            .unwrap_or(self.draft.text.len());
        let query = self.draft.mention_query(byte);
        let rows = query
            .as_ref()
            .map(|q| {
                speakers::matching(&roster.speakers, &q.query, |i| {
                    t!("studio.speaker", n = i + 1).into()
                })
            })
            .unwrap_or_default();
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
        egui::Area::new(ui.id().with("speaker-mentions"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(p.card)
                    .stroke(egui::Stroke::new(1.0, p.border))
                    .corner_radius(6)
                    .inner_margin(4.0)
                    .show(ui, |ui| {
                        ui.set_min_width(240.0);
                        egui::ScrollArea::vertical()
                            .max_height(6.0 * 26.0)
                            .show(ui, |ui| {
                                for (i, &row) in rows.iter().enumerate() {
                                    let Some(who) = roster.speakers.get(row) else {
                                        continue;
                                    };
                                    let (r, resp) = ui.allocate_exact_size(
                                        egui::vec2(ui.available_width().max(240.0), 26.0),
                                        egui::Sense::click(),
                                    );
                                    if i == selected || resp.hovered() {
                                        ui.painter().rect_filled(
                                            r,
                                            egui::CornerRadius::same(4),
                                            if i == selected {
                                                p.accent_soft
                                            } else {
                                                p.card_hover
                                            },
                                        );
                                    }
                                    ui.painter().text(
                                        egui::pos2(r.left() + 8.0, r.center().y),
                                        egui::Align2::LEFT_CENTER,
                                        Roster::row_label(who),
                                        egui::FontId::proportional(13.0),
                                        p.text,
                                    );
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

fn place_cursor(ui: &egui::Ui, id: egui::Id, text: &str, byte: usize) {
    use egui::text::{CCursor, CCursorRange};
    let chars = text
        .get(..byte.min(text.len()))
        .map(|s| s.chars().count())
        .unwrap_or(0);
    let mut state = egui::text_edit::TextEditState::load(ui.ctx(), id).unwrap_or_default();
    state
        .cursor
        .set_char_range(Some(CCursorRange::one(CCursor::new(chars))));
    state.store(ui.ctx(), id);
}

fn chars_to_byte(text: &str, chars: usize) -> usize {
    text.char_indices()
        .nth(chars)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

fn caret_byte(ui: &egui::Ui, id: egui::Id, text: &str) -> Option<usize> {
    let state = egui::text_edit::TextEditState::load(ui.ctx(), id)?;
    let range = state.cursor.char_range()?;
    if range.primary.index != range.secondary.index {
        return None;
    }
    Some(chars_to_byte(text, range.primary.index.0))
}

fn syntax_hint(ui: &mut egui::Ui, pane: Rect, roster: &Roster) {
    let title = t!("studio.chat").to_string();
    let title_w = ui
        .fonts_mut(|f| {
            f.layout_no_wrap(
                title,
                FontId::proportional(12.0),
                egui::Color32::PLACEHOLDER,
            )
        })
        .size()
        .x;
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
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon::INFO,
            FontId::proportional(14.0),
            color,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    topics::text_tip(&resp, &t!("chat.syntax"), roster);
}

fn continued_cursor(cursor: &serde_json::Value, has_live: bool) -> bool {
    if has_live || cursor.is_null() {
        return false;
    }
    cursor
        .get("steps")
        .and_then(|v| v.as_array())
        .is_some_and(|steps| !steps.is_empty())
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
    let found = queue::phrases(tasks, answer);
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
    let found = queue::phrases(tasks, id);
    if found.is_empty() {
        return message.clone();
    }
    let mut message = message.clone();
    let mut timeline = message.timeline();
    apply_phrases(&mut timeline.entries, &found);
    message.details["timeline"] = timeline.to_value();
    if let Some(steps) = message
        .details
        .get_mut("steps")
        .and_then(|v| v.as_array_mut())
    {
        for (index, phrase) in &found {
            if let Some(step) = steps.get_mut(*index).and_then(|v| v.as_object_mut()) {
                step.insert("title".to_string(), serde_json::json!(phrase));
            }
        }
    }
    message
}

fn shown_title(title: &str) -> String {
    if title.is_empty() {
        t!("studio.new_chat").to_string()
    } else {
        title.to_string()
    }
}

fn tab_list(
    chats: &[ChatInfo],
    open: &[ChatId],
    current: Option<ChatId>,
    new_title: &str,
    busy: &[ChatId],
) -> Vec<Tab<Option<ChatId>>> {
    let mut tabs: Vec<Tab<Option<ChatId>>> = open
        .iter()
        .filter_map(|&id| chats.iter().find(|c| c.id == id))
        .map(|c| {
            let title = if c.title.is_empty() {
                new_title.to_string()
            } else {
                c.title.clone()
            };
            let title = if busy.contains(&c.id) {
                format!("● {title}")
            } else {
                title
            };
            Tab::new(Some(c.id), title, current == Some(c.id))
        })
        .collect();
    if current.is_none() {
        tabs.push(Tab::new(None, new_title, true).fixed());
    }
    tabs
}

fn without_chat(open: &[ChatId], chat: ChatId) -> Vec<ChatId> {
    open.iter().copied().filter(|&c| c != chat).collect()
}

fn neighbour_after_close(open: &[ChatId], at: usize) -> Option<ChatId> {
    open.get(at)
        .or_else(|| at.checked_sub(1).and_then(|i| open.get(i)))
        .copied()
}

fn with_restored(open: &[ChatId], chat: ChatId) -> Vec<ChatId> {
    let mut out = open.to_vec();
    if !out.contains(&chat) {
        out.push(chat);
    }
    out
}

fn snapshot_tasks() -> Vec<Task> {
    super::live_buffer::snapshot()
        .map(|snapshot| queue::tasks(&snapshot))
        .unwrap_or_default()
}

fn pending_steers(messages: &[ChatMessage], answer: i64, taken: usize) -> Vec<(usize, String)> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, message)| {
            message.author == Author::User
                && message.details["steer"]["task"].as_i64() == Some(answer)
        })
        .skip(taken)
        .map(|(index, message)| (index, message.text.clone()))
        .collect()
}

fn wait_line(task: &Task, tasks: &[Task], store: &str, chats: &[ChatInfo]) -> String {
    let Some(reason) = task.waiting.as_ref() else {
        return String::new();
    };
    let holder = tasks.iter().find(|other| {
        other.kind == ANSWER_KIND
            && other.scope.as_deref() == Some(store)
            && answer_busy(other)
            && other.resource == Some(Resource::Llm)
            && other.input.get("chat").and_then(|v| v.as_i64())
                != task.input.get("chat").and_then(|v| v.as_i64())
    });
    let model_wait = matches!(
        reason,
        WaitReason::Turn | WaitReason::Resource(Resource::Llm)
    );
    if model_wait {
        if let Some(other) = holder {
            let id = other.input.get("chat").and_then(|v| v.as_i64());
            let title = id
                .and_then(|id| chats.iter().find(|c| c.id == id))
                .map(|c| shown_title(&c.title))
                .unwrap_or_else(|| t!("studio.new_chat").into());
            return t!("studio.wait_model_chat", title = title).into();
        }
    }
    match reason {
        WaitReason::Dependency => t!("queue.wait_dependency"),
        WaitReason::Resource(Resource::Speech) => t!("queue.wait_speech"),
        WaitReason::Resource(Resource::Embed) => t!("queue.wait_embed"),
        WaitReason::Resource(Resource::Provider) => t!("queue.wait_provider"),
        WaitReason::Memory => t!("queue.wait_memory_pool"),
        WaitReason::Paused => t!("queue.wait_paused"),
        _ => t!("studio.wait_turn"),
    }
    .into()
}

fn chat_date(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|d| {
            d.with_timezone(&chrono::Local)
                .format("%d.%m.%Y")
                .to_string()
        })
        .unwrap_or_default()
}

fn message_ui(
    ui: &mut egui::Ui,
    n: usize,
    m: &ChatMessage,
    bare_times: bool,
    roster: &Roster,
    p: theme::Palette,
) -> Option<String> {
    match m.author {
        Author::User => {
            let mut seek = None;
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                egui::Frame::new()
                    .fill(p.accent_soft)
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width() * 0.85);
                        seek = composer::show_user(
                            ui,
                            &m.text,
                            |label| roster.name_of(label),
                            FontId::proportional(13.5),
                            p.text,
                            p.accent,
                            p.accent_soft,
                        )
                        .map(|t| t.to_string());
                    });
            });
            ui.add_space(timeline::GAP);
            seek
        }
        Author::Agent => {
            let timeline = m.timeline();
            let mut seek = None;
            if timeline.is_empty() {
                if !m.text.is_empty() {
                    seek = super::markdown::show(ui, &linkify(&m.text, bare_times), roster, p);
                }
            } else {
                let forced = m.details["forced"].as_bool() == Some(true);
                let steps: Vec<StepRecord> =
                    serde_json::from_value(m.details["steps"].clone()).unwrap_or_default();
                seek = timeline::timeline_ui(
                    ui,
                    ("message", n),
                    &timeline.entries,
                    &steps,
                    true,
                    forced,
                    bare_times,
                    roster,
                    p,
                );
                let metrics: Option<mediaa_core::assistant::Metrics> =
                    serde_json::from_value(m.details["metrics"].clone()).ok();
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
