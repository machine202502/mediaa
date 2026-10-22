//! The timeline of a chat answer in the pane: steps, reasoning and parts of the answer in the
//! order they happened, growing downward while the model works. The same blocks draw the answer
//! being written and every saved answer; a finished answer folds its steps and reasoning behind
//! one row above the final text.
//!
//! What is drawn is decided by pure functions ([`blocks`], [`status_of`], [`hidden_sections`]);
//! the drawing only follows them. Rows have fixed heights and one gap ([`GAP`]), so nothing
//! jumps when a block opens or closes or when the text starts to stream.

use super::chat::linkify;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_agent::assistant::chat::Live;
use mediaa_agent::assistant::{StepRecord, WaitReason};
use mediaa_core::chat::{ChatMessage, TimelineEntry, STEP_RUNNING};
use rust_i18n::t;
use std::hash::Hash;

/// The one gap between blocks of an answer and after a message.
pub const GAP: f32 = 8.0;
/// Text size of a step or a reasoning header.
const ROW_TEXT: f32 = 12.5;

/// A step or a piece of reasoning.
#[derive(Debug, Clone, PartialEq)]
pub enum Row<'a> {
    Step { title: &'a str, tool: &'a str, status: &'a str },
    Thinking { text: &'a str },
}

/// A block of the drawn answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Block<'a> {
    Row(Row<'a>),
    Answer(&'a str),
    /// Steps and reasoning of a finished answer, behind one row «Шаги и размышления (N)».
    Folded(Vec<Row<'a>>),
}

/// The blocks of a timeline. While the answer is written every entry is drawn where it happened;
/// with `fold` (a stored answer, finished or stopped) the steps and reasoning are gathered in
/// one folded group above the parts of the answer. Empty reasoning and empty text are dropped.
pub fn blocks(entries: &[TimelineEntry], fold: bool) -> Vec<Block<'_>> {
    let mut out = Vec::new();
    let mut folded = Vec::new();
    for e in entries {
        let block = match e {
            TimelineEntry::Step { title, tool, status } => Block::Row(Row::Step { title, tool, status }),
            TimelineEntry::Thinking { text } if text.trim().is_empty() => continue,
            TimelineEntry::Thinking { text } => Block::Row(Row::Thinking { text }),
            TimelineEntry::Answer { text, .. } if text.trim().is_empty() => continue,
            TimelineEntry::Answer { text, .. } => Block::Answer(text),
        };
        match block {
            Block::Row(row) if fold => folded.push(row),
            block => out.push(block),
        }
    }
    if !folded.is_empty() {
        out.insert(0, Block::Folded(folded));
    }
    out
}

/// What the line under the timeline says while the answer is written.
#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Before the model writes: what it waits for.
    Waiting(WaitReason),
    /// The model is reasoning.
    Reasoning,
    /// The answer's text is streaming.
    Writing,
    /// Nothing has come yet and nothing is known to wait for.
    Starting,
}

/// The status line of a running answer; `None` when a step or a scan shows the progress itself.
pub fn status_of(live: &Live) -> Option<Status> {
    if let Some(reason) = &live.waiting {
        return Some(Status::Waiting(reason.clone()));
    }
    if live.scanning.is_some() {
        return None;
    }
    match live.timeline.entries.last() {
        None => Some(Status::Starting),
        Some(TimelineEntry::Thinking { .. }) => Some(Status::Reasoning),
        Some(TimelineEntry::Answer { section: false, .. }) => Some(Status::Writing),
        Some(TimelineEntry::Step { status, .. }) if status == STEP_RUNNING => None,
        // A step is over or a section was sent: the model is on its next one.
        Some(_) => Some(Status::Starting),
    }
}

/// Which messages are not drawn as messages of their own because the timeline draws them: the
/// sections of an answer (`section` in the details) that has its final message with a timeline,
/// or that is being written now (`running`). Same length as `messages`.
pub fn hidden_sections(messages: &[ChatMessage], running: bool) -> Vec<bool> {
    let mut hidden = vec![false; messages.len()];
    let mut end = messages.len();
    // The turns from the last one back: a turn is a question and the messages after it.
    let mut last = true;
    for start in (0..messages.len()).rev() {
        if messages[start].author != mediaa_core::chat::Author::User {
            continue;
        }
        let turn = start + 1..end;
        let drawn_by_timeline = (last && running) || messages[turn.clone()].iter().any(|m| !is_section(m) && !m.timeline().is_empty());
        if drawn_by_timeline {
            for i in turn {
                hidden[i] = is_section(&messages[i]);
            }
        }
        end = start;
        last = false;
    }
    hidden
}

fn is_section(m: &ChatMessage) -> bool {
    m.details["section"].as_bool() == Some(true)
}

/// The tasks the models are busy with, by their labels.
fn busy_names(tasks: &[mediaa_agent::scope::TaskRef]) -> String {
    tasks.iter().map(|t| super::task_label::label(&t.kind, &t.title)).collect::<Vec<_>>().join(", ")
}

/// What the model waits for, in words.
pub fn wait_text(reason: &WaitReason) -> String {
    let name = |model: &str| model.rsplit(['/', '\\']).next().unwrap_or(model).to_string();
    match reason {
        WaitReason::Loading { model } => t!("studio.wait_loading", model = name(model)).to_string(),
        WaitReason::Memory { model } => t!("studio.wait_memory", model = name(model)).to_string(),
        WaitReason::Downloading { model, done, total } => {
            let mb = |b: &u64| b / (1 << 20);
            t!("studio.wait_downloading", model = name(model), done = mb(done), total = mb(total)).to_string()
        }
        WaitReason::Busy { tasks } if tasks.is_empty() => t!("studio.wait_busy").to_string(),
        WaitReason::Busy { tasks } => t!("studio.wait_busy_with", tasks = busy_names(tasks)).to_string(),
        WaitReason::ProviderSlot => t!("studio.wait_provider").to_string(),
    }
}

fn status_text(status: &Status) -> String {
    match status {
        Status::Waiting(reason) => wait_text(reason),
        Status::Reasoning => t!("studio.reasoning").to_string(),
        Status::Writing => t!("studio.writing").to_string(),
        Status::Starting => t!("studio.starting").to_string(),
    }
}

/// A step as one line: the icon of how it went and the title the model gave it.
fn step_row(ui: &mut egui::Ui, title: &str, status: &str, roster: &super::mentions::Roster, p: theme::Palette) {
    let (glyph, color) = match status {
        "found" => (icon::CHECK_CIRCLE, p.success),
        "nothing" => (icon::CIRCLE_DASHED, p.text_weak),
        "repeated" => (icon::ARROW_COUNTER_CLOCKWISE, p.text_weak),
        "failed" => (icon::WARNING_CIRCLE, p.danger),
        "stopped" => (icon::STOP_CIRCLE, p.text_weak),
        _ => (icon::DOTS_THREE_CIRCLE, p.accent),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).color(color));
        let galley = super::mentions::galley(ui, title, roster, egui::FontId::proportional(ROW_TEXT), p.text, ui.available_width(), true, false, p);
        ui.add(egui::Label::new(galley).truncate());
    });
}

