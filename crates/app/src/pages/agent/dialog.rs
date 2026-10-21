//! The studio's dialog pane: the transcript of the file as replies of speakers, with the time of
//! each; the word being played is highlighted, a click on a word or a time seeks there, a click
//! on a speaker names them. Without a transcript: the «Transcribe» button; while transcription
//! runs: its progress and the text so far.

use super::studio::{panel, StudioAction};
use super::topics;
use crate::player::Player;
use crate::theme;
use eframe::egui::{self, text::LayoutJob, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, TextFormat};
use egui_phosphor::regular as icon;
use super::Services;
use crossbeam_channel::Receiver;
use mediaa_agent::transcribe::KIND;
use mediaa_core::retrieval::Hit;
use mediaa_core::library::human_duration;
use mediaa_core::outline::Chapter;
use mediaa_core::queue::{Queue, Task, TaskStatus};
use mediaa_core::transcript::{self, Reply, Transcript};
use rust_i18n::t;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the transcript is re-read while transcription runs / otherwise.
const ACTIVE_REFRESH: Duration = Duration::from_secs(1);
const IDLE_REFRESH: Duration = Duration::from_secs(5);

/// The search card's clear button: its side, the gap before it and the card's right padding.
/// The slot is reserved even while the button is hidden, so typing never moves the field.
const SEARCH_CLEAR: f32 = 24.0;
const SEARCH_GAP: f32 = 8.0;
const SEARCH_PAD: f32 = 12.0;

/// Speaker colours (by speaker number).
const SPEAKER_COLORS: [Color32; 6] = [
    Color32::from_rgb(61, 139, 255),
    Color32::from_rgb(232, 121, 54),
    Color32::from_rgb(47, 178, 120),
    Color32::from_rgb(176, 98, 230),
    Color32::from_rgb(214, 70, 112),
    Color32::from_rgb(20, 160, 180),
];

pub fn speaker_color(speaker: u32) -> Color32 {
    SPEAKER_COLORS[speaker as usize % SPEAKER_COLORS.len()]
}

/// The latest transcription task of `path` among `tasks`.
pub fn task_of<'a>(tasks: &'a [Task], path: &Path) -> Option<&'a Task> {
    let path = path.to_string_lossy();
    tasks.iter().filter(|t| t.kind == KIND && t.input["path"].as_str() == Some(&path)).max_by_key(|t| t.id)
}

/// The word of `reply` at character `index` of its text (words joined by single spaces).
pub fn word_at(reply: &Reply, index: usize) -> Option<usize> {
    let mut at = 0;
    for (i, w) in reply.words.iter().enumerate() {
        let len = w.text.chars().count();
        if index < at + len + 1 {
            return Some(i);
        }
        at += len + 1;
    }
    None
}

/// Hits shown for a search.
const SEARCH_HITS: usize = 8;

/// The search field of the dialog and its results.
#[derive(Default)]
struct Search {
    text: String,
    running: Option<Receiver<anyhow::Result<Vec<Hit>>>>,
    /// Results of the last search (`None`: the dialog is shown).
    hits: Option<anyhow::Result<Vec<Hit>>>,
}

/// Search in the transcript of `id` (a user waits).
fn search(services: &Services, id: &str, query: &str) -> anyhow::Result<Vec<Hit>> {
    let settings = services.settings.read().expect("settings lock").clone();
    Ok(mediaa_agent::search::search(services.engine.as_ref(), &services.slots, &settings, &services.ai_dir, id, query, SEARCH_HITS, true)?.0)
}

pub struct Dialog {
    path: PathBuf,
    ai_dir: PathBuf,
    /// Content id of the file (`None` when it cannot be read).
    file_id: Option<String>,
    queue: Option<Queue>,
    transcript: Option<Transcript>,
    replies: Vec<Reply>,
    chapters: Vec<Chapter>,
    names: BTreeMap<u32, String>,
    task: Option<Task>,
    read: Option<Instant>,
    /// Speaker being named and the name typed so far.
    naming: Option<(u32, String)>,
    /// The reply being played (to keep it in view).
    playing: Option<usize>,
    /// The reply to bring into view (a click on a topic).
    scroll_to: Option<usize>,
    /// Chunks indexed for search.
    indexed: usize,
    search: Search,
}

