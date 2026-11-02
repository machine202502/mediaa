use super::super::header;
use super::analysis::{clock, left, left_text};
use super::task_details;
use super::task_label;
use super::topics;
use crate::features::library::library_dir;
use crate::ui::pages::ctx::Ctx;
use crate::ui::theme::{self, Palette};
use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, RichText, Sense};
use egui_phosphor::regular as icon;
use mediaa_core::slots;

use crate::features::agent::view as queue;
use crate::features::agent::view::{
    note_text, Filter, KindGroup, KindStat, Stats, Status as TaskStatus, Task, TaskId, WaitReason,
};
use crate::features::intent::Intent;
use mediaa_core::catalogs::ai_dir;
use mediaa_protocol::backend::AgentSnapshotWire;
use rust_i18n::t;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const REFRESH: Duration = Duration::from_secs(1);

#[derive(Default)]
struct Snapshot {
    tops: Vec<TaskId>,
    tasks: Vec<Task>,
    kind_stats: Vec<KindStat>,
    scopes: Vec<(String, String)>,
    waits: HashMap<TaskId, String>,
}

fn read(snapshot: &AgentSnapshotWire, filter: &Filter) -> Snapshot {
    let everything = queue::tasks(snapshot);
    let tops: Vec<TaskId> = everything
        .iter()
        .filter(|task| task.parent.is_none() && filter.matches(task))
        .map(|task| task.id)
        .collect();
    let tasks = everything
        .iter()
        .filter(|task| belongs(task, &everything, &tops))
        .cloned()
        .collect::<Vec<_>>();
    let scopes = snapshot
        .scopes
        .iter()
        .map(|scope| (scope.key.clone(), scope.name.clone()))
        .collect();
    let waits = tasks
        .iter()
        .filter_map(|task| {
            task.waiting
                .as_ref()
                .map(|reason| (task.id, wait_label(reason)))
        })
        .collect();
    Snapshot {
        tops,
        tasks,
        kind_stats: queue::kind_stats(snapshot),
        scopes,
        waits,
    }
}

fn belongs(task: &Task, tasks: &[Task], tops: &[TaskId]) -> bool {
    if tops.contains(&task.id) {
        return true;
    }
    let mut parent = task.parent;
    while let Some(id) = parent {
        if tops.contains(&id) {
            return true;
        }
        parent = tasks
            .iter()
            .find(|task| task.id == id)
            .and_then(|task| task.parent);
    }
    false
}

