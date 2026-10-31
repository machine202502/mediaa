use super::super::header;
use super::analysis::{clock, left, left_text};
use super::task_details;
use super::task_label;
use super::topics;
use crate::app::App;
use crate::features::library::library_dir;
use crate::theme::{self, Palette};
use eframe::egui::{self, Align2, CornerRadius, FontId, Rect, RichText, Sense};
use egui_phosphor::regular as icon;
use mediaa_core::slots;

use mediaa_core::catalogs::ai_dir;
use mediaa_queue::{
    self as queue, Filter, KindGroup, KindStat, Scheduler, Stats, Status as TaskStatus, Store,
    Task, TaskId, WaitReason,
};
use rust_i18n::t;
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet};
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

fn read(store: &dyn Store, filter: &Filter) -> anyhow::Result<Snapshot> {
    let everything = store.top_level(&Filter::default())?;
    let tops = if filter.scope.is_none() && filter.group == KindGroup::All {
        everything.clone()
    } else {
        store.top_level(filter)?
    };
    let mut tasks = Vec::new();
    for top in &tops {
        tasks.extend(store.subtree(top.id)?);
    }
    Ok(Snapshot {
        tops: tops.iter().map(|t| t.id).collect(),
        tasks,
        kind_stats: store.kind_stats()?,
        scopes: scopes_of(&everything),
        waits: HashMap::new(),
    })
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

fn fill_waits(snap: &mut Snapshot, views: &[queue::TaskView]) {
    snap.waits = views
        .iter()
        .filter_map(|view| {
            view.wait
                .as_ref()
                .map(|reason| (view.task.id, wait_label(reason)))
        })
        .collect();
}

fn scopes_of(tops: &[Task]) -> Vec<(String, String)> {
    let mut names: BTreeMap<String, Option<String>> = BTreeMap::new();
    for task in tops {
        let Some(scope) = &task.scope else { continue };
        let name = names.entry(scope.clone()).or_default();
        if task.kind == "analysis" && name.is_none() {
            *name = Some(task.title.clone());
        }
    }
    let mut scopes: Vec<(String, String)> = names
        .into_iter()
        .map(|(key, name)| {
            (
                key.clone(),
                name.unwrap_or_else(|| format!("{}…", key.chars().take(8).collect::<String>())),
            )
        })
        .collect();
    scopes.sort_by_key(|s| s.1.to_lowercase());
    scopes
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

pub fn tool_text(detail: &Value) -> Option<String> {
    let tool = detail.get("tool")?.as_str()?;
    let args = detail
        .get("args")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let mut text = if args.is_empty() {
        tool.to_string()
    } else {
        format!("{tool} {args}")
    };
    if let Some(chars) = detail.get("result_chars").and_then(Value::as_u64) {
        text = format!("{text} · {}", t!("queue.tool_result", n = chars));
    }
    Some(text)
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

pub fn note_text(note: Option<&queue::Note>, locale: &str) -> String {
    match note {
        Some(queue::Note::PausedBy { title }) => t!(
            "queue.note_paused_by",
            title = title.as_str(),
            locale = locale
        )
        .to_string(),
        Some(queue::Note::FailedBy { title }) => t!(
            "queue.note_failed_by",
            title = title.as_str(),
            locale = locale
        )
        .to_string(),
        Some(queue::Note::Hang { resource, idle_s }) => hang_text(*resource, *idle_s, locale),
        None => String::new(),
    }
}

pub fn hang_text(resource: queue::Resource, idle_s: u64, locale: &str) -> String {
    let what = match resource {
        queue::Resource::Speech => t!("queue.hang_speech", locale = locale).to_string(),
        queue::Resource::Embed => t!("queue.hang_embed", locale = locale).to_string(),
        queue::Resource::Llm => t!("queue.hang_llm", locale = locale).to_string(),
        queue::Resource::Provider => t!("queue.hang_provider", locale = locale).to_string(),
    };
    let idle = if idle_s >= 120 {
        t!("queue.hang_minutes", n = idle_s / 60, locale = locale).to_string()
    } else {
        t!("queue.hang_seconds", n = idle_s, locale = locale).to_string()
    };
    t!(
        "queue.hang_note",
        what = what.as_str(),
        idle = idle.as_str(),
        locale = locale
    )
    .to_string()
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

fn run(sched: &Scheduler, op: &Op) -> anyhow::Result<()> {
    match *op {
        Op::Pause(id) => {
            sched.pause(id)?;
        }
        Op::Resume(id) => {
            sched.resume(id)?;
        }
        Op::Cancel(id) => {
            sched.cancel(id)?;
        }
        Op::Restart(id) => {
            sched.restart(id)?;
        }
        Op::Remove(id) => {
            sched.remove(id)?;
        }
        Op::ClearDone => {
            sched.clear_done()?;
        }
    }
    Ok(())
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
    seen: usize,
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

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("queue.title"), "");
    let dir = ai_dir(&library_dir(&app.settings));

    let slots = app.agent.slots().status();
    runtime_card(ui, &slots);
    let host = app
        .agent
        .services()
        .map(|s| (s.sched.clone(), s.feed.clone()));
    let state = &mut app.agent.queue;
    if state.dir.as_deref() != Some(dir.as_path()) {
        state.dir = Some(dir.clone());
        state.seen = 0;
        state.read = None;
        state.error = None;
    }
    let mut eventful = false;
    if let Some((_, feed)) = &host {
        let (next, events, gap) = feed.since(state.seen);
        let restarted = next < state.seen;
        state.seen = next;
        eventful = gap || restarted || !events.is_empty();
    }
    let filter = state.filter();
    if state.read.is_none_or(|r| r.elapsed() >= REFRESH) || eventful {
        match &host {
            Some((sched, _)) => match read(sched.store().as_ref(), &filter) {
                Ok(mut snap) => {
                    if let Ok(live) = sched.snapshot() {
                        fill_waits(&mut snap, &live.tasks);
                    }
                    state.snapshot = snap;
                    state.error = None;
                }
                Err(e) => {
                    log::warn!("agent queue: {e:#}");
                    state.error = Some(format!("{e:#}"));
                }
            },
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
        egui::ComboBox::from_id_salt("queue-scope")
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

    let mut op_error = None;
    if let Some(op) = op {
        state.read = None;
        if let Some((sched, _)) = &host {
            if let Err(e) = run(sched, &op) {
                op_error = Some(format!("{e:#}"));
            }
        }
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
    if let Some(e) = op_error {
        app.error(e);
    }
    if let Some(e) = copy_error {
        app.error(e);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_load_lists_pools_and_the_web_tool() {
        let state = slots::Status {
            groups: vec![
                slots::GroupLoad {
                    name: "Single".into(),
                    busy: 1,
                    limit: 2,
                    waiting: 0,
                },
                slots::GroupLoad {
                    name: "Local".into(),
                    busy: 0,
                    limit: 1,
                    waiting: 2,
                },
            ],
            web: slots::ToolLoad {
                busy: 2,
                limit: 2,
                waiting: 1,
            },
        };
        let s = rows(&state);
        assert_eq!(
            s,
            [
                ("Single".to_string(), 1, 2, 0),
                ("Local".to_string(), 0, 1, 2)
            ]
        );
        assert_eq!(
            (state.web.busy, state.web.limit, state.web.waiting),
            (2, 2, 1)
        );
    }

    use mediaa_queue::{FakeClock, MemStore, NewTask, Resource};
    use serde_json::json;
    use std::sync::Arc;

    type Tree = (
        Arc<dyn Store>,
        Arc<Scheduler>,
        TaskId,
        Vec<TaskId>,
        Arc<FakeClock>,
    );

    fn tree() -> Tree {
        let clock = Arc::new(FakeClock::new(0));
        let store: Arc<dyn Store> = Arc::new(MemStore::new(clock.clone()));
        let sched = Arc::new(Scheduler::new(store.clone(), vec![]));
        let root = store
            .enqueue(
                NewTask::new("analysis", "talk.mp4", &json!({}))
                    .composite()
                    .scope("rec-1"),
            )
            .unwrap();
        let mut kids = Vec::new();
        for kind in ["transcribe", "index", "topics"] {
            kids.push(
                store
                    .enqueue(
                        NewTask::new(kind, "talk.mp4", &json!({}))
                            .resource(Resource::Llm)
                            .parent(root)
                            .scope("rec-1")
                            .units(2.0),
                    )
                    .unwrap(),
            );
        }
        (store, sched, root, kids, clock)
    }

    #[test]
    fn rows_flatten_in_order_and_open_on_demand() {
        let (store, _sched, root, kids, clock) = tree();
        clock.advance(1_000);
        let chat = store
            .begin_inline("chat", "Когда?", &json!({}), None, Some("rec-2"))
            .unwrap();
        let tool = store
            .begin_inline("tool", "Ищу", &json!({}), Some(chat), Some("rec-2"))
            .unwrap();
        let snap = read(store.as_ref(), &Filter::default()).unwrap();
        let mut open = HashSet::new();
        let ids = |rows: &[Row]| {
            rows.iter()
                .map(|r| (snap.tasks[r.index].id, r.depth))
                .collect::<Vec<_>>()
        };
        let rows = flatten(&snap.tasks, &snap.tops, &open);
        assert_eq!(
            ids(&rows),
            [(chat, 0), (root, 0)],
            "closed: top-level tasks only, running first"
        );
        assert_eq!(
            (rows[0].children, rows[1].children, rows[1].open),
            (1, 3, false)
        );
        open.insert(root);
        open.insert(chat);
        let rows = flatten(&snap.tasks, &snap.tops, &open);
        assert_eq!(
            ids(&rows),
            [
                (chat, 0),
                (tool, 1),
                (root, 0),
                (kids[0], 1),
                (kids[1], 1),
                (kids[2], 1)
            ]
        );
        assert!(rows[0].open && rows[2].open && !rows[1].open);
    }

    #[test]
    fn filters_pick_the_group_and_the_recording() {
        let (store, _sched, root, _, _clock) = tree();
        let chat = store
            .begin_inline("chat", "Когда?", &json!({}), None, Some("rec-2"))
            .unwrap();
        let tops = |f: &Filter| read(store.as_ref(), f).unwrap().tops;
        assert_eq!(tops(&Filter::group(KindGroup::Analysis)), [root]);
        assert_eq!(tops(&Filter::group(KindGroup::Chat)), [chat]);
        assert_eq!(tops(&Filter::scope("rec-2")), [chat]);
        let snap = read(store.as_ref(), &Filter::group(KindGroup::Chat)).unwrap();
        assert_eq!(snap.tasks.len(), 1, "only the tasks of the filter are read");
        assert_eq!(
            snap.scopes,
            [
                ("rec-2".to_string(), "rec-2…".to_string()),
                ("rec-1".to_string(), "talk.mp4".to_string())
            ],
            "the scopes are not narrowed by the filter; by name"
        );
    }

    fn task(kind: &str, status: TaskStatus) -> Task {
        let (store, ..) = tree();
        let id = store
            .enqueue(NewTask::new(kind, "x", &json!({})).resource(Resource::Llm))
            .unwrap();
        let mut t = store.get(id).unwrap().unwrap();
        t.status = status;
        t
    }

    #[test]
    fn the_time_column_says_elapsed_left_or_the_duration() {
        let mut t = task("transcribe", TaskStatus::Running);
        t.started_ms = Some(1_000);
        let running = time_text(
            &t,
            Duration::from_secs(83),
            Some(Duration::from_secs(250)),
            true,
        );
        assert!(
            running.starts_with(&t!("analysis.elapsed", time = "1:23").to_string())
                && running.contains('4'),
            "{running}"
        );
        assert!(
            time_text(&t, Duration::from_secs(5), None, true)
                .ends_with(&t!("analysis.left_unknown").to_string()),
            "no estimate yet"
        );
        assert!(time_text(
            &t,
            Duration::from_secs(5),
            Some(Duration::from_secs(20)),
            true
        )
        .ends_with(&t!("analysis.left_under").to_string()));
        assert_eq!(
            time_text(
                &t,
                Duration::from_secs(5),
                Some(Duration::from_secs(20)),
                false
            ),
            t!("analysis.elapsed", time = "0:05").to_string(),
            "chat: no estimate"
        );
        t.status = TaskStatus::Done;
        assert_eq!(
            time_text(&t, Duration::from_secs(754), Some(Duration::ZERO), true),
            "12:34"
        );
        t.status = TaskStatus::Queued;
        t.started_ms = None;
        assert_eq!(
            time_text(&t, Duration::ZERO, Some(Duration::from_secs(600)), true),
            left_text(left(Some(Duration::from_secs(600))))
        );
        t.status = TaskStatus::Cancelled;
        assert_eq!(
            time_text(&t, Duration::ZERO, None, true),
            "",
            "never started"
        );
        t.status = TaskStatus::Failed;
        t.started_ms = Some(1_000);
        assert_eq!(
            time_text(&t, Duration::from_secs(3), None, true),
            "0:03",
            "a failure shows when it started"
        );
        t.status = TaskStatus::Stopping;
        assert!(
            time_text(&t, Duration::from_secs(5), None, true).contains("0:05"),
            "stopping keeps the clock"
        );
    }

    #[test]
    fn a_composite_counts_its_children_and_sums_their_work() {
        let (store, _sched, root, kids, _clock) = tree();
        store.claim(kids[0]).unwrap();
        store
            .add_stats(
                kids[0],
                &Stats {
                    model_calls: 2,
                    prompt_tokens: 100,
                    completion_tokens: 20,
                    model_ms: 5,
                    models: vec!["C:\\m\\Qwen3-Q4.gguf#4096".into()],
                },
            )
            .unwrap();
        store
            .add_stats(
                kids[1],
                &Stats {
                    model_calls: 1,
                    prompt_tokens: 10,
                    completion_tokens: 5,
                    model_ms: 1,
                    models: vec!["owner/embed".into()],
                },
            )
            .unwrap();
        store.complete(kids[0], &json!({})).unwrap();
        let snap = read(store.as_ref(), &Filter::default()).unwrap();
        let top = snap.tasks.iter().find(|t| t.id == root).unwrap();
        assert_eq!(
            count_text(top, &snap.tasks),
            Some(t!("analysis.of", done = 1, total = 3).to_string())
        );
        let leaf = snap.tasks.iter().find(|t| t.id == kids[2]).unwrap();
        assert_eq!(count_text(leaf, &snap.tasks), None, "a leaf has no count");
        let stats = subtree_stats(top, &snap.tasks);
        assert_eq!(
            (
                stats.model_calls,
                stats.prompt_tokens,
                stats.completion_tokens
            ),
            (3, 110, 25)
        );
        let text = stats_text(&stats);
        assert!(
            text.contains("110")
                && text.contains("25")
                && text.contains("Qwen3-Q4.gguf")
                && text.contains("embed")
                && !text.contains("owner"),
            "{text}"
        );
        assert_eq!(stats_text(&Stats::default()), "", "nothing called");
    }

    #[test]
    fn a_tool_row_shows_the_tool_its_arguments_and_the_size_of_the_result() {
        assert_eq!(tool_text(&json!({})), None);
        let detail =
            json!({ "tool": "search", "args": "{\"query\":\"отчёт\"}", "result_chars": 1234 });
        let text = tool_text(&detail).unwrap();
        assert!(
            text.starts_with("search {\"query\"")
                && text.ends_with(&t!("queue.tool_result", n = 1234).to_string()),
            "{text}"
        );
        let mut tool = task("tool", TaskStatus::Done);
        tool.detail = detail;
        tool.stats = Stats {
            model_calls: 1,
            prompt_tokens: 7,
            completion_tokens: 0,
            model_ms: 1,
            models: vec!["embed".into()],
        };
        let stats = stats_text(&tool.stats);
        assert!(stats.contains('7') && stats.ends_with("embed"), "{stats}");
    }

    #[test]
    fn the_row_says_the_label_and_the_error() {
        let mut t = task("topic-digest", TaskStatus::Failed);
        t.title = "#3".into();
        t.error = Some("  модель упала \n".into());
        let texts = row_texts(&t, std::slice::from_ref(&t), &[], 0, "ru");
        assert_eq!(texts.label, task_label::label("topic-digest", "#3"));
        assert_eq!(texts.error.as_deref(), Some("модель упала"));
        t.note = Some(queue::pause_note("Индекс: rec"));
        assert_eq!(
            row_texts(&t, std::slice::from_ref(&t), &[], 0, "ru")
                .error
                .as_deref(),
            Some("ждёт: Индекс: rec на паузе")
        );
        assert_eq!(
            row_texts(&t, std::slice::from_ref(&t), &[], 0, "en")
                .error
                .as_deref(),
            Some("waiting: Индекс: rec is paused")
        );
        t.note = None;
        t.error = Some("   ".into());
        assert_eq!(
            row_texts(&t, std::slice::from_ref(&t), &[], 0, "ru").error,
            None
        );
    }

    #[test]
    fn a_note_is_words_in_the_language_of_the_window() {
        let failed = queue::failure_note("Темы: rec");
        assert_eq!(note_text(Some(&failed), "ru"), "из-за ошибки в Темы: rec");
        assert_eq!(
            note_text(Some(&failed), "en"),
            "because of a failure in Темы: rec"
        );
        let hang = queue::hang_note(queue::Resource::Llm, Duration::from_secs(300));
        assert_eq!(
            note_text(Some(&hang), "ru"),
            "модель не отвечает дольше 5 мин — вызов прерван, задача продолжит с последней точки"
        );
        assert_eq!(
            note_text(Some(&hang), "en"),
            "the model has not responded for over 5 min — the call was interrupted; the task will continue from the last point"
        );
        let short = queue::hang_note(queue::Resource::Embed, Duration::from_secs(45));
        assert_eq!(
            note_text(Some(&short), "ru"),
            "индексация не отвечает дольше 45 с — вызов прерван, задача продолжит с последней точки"
        );
        assert_eq!(note_text(None, "ru"), "");
    }

    #[test]
    fn actions_follow_the_state_and_the_kind_of_task() {
        let a = |kind: &str, status| actions(&task(kind, status));
        assert_eq!(
            a("transcribe", TaskStatus::Running),
            Actions {
                pause: true,
                cancel: true,
                remove: true,
                ..Actions::default()
            }
        );
        let failed = a("index", TaskStatus::Failed);
        assert!(failed.restart && !failed.resume && !failed.pause && !failed.cancel);
        let done = a("index", TaskStatus::Done);
        assert!(done.remove && !done.restart && !done.pause && !done.resume && !done.cancel);
        let paused = a("index", TaskStatus::Paused);
        assert!(paused.resume && paused.cancel && !paused.restart);
        assert!(a("index", TaskStatus::Cancelled).restart);
        assert!(!a("index", TaskStatus::Queued).restart);
        let mut chat = task("chat", TaskStatus::Running);
        chat.inline = true;
        assert_eq!(actions(&chat), Actions::default());
        chat.status = TaskStatus::Done;
        assert_eq!(
            actions(&chat),
            Actions {
                remove: true,
                ..Actions::default()
            }
        );
        let mut answer = task("chat-answer", TaskStatus::Running);
        answer.inline = false;
        let offered = actions(&answer);
        assert!(
            offered.cancel
                && !offered.pause
                && !offered.resume
                && !offered.restart
                && !offered.remove
        );
        answer.status = TaskStatus::Stopping;
        assert!(actions(&answer).cancel && !actions(&answer).resume);
        answer.status = TaskStatus::Done;
        assert_eq!(
            actions(&answer),
            Actions {
                remove: true,
                ..Actions::default()
            }
        );
        let mut child = task("index", TaskStatus::Done);
        child.parent = Some(1);
        assert!(!actions(&child).remove);
    }

    #[test]
    fn the_actions_of_a_composite_reach_its_subtree() {
        let (store, sched, root, kids, _clock) = tree();
        run(&sched, &Op::Pause(kids[0])).unwrap();
        assert_eq!(
            store.get(kids[0]).unwrap().unwrap().status,
            TaskStatus::Paused
        );
        assert_eq!(
            store.get(kids[1]).unwrap().unwrap().status,
            TaskStatus::Queued
        );
        assert_eq!(
            store.get(root).unwrap().unwrap().status,
            TaskStatus::Running
        );
        run(&sched, &Op::Resume(root)).unwrap();
        assert_eq!(
            store.get(kids[0]).unwrap().unwrap().status,
            TaskStatus::Queued
        );
        run(&sched, &Op::Cancel(root)).unwrap();
        for id in kids.iter().chain([&root]) {
            assert_eq!(
                store.get(*id).unwrap().unwrap().status,
                TaskStatus::Cancelled
            );
        }
        run(&sched, &Op::Restart(root)).unwrap();
        assert_eq!(
            store.get(kids[2]).unwrap().unwrap().status,
            TaskStatus::Queued
        );
        assert_eq!(
            store.get(root).unwrap().unwrap().status,
            TaskStatus::Running,
            "queued children are free to run"
        );
        run(&sched, &Op::Remove(root)).unwrap();
        assert!(store.list().unwrap().is_empty(), "the children go with it");
    }

    #[test]
    fn the_columns_stay_put_whatever_the_depth() {
        let row = Rect::from_min_size(egui::pos2(10.0, 100.0), egui::vec2(1000.0, ROW_H));
        let (a, b) = (geom(row, 0), geom(row, 3));
        assert_eq!(
            (a.bar, a.count, a.time, a.actions),
            (b.bar, b.count, b.time, b.actions)
        );
        assert_eq!(b.chevron.left() - a.chevron.left(), 3.0 * INDENT);
        assert!(
            a.label.right() <= a.bar.left()
                && a.bar.right() < a.count.left()
                && a.count.right() < a.time.left()
                && a.time.right() < a.actions[0].left()
        );
        assert_eq!(a.count.left() - a.bar.right(), GAP);
        assert_eq!(a.time.left() - a.count.right(), GAP);
        assert_eq!(a.actions[0].left() - a.time.right(), GAP);
        let narrow = geom(
            Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, ROW_H)),
            5,
        );
        assert!(narrow.label.width() >= 0.0);
    }

    #[test]
    fn a_click_opens_details_and_only_the_chevron_folds() {
        assert_eq!(row_intent(true, true), RowIntent::Fold);
        assert_eq!(row_intent(true, false), RowIntent::Details);
        assert_eq!(
            row_intent(false, true),
            RowIntent::Details,
            "a leaf has no chevron"
        );
        assert_eq!(row_intent(false, false), RowIntent::Details);
    }
}