impl Dialog {
    pub fn new(path: &Path, ai_dir: &Path) -> Dialog {
        let file_id = transcript::file_id(path).map_err(|e| log::warn!("{}: {e:#}", path.display())).ok();
        Dialog {
            path: path.to_path_buf(),
            ai_dir: ai_dir.to_path_buf(),
            file_id,
            queue: None,
            transcript: None,
            replies: Vec::new(),
            chapters: Vec::new(),
            names: BTreeMap::new(),
            task: None,
            read: None,
            naming: None,
            playing: None,
            scroll_to: None,
            indexed: 0,
            search: Search::default(),
        }
    }

    fn active(&self) -> bool {
        self.task.as_ref().is_some_and(|t| matches!(t.status, TaskStatus::Queued | TaskStatus::Running))
    }

    fn refresh(&mut self) {
        let every = if self.active() { ACTIVE_REFRESH } else { IDLE_REFRESH };
        if self.read.is_some_and(|r| r.elapsed() < every) {
            return;
        }
        self.read = Some(Instant::now());
        if self.queue.is_none() {
            self.queue = Queue::open(&self.ai_dir).map_err(|e| log::warn!("queue: {e:#}")).ok();
        }
        if let Some(q) = &self.queue {
            match q.list() {
                Ok(tasks) => self.task = task_of(&tasks, &self.path).cloned(),
                Err(e) => log::warn!("queue: {e:#}"),
            }
        }
        let Some(id) = &self.file_id else { return };
        if self.transcript.is_none() && Transcript::exists(&self.ai_dir, id) {
            self.transcript = Transcript::open(&self.ai_dir, id).map_err(|e| log::warn!("transcript: {e:#}")).ok();
        }
        if let Some(t) = &self.transcript {
            match (t.dialog(), t.speaker_names()) {
                (Ok(r), Ok(n)) => {
                    self.replies = r;
                    self.names = n;
                    self.indexed = t.chunks().map_or(0, |c| c.len());
                    self.chapters = t.chapters().unwrap_or_default();
                }
                (Err(e), _) | (_, Err(e)) => log::warn!("transcript: {e:#}"),
            }
        }
    }