pub fn wait_label(reason: &WaitReason) -> String {
    match reason {
        WaitReason::Dependency => t!("queue.wait_dependency").into(),
        WaitReason::Resource(queue::Resource::Speech) => t!("queue.wait_speech").into(),
        WaitReason::Resource(queue::Resource::Embed) => t!("queue.wait_embed").into(),
        WaitReason::Resource(queue::Resource::Llm) => t!("queue.wait_llm").into(),
        WaitReason::Resource(queue::Resource::Provider) => t!("queue.wait_provider").into(),
        WaitReason::Memory => t!("queue.wait_memory_pool").into(),
        WaitReason::Paused => t!("queue.wait_paused").into(),
        WaitReason::Turn => t!("queue.reason_turn").into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Row {
    pub index: usize,
    pub depth: usize,
    pub children: usize,
    pub open: bool,
}

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
    let mut stack: Vec<(usize, usize)> = tops
        .iter()
        .rev()
        .filter_map(|id| by_id.get(id))
        .map(|&i| (i, 0))
        .collect();
    while let Some((index, depth)) = stack.pop() {
        let kids = children
            .get(&tasks[index].id)
            .map_or(&[][..], Vec::as_slice);
        let is_open = open.contains(&tasks[index].id) && !kids.is_empty();
        rows.push(Row {
            index,
            depth,
            children: kids.len(),
            open: is_open,
        });
        if is_open {
            stack.extend(kids.iter().rev().map(|&i| (i, depth + 1)));
        }
    }
    rows
}

pub fn count_text(task: &Task, tasks: &[Task]) -> Option<String> {
    let kids: Vec<&Task> = tasks.iter().filter(|t| t.parent == Some(task.id)).collect();
    if kids.is_empty() {
        return None;
    }
    let done = kids.iter().filter(|t| t.status == TaskStatus::Done).count();
    Some(t!("analysis.of", done = done, total = kids.len()).into())
}

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

fn short_model(model: &str) -> &str {
    let name = model.split_once('#').map_or(model, |(name, _)| name);
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

pub fn stats_text(stats: &Stats) -> String {
    if stats.model_calls == 0 && stats.prompt_tokens == 0 && stats.completion_tokens == 0 {
        return String::new();
    }
    let mut text = t!(
        "queue.stats",
        calls = stats.model_calls,
        input = stats.prompt_tokens,
        output = stats.completion_tokens
    )
    .to_string();
    let models: Vec<&str> = stats.models.iter().map(|m| short_model(m)).collect();
    if !models.is_empty() {
        text = format!("{text} · {}", models.join(", "));
    }
    text
}

pub fn time_text(
    task: &Task,
    elapsed: Duration,
    estimate: Option<Duration>,
    estimated: bool,
) -> String {
    let left_part = || left_text(left(estimate));
    match task.status {
        TaskStatus::Running | TaskStatus::Stopping => {
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

#[derive(Debug, Clone, PartialEq)]
pub struct RowTexts {
    pub label: String,
    pub count: Option<String>,
    pub time: String,
    pub error: Option<String>,
}

pub fn row_texts(
    task: &Task,
    tasks: &[Task],
    kind_stats: &[KindStat],
    now_ms: i64,
    locale: &str,
) -> RowTexts {
    let estimated = !task.inline;
    let estimate = if estimated {
        queue::estimate_tree(task, tasks, now_ms, kind_stats)
    } else {
        None
    };
    let note = note_text(task.note.as_ref(), locale);
    RowTexts {
        label: task_label::label(&task.kind, &task.title),
        count: count_text(task, tasks),
        time: time_text(task, queue::elapsed(task, now_ms), estimate, estimated),
        error: if note.is_empty() {
            task.error
                .as_ref()
                .map(|e| e.trim().to_string())
                .filter(|e| !e.is_empty())
        } else {
            Some(note)
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Actions {
    pub pause: bool,
    pub resume: bool,
    pub cancel: bool,
    pub restart: bool,
    pub remove: bool,
}

pub fn actions(task: &Task) -> Actions {
    let chat = matches!(task.kind.as_str(), "chat" | "chat-answer");
    let active = matches!(
        task.status,
        TaskStatus::Queued | TaskStatus::Running | TaskStatus::Waiting
    );
    let stopping = task.status == TaskStatus::Stopping;
    let own = !task.inline && !chat;
    Actions {
        pause: own && active,
        resume: own && task.status == TaskStatus::Paused,
        cancel: !task.inline
            && !task.status.is_finished()
            && (own || chat)
            && (active || stopping || task.status == TaskStatus::Paused),
        restart: own && matches!(task.status, TaskStatus::Failed | TaskStatus::Cancelled),
        remove: task.parent.is_none() && (task.status.is_finished() || own),
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

fn command(op: &Op) -> mediaa_protocol::backend::AgentOpRequestWire {
    use mediaa_protocol::backend::{AgentOpRequestWire, AgentOpWire};
    match *op {
        Op::Pause(id) => AgentOpRequestWire {
            task: Some(id),
            op: AgentOpWire::Pause,
        },
        Op::Resume(id) => AgentOpRequestWire {
            task: Some(id),
            op: AgentOpWire::Resume,
        },
        Op::Cancel(id) => AgentOpRequestWire {
            task: Some(id),
            op: AgentOpWire::Cancel,
        },
        Op::Restart(id) => AgentOpRequestWire {
            task: Some(id),
            op: AgentOpWire::Restart,
        },
        Op::Remove(id) => AgentOpRequestWire {
            task: Some(id),
            op: AgentOpWire::Remove,
        },
        Op::ClearDone => AgentOpRequestWire {
            task: None,
            op: AgentOpWire::ClearDone,
        },
    }
}

const ROW_H: f32 = 28.0;
const GAP: f32 = 8.0;
const INDENT: f32 = 18.0;
const CHEVRON_W: f32 = 16.0;
const ICON_W: f32 = 18.0;
const BAR_W: f32 = 80.0;
const COUNT_W: f32 = 54.0;
const TIME_W: f32 = 236.0;
const ACTION: f32 = theme::ICON_BUTTON - 2.0;
const SLOTS: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Geom {
    pub chevron: Rect,
    pub icon: Rect,
    pub label: Rect,
    pub bar: Rect,
    pub count: Rect,
    pub time: Rect,
    pub actions: [Rect; SLOTS],
}

pub fn geom(row: Rect, depth: usize) -> Geom {
    let (top, bottom) = (row.top(), row.top() + ROW_H);
    let centre = (top + bottom) / 2.0;
    let column = |right: f32, width: f32| {
        Rect::from_min_max(egui::pos2(right - width, top), egui::pos2(right, bottom))
    };
    let mut edge = row.right();
    let mut actions = [Rect::NOTHING; SLOTS];
    for slot in actions.iter_mut().rev() {
        *slot = Rect::from_min_max(
            egui::pos2(edge - ACTION, centre - ACTION / 2.0),
            egui::pos2(edge, centre + ACTION / 2.0),
        );
        edge -= ACTION + GAP / 2.0;
    }
    edge -= GAP / 2.0;
    let time = column(edge, TIME_W);
    edge = time.left() - GAP;
    let count = column(edge, COUNT_W);
    edge = count.left() - GAP;
    let bar = Rect::from_min_max(
        egui::pos2(edge - BAR_W, centre - 3.0),
        egui::pos2(edge, centre + 3.0),
    );
    edge = bar.left() - GAP;
    let start = row.left() + depth as f32 * INDENT;
    let chevron = Rect::from_min_max(
        egui::pos2(start, top),
        egui::pos2(start + CHEVRON_W, bottom),
    );
    let icon_left = chevron.right() + GAP / 2.0;
    let icon = Rect::from_min_max(
        egui::pos2(icon_left, top),
        egui::pos2(icon_left + ICON_W, bottom),
    );
    let label_left = icon.right() + GAP;
    let label = Rect::from_min_max(
        egui::pos2(label_left, top),
        egui::pos2(edge.max(label_left), bottom),
    );
    Geom {
        chevron,
        icon,
        label,
        bar,
        count,
        time,
        actions,
    }
}

#[derive(Default)]
pub struct QueueUi {
    dir: Option<PathBuf>,
    error: Option<String>,
    snapshot: Snapshot,
    read: Option<Instant>,
    group: KindGroup,
    scope: Option<String>,
    open: HashSet<TaskId>,
    details: Option<TaskId>,
    details_log: String,
    details_copied: bool,
}

impl QueueUi {
    fn filter(&self) -> Filter {
        Filter {
            scope: self.scope.clone(),
            group: self.group,
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

fn status_label(s: TaskStatus) -> (&'static str, String) {
    match s {
        TaskStatus::Queued => (icon::CLOCK, t!("queue.queued").into()),
        TaskStatus::Running => (icon::SPINNER_GAP, t!("queue.running").into()),
        TaskStatus::Stopping => (icon::SPINNER_GAP, t!("queue.stopping").into()),
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
        TaskStatus::Running | TaskStatus::Stopping => p.accent,
        _ => p.text_weak,
    }
}

fn elided(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font: FontId,
    color: egui::Color32,
    roster: &super::mentions::Roster,
    p: Palette,
) -> std::sync::Arc<egui::Galley> {
    super::mentions::galley(ui, text, roster, font, color, width, true, false, p)
}

#[allow(clippy::too_many_arguments)]
fn column_text(
    ui: &egui::Ui,
    rect: Rect,
    text: &str,
    font: FontId,
    color: egui::Color32,
    right: bool,
    roster: &super::mentions::Roster,
    p: Palette,
) {
    if text.is_empty() || rect.width() <= 0.0 {
        return;
    }
    let galley = elided(ui, text, rect.width(), font, color, roster, p);
    let x = if right {
        rect.right() - galley.size().x
    } else {
        rect.left()
    };
    ui.painter_at(rect).galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
}

struct RowClick {
    op: Option<Op>,
    details: Option<TaskId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowIntent {
    Fold,
    Details,
}

pub fn row_intent(has_children: bool, on_chevron: bool) -> RowIntent {
    if has_children && on_chevron {
        RowIntent::Fold
    } else {
        RowIntent::Details
    }
}

#[allow(clippy::too_many_arguments)]
fn row_ui(
    ui: &mut egui::Ui,
    row: Row,
    snap: &Snapshot,
    open: &mut HashSet<TaskId>,
    now: i64,
    ai: &std::path::Path,
    rosters: &mut HashMap<String, super::mentions::Roster>,
    p: Palette,
) -> RowClick {
    let task = &snap.tasks[row.index];
    let key = task.scope.clone().unwrap_or_default();
    let roster = rosters
        .entry(key.clone())
        .or_insert_with(|| super::mentions::Roster::of_file(ai, &key))
        .clone();
    let texts = row_texts(
        task,
        &snap.tasks,
        &snap.kind_stats,
        now,
        &rust_i18n::locale(),
    );
    let shown_error = texts.error.as_deref();
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_H), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let g = geom(rect, row.depth);
    let painter = ui.painter_at(rect);
    let mut op = None;
    if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.card_hover);
    }
    if row.children > 0 {
        painter.text(
            g.chevron.center(),
            Align2::CENTER_CENTER,
            if row.open {
                icon::CARET_DOWN
            } else {
                icon::CARET_RIGHT
            },
            FontId::proportional(12.0),
            p.text_weak,
        );
    }
    let (glyph, _) = status_label(task.status);
    let color = status_color(task.status, p);
    if matches!(task.status, TaskStatus::Running | TaskStatus::Stopping) {
        egui::Spinner::new().size(14.0).color(color).paint_at(
            ui,
            Rect::from_center_size(g.icon.center(), egui::vec2(14.0, 14.0)),
        );
    } else {
        painter.text(
            g.icon.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(14.0),
            color,
        );
    }
    let weak = matches!(
        task.status,
        TaskStatus::Done
            | TaskStatus::Cancelled
            | TaskStatus::Queued
            | TaskStatus::Waiting
            | TaskStatus::Paused
    );
    let label_color = if weak && task.status != TaskStatus::Queued {
        p.text_weak
    } else {
        p.text
    };
    match shown_error {
        Some(error) => column_text(
            ui,
            g.label,
            &format!("{} — {error}", texts.label),
            FontId::proportional(13.0),
            p.danger,
            false,
            &roster,
            p,
        ),
        None => column_text(
            ui,
            g.label,
            &texts.label,
            FontId::proportional(13.0),
            label_color,
            false,
            &roster,
            p,
        ),
    }
    if matches!(
        task.status,
        TaskStatus::Running
            | TaskStatus::Stopping
            | TaskStatus::Paused
            | TaskStatus::Queued
            | TaskStatus::Waiting
    ) && task.progress > 0.0
    {
        painter.rect_filled(g.bar, CornerRadius::same(3), p.border);
        let fill = Rect::from_min_size(
            g.bar.min,
            egui::vec2(
                g.bar.width() * task.progress.clamp(0.0, 1.0),
                g.bar.height(),
            ),
        );
        painter.rect_filled(fill, CornerRadius::same(3), p.accent);
    }
    if let Some(count) = &texts.count {
        column_text(
            ui,
            g.count,
            count,
            FontId::proportional(11.5),
            p.text_weak,
            true,
            &roster,
            p,
        );
    }
    let mut time = texts.time.clone();
    if let Some(wait) = snap.waits.get(&task.id) {
        time = if time.is_empty() {
            wait.clone()
        } else {
            format!("{time} · {wait}")
        };
    }
    column_text(
        ui,
        g.time,
        &time,
        FontId::proportional(11.5),
        p.text_weak,
        true,
        &roster,
        p,
    );
    let status_resp = ui.interact(
        g.icon,
        ui.id().with(("queue-status", task.id)),
        Sense::hover(),
    );
    let mut status_tip = status_label(task.status).1;
    if let Some(wait) = snap.waits.get(&task.id) {
        status_tip = format!("{status_tip}\n{wait}");
    }
    topics::text_tip(
        &status_resp,
        &status_tip,
        &super::mentions::Roster::default(),
    );
    let label_resp = ui.interact(
        g.label,
        ui.id().with(("queue-label", task.id)),
        Sense::hover(),
    );
    let mut full = texts.label.clone();
    if let Some(error) = shown_error {
        full = format!("{full}\n{error}");
    }
    topics::text_tip(&label_resp, &full, &roster);
    let offered = actions(task);
    let buttons: [Option<(&str, String, Op)>; SLOTS] = [
        if offered.pause {
            Some((
                icon::PAUSE,
                t!("queue.pause").to_string(),
                Op::Pause(task.id),
            ))
        } else if offered.resume {
            Some((
                icon::PLAY,
                t!("queue.resume").to_string(),
                Op::Resume(task.id),
            ))
        } else {
            None
        },
        offered
            .cancel
            .then(|| (icon::X, t!("queue.cancel").to_string(), Op::Cancel(task.id))),
        offered.restart.then(|| {
            (
                icon::ARROW_COUNTER_CLOCKWISE,
                t!("queue.restart").to_string(),
                Op::Restart(task.id),
            )
        }),
        offered.remove.then(|| {
            (
                icon::TRASH,
                t!("queue.remove").to_string(),
                Op::Remove(task.id),
            )
        }),
    ];
    for (slot, button) in g.actions.iter().zip(buttons) {
        let Some((glyph, tip, wanted)) = button else {
            continue;
        };
        let clicked = ui
            .scope_builder(egui::UiBuilder::new().max_rect(*slot), |ui| {
                theme::icon_button_sized(ui, glyph, &tip, ACTION)
            })
            .inner
            .clicked();
        if clicked {
            op = Some(wanted);
        }
    }
    let mut details = None;
    if resp.clicked() && op.is_none() {
        let on_chevron = resp
            .interact_pointer_pos()
            .is_some_and(|p| g.chevron.contains(p));
        match row_intent(row.children > 0, on_chevron) {
            RowIntent::Details => details = Some(task.id),
            RowIntent::Fold => {
                if !open.remove(&task.id) {
                    open.insert(task.id);
                }
            }
        }
    }
    RowClick { op, details }
}

pub fn ui(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    header(ui, &t!("queue.title"), "");
    let dir = ai_dir(&library_dir(ctx.settings));
    let snapshot = super::live_buffer::snapshot();
    let slots = snapshot.as_ref().map(queue::slots).unwrap_or_default();
    runtime_card(ui, &slots);

    let state = &mut ctx.agent_ui.queue;
    if state.dir.as_deref() != Some(dir.as_path()) {
        state.dir = Some(dir.clone());
        state.read = None;
        state.error = None;
    }
    let filter = state.filter();
    if state.read.is_none_or(|r| r.elapsed() >= REFRESH) {
        match &snapshot {
            Some(snapshot) => {
                state.snapshot = read(snapshot, &filter);
                state.error = None;
            }
            None => state.error = Some(t!("studio.agent_not_ready").into()),
        }
        state.read = Some(Instant::now());
    }
    if let Some(e) = &state.error {
        ui.colored_label(theme::palette(ui).danger, e);
    }
    ui.ctx().request_repaint_after(REFRESH);

    let mut op = None;
    let mut refilter = false;
    ui.horizontal(|ui| {
        let groups = [
            (KindGroup::All, t!("queue.filter_all").to_string()),
            (KindGroup::Analysis, t!("queue.filter_analysis").to_string()),
            (KindGroup::Chat, t!("queue.filter_chat").to_string()),
        ];
        refilter |= theme::segmented(ui, &mut state.group, &groups);
        let selected = state
            .scope
            .as_ref()
            .and_then(|key| state.snapshot.scopes.iter().find(|(k, _)| k == key))
            .map_or_else(
                || t!("queue.scope_any").to_string(),
                |(_, name)| name.clone(),
            );
        crate::ui::theme::combo_box("queue-scope")
            .width(220.0)
            .selected_text(selected)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(state.scope.is_none(), t!("queue.scope_any"))
                    .clicked()
                {
                    state.scope = None;
                    refilter = true;
                }
                for (key, name) in &state.snapshot.scopes {
                    if ui
                        .selectable_label(state.scope.as_ref() == Some(key), name)
                        .clicked()
                    {
                        state.scope = Some(key.clone());
                        refilter = true;
                    }
                }
            });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let any_done = state
                .snapshot
                .tasks
                .iter()
                .any(|t| t.parent.is_none() && t.status == TaskStatus::Done);
            if ui
                .add_enabled(
                    any_done,
                    theme::secondary(icon::BROOM, &t!("queue.clear_done")),
                )
                .clicked()
            {
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
    let mut rosters: HashMap<String, super::mentions::Roster> = HashMap::new();
    let mut open_details = None;
    if !rows.is_empty() {
        theme::card(ui)
            .inner_margin(egui::Margin::symmetric(8, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                for row in rows {
                    let click = row_ui(
                        ui,
                        row,
                        &state.snapshot,
                        &mut state.open,
                        now,
                        &dir,
                        &mut rosters,
                        p,
                    );
                    if let Some(wanted) = click.op {
                        op = Some(wanted);
                    }
                    if let Some(id) = click.details {
                        open_details = Some(id);
                    }
                }
            });
    }
    if let Some(id) = open_details {
        if state.details != Some(id) {
            let around = state
                .snapshot
                .tasks
                .iter()
                .find(|t| t.id == id)
                .and_then(|t| t.finished_ms)
                .and_then(task_details::local_time);
            state.details_log =
                task_details::log_excerpt(&task_details::read_program_log(), id, around);
            state.details = Some(id);
            state.details_copied = false;
        }
    }

    if let Some(op) = op {
        state.read = None;
        ctx.intents.push(Intent::AgentOp(command(&op)));
    }
    let copy_error = {
        let task = state
            .details
            .and_then(|id| state.snapshot.tasks.iter().find(|t| t.id == id).cloned());
        let stats = task
            .as_ref()
            .map(|task| subtree_stats(task, &state.snapshot.tasks));
        let log = state.details_log.clone();
        match (task, stats) {
            (Some(task), Some(stats)) => task_window(
                ui.ctx(),
                &task,
                &stats,
                &log,
                &mut state.details_copied,
                &mut state.details,
                &dir,
            ),
            (None, _) if state.details.is_some() => {
                state.details = None;
                None
            }
            _ => None,
        }
    };
    if let Some(e) = copy_error {
        ctx.error(e);
    }
}

fn task_window(
    ctx: &egui::Context,
    task: &Task,
    stats: &Stats,
    log: &str,
    copied: &mut bool,
    open: &mut Option<TaskId>,
    ai: &std::path::Path,
) -> Option<String> {
    let roster = task
        .scope
        .as_deref()
        .map(|id| super::mentions::Roster::of_file(ai, id))
        .unwrap_or_default();
    match task_details::show(ctx, task, stats, now_ms(), log, copied, &roster) {
        Some(task_details::Cmd::Close) => {
            *open = None;
            None
        }
        Some(task_details::Cmd::CopyFailed(e)) => Some(e),
        None => None,
    }
}

fn rows(state: &slots::Status) -> Vec<(String, u32, u32, usize)> {
    state
        .groups
        .iter()
        .map(|group| (group.name.clone(), group.busy, group.limit, group.waiting))
        .collect()
}

fn runtime_card(ui: &mut egui::Ui, state: &slots::Status) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::GAUGE, &t!("queue.budget"));
        let p = theme::palette(ui);
        theme::section(ui, icon::STACK, &t!("queue.pools"));
        for (name, busy, limit, waiting) in rows(state) {
            load_row(ui, p, &name, busy, limit, waiting);
        }
        ui.add_space(4.0);
        theme::section(ui, icon::GLOBE, &t!("queue.tools"));
        load_row(
            ui,
            p,
            &t!("agent.web_search"),
            state.web.busy,
            state.web.limit,
            state.web.waiting,
        );
    });
    ui.add_space(4.0);
}

fn load_row(ui: &mut egui::Ui, p: Palette, name: &str, busy: u32, limit: u32, waiting: usize) {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.spacing_mut().item_spacing.x = 3.0;
        for i in 0..limit.max(busy) {
            let taken = i < busy;
            ui.label(
                RichText::new(if taken {
                    icon::CIRCLE
                } else {
                    icon::CIRCLE_DASHED
                })
                .color(if taken { p.accent } else { p.text_weak }),
            );
        }
        theme::hint(ui, t!("queue.slots", busy = busy, max = limit));
        if waiting > 0 {
            ui.label(RichText::new(t!("queue.waits", n = waiting)).color(p.warning));
        }
    });
}