/// A piece of reasoning: a header, closed until the user opens it; the text grows while the
/// model thinks.
fn thinking_row(ui: &mut egui::Ui, id: impl Hash + std::fmt::Debug, text: &str, roster: &super::mentions::Roster, p: theme::Palette) {
    egui::CollapsingHeader::new(RichText::new(t!("studio.thinking_block")).size(ROW_TEXT).color(p.text_weak))
        .id_salt(id)
        .default_open(false)
        .show(ui, |ui| {
            super::mentions::show(ui, text.trim(), roster, egui::FontId::proportional(12.0), p.text_weak, false, p);
        });
}

/// The title shown for a step. The draft and the check use the interface's language; the text
/// stored with the answer stays the one the queue shows.
fn step_title(tool: &str, title: &str) -> String {
    match tool {
        "verify" => t!("studio.verify_step").to_string(),
        _ => title.to_string(),
    }
}

/// The draft, folded until the user opens it. Its text is the step's result.
fn draft_row(ui: &mut egui::Ui, id: impl Hash + std::fmt::Debug, body: &str, bare: bool, roster: &super::mentions::Roster, p: theme::Palette) -> Option<String> {
    let mut seek = None;
    egui::CollapsingHeader::new(RichText::new(t!("studio.draft_step")).size(ROW_TEXT).color(p.text_weak))
        .id_salt(id)
        .default_open(false)
        .show(ui, |ui| {
            seek = super::markdown::show(ui, &linkify(body, bare), roster, p);
        });
    seek
}

#[allow(clippy::too_many_arguments)]
fn row(ui: &mut egui::Ui, id: impl Hash + std::fmt::Debug, row: &Row, steps: &[StepRecord], draft_at: usize, bare: bool, roster: &super::mentions::Roster, p: theme::Palette) -> Option<String> {
    match row {
        Row::Step { tool, .. } if *tool == "draft" => {
            let body = steps.iter().filter(|s| s.tool == "draft").nth(draft_at).map(|s| s.result.as_str()).unwrap_or("");
            draft_row(ui, id, body, bare, roster, p)
        }
        Row::Step { title, tool, status } => {
            step_row(ui, &step_title(tool, title), status, roster, p);
            None
        }
        Row::Thinking { text } => {
            thinking_row(ui, id, text, roster, p);
            None
        }
    }
}