    /// The topics of the recording (read with the transcript).
    pub fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    fn speaker_name(&self, speaker: u32) -> String {
        self.names.get(&speaker).cloned().unwrap_or_else(|| t!("studio.speaker", n = speaker + 1).into())
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, rect: Rect, player: &mut Player, services: Option<&Services>, p: theme::Palette) -> Option<StudioAction> {
        self.refresh();
        // Changes come from the worker thread: look again while the pane is shown.
        ui.ctx().request_repaint_after(if self.active() { ACTIVE_REFRESH } else { IDLE_REFRESH });
        let body = panel(ui, rect, &t!("studio.dialog"), p);
        let mut action = None;
        // Header tool: transcribe again.
        let head = Rect::from_min_max(egui::pos2(rect.max.x - 40.0, rect.min.y), egui::pos2(rect.max.x - 6.0, body.min.y - 1.0));
        if !self.replies.is_empty() && !self.active() {
            ui.scope_builder(egui::UiBuilder::new().max_rect(head), |ui| {
                ui.centered_and_justified(|ui| {
                    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("studio.retranscribe")).clicked() {
                        action = Some(StudioAction::Transcribe(self.path.clone()));
                    }
                });
            });
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(12.0, 8.0))), |ui| {
            if let Some(a) = self.status(ui, p) {
                action = Some(a);
            }
            if self.replies.is_empty() {
                if !self.active() && self.task.as_ref().is_none_or(|t| t.status == TaskStatus::Done) {
                    action = action.take().or(self.empty(ui, p));
                }
                return;
            }
            // The topics stay at the top of the pane while the replies scroll below.
            if !self.chapters.is_empty() {
                // A topic or one of its parts was clicked: seek there and show its first reply.
                if let Some(start) = topics::block(ui, &self.chapters, player.position(), player, body.height() * 0.4, p) {
                    player.seek(start);
                    self.scroll_to = self.replies.iter().position(|r| r.end > start + 0.001).or(Some(0));
                    // The reply to scroll to is in the dialog, not in the search results.
                    self.search.hits = None;
                }
                ui.add_space(6.0);
            }
            if self.search_ui(ui, player, services, p) {
                return;
            }
            self.replies_ui(ui, player, p);
        });
        action
    }

    /// The transcription task's state, when there is one to show.
    fn status(&mut self, ui: &mut egui::Ui, p: theme::Palette) -> Option<StudioAction> {
        let task = self.task.as_ref()?;
        let mut action = None;
        match task.status {
            TaskStatus::Queued | TaskStatus::Running | TaskStatus::Paused => {
                let label = match task.status {
                    TaskStatus::Queued => t!("studio.transcribe_queued"),
                    TaskStatus::Paused => t!("studio.transcribe_paused"),
                    _ => t!("studio.transcribing"),
                };
                ui.horizontal(|ui| {
                    if task.status == TaskStatus::Running {
                        ui.spinner();
                    }
                    ui.label(RichText::new(label).color(p.text_weak));
                });
                ui.add(egui::ProgressBar::new(task.progress).desired_width(ui.available_width()).desired_height(6.0));
                ui.add_space(6.0);
            }
            TaskStatus::Failed => {
                ui.colored_label(p.danger, t!("studio.transcribe_failed", error = task.error.clone().unwrap_or_default()));
                if ui.add(theme::secondary(icon::PLAY, &t!("queue.resume"))).clicked() {
                    action = Some(StudioAction::ResumeTask(task.id));
                }
                ui.add_space(6.0);
            }
            TaskStatus::Done => {}
        }
        action
    }

    /// No transcript yet: what it gives and the button.
    fn empty(&self, ui: &mut egui::Ui, p: theme::Palette) -> Option<StudioAction> {
        let mut action = None;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 70.0).max(0.0));
            ui.label(RichText::new(icon::CHAT_TEXT).size(28.0).color(p.text_weak));
            ui.add_space(4.0);
            let done = self.task.as_ref().is_some_and(|t| t.status == TaskStatus::Done);
            theme::hint(ui, if done { t!("studio.no_speech") } else { t!("studio.no_transcript") });
            ui.add_space(8.0);
            if ui.add(theme::primary(ui, icon::WAVEFORM, &t!("studio.transcribe"))).clicked() {
                action = Some(StudioAction::Transcribe(self.path.clone()));
            }
        });
        action
    }

    /// The search field; returns whether results are shown instead of the dialog.
    fn search_ui(&mut self, ui: &mut egui::Ui, player: &mut Player, services: Option<&Services>, p: theme::Palette) -> bool {
        if self.indexed == 0 {
            if self.task.as_ref().is_none_or(|t| t.status == TaskStatus::Done) {
                theme::hint(ui, t!("studio.search_soon"));
                ui.add_space(4.0);
            }
            return false;
        }
        // The same card as the folded topics block — one fixed row, the same icon and text x: the
        // card does not change height or move its contents when text appears, and the clear
        // button's slot is reserved even while the button is hidden.
        let (card, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), topics::HEADER_H), Sense::hover());
        ui.painter().rect(card, CornerRadius::same(8), p.card, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
        ui.painter().text(egui::pos2(card.left() + topics::HEADER_ICON_X, card.center().y), egui::Align2::LEFT_CENTER, icon::MAGNIFYING_GLASS, FontId::proportional(14.0), p.accent);
        let clear = self.search.hits.is_some() || !self.search.text.is_empty();
        let clear_slot = Rect::from_center_size(egui::pos2(card.right() - SEARCH_PAD - SEARCH_CLEAR / 2.0, card.center().y), egui::vec2(SEARCH_CLEAR, SEARCH_CLEAR));
        let field = Rect::from_min_max(egui::pos2(card.left() + topics::HEADER_TEXT_X, card.top() + 2.0), egui::pos2(clear_slot.left() - SEARCH_GAP, card.bottom() - 2.0));
        let edit = ui
            .scope_builder(egui::UiBuilder::new().max_rect(field).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.search.text)
                        .hint_text(t!("studio.search_hint"))
                        .desired_width(field.width())
                        .margin(Margin::ZERO)
                        .frame(egui::Frame::NONE),
                )
            })
            .inner;
        let start = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.search.text.trim().is_empty();
        if clear {
            let close = ui
                .scope_builder(egui::UiBuilder::new().max_rect(clear_slot).layout(egui::Layout::left_to_right(egui::Align::Center)), |ui| {
                    theme::icon_button_sized(ui, icon::X, &t!("studio.search_close"), SEARCH_CLEAR)
                })
                .inner;
            if close.clicked() {
                self.search = Search::default();
            }
        }
        ui.add_space(4.0);
        if start {
            if let (Some(s), Some(id)) = (services.cloned(), self.file_id.clone()) {
                let (tx, rx) = crossbeam_channel::bounded(1);
                let query = self.search.text.trim().to_string();
                let ctx = ui.ctx().clone();
                let _ = std::thread::Builder::new().name("dialog-search".into()).spawn(move || {
                    let _ = tx.send(search(&s, &id, &query));
                    ctx.request_repaint();
                });
                self.search.running = Some(rx);
            }
        }
        if let Some(rx) = &self.search.running {
            match rx.try_recv() {
                Ok(r) => {
                    self.search.hits = Some(r);
                    self.search.running = None;
                }
                Err(_) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        theme::hint(ui, t!("studio.searching"));
                    });
                    return true;
                }
            }
        }
        let Some(hits) = &self.search.hits else { return false };
        let mut seek = None;
        match hits {
            Err(e) => {
                ui.colored_label(p.danger, format!("{e:#}"));
            }
            Ok(hits) if hits.is_empty() => theme::hint(ui, t!("studio.nothing_found")),
            Ok(hits) => {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for h in hits {
                        let card = theme::card(ui).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(format!("{} – {}", human_duration(h.chunk.start), human_duration(h.chunk.end))).font(FontId::monospace(11.5)).color(p.accent));
                                for s in &h.chunk.speakers {
                                    ui.label(RichText::new(self.speaker_name(*s)).color(speaker_color(*s)).size(12.0));
                                }
                            });
                            ui.add(egui::Label::new(RichText::new(&h.chunk.text).size(13.0)).wrap());
                        });
                        let resp = ui.interact(card.response.rect, ui.id().with(("hit", h.id)), Sense::click());
                        if resp.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if resp.on_hover_text(t!("studio.go_there")).clicked() {
                            seek = Some(h.chunk.start);
                        }
                        ui.add_space(4.0);
                    }
                });
            }
        }
        if let Some(t) = seek {
            player.seek(t);
            // Back to the dialog at that place.
            self.search.hits = None;
        }
        self.search.hits.is_some()
    }

    fn replies_ui(&mut self, ui: &mut egui::Ui, player: &mut Player, p: theme::Palette) {
        let now = player.position();
        let playing = self.replies.iter().position(|r| now >= r.start && now < r.end + 0.3);
        let follow = !player.is_paused() && playing.is_some() && playing != self.playing;
        self.playing = playing;
        let mut seek = None;
        let mut name_click = None;
        let scroll_to = self.scroll_to.take();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            for (i, reply) in self.replies.iter().enumerate() {
                let current = playing == Some(i);
                let top = ui.horizontal(|ui| {
                    if let Some(s) = reply.speaker {
                        let color = speaker_color(s);
                        let chip = ui.add(egui::Button::new(RichText::new(self.speaker_name(s)).color(color).strong().size(12.5)).frame(false));
                        if chip.on_hover_text(t!("studio.rename_speaker")).clicked() {
                            name_click = Some(s);
                        }
                    }
                    let time = ui.add(egui::Button::new(RichText::new(human_duration(reply.start)).font(FontId::monospace(11.5)).color(p.text_weak)).frame(false));
                    if time.clicked() {
                        seek = Some(reply.start);
                    }
                });
                if (current && follow) || scroll_to == Some(i) {
                    top.response.scroll_to_me(Some(egui::Align::Center));
                }
                if let Some(t) = reply_text(ui, reply, if current { Some(now) } else { None }, p) {
                    seek = Some(t);
                }
                ui.add_space(8.0);
            }
        });
        if let Some(t) = seek {
            player.seek(t);
        }
        if let Some(s) = name_click {
            self.naming = Some((s, self.names.get(&s).cloned().unwrap_or_default()));
        }
        self.naming_window(ui);
    }

    /// A small window to name a speaker (an empty name brings the number back).
    fn naming_window(&mut self, ui: &mut egui::Ui) {
        let Some((speaker, name)) = &mut self.naming else { return };
        let speaker = *speaker;
        let mut open = true;
        let mut done = false;
        egui::Window::new(t!("studio.rename_speaker"))
            .id(egui::Id::new(("speaker-name", &self.path)))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ui.ctx(), |ui| {
                let edit = ui.add(egui::TextEdit::singleline(name).desired_width(240.0).hint_text(t!("studio.speaker", n = speaker + 1)));
                edit.request_focus();
                if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    done = true;
                }
                if ui.add(theme::primary(ui, icon::CHECK, &t!("studio.save_name"))).clicked() {
                    done = true;
                }
            });
        if done {
            let name = name.clone();
            if let Some(t) = &self.transcript {
                if let Err(e) = t.rename_speaker(speaker, &name) {
                    log::warn!("transcript: {e:#}");
                }
            }
            self.read = None;
            self.naming = None;
        } else if !open {
            self.naming = None;
        }
    }
}

