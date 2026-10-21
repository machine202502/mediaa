//! Queue: the work of the library (`.ai/queue.db`) as a tree — the analysis of recordings with
//! its stages and their parts, chat answers with their model steps and tools — with status,
//! progress, time, the model calls and tokens it took, and the actions on it (pause, resume,
//! cancel, start over, remove; on a composite they act on its whole subtree). Also what the
//! engine is doing: the memory it uses, the models it keeps in memory and the requests that wait
//! for it (a download, memory, a free slot).
//!
//! The decisions (the flat list of rows, their texts, the geometry, which actions a row offers)
//! are pure functions on `mediaa_core::queue` data, tested without a window; the drawing follows
//! them.

use super::super::header;
use super::super::library::library_dir;
use super::analysis::{clock, left, left_text};
use super::engine_state::{download_bar, EngineState};
use super::task_label;
use super::topics;
use crate::app::App;
use crate::theme::{self, Palette};
use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, RichText, Sense};
use egui_phosphor::regular as icon;
use mediaa_agent::slots;
use mediaa_core::agent::{Budget, WorkKind};
use mediaa_core::catalogs::ai_dir;
use mediaa_core::queue::{self, Filter, KindGroup, KindStat, Queue, Stats, Task, TaskId, TaskStatus};
use mediaa_engine::{LoadedModel, Pool, Status, WaitingModel};
use rust_i18n::t;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the list is re-read while the page is open.
const REFRESH: Duration = Duration::from_secs(1);

// ---------------------------------------------------------------- what is read

/// One reading of the queue: the top-level tasks of the filter with everything below them.
#[derive(Default)]
struct Snapshot {
    /// Ids of the top-level tasks, in the queue's order.
    tops: Vec<TaskId>,
    /// The top-level tasks and all their descendants.
    tasks: Vec<Task>,
    kind_stats: Vec<KindStat>,
    /// Recordings (scopes) that have top-level tasks: key and the name to show.
    scopes: Vec<(String, String)>,
}

/// Reads the queue for `filter`.
fn read(queue: &Queue, filter: &Filter) -> anyhow::Result<Snapshot> {
    let everything = queue.top_level(&Filter::default())?;
    let tops = if filter.scope.is_none() && filter.group == KindGroup::All { everything.clone() } else { queue.top_level(filter)? };
    let mut tasks = Vec::new();
    for top in &tops {
        tasks.extend(queue.subtree(top.id)?);
    }
    Ok(Snapshot { tops: tops.iter().map(|t| t.id).collect(), tasks, kind_stats: queue.kind_stats()?, scopes: scopes_of(&everything) })
}

/// The recordings that have work in the queue: the key and the name (the file of its analysis, or
/// the start of the key when there is no analysis of it).
fn scopes_of(tops: &[Task]) -> Vec<(String, String)> {
    let mut names: BTreeMap<String, Option<String>> = BTreeMap::new();
    for task in tops {
        let Some(scope) = &task.scope else { continue };
        let name = names.entry(scope.clone()).or_default();
        if task.kind == "analysis" && name.is_none() {
            *name = Some(task.title.clone());
        }
    }
    let mut scopes: Vec<(String, String)> =
        names.into_iter().map(|(key, name)| (key.clone(), name.unwrap_or_else(|| format!("{}…", key.chars().take(8).collect::<String>())))).collect();
    scopes.sort_by_key(|s| s.1.to_lowercase());
    scopes
}

// ---------------------------------------------------------------- the rows

/// A row of the tree on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    /// Index into the flat list of tasks.
    pub index: usize,
    pub depth: usize,
    /// Direct children.
    pub children: usize,
    pub open: bool,
}

/// The rows to show: the top-level tasks in their order, each followed (when open) by its
/// children by id and, below them, theirs.
pub fn flatten(tasks: &[Task], tops: &[TaskId], open: &HashSet<TaskId>) -> Vec<Row> {
    let mut children: HashMap<TaskId, Vec<usize>> = HashMap::new();
    for (i, task) in tasks.iter().enumerate() {
        if let Some(parent) = task.parent {
            children.entry(parent).or_default().push(i);
        }
    }
    for list in children.values_mut() {
        list.sort_by_key(|&i| tasks[i].id);
    }
    let by_id: HashMap<TaskId, usize> = tasks.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let mut rows = Vec::new();
    // A stack of (index, depth), the next sibling on top.
    let mut stack: Vec<(usize, usize)> = tops.iter().rev().filter_map(|id| by_id.get(id)).map(|&i| (i, 0)).collect();
    while let Some((index, depth)) = stack.pop() {
        let kids = children.get(&tasks[index].id).map_or(&[][..], Vec::as_slice);
        let is_open = open.contains(&tasks[index].id) && !kids.is_empty();
        rows.push(Row { index, depth, children: kids.len(), open: is_open });
        if is_open {
            stack.extend(kids.iter().rev().map(|&i| (i, depth + 1)));
        }
    }
    rows
}

/// «7 из 12»: the done children of a composite and all of them.
pub fn count_text(task: &Task, tasks: &[Task]) -> Option<String> {
    let kids: Vec<&Task> = tasks.iter().filter(|t| t.parent == Some(task.id)).collect();
    if kids.is_empty() {
        return None;
    }
    let done = kids.iter().filter(|t| t.status == TaskStatus::Done).count();
    Some(t!("analysis.of", done = done, total = kids.len()).into())
}