/// Draws the blocks of one answer's timeline; `forced`: it was given before the agent finished
/// looking. Returns the target of a clicked citation.
#[allow(clippy::too_many_arguments)]
pub fn timeline_ui(
    ui: &mut egui::Ui,
    id: impl Hash + std::fmt::Debug,
    entries: &[TimelineEntry],
    steps: &[StepRecord],
    fold: bool,
    forced: bool,
    bare_times: bool,
    roster: &super::mentions::Roster,
    p: theme::Palette,
) -> Option<String> {
    let mut seek = None;
    let mut draft_at = 0usize;
    ui.push_id(id, |ui| {
        ui.spacing_mut().item_spacing.y = GAP;
        for (n, block) in blocks(entries, fold).iter().enumerate() {
            ui.push_id(n, |ui| match block {
                Block::Row(r) => {
                    let at = draft_at;
                    if matches!(r, Row::Step { tool: "draft", .. }) {
                        draft_at += 1;
                    }
                    if let Some(t) = row(ui, "row", r, steps, at, bare_times, roster, p) {
                        seek = Some(t);
                    }
                }
                Block::Folded(rows) => {
                    let header = RichText::new(t!("studio.steps_folded", n = rows.len())).size(11.5).color(p.text_weak);
                    egui::CollapsingHeader::new(header).id_salt("folded").default_open(false).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = GAP / 2.0;
                        for (i, r) in rows.iter().enumerate() {
                            let at = draft_at;
                            if matches!(r, Row::Step { tool: "draft", .. }) {
                                draft_at += 1;
                            }
                            if let Some(t) = row(ui, ("folded-row", i), r, steps, at, bare_times, roster, p) {
                                seek = Some(t);
                            }
                        }
                        if forced {
                            theme::hint(ui, t!("studio.forced"));
                        }
                    });
                }
                Block::Answer(text) => {
                    if let Some(t) = super::markdown::show(ui, &linkify(text, bare_times), roster, p) {
                        seek = Some(t);
                    }
                }
            });
        }
    });
    seek
}