/// The words of a reply as one wrapped text, the one at `now` highlighted; returns the time of
/// a clicked word.
fn reply_text(ui: &mut egui::Ui, reply: &Reply, now: Option<f64>, p: theme::Palette) -> Option<f64> {
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let plain = TextFormat { font_id: FontId::proportional(14.0), color: p.text, ..Default::default() };
    let lit = TextFormat { background: p.accent_soft, color: p.accent, ..plain.clone() };
    for (i, w) in reply.words.iter().enumerate() {
        let on = now.is_some_and(|t| t >= w.start && t < w.end.max(w.start + 0.15));
        job.append(&w.text, 0.0, if on { lit.clone() } else { plain.clone() });
        if i + 1 < reply.words.len() {
            job.append(" ", 0.0, plain.clone());
        }
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    ui.painter().galley(rect.min, galley.clone(), p.text);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let pos = resp.interact_pointer_pos().filter(|_| resp.clicked())?;
    let cursor = galley.cursor_from_pos(pos - rect.min);
    word_at(reply, cursor.index.0).map(|i| reply.words[i].start)
}


#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::transcript::Word;
    use serde_json::json;

    fn word(text: &str, start: f64) -> Word {
        Word { text: text.into(), start, end: start + 0.3 }
    }

    #[test]
    fn clicks_find_words() {
        let r = Reply { start: 0.0, end: 2.0, speaker: None, words: vec![word("привет,", 0.0), word("мир", 1.0)] };
        assert_eq!(word_at(&r, 0), Some(0));
        assert_eq!(word_at(&r, 6), Some(0));
        assert_eq!(word_at(&r, 7), Some(0), "the space after a word");
        assert_eq!(word_at(&r, 8), Some(1));
        assert_eq!(word_at(&r, 11), Some(1), "after the last letter");
        assert_eq!(word_at(&r, 12), None);
    }

    #[test]
    fn the_latest_task_of_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let a = Path::new("/m/a.mp4");
        q.enqueue(KIND, "a", &json!({"path": "/m/a.mp4"}), 0).unwrap();
        q.enqueue(KIND, "b", &json!({"path": "/m/b.mp4"}), 0).unwrap();
        let again = q.enqueue(KIND, "a", &json!({"path": "/m/a.mp4"}), 0).unwrap();
        q.enqueue("other", "a", &json!({"path": "/m/a.mp4"}), 0).unwrap();
        let tasks = q.list().unwrap();
        assert_eq!(task_of(&tasks, a).map(|t| t.id), Some(again));
        assert!(task_of(&tasks, Path::new("/m/c.mp4")).is_none());
        assert_ne!(speaker_color(0), speaker_color(1));
        assert_eq!(speaker_color(0), speaker_color(SPEAKER_COLORS.len() as u32));
    }
}