/// What the task and everything below it used.
pub fn subtree_stats(task: &Task, tasks: &[Task]) -> Stats {
    let mut total = Stats::default();
    let mut stack = vec![task.id];
    while let Some(id) = stack.pop() {
        for t in tasks.iter().filter(|t| t.id == id) {
            total.add(&t.stats);
        }
        stack.extend(tasks.iter().filter(|t| t.parent == Some(id)).map(|t| t.id));
    }
    total
}

/// A model as the page names it: the file or the id, without the folder and the context size.
fn short_model(model: &str) -> &str {
    let name = model.split_once('#').map_or(model, |(name, _)| name);
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// «вызовов: 3 · токены: 1240 / 85 · Qwen3.5-4B»; empty when nothing was called.
pub fn stats_text(stats: &Stats) -> String {
    if stats.model_calls == 0 && stats.prompt_tokens == 0 && stats.completion_tokens == 0 {
        return String::new();
    }
    let mut text = t!("queue.stats", calls = stats.model_calls, input = stats.prompt_tokens, output = stats.completion_tokens).to_string();
    let models: Vec<&str> = stats.models.iter().map(|m| short_model(m)).collect();
    if !models.is_empty() {
        text = format!("{text} · {}", models.join(", "));
    }
    text
}

/// A tool's row: its name and short arguments, and how much it returned; `None` for a task that
/// is not a tool call.
pub fn tool_text(detail: &Value) -> Option<String> {
    let tool = detail.get("tool")?.as_str()?;
    let args = detail.get("args").and_then(Value::as_str).unwrap_or_default();
    let mut text = if args.is_empty() { tool.to_string() } else { format!("{tool} {args}") };
    if let Some(chars) = detail.get("result_chars").and_then(Value::as_u64) {
        text = format!("{text} · {}", t!("queue.tool_result", n = chars));
    }
    Some(text)
}

/// The right-hand text of a row: the tool call for a tool, else what the models used (a tool
/// that called a model adds that).
pub fn right_text(task: &Task, tasks: &[Task]) -> String {
    let stats = stats_text(&subtree_stats(task, tasks));
    match tool_text(&task.detail) {
        Some(tool) if stats.is_empty() => tool,
        Some(tool) => format!("{tool} · {stats}"),
        None => stats,
    }
}

/// The time column: «идёт 1:23 · ≈ осталось 4 мин» while it runs, the estimate while it waits,
/// the duration when it is over. A chat task has no estimate to show (`estimated` is false).
pub fn time_text(task: &Task, elapsed: Duration, estimate: Option<Duration>, estimated: bool) -> String {
    let left_part = || left_text(left(estimate));
    match task.status {
        TaskStatus::Running => {
            let gone = t!("analysis.elapsed", time = clock(elapsed)).to_string();
            if estimated {
                format!("{gone} · {}", left_part())
            } else {
                gone
            }
        }
        TaskStatus::Queued | TaskStatus::Waiting | TaskStatus::Paused => {
            if estimated {
                left_part()
            } else {
                String::new()
            }
        }
        TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => {
            if task.started_ms.is_some() {
                clock(elapsed)
            } else {
                String::new()
            }
        }
    }
}

/// Everything a row says, as text.
#[derive(Debug, Clone, PartialEq)]
pub struct RowTexts {
    pub label: String,
    pub count: Option<String>,
    pub time: String,
    pub right: String,
    pub error: Option<String>,
}

pub fn row_texts(task: &Task, tasks: &[Task], kind_stats: &[KindStat], now_ms: i64) -> RowTexts {
    let estimated = !task.inline;
    let estimate = if estimated { queue::estimate_tree(task, tasks, now_ms, kind_stats) } else { None };
    RowTexts {
        label: task_label::label(&task.kind, &task.title),
        count: count_text(task, tasks),
        time: time_text(task, queue::elapsed(task, now_ms), estimate, estimated),
        right: right_text(task, tasks),
        error: task.error.as_ref().map(|e| e.trim().to_string()).filter(|e| !e.is_empty()),
    }
}

// ---------------------------------------------------------------- actions

/// What the row offers (they act on the task's whole subtree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Actions {
    pub pause: bool,
    pub resume: bool,
    pub cancel: bool,
    pub restart: bool,
    pub remove: bool,
}

pub fn actions(task: &Task) -> Actions {
    let active = matches!(task.status, TaskStatus::Queued | TaskStatus::Running | TaskStatus::Waiting);
    // A chat answer runs in the program that asked for it: the queue cannot stop or restart it
    // (the answer's own stop button does); a finished one can only be removed.
    let own = !task.inline;
    Actions {
        pause: own && active,
        resume: own && matches!(task.status, TaskStatus::Paused | TaskStatus::Failed),
        cancel: own && (active || task.status == TaskStatus::Paused),
        restart: own && task.status != TaskStatus::Running,
        remove: task.parent.is_none() && (own || task.status.is_finished()),
    }
}

enum Op {
    Pause(TaskId),
    Resume(TaskId),
    Cancel(TaskId),
    Restart(TaskId),
    Remove(TaskId),
    ClearDone,
}