/// The answer being written: its timeline as it is, the progress of a scan and the status line.
pub fn live_ui(ui: &mut egui::Ui, live: &Live, bare_times: bool, roster: &super::mentions::Roster, p: theme::Palette) -> Option<String> {
    let seek = timeline_ui(ui, "live", &live.timeline.entries, &live.steps, false, false, bare_times, roster, p);
    ui.spacing_mut().item_spacing.y = GAP;
    if let Some((done, total)) = live.scanning {
        ui.label(RichText::new(t!("studio.scanning", done = done, total = total)).size(ROW_TEXT).color(p.text_weak));
        ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_height(5.0));
    }
    if let Some(status) = status_of(live) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(status_text(&status)).color(p.text_weak));
        });
    }
    seek
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(kind: &str, title: &str) -> mediaa_agent::scope::TaskRef {
        mediaa_agent::scope::TaskRef { kind: kind.into(), title: title.into() }
    }

    use mediaa_agent::assistant::Event;
    use mediaa_core::chat::{Author, Timeline};
    use serde_json::json;

    fn step(title: &str, status: &str) -> TimelineEntry {
        TimelineEntry::Step { title: title.into(), tool: "search".into(), status: status.into() }
    }
    fn thinking(text: &str) -> TimelineEntry {
        TimelineEntry::Thinking { text: text.into() }
    }
    fn answer(text: &str, section: bool) -> TimelineEntry {
        TimelineEntry::Answer { text: text.into(), section }
    }

    #[test]
    fn a_running_or_stopped_answer_is_drawn_in_order_and_a_final_one_is_folded() {
        let entries = [thinking("Надо искать."), step("Ищу сроки", "found"), thinking("  "), step("Читаю", "found"), answer("", false), answer("В пятницу.", false)];
        let row = |i: usize| match &entries[i] {
            TimelineEntry::Step { title, tool, status } => Row::Step { title, tool, status },
            TimelineEntry::Thinking { text } => Row::Thinking { text },
            _ => unreachable!(),
        };
        // Written now or stopped: where it happened (empty ones left out).
        assert_eq!(blocks(&entries, false), [Block::Row(row(0)), Block::Row(row(1)), Block::Row(row(3)), Block::Answer("В пятницу.")]);
        // Final: one group above the text; its count is what the header shows.
        let done = blocks(&entries, true);
        assert_eq!(done, [Block::Folded(vec![row(0), row(1), row(3)]), Block::Answer("В пятницу.")]);
        // Only an answer: nothing to fold.
        assert_eq!(blocks(&[answer("Привет.", false)], true), [Block::Answer("Привет.")]);
        assert!(blocks(&[], true).is_empty());
        // Parts of the answer between steps keep their order, after the group.
        let parts = [step("А", "found"), answer("Часть 1.", true), step("Б", "found"), answer("Итог.", false)];
        let folded = blocks(&parts, true);
        assert_eq!(folded.len(), 3);
        assert!(matches!(&folded[0], Block::Folded(rows) if rows.len() == 2));
        assert_eq!((&folded[1], &folded[2]), (&Block::Answer("Часть 1."), &Block::Answer("Итог.")));
    }

    #[test]
    fn the_status_line_follows_what_the_model_does() {
        let mut live = Live::new();
        assert_eq!(status_of(&live), Some(Status::Starting));
        live.apply(&Event::Waiting { reason: WaitReason::Busy { tasks: vec![task("analysis", "a.mp4")] } });
        assert_eq!(status_of(&live), Some(Status::Waiting(WaitReason::Busy { tasks: vec![task("analysis", "a.mp4")] })));
        // Any other event ends the waiting; reasoning, then a step (it shows itself), then text.
        live.apply(&Event::Thinking("Думаю".into()));
        assert_eq!(status_of(&live), Some(Status::Reasoning));
        live.apply(&Event::StepStarted { title: "Ищу".into(), tool: "search".into() });
        assert_eq!(status_of(&live), None);
        live.apply(&Event::Scanning { done: 1, total: 4 });
        assert_eq!(status_of(&live), None);
        live.scanning = None;
        live.apply(&Event::AnswerText("В пят".into()));
        assert_eq!(status_of(&live), Some(Status::Writing));
        assert_eq!(live.timeline.draft(), "В пят");
    }

    #[test]
    fn the_waiting_reason_is_in_words() {
        let busy = WaitReason::Busy { tasks: vec![task("transcribe", "a.mp4")] };
        assert_eq!(wait_text(&busy), t!("studio.wait_busy_with", tasks = crate::pages::agent::task_label::label("transcribe", "a.mp4")).to_string());
        assert_eq!(wait_text(&WaitReason::Loading { model: "owner/Qwen3-4B".into() }), t!("studio.wait_loading", model = "Qwen3-4B").to_string(), "the model, not its owner");
    }

    fn message(author: Author, text: &str, details: serde_json::Value) -> ChatMessage {
        ChatMessage { author, text: text.into(), details }
    }

    #[test]
    fn sections_are_not_drawn_twice() {
        let mut tl = Timeline::default();
        tl.step("Ищу", "search", "found");
        tl.section("Часть 1.");
        tl.finish_answer("Итог.");
        let section = || message(Author::Agent, "Часть 1.", json!({ "section": true }));
        let last = || message(Author::Agent, "Итог.", json!({ "steps": [], "timeline": tl.to_value() }));
        let user = || message(Author::User, "в?", serde_json::Value::Null);

        // Saved: the final message draws the sections inside its timeline.
        let saved = [user(), section(), last(), user(), section(), last()];
        assert_eq!(hidden_sections(&saved, false), [false, true, false, false, true, false]);
        // Written now: the sections of the last turn are in the live timeline.
        let running = [user(), section(), last(), user(), section()];
        assert_eq!(hidden_sections(&running, true), [false, true, false, false, true]);
        // An answer that failed has no final message: its sections stay as they are.
        let failed = [user(), section()];
        assert_eq!(hidden_sections(&failed, false), [false, false]);
        // An old answer without a timeline is not a reason to hide anything.
        let old = [user(), section(), message(Author::Agent, "Итог.", json!({ "steps": [] }))];
        assert_eq!(hidden_sections(&old, false), [false, false, false]);
        // A stopped answer keeps its timeline in the message it stored.
        let stopped = [user(), section(), message(Author::Agent, "", json!({ "steps": [], "timeline": tl.to_value(), "stopped": true }))];
        assert_eq!(hidden_sections(&stopped, false), [false, true, false]);
    }

    #[test]
    fn a_draft_and_its_check_fold_above_the_answer() {
        let draft = TimelineEntry::Step { title: "Черновик ответа".into(), tool: "draft".into(), status: "found".into() };
        let verify = TimelineEntry::Step { title: "Проверка ответа".into(), tool: "verify".into(), status: "found".into() };
        let entries = [draft, verify, answer("В пятницу.", false)];
        let done = blocks(&entries, true);
        assert!(matches!(&done[0], Block::Folded(rows) if rows.len() == 2 && matches!(rows[0], Row::Step { tool: "draft", .. }) && matches!(rows[1], Row::Step { tool: "verify", .. })));
        assert_eq!(done[1], Block::Answer("В пятницу."));
    }
}