fn run(queue: &Queue, op: &Op) -> anyhow::Result<()> {
    match *op {
        Op::Pause(id) => queue.pause(id),
        Op::Resume(id) => queue.resume(id),
        Op::Cancel(id) => queue.cancel(id),
        Op::Restart(id) => queue.restart(id),
        Op::Remove(id) => queue.remove(id),
        Op::ClearDone => queue.clear_done(),
    }
}

// ---------------------------------------------------------------- geometry

/// Height of a row.
const ROW_H: f32 = 28.0;
/// The gap between the columns of a row — one constant for all of them.
const GAP: f32 = 8.0;
/// Shift of a row's start per level of the tree.
const INDENT: f32 = 18.0;
const CHEVRON_W: f32 = 16.0;
const ICON_W: f32 = 18.0;
const BAR_W: f32 = 80.0;
const COUNT_W: f32 = 54.0;
const TIME_W: f32 = 236.0;
const RIGHT_W: f32 = 270.0;
/// Side of an action button.
const ACTION: f32 = theme::ICON_BUTTON - 2.0;
/// Actions per row: pause or continue, cancel, start over, remove.
const SLOTS: usize = 4;

/// The columns of one row. Everything right of the label sits at the same place in every row,
/// whatever the depth or the state; only the start of a row moves with its depth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geom {
    pub chevron: Rect,
    pub icon: Rect,
    pub label: Rect,
    pub bar: Rect,
    pub count: Rect,
    pub time: Rect,
    pub right: Rect,
    pub actions: [Rect; SLOTS],
}

pub fn geom(row: Rect, depth: usize) -> Geom {
    let (top, bottom) = (row.top(), row.top() + ROW_H);
    let centre = (top + bottom) / 2.0;
    let column = |right: f32, width: f32| Rect::from_min_max(egui::pos2(right - width, top), egui::pos2(right, bottom));
    let mut edge = row.right();
    let mut actions = [Rect::NOTHING; SLOTS];
    for slot in actions.iter_mut().rev() {
        *slot = Rect::from_min_max(egui::pos2(edge - ACTION, centre - ACTION / 2.0), egui::pos2(edge, centre + ACTION / 2.0));
        edge -= ACTION + GAP / 2.0;
    }
    edge -= GAP / 2.0;
    let right = column(edge, RIGHT_W);
    edge = right.left() - GAP;
    let time = column(edge, TIME_W);
    edge = time.left() - GAP;
    let count = column(edge, COUNT_W);
    edge = count.left() - GAP;
    let bar = Rect::from_min_max(egui::pos2(edge - BAR_W, centre - 3.0), egui::pos2(edge, centre + 3.0));
    edge = bar.left() - GAP;
    let start = row.left() + depth as f32 * INDENT;
    let chevron = Rect::from_min_max(egui::pos2(start, top), egui::pos2(start + CHEVRON_W, bottom));
    let icon_left = chevron.right() + GAP / 2.0;
    let icon = Rect::from_min_max(egui::pos2(icon_left, top), egui::pos2(icon_left + ICON_W, bottom));
    let label_left = icon.right() + GAP;
    let label = Rect::from_min_max(egui::pos2(label_left, top), egui::pos2(edge.max(label_left), bottom));
    Geom { chevron, icon, label, bar, count, time, right, actions }
}

// ---------------------------------------------------------------- the page

#[derive(Default)]
pub struct QueueUi {
    queue: Option<(PathBuf, Result<Queue, String>)>,
    snapshot: Snapshot,
    read: Option<Instant>,
    group: KindGroup,
    scope: Option<String>,
    /// Composites whose children are shown.
    open: HashSet<TaskId>,
}

impl QueueUi {
    fn filter(&self) -> Filter {
        Filter { scope: self.scope.clone(), group: self.group }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn status_label(s: TaskStatus) -> (&'static str, String) {
    match s {
        TaskStatus::Queued => (icon::CLOCK, t!("queue.queued").into()),
        TaskStatus::Running => (icon::SPINNER_GAP, t!("queue.running").into()),
        TaskStatus::Waiting => (icon::HOURGLASS, t!("queue.status_waiting").into()),
        TaskStatus::Paused => (icon::PAUSE_CIRCLE, t!("queue.paused").into()),
        TaskStatus::Done => (icon::CHECK_CIRCLE, t!("queue.done").into()),
        TaskStatus::Failed => (icon::WARNING_CIRCLE, t!("queue.failed").into()),
        TaskStatus::Cancelled => (icon::X_CIRCLE, t!("queue.cancelled").into()),
    }
}

fn status_color(status: TaskStatus, p: Palette) -> egui::Color32 {
    match status {
        TaskStatus::Failed => p.danger,
        TaskStatus::Done => p.success,
        TaskStatus::Running => p.accent,
        _ => p.text_weak,
    }
}

fn elided(ui: &egui::Ui, text: &str, width: f32, font: FontId, color: egui::Color32) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(RichText::new(text).font(font).color(color)).into_galley(ui, Some(egui::TextWrapMode::Truncate), width.max(0.0), egui::TextStyle::Body)
}

/// Text in a column: left-aligned, cut with an ellipsis; `right` aligns it to the column's end.
fn column_text(ui: &egui::Ui, rect: Rect, text: &str, font: FontId, color: egui::Color32, right: bool) {
    if text.is_empty() || rect.width() <= 0.0 {
        return;
    }
    let galley = elided(ui, text, rect.width(), font, color);
    let x = if right { rect.right() - galley.size().x } else { rect.left() };
    ui.painter_at(rect).galley(egui::pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
}

/// One row of the tree. Returns the operation a button of it asked for.
fn row_ui(ui: &mut egui::Ui, row: Row, snap: &Snapshot, open: &mut HashSet<TaskId>, now: i64, p: Palette) -> Option<Op> {
    let task = &snap.tasks[row.index];
    let texts = row_texts(task, &snap.tasks, &snap.kind_stats, now);
    let shown_error = texts.error.as_deref();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::click());
    let g = geom(rect, row.depth);
    let painter = ui.painter_at(rect);
    let mut op = None;
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.card_hover);
    }
    if row.children > 0 && resp.clicked() && !open.remove(&task.id) {
        open.insert(task.id);
    }
    if row.children > 0 {
        painter.text(g.chevron.center(), Align2::CENTER_CENTER, if row.open { icon::CARET_DOWN } else { icon::CARET_RIGHT }, FontId::proportional(12.0), p.text_weak);
    }
    let (glyph, _) = status_label(task.status);
    let color = status_color(task.status, p);
    if task.status == TaskStatus::Running {
        egui::Spinner::new().size(14.0).color(color).paint_at(ui, Rect::from_center_size(g.icon.center(), egui::vec2(14.0, 14.0)));
    } else {
        painter.text(g.icon.center(), Align2::CENTER_CENTER, glyph, FontId::proportional(14.0), color);
    }
    let weak = matches!(task.status, TaskStatus::Done | TaskStatus::Cancelled | TaskStatus::Queued | TaskStatus::Waiting | TaskStatus::Paused);
    let label_color = if weak && task.status != TaskStatus::Queued { p.text_weak } else { p.text };
    // A failed task says why in its label column, in red (the whole text is in the hint).
    match shown_error {
        Some(error) => column_text(ui, g.label, &format!("{} — {error}", texts.label), FontId::proportional(13.0), p.danger, false),
        None => column_text(ui, g.label, &texts.label, FontId::proportional(13.0), label_color, false),
    }
    // The progress bar: the same place in every row.
    if matches!(task.status, TaskStatus::Running | TaskStatus::Paused | TaskStatus::Queued | TaskStatus::Waiting) && task.progress > 0.0 {
        painter.rect_filled(g.bar, CornerRadius::same(3), p.border);
        let fill = Rect::from_min_size(g.bar.min, egui::vec2(g.bar.width() * task.progress.clamp(0.0, 1.0), g.bar.height()));
        painter.rect_filled(fill, CornerRadius::same(3), p.accent);
    }
    if let Some(count) = &texts.count {
        column_text(ui, g.count, count, FontId::proportional(11.5), p.text_weak, true);
    }
    column_text(ui, g.time, &texts.time, FontId::proportional(11.5), p.text_weak, true);
    column_text(ui, g.right, &texts.right, FontId::proportional(11.5), p.text_weak, true);
    // Hints of the parts that may be cut.
    let status_resp = ui.interact(g.icon, ui.id().with(("queue-status", task.id)), Sense::hover());
    topics::text_tip(&status_resp, &status_label(task.status).1);
    let label_resp = ui.interact(g.label, ui.id().with(("queue-label", task.id)), Sense::hover());
    let mut full = texts.label.clone();
    if let Some(error) = shown_error {
        full = format!("{full}\n{error}");
    }
    topics::text_tip(&label_resp, &full);
    if !texts.right.is_empty() {
        let right_resp = ui.interact(g.right, ui.id().with(("queue-right", task.id)), Sense::hover());
        topics::text_tip(&right_resp, &texts.right);
    }
    // The buttons: four fixed slots; one that does not apply leaves its place empty.
    let offered = actions(task);
    let buttons: [Option<(&str, String, Op)>; SLOTS] = [
        if offered.pause {
            Some((icon::PAUSE, t!("queue.pause").to_string(), Op::Pause(task.id)))
        } else if offered.resume {
            Some((icon::PLAY, t!("queue.resume").to_string(), Op::Resume(task.id)))
        } else {
            None
        },
        offered.cancel.then(|| (icon::X, t!("queue.cancel").to_string(), Op::Cancel(task.id))),
        offered.restart.then(|| (icon::ARROW_COUNTER_CLOCKWISE, t!("queue.restart").to_string(), Op::Restart(task.id))),
        offered.remove.then(|| (icon::TRASH, t!("queue.remove").to_string(), Op::Remove(task.id))),
    ];
    for (slot, button) in g.actions.iter().zip(buttons) {
        let Some((glyph, tip, wanted)) = button else { continue };
        let clicked = ui.scope_builder(egui::UiBuilder::new().max_rect(*slot), |ui| theme::icon_button_sized(ui, glyph, &tip, ACTION)).inner.clicked();
        if clicked {
            op = Some(wanted);
        }
    }
    op
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("queue.title"), "");
    let dir = ai_dir(&library_dir(&app.settings));
    let budget = app.settings.agent.budget.clone();
    let state = &mut app.agent.queue;
    let slots = app.agent.slots.status();
    runtime_card(ui, &app.agent.engine_state, &slots, &budget);
    if state.queue.as_ref().is_none_or(|(d, _)| *d != dir) {
        state.queue = Some((dir.clone(), Queue::open(&dir).map_err(|e| format!("{e:#}"))));
        state.read = None;
    }
    let filter = state.filter();
    let Some((_, opened)) = &state.queue else { return };
    let queue = match opened {
        Ok(q) => q,
        Err(e) => {
            ui.colored_label(theme::palette(ui).danger, e);
            return;
        }
    };
    if state.read.is_none_or(|r| r.elapsed() >= REFRESH) {
        match read(queue, &filter) {
            Ok(s) => state.snapshot = s,
            Err(e) => log::warn!("agent queue: {e:#}"),
        }
        state.read = Some(Instant::now());
    }
    ui.ctx().request_repaint_after(REFRESH);

    let mut op = None;
    let mut refilter = false;
    // The filters and «remove finished»: always there, so nothing moves when they matter.
    ui.horizontal(|ui| {
        let groups = [(KindGroup::All, t!("queue.filter_all").to_string()), (KindGroup::Analysis, t!("queue.filter_analysis").to_string()), (KindGroup::Chat, t!("queue.filter_chat").to_string())];
        refilter |= theme::segmented(ui, &mut state.group, &groups);
        let selected = state.scope.as_ref().and_then(|key| state.snapshot.scopes.iter().find(|(k, _)| k == key)).map_or_else(|| t!("queue.scope_any").to_string(), |(_, name)| name.clone());
        egui::ComboBox::from_id_salt("queue-scope").width(220.0).selected_text(selected).show_ui(ui, |ui| {
            if ui.selectable_label(state.scope.is_none(), t!("queue.scope_any")).clicked() {
                state.scope = None;
                refilter = true;
            }
            for (key, name) in &state.snapshot.scopes {
                if ui.selectable_label(state.scope.as_ref() == Some(key), name).clicked() {
                    state.scope = Some(key.clone());
                    refilter = true;
                }
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any_done = state.snapshot.tasks.iter().any(|t| t.parent.is_none() && t.status == TaskStatus::Done);
            if ui.add_enabled(any_done, theme::secondary(icon::BROOM, &t!("queue.clear_done"))).clicked() {
                op = Some(Op::ClearDone);
            }
        });
    });
    if refilter {
        state.read = None;
    }
    ui.add_space(6.0);
    if state.snapshot.tops.is_empty() {
        theme::hint(ui, t!("queue.empty"));
    }
    let rows = flatten(&state.snapshot.tasks, &state.snapshot.tops, &state.open);
    let now = now_ms();
    let p = theme::palette(ui);
    if !rows.is_empty() {
        theme::card(ui).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            for row in rows {
                if let Some(wanted) = row_ui(ui, row, &state.snapshot, &mut state.open, now, p) {
                    op = Some(wanted);
                }
            }
        });
    }

    if let Some(op) = op {
        state.read = None;
        if let Err(e) = run(queue, &op) {
            app.error(format!("{e:#}"));
        }
    }
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / f64::from(1u32 << 30))
}

/// Memory taken against its budget: the bar's fill and `4.1 / 8.0 GB`.
fn memory_bar(used: u64, max: u64) -> (f32, String) {
    let fill = if max == 0 { 1.0 } else { (used as f64 / max as f64).clamp(0.0, 1.0) as f32 };
    (fill, format!("{} / {} GB", gb(used), gb(max)))
}

/// Per kind of work: (kind, jobs running, its limit, jobs waiting).
fn slots(status: &slots::Status, budget: &Budget) -> Vec<(WorkKind, u32, u32, usize)> {
    WorkKind::ALL
        .iter()
        .map(|&k| {
            let running = status.running.iter().find(|(r, _)| *r == k).map_or(0, |(_, n)| *n);
            (k, running, budget.limit(k), status.waiting.iter().filter(|(_, w)| *w == k).count())
        })
        .collect()
}

/// A loaded model: its file name, where it runs, its size and how many jobs use it.
fn model_line(m: &LoadedModel) -> String {
    let path = m.model.rsplit_once('#').map_or(m.model.as_str(), |(p, _)| p);
    let name = Path::new(path).file_name().map_or(path.into(), |n| n.to_string_lossy());
    let pool = match m.pool {
        Pool::Gpu => t!("queue.on_gpu"),
        Pool::Cpu => t!("queue.on_cpu"),
    };
    format!("{name} · {pool} · {} GB · {}", gb(m.bytes), t!("queue.jobs", n = m.users))
}

/// Why a request waits for the engine.
fn reason_label(reason: &str) -> String {
    match reason {
        "downloading" => t!("queue.reason_downloading").into(),
        "memory" => t!("queue.reason_memory").into(),
        "slot" => t!("queue.reason_slot").into(),
        "turn" => t!("queue.reason_turn").into(),
        other => t!("queue.reason_other", reason = other).into(),
    }
}

/// A waiting request: its model (the engine may not know it yet) and why it waits.
fn waiting_line(ui: &mut egui::Ui, w: &WaitingModel) {
    let name = w.model.clone().unwrap_or_else(|| t!("queue.a_job").to_string());
    ui.add(egui::Label::new(RichText::new(name).strong()).truncate());
    ui.horizontal(|ui| {
        theme::hint(ui, reason_label(&w.reason));
        if w.reason == "downloading" {
            if let Some(p) = &w.progress {
                let (fill, text) = download_bar(p.done, p.total);
                ui.add(egui::ProgressBar::new(fill).desired_width(200.0).text(text));
            }
        }
    });
    ui.end_row();
}

/// The engine's memory and models, and the slots of each kind of work.
fn runtime_card(ui: &mut egui::Ui, engine: &EngineState, state: &slots::Status, budget: &Budget) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::GAUGE, &t!("queue.budget"));
        let p = theme::palette(ui);
        let status: Option<&Status> = engine.status();
        let (gpu_used, ram_used) = status.map_or((0, 0), |s| (s.used.gpu_bytes, s.used.ram_bytes));
        egui::Grid::new("queue-budget").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            for (label, used, max) in [(t!("budget.gpu_memory"), gpu_used, budget.gpu_bytes()), (t!("budget.ram_memory"), ram_used, budget.ram_bytes())] {
                let (fill, text) = memory_bar(used, max);
                ui.label(label);
                let color = if fill >= 0.95 { p.danger } else { p.accent };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(220.0, 8.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, p.accent_soft);
                if fill > 0.0 {
                    let done = egui::Rect::from_min_size(rect.min, egui::vec2((rect.width() * fill).max(rect.height()), rect.height()));
                    ui.painter().rect_filled(done, 4.0, color);
                }
                theme::hint(ui, text);
                ui.end_row();
            }
            for (kind, running, limit, waiting) in slots(state, budget) {
                ui.label(super::budget::kind_label(kind));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for i in 0..limit.max(running) {
                        let busy = i < running;
                        ui.label(RichText::new(if busy { icon::CIRCLE } else { icon::CIRCLE_DASHED }).color(if busy { p.accent } else { p.text_weak }));
                    }
                    theme::hint(ui, t!("queue.slots", busy = running, max = limit));
                });
                if waiting > 0 {
                    ui.label(RichText::new(t!("queue.waits", n = waiting)).color(p.warning));
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
        if let Some(status) = status {
            if !status.waiting.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{}  {}", icon::HOURGLASS, t!("queue.waiting"))).strong());
                egui::Grid::new("queue-engine-waiting").num_columns(2).spacing([12.0, 4.0]).show(ui, |ui| {
                    for w in &status.waiting {
                        waiting_line(ui, w);
                    }
                });
            }
            if !status.loaded.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(format!("{}  {}", icon::MEMORY, t!("queue.models"))).strong());
                for m in &status.loaded {
                    theme::hint(ui, model_line(m));
                }
            }
        } else {
            ui.add_space(4.0);
            engine.offline_hint(ui);
        }
    });
    ui.add_space(4.0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_engine::{Budget as EngineBudget, Progress, Used};

    #[test]
    fn budget_in_use() {
        let engine = Status {
            loaded: vec![LoadedModel { model: "C:\\m\\models\\q\\Qwen3-Q4_K_M.gguf#4096".into(), pool: Pool::Gpu, bytes: 3 << 30, users: 2 }],
            waiting: vec![],
            budget: EngineBudget { gpu_bytes: 8 << 30, ram_bytes: 8 << 30 },
            used: Used { gpu_bytes: 3 << 30, ram_bytes: 0 },
        };
        let state = slots::Status { running: vec![(WorkKind::Agent, 1), (WorkKind::WebSearch, 2)], waiting: vec![(7, WorkKind::WebSearch), (8, WorkKind::Transcription)] };
        let budget = Budget::default();
        assert_eq!(memory_bar(engine.used.gpu_bytes, budget.gpu_bytes()), (0.375, "3.0 / 8.0 GB".to_string()));
        assert_eq!(memory_bar(5, 0).0, 1.0, "no budget: full");
        let s = slots(&state, &budget);
        assert_eq!(s.len(), WorkKind::ALL.len());
        assert!(s.contains(&(WorkKind::Agent, 1, 2, 0)) && s.contains(&(WorkKind::WebSearch, 2, 2, 1)) && s.contains(&(WorkKind::Transcription, 0, 1, 1)), "{s:?}");
        let line = model_line(&engine.loaded[0]);
        assert!(line.starts_with("Qwen3-Q4_K_M.gguf · ") && line.contains(" · 3.0 GB · ") && line.ends_with('2'), "{line}");
        let waiting = WaitingModel { id: 1, model: Some("owner/repo".into()), reason: "downloading".into(), progress: Some(Progress { done: 1 << 29, total: 1 << 30 }) };
        assert_eq!(waiting.model.as_deref(), Some("owner/repo"));
        assert_eq!(reason_label(&waiting.reason), t!("queue.reason_downloading").to_string());
        assert_eq!(reason_label("mystery"), "mystery", "an unknown reason is shown as the engine sent it");
        assert_eq!(download_bar(waiting.progress.as_ref().unwrap().done, 1 << 30).0, 0.5);
    }

    use mediaa_core::queue::NewTask;
    use serde_json::json;

    fn tree() -> (tempfile::TempDir, Queue, TaskId, Vec<TaskId>) {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let root = q.enqueue(NewTask::new("analysis", "talk.mp4", &json!({})).composite().scope("rec-1")).unwrap();
        let mut kids = Vec::new();
        for kind in ["transcribe", "index", "topics"] {
            kids.push(q.enqueue(NewTask::new(kind, "talk.mp4", &json!({})).parent(root).scope("rec-1").units(2.0)).unwrap());
        }
        (dir, q, root, kids)
    }

    #[test]
    fn rows_flatten_in_order_and_open_on_demand() {
        let (_dir, q, root, kids) = tree();
        let chat = q.begin_inline("chat", "Когда?", &json!({}), None, Some("rec-2")).unwrap();
        let tool = q.begin_inline("tool", "Ищу", &json!({}), Some(chat), Some("rec-2")).unwrap();
        let snap = read(&q, &Filter::default()).unwrap();
        let mut open = HashSet::new();
        let ids = |rows: &[Row]| rows.iter().map(|r| (snap.tasks[r.index].id, r.depth)).collect::<Vec<_>>();
        let rows = flatten(&snap.tasks, &snap.tops, &open);
        assert_eq!(ids(&rows), [(chat, 0), (root, 0)], "closed: top-level tasks only, running first");
        assert_eq!((rows[0].children, rows[1].children, rows[1].open), (1, 3, false));
        open.insert(root);
        open.insert(chat);
        let rows = flatten(&snap.tasks, &snap.tops, &open);
        assert_eq!(ids(&rows), [(chat, 0), (tool, 1), (root, 0), (kids[0], 1), (kids[1], 1), (kids[2], 1)]);
        assert!(rows[0].open && rows[2].open && !rows[1].open);
    }

    #[test]
    fn filters_pick_the_group_and_the_recording() {
        let (_dir, q, root, _) = tree();
        let chat = q.begin_inline("chat", "Когда?", &json!({}), None, Some("rec-2")).unwrap();
        let tops = |f: &Filter| read(&q, f).unwrap().tops;
        assert_eq!(tops(&Filter::group(KindGroup::Analysis)), [root]);
        assert_eq!(tops(&Filter::group(KindGroup::Chat)), [chat]);
        assert_eq!(tops(&Filter::scope("rec-2")), [chat]);
        let snap = read(&q, &Filter::group(KindGroup::Chat)).unwrap();
        assert_eq!(snap.tasks.len(), 1, "only the tasks of the filter are read");
        assert_eq!(snap.scopes, [("rec-2".to_string(), "rec-2…".to_string()), ("rec-1".to_string(), "talk.mp4".to_string())], "the scopes are not narrowed by the filter; by name");
    }

    fn task(kind: &str, status: TaskStatus) -> Task {
        let (_dir, q, ..) = tree();
        let id = q.enqueue(NewTask::new(kind, "x", &json!({}))).unwrap();
        let mut t = q.get(id).unwrap().unwrap();
        t.status = status;
        t
    }

    #[test]
    fn the_time_column_says_elapsed_left_or_the_duration() {
        let mut t = task("transcribe", TaskStatus::Running);
        t.started_ms = Some(1_000);
        let running = time_text(&t, Duration::from_secs(83), Some(Duration::from_secs(250)), true);
        assert!(running.starts_with(&t!("analysis.elapsed", time = "1:23").to_string()) && running.contains('4'), "{running}");
        assert!(time_text(&t, Duration::from_secs(5), None, true).ends_with(&t!("analysis.left_unknown").to_string()), "no estimate yet");
        assert!(time_text(&t, Duration::from_secs(5), Some(Duration::from_secs(20)), true).ends_with(&t!("analysis.left_under").to_string()));
        assert_eq!(time_text(&t, Duration::from_secs(5), Some(Duration::from_secs(20)), false), t!("analysis.elapsed", time = "0:05").to_string(), "chat: no estimate");
        t.status = TaskStatus::Done;
        assert_eq!(time_text(&t, Duration::from_secs(754), Some(Duration::ZERO), true), "12:34");
        t.status = TaskStatus::Queued;
        t.started_ms = None;
        assert_eq!(time_text(&t, Duration::ZERO, Some(Duration::from_secs(600)), true), left_text(left(Some(Duration::from_secs(600)))));
        t.status = TaskStatus::Cancelled;
        assert_eq!(time_text(&t, Duration::ZERO, None, true), "", "never started");
    }

    #[test]
    fn a_composite_counts_its_children_and_sums_their_work() {
        let (_dir, q, root, kids) = tree();
        q.claim(&["transcribe"]).unwrap();
        q.add_stats(kids[0], &Stats { model_calls: 2, prompt_tokens: 100, completion_tokens: 20, model_ms: 5, models: vec!["C:\\m\\Qwen3-Q4.gguf#4096".into()] }).unwrap();
        q.add_stats(kids[1], &Stats { model_calls: 1, prompt_tokens: 10, completion_tokens: 5, model_ms: 1, models: vec!["owner/embed".into()] }).unwrap();
        q.complete(kids[0], &json!({})).unwrap();
        let snap = read(&q, &Filter::default()).unwrap();
        let top = snap.tasks.iter().find(|t| t.id == root).unwrap();
        assert_eq!(count_text(top, &snap.tasks), Some(t!("analysis.of", done = 1, total = 3).to_string()));
        let leaf = snap.tasks.iter().find(|t| t.id == kids[2]).unwrap();
        assert_eq!(count_text(leaf, &snap.tasks), None, "a leaf has no count");
        let stats = subtree_stats(top, &snap.tasks);
        assert_eq!((stats.model_calls, stats.prompt_tokens, stats.completion_tokens), (3, 110, 25));
        let text = right_text(top, &snap.tasks);
        assert!(text.contains("110") && text.contains("25") && text.contains("Qwen3-Q4.gguf") && text.contains("embed") && !text.contains("owner"), "{text}");
        assert_eq!(stats_text(&Stats::default()), "", "nothing called");
    }

    #[test]
    fn a_tool_row_shows_the_tool_its_arguments_and_the_size_of_the_result() {
        assert_eq!(tool_text(&json!({})), None);
        let detail = json!({ "tool": "search", "args": "{\"query\":\"отчёт\"}", "result_chars": 1234 });
        let text = tool_text(&detail).unwrap();
        assert!(text.starts_with("search {\"query\"") && text.ends_with(&t!("queue.tool_result", n = 1234).to_string()), "{text}");
        let mut tool = task("tool", TaskStatus::Done);
        tool.detail = detail;
        tool.stats = Stats { model_calls: 1, prompt_tokens: 7, completion_tokens: 0, model_ms: 1, models: vec!["embed".into()] };
        let right = right_text(&tool, std::slice::from_ref(&tool));
        assert!(right.starts_with(&text) && right.ends_with("embed"), "{right}");
    }

    #[test]
    fn the_row_says_the_label_and_the_error() {
        let mut t = task("topic-digest", TaskStatus::Failed);
        t.title = "#3".into();
        t.error = Some("  модель упала \n".into());
        let texts = row_texts(&t, std::slice::from_ref(&t), &[], 0);
        assert_eq!(texts.label, task_label::label("topic-digest", "#3"));
        assert_eq!(texts.error.as_deref(), Some("модель упала"));
        t.error = Some("   ".into());
        assert_eq!(row_texts(&t, std::slice::from_ref(&t), &[], 0).error, None);
    }

    #[test]
    fn actions_follow_the_state_and_the_kind_of_task() {
        let a = |kind: &str, status| actions(&task(kind, status));
        assert_eq!(a("transcribe", TaskStatus::Running), Actions { pause: true, cancel: true, remove: true, ..Actions::default() });
        let failed = a("index", TaskStatus::Failed);
        assert!(failed.resume && failed.restart && !failed.pause && !failed.cancel);
        let done = a("index", TaskStatus::Done);
        assert!(done.restart && done.remove && !done.pause && !done.resume && !done.cancel);
        let paused = a("index", TaskStatus::Paused);
        assert!(paused.resume && paused.cancel);
        // A chat task belongs to the program that runs it: only a finished one can be removed.
        let mut chat = task("chat", TaskStatus::Running);
        chat.inline = true;
        assert_eq!(actions(&chat), Actions::default());
        chat.status = TaskStatus::Done;
        assert_eq!(actions(&chat), Actions { remove: true, ..Actions::default() });
        // A child is not removed on its own.
        let mut child = task("index", TaskStatus::Done);
        child.parent = Some(1);
        assert!(!actions(&child).remove);
    }

    #[test]
    fn the_actions_of_a_composite_reach_its_subtree() {
        let (_dir, q, root, kids) = tree();
        run(&q, &Op::Pause(root)).unwrap();
        for id in kids.iter().chain([&root]) {
            assert_eq!(q.get(*id).unwrap().unwrap().status, TaskStatus::Paused);
        }
        run(&q, &Op::Resume(root)).unwrap();
        assert_eq!(q.get(kids[1]).unwrap().unwrap().status, TaskStatus::Queued);
        run(&q, &Op::Cancel(root)).unwrap();
        assert_eq!(q.get(kids[2]).unwrap().unwrap().status, TaskStatus::Cancelled);
        run(&q, &Op::Restart(root)).unwrap();
        assert_eq!(q.get(kids[2]).unwrap().unwrap().status, TaskStatus::Queued);
        assert_eq!(q.get(root).unwrap().unwrap().status, TaskStatus::Waiting);
        run(&q, &Op::Remove(root)).unwrap();
        assert!(q.list().unwrap().is_empty(), "the children go with it");
    }

    #[test]
    fn the_columns_stay_put_whatever_the_depth() {
        let row = Rect::from_min_size(egui::pos2(10.0, 100.0), egui::vec2(1000.0, ROW_H));
        let (a, b) = (geom(row, 0), geom(row, 3));
        assert_eq!((a.bar, a.count, a.time, a.right, a.actions), (b.bar, b.count, b.time, b.right, b.actions));
        assert_eq!(b.chevron.left() - a.chevron.left(), 3.0 * INDENT);
        assert!(a.label.right() <= a.bar.left() && a.bar.right() < a.count.left() && a.count.right() < a.time.left() && a.time.right() < a.right.left());
        assert!(a.right.right() < a.actions[0].left());
        // The gaps between neighbouring columns are the same constant.
        assert_eq!(a.count.left() - a.bar.right(), GAP);
        assert_eq!(a.time.left() - a.count.right(), GAP);
        assert_eq!(a.right.left() - a.time.right(), GAP);
        // A narrow row only squeezes the label.
        let narrow = geom(Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, ROW_H)), 5);
        assert!(narrow.label.width() >= 0.0);
    }
}
