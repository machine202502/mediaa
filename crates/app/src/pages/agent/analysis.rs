use super::topics;
use crate::theme::{self, Palette};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, StrokeKind,
};
use egui_phosphor::regular as icon;
use mediaa_agent::outline::{
    DETAILS_KIND, DETAIL_KIND, DIGEST_KIND, MERGE_KIND, PLAN_KIND, TOPICS_KIND,
};
use mediaa_agent::pipeline::{Readiness, Stage};
use mediaa_core::agent::AnalysisOptions;
use mediaa_queue::{self as queue, KindStat, Status as TaskStatus, Task, TaskId};
use rust_i18n::t;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    NoReplies,
    NoModel,
    NeedsIndex,
    NeedsTopics,
    Busy,
    NoSpeakers,
    NoNamesModel,
}

impl Why {
    pub fn text(self) -> String {
        match self {
            Why::NoReplies => t!("analysis.why_replies"),
            Why::NoModel => t!("analysis.why_model"),
            Why::NeedsIndex => t!("analysis.why_index"),
            Why::NeedsTopics => t!("analysis.why_topics"),
            Why::Busy => t!("analysis.why_busy"),
            Why::NoSpeakers => t!("analysis.why_speakers"),
            Why::NoNamesModel => t!("analysis.why_names_model"),
        }
        .into()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offer {
    Redo,
    Add,
    Unavailable(Why),
}

pub fn offer(stage: Stage, ready: &Readiness, diarization_model: bool) -> Offer {
    if stage == Stage::Transcribe {
        return Offer::Redo;
    }
    if !ready.replies {
        return Offer::Unavailable(Why::NoReplies);
    }
    let (have, blocked) = match stage {
        Stage::Diarize => (ready.speakers, (!diarization_model).then_some(Why::NoModel)),
        Stage::Index => (ready.index, None),
        Stage::Topics => (ready.topics, (!ready.index).then_some(Why::NeedsIndex)),
        Stage::Details => (ready.details, (!ready.topics).then_some(Why::NeedsTopics)),
        Stage::Transcribe => unreachable!("answered above"),
    };
    match blocked {
        Some(why) => Offer::Unavailable(why),
        None if have => Offer::Redo,
        None => Offer::Add,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    Analyse,
    Stage(Stage),
    Stop,
    Names,
    StopNames,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Live {
    Queued,
    Running {
        percent: u8,
        count: Option<(usize, usize)>,
    },
    Paused,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamesLive {
    Queued,
    Running { percent: u8 },
    Paused,
}

impl NamesLive {
    pub fn of(task: Option<&Task>) -> Option<NamesLive> {
        let task = task?;
        match task.status {
            TaskStatus::Queued | TaskStatus::Waiting => Some(NamesLive::Queued),
            TaskStatus::Running | TaskStatus::Stopping => Some(NamesLive::Running {
                percent: percent(task.progress),
            }),
            TaskStatus::Paused => Some(NamesLive::Paused),
            TaskStatus::Done | TaskStatus::Failed | TaskStatus::Cancelled => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MenuItem {
    pub command: Command,
    pub offer: Option<Offer>,
    pub live: Option<Live>,
    pub names: Option<NamesLive>,
    pub enabled: bool,
    pub why: Option<Why>,
}

fn live_of(row: &StageRow) -> Option<Live> {
    match &row.state {
        RowState::Waiting => Some(Live::Queued),
        RowState::Running => Some(Live::Running {
            percent: percent(row.progress),
            count: row.count,
        }),
        RowState::Paused => Some(Live::Paused),
        RowState::Failed(_) => Some(Live::Failed),
        RowState::Cancelled => Some(Live::Cancelled),
        RowState::Done | RowState::Off => None,
    }
}

fn percent(progress: f32) -> u8 {
    (progress.clamp(0.0, 1.0) * 100.0).round() as u8
}

pub fn menu(
    ready: &Readiness,
    rows: &[StageRow],
    active: bool,
    diarization_model: bool,
    names: Option<&Task>,
    names_model: bool,
) -> Vec<MenuItem> {
    let mut items = Vec::new();
    if active {
        items.push(MenuItem {
            command: Command::Stop,
            offer: None,
            live: None,
            names: None,
            enabled: true,
            why: None,
        });
    }
    let busy = active.then_some(Why::Busy);
    items.push(MenuItem {
        command: Command::Analyse,
        offer: None,
        live: None,
        names: None,
        enabled: !active,
        why: busy,
    });
    for stage in Stage::ALL {
        if let Some(live) = rows.iter().find(|row| row.stage == stage).and_then(live_of) {
            items.push(MenuItem {
                command: Command::Stage(stage),
                offer: None,
                live: Some(live),
                names: None,
                enabled: false,
                why: None,
            });
            continue;
        }
        let offer = offer(stage, ready, diarization_model);
        let why = busy.or(match offer {
            Offer::Unavailable(why) => Some(why),
            _ => None,
        });
        items.push(MenuItem {
            command: Command::Stage(stage),
            offer: Some(offer),
            live: None,
            names: None,
            enabled: why.is_none(),
            why,
        });
    }
    match NamesLive::of(names) {
        Some(live) => {
            items.push(MenuItem {
                command: Command::Names,
                offer: None,
                live: None,
                names: Some(live),
                enabled: false,
                why: None,
            });
            items.push(MenuItem {
                command: Command::StopNames,
                offer: None,
                live: None,
                names: None,
                enabled: true,
                why: None,
            });
        }
        None => {
            let why = if !ready.replies {
                Some(Why::NoReplies)
            } else if !ready.speakers {
                Some(Why::NoSpeakers)
            } else if !names_model {
                Some(Why::NoNamesModel)
            } else {
                busy
            };
            items.push(MenuItem {
                command: Command::Names,
                offer: None,
                live: None,
                names: None,
                enabled: why.is_none(),
                why,
            });
        }
    }
    items
}

pub fn descendants(tasks: &[Task], root: TaskId) -> Vec<Task> {
    let mut keep = vec![root];
    loop {
        let n = keep.len();
        for task in tasks {
            if task.parent.is_some_and(|parent| keep.contains(&parent)) && !keep.contains(&task.id)
            {
                keep.push(task.id);
            }
        }
        if keep.len() == n {
            break;
        }
    }
    tasks
        .iter()
        .filter(|task| keep.contains(&task.id))
        .cloned()
        .collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub options: AnalysisOptions,
    pub stages: Vec<Stage>,
}

pub fn plan(
    from: Stage,
    ready: &Readiness,
    defaults: &AnalysisOptions,
    diarization_model: bool,
    speakers: Option<u32>,
) -> Plan {
    let on = |stage: Stage, have: bool, default: bool| stage <= from || have || default;
    let options = AnalysisOptions {
        diarize: diarization_model && on(Stage::Diarize, ready.speakers, defaults.diarize),
        speakers,
        index: on(Stage::Index, ready.index, defaults.index),
        topics: on(Stage::Topics, ready.topics, defaults.topics),
        details: on(Stage::Details, ready.details, defaults.details),
    }
    .enforced();
    let chosen = |stage: Stage| match stage {
        Stage::Transcribe => true,
        Stage::Diarize => options.diarize,
        Stage::Index => options.index,
        Stage::Topics => options.topics,
        Stage::Details => options.details,
    };
    let stages = Stage::ALL
        .into_iter()
        .filter(|s| *s >= from && (*s == from || chosen(*s)))
        .collect();
    Plan { options, stages }
}

pub fn confirm_text(plan: &Plan, add: bool) -> String {
    let stages = plan
        .stages
        .iter()
        .map(|s| stage_name(*s).to_lowercase())
        .collect::<Vec<_>>()
        .join(", ");
    if add {
        t!("analysis.confirm_add", stages = stages).into()
    } else {
        t!("analysis.confirm_redo", stages = stages).into()
    }
}

pub fn stage_name(stage: Stage) -> String {
    match stage {
        Stage::Transcribe => t!("analysis.stage_transcribe"),
        Stage::Diarize => t!("analysis.stage_diarize"),
        Stage::Index => t!("analysis.stage_index"),
        Stage::Topics => t!("analysis.stage_topics"),
        Stage::Details => t!("analysis.stage_details"),
    }
    .into()
}

pub fn parse_speakers(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|n| *n >= 1)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub diarize: bool,
    pub speakers: String,
    pub index: bool,
    pub topics: bool,
    pub details: bool,
    model: bool,
}

impl Draft {
    pub fn new(options: &AnalysisOptions, diarization_model: bool) -> Draft {
        let mut draft = Draft {
            diarize: options.diarize,
            speakers: options.speakers.map(|n| n.to_string()).unwrap_or_default(),
            index: options.index,
            topics: options.topics,
            details: options.details,
            model: diarization_model,
        };
        draft.normalize();
        draft
    }

    pub fn can_diarize(&self) -> bool {
        self.model
    }

    pub fn can_topics(&self) -> bool {
        self.index
    }

    pub fn can_details(&self) -> bool {
        self.index && self.topics
    }

    pub fn normalize(&mut self) {
        if !self.index {
            self.topics = false;
        }
        if !self.topics {
            self.details = false;
        }
    }

    pub fn options(&self) -> AnalysisOptions {
        AnalysisOptions {
            diarize: self.diarize,
            speakers: self
                .diarize
                .then(|| parse_speakers(&self.speakers))
                .flatten(),
            index: self.index,
            topics: self.topics,
            details: self.details,
        }
        .enforced()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pane {
    Start,
    Progress,
    NoSpeech,
    Replies,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Availability {
    pub pane: Pane,
    pub chat: bool,
    pub search: bool,
    pub topics: bool,
    pub compact: bool,
}

pub fn availability(ready: &Readiness, analysis: Option<TaskStatus>) -> Availability {
    let pane = match (ready.replies, analysis) {
        (true, _) => Pane::Replies,
        (false, None) => Pane::Start,
        (false, Some(TaskStatus::Done)) => Pane::NoSpeech,
        (false, Some(_)) => Pane::Progress,
    };
    let shown = analysis.is_some_and(|s| !s.is_finished() || s == TaskStatus::Failed);
    Availability {
        pane,
        chat: ready.replies,
        search: ready.replies && ready.index,
        topics: ready.replies && ready.topics,
        compact: ready.replies && shown,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Left {
    Unknown,
    Under,
    Minutes(u64),
}

pub fn left(estimate: Option<Duration>) -> Left {
    match estimate {
        None => Left::Unknown,
        Some(d) if d < Duration::from_secs(60) => Left::Under,
        Some(d) => Left::Minutes((d.as_secs_f64() / 60.0).round() as u64),
    }
}

pub fn left_text(left: Left) -> String {
    match left {
        Left::Unknown => t!("analysis.left_unknown").into(),
        Left::Under => t!("analysis.left_under").into(),
        Left::Minutes(n) => t!("analysis.left_minutes", n = n).into(),
    }
}

pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    let (h, m, s) = (s / 3600, (s / 60) % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowState {
    Done,
    Running,
    Waiting,
    Paused,
    Cancelled,
    Off,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct StageRow {
    pub stage: Stage,
    pub state: RowState,
    pub progress: f32,
    pub elapsed: Option<Duration>,
    pub left: Option<Duration>,
    pub count: Option<(usize, usize)>,
    pub task: Option<TaskId>,
}

fn stage_ready(stage: Stage, ready: &Readiness) -> bool {
    match stage {
        Stage::Transcribe => ready.replies,
        Stage::Diarize => ready.speakers,
        Stage::Index => ready.index,
        Stage::Topics => ready.topics,
        Stage::Details => ready.details,
    }
}

pub fn rows(
    analysis: &Task,
    tree: &[Task],
    stats: &[KindStat],
    ready: &Readiness,
    now_ms: i64,
) -> Vec<StageRow> {
    let find = |kind: &str| tree.iter().filter(|t| t.kind == kind).max_by_key(|t| t.id);
    let counted = |kind: &str| {
        let all: Vec<&Task> = tree.iter().filter(|t| t.kind == kind).collect();
        let done = all.iter().filter(|t| t.status == TaskStatus::Done).count();
        (!all.is_empty()).then_some((done, all.len()))
    };
    let stopped = matches!(analysis.status, TaskStatus::Failed | TaskStatus::Cancelled);
    Stage::ALL
        .into_iter()
        .map(|stage| {
            let kind = match stage {
                Stage::Details => DETAILS_KIND,
                _ => stage.kind().expect("every stage has a task kind"),
            };
            let count = match stage {
                Stage::Topics => counted(DIGEST_KIND),
                Stage::Details => counted(DETAIL_KIND),
                _ => None,
            };
            let Some(task) = find(kind) else {
                let expected = stage == Stage::Details
                    && find(stage_kind(Stage::Topics)).is_some_and(|t| {
                        !t.status.is_finished() && t.input["options"]["details"] == true
                    });
                let state = if expected {
                    RowState::Waiting
                } else if stage_ready(stage, ready) {
                    RowState::Done
                } else {
                    RowState::Off
                };
                let progress = if state == RowState::Done { 1.0 } else { 0.0 };
                return StageRow {
                    stage,
                    state,
                    progress,
                    elapsed: None,
                    left: None,
                    count,
                    task: None,
                };
            };
            let mut state = match task.status {
                TaskStatus::Queued | TaskStatus::Waiting => RowState::Waiting,
                TaskStatus::Running | TaskStatus::Stopping => RowState::Running,
                TaskStatus::Paused => RowState::Paused,
                TaskStatus::Done => RowState::Done,
                TaskStatus::Failed => RowState::Failed(task.error.clone().unwrap_or_default()),
                TaskStatus::Cancelled => RowState::Cancelled,
            };
            if state == RowState::Waiting && stopped {
                state = RowState::Off;
            }
            let unfinished = matches!(
                state,
                RowState::Running | RowState::Waiting | RowState::Paused
            );
            StageRow {
                stage,
                progress: if state == RowState::Done {
                    1.0
                } else {
                    task.progress.clamp(0.0, 1.0)
                },
                elapsed: task.started_ms.map(|_| queue::elapsed(task, now_ms)),
                left: if unfinished {
                    queue::estimate_tree(task, tree, now_ms, stats)
                } else {
                    None
                },
                count,
                task: Some(task.id),
                state,
            }
        })
        .collect()
}

fn stage_kind(stage: Stage) -> &'static str {
    stage.kind().expect("every stage has a task kind")
}

pub fn current(rows: &[StageRow]) -> Option<&StageRow> {
    rows.iter()
        .find(|r| r.state == RowState::Running)
        .or_else(|| rows.iter().find(|r| r.state == RowState::Waiting))
}

pub fn total_title(status: TaskStatus, total: Left) -> String {
    match status {
        TaskStatus::Failed => t!("analysis.total_failed"),
        TaskStatus::Cancelled => t!("analysis.total_cancelled"),
        TaskStatus::Paused => t!("analysis.total_paused"),
        TaskStatus::Done => t!("analysis.total_done"),
        TaskStatus::Queued | TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Stopping => {
            t!("analysis.total", left = left_text(total))
        }
    }
    .into()
}

pub fn compact_title(status: TaskStatus, rows: &[StageRow], total: Left) -> String {
    if let Some(row) = rows.iter().find(|r| r.state == RowState::Running) {
        return running_title(row, total);
    }
    if status == TaskStatus::Failed {
        return failed_compact(rows);
    }
    if status == TaskStatus::Paused {
        return total_title(status, total);
    }
    let Some(row) = current(rows) else {
        return total_title(status, total);
    };
    running_title(row, total)
}

fn running_title(row: &StageRow, total: Left) -> String {
    let stage = stage_name(row.stage).to_lowercase();
    match row.count {
        Some((done, all)) => t!(
            "analysis.compact_count",
            stage = stage,
            done = done,
            total = all,
            left = left_text(total)
        )
        .into(),
        None => t!("analysis.compact", stage = stage, left = left_text(total)).into(),
    }
}

fn failed_compact(rows: &[StageRow]) -> String {
    let Some(row) = rows.iter().find(|r| matches!(r.state, RowState::Failed(_))) else {
        return t!("analysis.total_failed").into();
    };
    let reason = match &row.state {
        RowState::Failed(error) => short_reason(error),
        _ => String::new(),
    };
    let stage = stage_name(row.stage).to_lowercase();
    let text = match (stage.is_empty(), reason.is_empty()) {
        (true, true) => return t!("analysis.total_failed").into(),
        (false, true) => stage,
        (true, false) => reason,
        (false, false) => format!("{stage}: {reason}"),
    };
    t!("analysis.compact_failed", stage = text).into()
}

pub fn short_reason(error: &str) -> String {
    let line = error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    const MAX: usize = 80;
    if line.chars().count() <= MAX {
        return line.to_string();
    }
    let short: String = line.chars().take(MAX).collect();
    format!("{short}…")
}

pub fn shows_spinner(status: TaskStatus, rows: &[StageRow]) -> bool {
    rows.iter().any(|r| r.state == RowState::Running)
        || matches!(
            status,
            TaskStatus::Running | TaskStatus::Stopping | TaskStatus::Queued | TaskStatus::Waiting
        )
}

pub fn topics_running(tree: &[Task]) -> bool {
    tree.iter().any(|task| {
        matches!(task.status, TaskStatus::Running | TaskStatus::Stopping)
            && matches!(
                task.kind.as_str(),
                TOPICS_KIND | PLAN_KIND | DIGEST_KIND | MERGE_KIND | DETAILS_KIND | DETAIL_KIND
            )
    })
}

pub fn row_time_text(row: &StageRow) -> String {
    let left_part = row.left.map(|d| left_text(left(Some(d))));
    match &row.state {
        RowState::Done => row.elapsed.map(clock).unwrap_or_default(),
        RowState::Running => {
            let elapsed = t!(
                "analysis.elapsed",
                time = clock(row.elapsed.unwrap_or_default())
            );
            match left_part {
                Some(left) => format!("{elapsed} · {left}"),
                None => elapsed.into(),
            }
        }
        RowState::Waiting => t!("analysis.state_waiting").into(),
        RowState::Paused => t!("analysis.state_paused").into(),
        RowState::Cancelled => t!("analysis.state_cancelled").into(),
        RowState::Off => t!("analysis.state_off").into(),
        RowState::Failed(_) => String::new(),
    }
}

pub fn row_name(row: &StageRow) -> String {
    let name = stage_name(row.stage);
    match (row.count, &row.state) {
        (
            Some((done, total)),
            RowState::Running | RowState::Waiting | RowState::Paused | RowState::Failed(_),
        ) => {
            format!("{name} · {}", t!("analysis.of", done = done, total = total))
        }
        _ => name,
    }
}

const TIME_W: f32 = 190.0;
const BAR_W: f32 = 64.0;
const COL_GAP: f32 = 8.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowGeom {
    pub icon: egui::Pos2,
    pub name: Rect,
    pub bar: Rect,
    pub time: Rect,
}

pub fn row_geom(rect: Rect) -> RowGeom {
    let h = topics::header(rect);
    let left = h.text.left();
    let time = Rect::from_min_max(
        egui::pos2((h.caret.right() - TIME_W).max(left), rect.top()),
        egui::pos2(h.caret.right().max(left), rect.bottom()),
    );
    let bar_right = (time.left() - COL_GAP).max(left);
    let bar = Rect::from_min_max(
        egui::pos2((bar_right - BAR_W).max(left), rect.center().y - 3.0),
        egui::pos2(bar_right, rect.center().y + 3.0),
    );
    let name = Rect::from_min_max(
        egui::pos2(left, rect.top()),
        egui::pos2((bar.left() - COL_GAP).max(left), rect.bottom()),
    );
    RowGeom {
        icon: h.icon,
        name,
        bar,
        time,
    }
}

fn row_icon(state: &RowState, p: Palette) -> (&'static str, Color32) {
    match state {
        RowState::Done => (icon::CHECK_CIRCLE, p.success),
        RowState::Running => (icon::CIRCLE_NOTCH, p.accent),
        RowState::Waiting => (icon::CLOCK, p.text_weak),
        RowState::Paused => (icon::PAUSE_CIRCLE, p.text_weak),
        RowState::Cancelled => (icon::PROHIBIT, p.text_weak),
        RowState::Off => (icon::MINUS_CIRCLE, p.text_weak),
        RowState::Failed(_) => (icon::WARNING_CIRCLE, p.danger),
    }
}

fn elided(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font: FontId,
    color: Color32,
) -> std::sync::Arc<egui::Galley> {
    egui::WidgetText::from(RichText::new(text).font(font).color(color)).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        width.max(0.0),
        egui::TextStyle::Body,
    )
}

fn spinner_at(ui: &egui::Ui, at: egui::Pos2, color: Color32) {
    egui::Spinner::new().size(14.0).color(color).paint_at(
        ui,
        Rect::from_center_size(at + egui::vec2(7.0, 0.0), egui::vec2(14.0, 14.0)),
    );
}

pub struct CardView<'a> {
    pub title: &'a str,
    pub rows: &'a [StageRow],
    pub running: bool,
    pub failed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageHit {
    Retry(TaskId),
    Details(TaskId),
}

pub fn card(
    ui: &mut egui::Ui,
    view: &CardView,
    fold: Option<&mut bool>,
    p: Palette,
) -> Option<StageHit> {
    let mut retry = None;
    let mut fold = fold;
    let open = fold.as_ref().is_none_or(|open| **open);
    let card = egui::Frame::new()
        .fill(p.card)
        .corner_radius(CornerRadius::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let sense = if fold.is_some() {
                Sense::click()
            } else {
                Sense::hover()
            };
            let (head, resp) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), topics::HEADER_H), sense);
            if let Some(open) = fold.as_deref_mut() {
                let resp = resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                if resp.hovered() {
                    ui.painter()
                        .rect_filled(head, CornerRadius::same(8), p.card_hover);
                }
                if resp.clicked() {
                    *open = !*open;
                }
            }
            let h = topics::header(head);
            let painter = ui.painter_at(head);
            if view.running {
                spinner_at(ui, h.icon, p.accent);
            } else {
                let (glyph, color) = if view.failed {
                    (icon::WARNING_CIRCLE, p.danger)
                } else {
                    (icon::CHART_LINE, p.accent)
                };
                painter.text(
                    h.icon,
                    Align2::LEFT_CENTER,
                    glyph,
                    FontId::proportional(14.0),
                    color,
                );
            }
            let title_rect =
                Rect::from_min_max(h.text.min, egui::pos2(h.caret.left() - 4.0, head.bottom()));
            let galley = elided(
                ui,
                view.title,
                title_rect.width(),
                FontId::proportional(13.0),
                p.text,
            );
            painter.galley(
                egui::pos2(title_rect.left(), head.center().y - galley.size().y / 2.0),
                galley,
                p.text,
            );
            if fold.is_some() {
                painter.text(
                    h.caret.center(),
                    Align2::CENTER_CENTER,
                    if open {
                        icon::CARET_UP
                    } else {
                        icon::CARET_DOWN
                    },
                    FontId::proportional(12.0),
                    p.text_weak,
                );
            }
            if open {
                ui.add_space(2.0);
                for row in view.rows {
                    if let Some(id) = row_ui(ui, row, p) {
                        retry = Some(id);
                    }
                }
                ui.add_space(4.0);
            }
        })
        .response
        .rect;
    ui.painter().rect(
        card,
        CornerRadius::same(8),
        Color32::TRANSPARENT,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    retry
}

fn row_ui(ui: &mut egui::Ui, row: &StageRow, p: Palette) -> Option<StageHit> {
    let (rect, row_resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), topics::HEADER_H),
        Sense::click(),
    );
    let g = row_geom(rect);
    let painter = ui.painter_at(rect);
    let (glyph, color) = row_icon(&row.state, p);
    if row.state == RowState::Running {
        spinner_at(ui, g.icon, color);
    } else {
        painter.text(
            g.icon,
            Align2::LEFT_CENTER,
            glyph,
            FontId::proportional(14.0),
            color,
        );
    }
    let weak = matches!(
        row.state,
        RowState::Waiting | RowState::Off | RowState::Cancelled | RowState::Paused
    );
    let name_color = if weak { p.text_weak } else { p.text };
    let mut retry = None;
    match &row.state {
        RowState::Failed(error) => {
            let text = if error.trim().is_empty() {
                row_name(row)
            } else {
                format!("{} — {}", row_name(row), error.trim())
            };
            let width = g.bar.right() - g.name.left();
            let galley = elided(ui, &text, width, FontId::proportional(13.0), p.danger);
            let area = Rect::from_min_max(g.name.min, egui::pos2(g.bar.right(), rect.bottom()));
            ui.painter_at(area).galley(
                egui::pos2(area.left(), rect.center().y - galley.size().y / 2.0),
                galley,
                p.danger,
            );
            if !error.trim().is_empty() {
                topics::text_tip(
                    &ui.interact(
                        area,
                        ui.id().with(("analysis-error", row.stage as usize)),
                        Sense::hover(),
                    ),
                    error.trim(),
                    &super::mentions::Roster::default(),
                );
            }
            let button = ui
                .scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(g.time)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.add(
                            egui::Button::new(
                                RichText::new(format!("{}  {}", icon::PLAY, t!("analysis.retry")))
                                    .size(12.0),
                            )
                            .corner_radius(CornerRadius::same(6)),
                        )
                    },
                )
                .inner;
            if button.clicked() {
                retry = row.task.map(StageHit::Retry);
            } else if row_resp.clicked() {
                retry = row.task.map(StageHit::Details);
            }
            if row.task.is_some() {
                let _ = row_resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
        }
        state => {
            let galley = elided(
                ui,
                &row_name(row),
                g.name.width(),
                FontId::proportional(13.0),
                name_color,
            );
            painter.galley(
                egui::pos2(g.name.left(), rect.center().y - galley.size().y / 2.0),
                galley,
                name_color,
            );
            if matches!(state, RowState::Running | RowState::Paused) && g.bar.width() > 0.0 {
                painter.rect_filled(g.bar, CornerRadius::same(3), p.border);
                let fill = Rect::from_min_size(
                    g.bar.min,
                    egui::vec2(g.bar.width() * row.progress.clamp(0.0, 1.0), g.bar.height()),
                );
                painter.rect_filled(fill, CornerRadius::same(3), p.accent);
            }
            let text = row_time_text(row);
            let galley = elided(
                ui,
                &text,
                g.time.width(),
                FontId::proportional(11.5),
                p.text_weak,
            );
            let clip = ui.painter_at(g.time);
            clip.galley(
                egui::pos2(
                    g.time.right() - galley.size().x,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                p.text_weak,
            );
        }
    }
    retry
}

fn entry(
    ui: &mut egui::Ui,
    glyph: &str,
    label: &str,
    enabled: bool,
    why: Option<Why>,
    p: Palette,
) -> bool {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), Sense::click());
    let resp = resp.on_hover_cursor(if enabled {
        egui::CursorIcon::PointingHand
    } else {
        egui::CursorIcon::Default
    });
    let color = if enabled {
        p.text
    } else {
        p.text_weak.gamma_multiply(0.7)
    };
    if enabled && resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), p.accent_soft);
    }
    ui.painter().text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        glyph,
        FontId::proportional(15.0),
        color,
    );
    ui.painter().text(
        rect.left_center() + egui::vec2(36.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        color,
    );
    if let (false, Some(why)) = (enabled, why) {
        topics::text_tip(&resp, &why.text(), &super::mentions::Roster::default());
    }
    enabled && resp.clicked()
}

pub fn menu_label(stage: Stage, offer: Option<Offer>) -> String {
    let name = stage_name(stage).to_lowercase();
    let mut label = match offer {
        Some(Offer::Add) => t!("analysis.menu_add", stage = name).to_string(),
        _ => t!("analysis.menu_redo", stage = name).to_string(),
    };
    if stage == Stage::Diarize {
        label.push('…');
    }
    label
}

pub fn live_label(stage: Stage, live: Live) -> String {
    let name = stage_name(stage).to_lowercase();
    match live {
        Live::Queued => t!("analysis.menu_queued", stage = name).into(),
        Live::Running {
            count: Some((done, total)),
            ..
        } => t!(
            "analysis.menu_running_count",
            stage = name,
            done = done,
            total = total
        )
        .into(),
        Live::Running { percent, .. } => t!(
            "analysis.menu_running",
            stage = name,
            progress = format!("{percent}%")
        )
        .into(),
        Live::Paused => t!("analysis.menu_paused", stage = name).into(),
        Live::Failed => t!("analysis.menu_failed", stage = name).into(),
        Live::Cancelled => t!("analysis.menu_cancelled", stage = name).into(),
    }
}

pub fn names_label(live: NamesLive) -> String {
    match live {
        NamesLive::Queued => t!("analysis.menu_names_queued").into(),
        NamesLive::Running { percent } => t!(
            "analysis.menu_names_running",
            progress = format!("{percent}%")
        )
        .into(),
        NamesLive::Paused => t!("analysis.menu_names_paused").into(),
    }
}

pub fn menu_popup(button: &egui::Response, items: &[MenuItem], p: Palette) -> Option<Command> {
    egui::Popup::menu(button)
        .show(|ui| {
            ui.set_min_width(250.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut chosen = None;
            for item in items {
                let (glyph, label) = match item.command {
                    Command::Stop => (icon::STOP, t!("analysis.menu_stop").to_string()),
                    Command::StopNames => (icon::STOP, t!("analysis.menu_stop_names").to_string()),
                    Command::Names => match item.names {
                        Some(live) => {
                            let glyph = match live {
                                NamesLive::Queued => icon::CLOCK,
                                NamesLive::Running { .. } => icon::CIRCLE_NOTCH,
                                NamesLive::Paused => icon::PAUSE_CIRCLE,
                            };
                            (glyph, names_label(live))
                        }
                        None => (icon::USER, t!("analysis.menu_names").to_string()),
                    },
                    Command::Analyse => (
                        icon::ARROWS_CLOCKWISE,
                        t!("analysis.menu_analyse").to_string(),
                    ),
                    Command::Stage(stage) => {
                        let glyph = match item.live {
                            Some(Live::Queued) => icon::CLOCK,
                            Some(Live::Running { .. }) => icon::CIRCLE_NOTCH,
                            Some(Live::Paused) => icon::PAUSE_CIRCLE,
                            Some(Live::Failed) => icon::WARNING_CIRCLE,
                            Some(Live::Cancelled) => icon::PROHIBIT,
                            None if item.offer == Some(Offer::Add) => icon::PLUS,
                            None => icon::ARROW_COUNTER_CLOCKWISE,
                        };
                        let label = match item.live {
                            Some(live) => live_label(stage, live),
                            None => menu_label(stage, item.offer),
                        };
                        (glyph, label)
                    }
                };
                if entry(ui, glyph, &label, item.enabled, item.why, p) {
                    chosen = Some(item.command);
                }
            }
            chosen
        })
        .and_then(|r| r.inner)
}

pub enum Outcome<T> {
    Open,
    Cancel,
    Run(T),
}

pub struct OptionsWindow {
    pub restart: bool,
    pub draft: Draft,
}

impl OptionsWindow {
    pub fn new(restart: bool, options: &AnalysisOptions, diarization_model: bool) -> OptionsWindow {
        OptionsWindow {
            restart,
            draft: Draft::new(options, diarization_model),
        }
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        p: Palette,
    ) -> Outcome<AnalysisOptions> {
        let mut open = true;
        let mut outcome = Outcome::Open;
        let draft = &mut self.draft;
        let restart = self.restart;
        theme::with_compact_title(ctx, || {
            theme::modal_backdrop(ctx);
            let response = egui::Window::new(t!("analysis.options_title"))
                .id(id)
                .order(egui::Order::Foreground)
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(340.0);
                    if restart {
                        ui.add(
                            egui::Label::new(
                                RichText::new(t!("analysis.options_restart_warn"))
                                    .size(12.0)
                                    .color(p.danger),
                            )
                            .wrap(),
                        );
                        ui.add_space(6.0);
                    }
                    ui.add_enabled(
                        false,
                        egui::Checkbox::new(&mut true, t!("analysis.opt_recognition")),
                    );
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(draft.can_diarize(), |ui| {
                            ui.checkbox(&mut draft.diarize, t!("analysis.opt_diarize"));
                        });
                        ui.add_enabled(
                            draft.can_diarize() && draft.diarize,
                            egui::TextEdit::singleline(&mut draft.speakers)
                                .desired_width(48.0)
                                .hint_text(t!("analysis.opt_speakers_hint")),
                        );
                        ui.label(
                            RichText::new(t!("analysis.opt_speakers"))
                                .size(12.0)
                                .color(p.text_weak),
                        );
                    });
                    if !draft.can_diarize() {
                        theme::hint(ui, t!("analysis.why_model"));
                    }
                    ui.checkbox(&mut draft.index, t!("analysis.opt_index"));
                    ui.add_enabled_ui(draft.can_topics(), |ui| {
                        ui.checkbox(&mut draft.topics, t!("analysis.opt_topics"));
                    });
                    ui.add_enabled_ui(draft.can_details(), |ui| {
                        ui.checkbox(&mut draft.details, t!("analysis.opt_details"));
                    });
                    draft.normalize();
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(theme::primary(ui, icon::PLAY, &t!("analysis.run")))
                            .clicked()
                        {
                            outcome = Outcome::Run(draft.options());
                        }
                        if ui
                            .add(theme::secondary(icon::X, &t!("common.cancel")))
                            .clicked()
                        {
                            outcome = Outcome::Cancel;
                        }
                    });
                });
            theme::mark_modal(ctx, &response);
        });
        if !open {
            return Outcome::Cancel;
        }
        outcome
    }
}

pub struct ConfirmWindow {
    pub stage: Stage,
    pub add: bool,
    pub speakers: String,
}

impl ConfirmWindow {
    pub fn new(stage: Stage, add: bool, defaults: &AnalysisOptions) -> ConfirmWindow {
        ConfirmWindow {
            stage,
            add,
            speakers: defaults.speakers.map(|n| n.to_string()).unwrap_or_default(),
        }
    }

    pub fn plan(
        &self,
        ready: &Readiness,
        defaults: &AnalysisOptions,
        diarization_model: bool,
    ) -> Plan {
        let speakers = if self.stage == Stage::Diarize {
            parse_speakers(&self.speakers)
        } else {
            defaults.speakers
        };
        plan(self.stage, ready, defaults, diarization_model, speakers)
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        id: egui::Id,
        plan: &Plan,
        p: Palette,
    ) -> Outcome<Plan> {
        let mut open = true;
        let mut outcome = Outcome::Open;
        let (stage, add) = (self.stage, self.add);
        let speakers = &mut self.speakers;
        let title = menu_label(stage, Some(if add { Offer::Add } else { Offer::Redo }))
            .trim_end_matches('…')
            .to_string();
        theme::with_compact_title(ctx, || {
            theme::modal_backdrop(ctx);
            let response = egui::Window::new(&title)
                .id(id)
                .order(egui::Order::Foreground)
                .collapsible(false)
                .resizable(false)
                .open(&mut open)
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.set_width(340.0);
                    ui.add(
                        egui::Label::new(RichText::new(confirm_text(plan, add)).size(13.0)).wrap(),
                    );
                    if stage == Stage::Diarize {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(t!("analysis.opt_speakers"))
                                    .size(12.0)
                                    .color(p.text_weak),
                            );
                            ui.add(
                                egui::TextEdit::singleline(speakers)
                                    .desired_width(48.0)
                                    .hint_text(t!("analysis.opt_speakers_hint"))
                                    .margin(Margin::symmetric(4, 2)),
                            );
                        });
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        let label = if add {
                            t!("analysis.run_add")
                        } else {
                            t!("analysis.run_redo")
                        };
                        if ui.add(theme::primary(ui, icon::PLAY, &label)).clicked() {
                            outcome = Outcome::Run(plan.clone());
                        }
                        if ui
                            .add(theme::secondary(icon::X, &t!("common.cancel")))
                            .clicked()
                        {
                            outcome = Outcome::Cancel;
                        }
                    });
                });
            theme::mark_modal(ctx, &response);
        });
        if !open {
            return Outcome::Cancel;
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_agent::outline::{PLAN_KIND, TOPICS_KIND};
    use mediaa_queue::Role;
    use serde_json::json;

    fn ready(replies: bool, speakers: bool, index: bool, topics: bool, details: bool) -> Readiness {
        Readiness {
            replies,
            speakers,
            index,
            topics,
            details,
        }
    }

    const ALL: Readiness = Readiness {
        replies: true,
        speakers: true,
        index: true,
        topics: true,
        details: true,
    };

    #[test]
    fn stages_are_offered_as_redo_or_add_by_the_data() {
        assert_eq!(
            offer(Stage::Transcribe, &Readiness::default(), false),
            Offer::Redo,
            "the recognition is always offered"
        );
        for stage in [Stage::Diarize, Stage::Index, Stage::Topics, Stage::Details] {
            assert_eq!(
                offer(stage, &Readiness::default(), true),
                Offer::Unavailable(Why::NoReplies),
                "{stage:?} needs replies"
            );
            assert_eq!(offer(stage, &ALL, true), Offer::Redo, "{stage:?} exists");
        }
        let bare = ready(true, false, false, false, false);
        assert_eq!(offer(Stage::Diarize, &bare, true), Offer::Add);
        assert_eq!(offer(Stage::Index, &bare, true), Offer::Add);
        assert_eq!(
            offer(Stage::Topics, &bare, true),
            Offer::Unavailable(Why::NeedsIndex)
        );
        assert_eq!(
            offer(Stage::Details, &bare, true),
            Offer::Unavailable(Why::NeedsTopics)
        );
        let indexed = ready(true, false, true, false, false);
        assert_eq!(offer(Stage::Topics, &indexed, true), Offer::Add);
        let topics = ready(true, false, true, true, false);
        assert_eq!(offer(Stage::Details, &topics, true), Offer::Add);
        assert_eq!(
            offer(Stage::Diarize, &ALL, false),
            Offer::Unavailable(Why::NoModel),
            "no diarization model"
        );
    }

    #[test]
    fn while_an_analysis_runs_only_stop_is_usable() {
        let items = menu(&ALL, &[], true, true, None, true);
        assert_eq!(items[0].command, Command::Stop);
        assert!(items[0].enabled);
        assert!(items[1..].iter().all(|i| !i.enabled), "{items:?}");
        assert!(
            items[1..items.len() - 1]
                .iter()
                .all(|i| i.why == Some(Why::Busy) && i.live.is_none()),
            "{items:?}"
        );
        assert_eq!(items.len(), 1 + 1 + Stage::ALL.len() + 1);
        let idle = menu(&ALL, &[], false, true, None, true);
        assert!(idle.iter().all(|i| i.command != Command::Stop));
        assert!(idle.iter().all(|i| i.enabled && i.live.is_none()));
        assert_eq!(idle[0].command, Command::Analyse);
        let stages: Vec<Command> = idle
            .iter()
            .skip(1)
            .take(Stage::ALL.len())
            .map(|i| i.command)
            .collect();
        assert_eq!(stages, Stage::ALL.map(Command::Stage));
        assert_eq!(idle.last().unwrap().command, Command::Names);
        let bare = menu(
            &ready(true, false, false, false, false),
            &[],
            false,
            true,
            None,
            true,
        );
        let topics = bare
            .iter()
            .find(|i| i.command == Command::Stage(Stage::Topics))
            .unwrap();
        assert!(!topics.enabled);
        assert_eq!(topics.why, Some(Why::NeedsIndex));
        assert_eq!(topics.offer, Some(Offer::Unavailable(Why::NeedsIndex)));
    }

    #[test]
    fn the_names_item_follows_the_run() {
        let bare = menu(
            &ready(true, false, true, true, true),
            &[],
            false,
            true,
            None,
            true,
        );
        let names = bare.last().unwrap();
        assert_eq!(
            (names.command, names.enabled, names.why),
            (Command::Names, false, Some(Why::NoSpeakers))
        );
        let no_model = menu(&ALL, &[], false, true, None, false);
        assert_eq!(
            (
                no_model.last().unwrap().enabled,
                no_model.last().unwrap().why
            ),
            (false, Some(Why::NoNamesModel))
        );
        let idle = menu(&ALL, &[], false, true, None, true);
        assert_eq!(
            (
                idle.last().unwrap().command,
                idle.last().unwrap().enabled,
                idle.last().unwrap().why
            ),
            (Command::Names, true, None)
        );
        let mut running = task(5, "speaker-names", TaskStatus::Running, None);
        running.progress = 0.4;
        let items = menu(&ALL, &[], false, true, Some(&running), true);
        let names = &items[items.len() - 2];
        assert_eq!(
            (names.command, names.enabled, names.names),
            (
                Command::Names,
                false,
                Some(NamesLive::Running { percent: 40 })
            )
        );
        assert!(names_label(NamesLive::Running { percent: 40 }).contains("40"));
        assert_ne!(
            names_label(NamesLive::Queued),
            names_label(NamesLive::Paused)
        );
        let stop = items.last().unwrap();
        assert_eq!((stop.command, stop.enabled), (Command::StopNames, true));
        for (status, live) in [
            (TaskStatus::Queued, NamesLive::Queued),
            (TaskStatus::Paused, NamesLive::Paused),
        ] {
            let t = task(6, "speaker-names", status, None);
            let items = menu(&ALL, &[], false, true, Some(&t), true);
            let names = items.iter().find(|i| i.command == Command::Names).unwrap();
            assert_eq!(names.names, Some(live), "{status:?}");
        }
        for status in [TaskStatus::Done, TaskStatus::Failed, TaskStatus::Cancelled] {
            let t = task(7, "speaker-names", status, None);
            let items = menu(&ALL, &[], false, true, Some(&t), true);
            assert!(
                items.last().unwrap().enabled && items.last().unwrap().names.is_none(),
                "{status:?}"
            );
            assert!(
                items.iter().all(|i| i.command != Command::StopNames),
                "{status:?}"
            );
        }
    }

    fn stage_item(items: &[MenuItem], stage: Stage) -> &MenuItem {
        items
            .iter()
            .find(|item| item.command == Command::Stage(stage))
            .unwrap()
    }

    #[test]
    fn the_menu_reads_the_running_analysis() {
        let analysis = task(1, "analysis", TaskStatus::Running, None);
        let mut index = task(2, "index", TaskStatus::Running, Some(1));
        index.progress = 0.42;
        let mut topics = task(3, TOPICS_KIND, TaskStatus::Waiting, Some(1));
        topics.input = json!({ "options": { "details": true } });
        let flat = vec![
            analysis.clone(),
            index,
            topics,
            task(9, "transcribe", TaskStatus::Queued, Some(8)),
        ];
        let tree = descendants(&flat, 1);
        assert!(
            tree.iter().any(|item| item.id == 2) && tree.iter().all(|item| item.id != 9),
            "only this analysis"
        );
        let rows = super::rows(
            &analysis,
            &tree,
            &[],
            &ready(true, false, false, false, false),
            0,
        );
        let items = menu(
            &ready(true, false, false, false, false),
            &rows,
            true,
            true,
            None,
            true,
        );
        assert!(items[0].command == Command::Stop && items[0].enabled);
        assert!(
            !stage_item(&items, Stage::Transcribe).enabled
                && stage_item(&items, Stage::Transcribe).live.is_none()
        );
        assert_eq!(
            stage_item(&items, Stage::Transcribe).offer,
            Some(Offer::Redo),
            "the replies are already there"
        );
        let index = stage_item(&items, Stage::Index);
        assert_eq!(
            index.live,
            Some(Live::Running {
                percent: 42,
                count: None
            })
        );
        assert!(
            index.offer.is_none() && !index.enabled,
            "running is not «Добавить»"
        );
        assert!(live_label(Stage::Index, index.live.unwrap()).contains("42%"));
        assert_eq!(
            stage_item(&items, Stage::Diarize).offer,
            Some(Offer::Add),
            "left out of this run"
        );
        assert!(stage_item(&items, Stage::Diarize).live.is_none());
        assert_eq!(stage_item(&items, Stage::Topics).live, Some(Live::Queued));
        assert_eq!(
            stage_item(&items, Stage::Details).live,
            Some(Live::Queued),
            "the topics will add the breakdown"
        );
        let queued = live_label(Stage::Topics, Live::Queued);
        let added = menu_label(Stage::Topics, Some(Offer::Add));
        assert_ne!(queued, added);
        assert!(!queued.contains(&added));

        let row = |stage, state| StageRow {
            stage,
            state,
            progress: 0.0,
            elapsed: None,
            left: None,
            count: None,
            task: None,
        };
        let rows = [
            row(Stage::Diarize, RowState::Paused),
            row(Stage::Topics, RowState::Failed("no model".into())),
            row(Stage::Details, RowState::Cancelled),
        ];
        let items = menu(&bare_ready(), &rows, true, true, None, true);
        assert_eq!(stage_item(&items, Stage::Diarize).live, Some(Live::Paused));
        assert_eq!(stage_item(&items, Stage::Topics).live, Some(Live::Failed));
        assert_eq!(
            stage_item(&items, Stage::Details).live,
            Some(Live::Cancelled)
        );
        for stage in [Stage::Diarize, Stage::Topics, Stage::Details] {
            let item = stage_item(&items, stage);
            assert!(
                item.offer.is_none() && !item.enabled && item.why.is_none(),
                "{stage:?} is the run, not «Добавить»"
            );
        }
    }

    fn bare_ready() -> Readiness {
        ready(true, false, false, false, false)
    }

    #[test]
    fn a_restart_rebuilds_the_stage_and_every_later_enabled_one() {
        let defaults = AnalysisOptions::default();
        let p = plan(Stage::Index, &ALL, &defaults, true, None);
        assert_eq!(p.stages, [Stage::Index, Stage::Topics, Stage::Details]);
        let p = plan(Stage::Transcribe, &ALL, &defaults, true, None);
        assert_eq!(p.stages, Stage::ALL);
        let p = plan(Stage::Details, &ALL, &defaults, true, None);
        assert_eq!(p.stages, [Stage::Details]);
        let p = plan(Stage::Transcribe, &ALL, &defaults, false, None);
        assert_eq!(
            p.stages,
            [
                Stage::Transcribe,
                Stage::Index,
                Stage::Topics,
                Stage::Details
            ]
        );
        let off = AnalysisOptions {
            diarize: false,
            speakers: None,
            index: true,
            topics: false,
            details: false,
        };
        let bare = ready(true, false, true, false, false);
        assert_eq!(
            plan(Stage::Index, &bare, &off, true, None).stages,
            [Stage::Index]
        );
        assert_eq!(
            plan(Stage::Topics, &bare, &defaults, true, None).stages,
            [Stage::Topics, Stage::Details]
        );
        let odd = AnalysisOptions {
            topics: false,
            details: true,
            ..defaults
        };
        assert_eq!(
            plan(Stage::Index, &bare, &odd, true, None).stages,
            [Stage::Index]
        );
        let p = plan(Stage::Diarize, &bare, &defaults, true, Some(3));
        assert_eq!(p.options.speakers, Some(3));
        assert_eq!(p.stages[0], Stage::Diarize);
        let p = plan(
            Stage::Topics,
            &ready(true, false, true, true, true),
            &off,
            true,
            None,
        );
        assert!(p.options.index && p.options.topics);
        assert_eq!(p.stages, [Stage::Topics, Stage::Details]);
    }

    #[test]
    fn the_confirmation_lists_what_is_rebuilt() {
        let p = plan(Stage::Index, &ALL, &AnalysisOptions::default(), true, None);
        let text = confirm_text(&p, false);
        for stage in [Stage::Index, Stage::Topics, Stage::Details] {
            assert!(text.contains(&stage_name(stage).to_lowercase()), "{text}");
        }
        assert!(
            !text.contains(&stage_name(Stage::Transcribe).to_lowercase()),
            "{text}"
        );
        assert_ne!(confirm_text(&p, true), text, "adding is worded differently");
    }

    #[test]
    fn the_options_keep_their_dependencies() {
        let mut draft = Draft::new(&AnalysisOptions::default(), true);
        assert!(!draft.diarize && draft.index && draft.topics && draft.details);
        draft.index = false;
        draft.normalize();
        assert!(!draft.topics && !draft.details && !draft.can_topics() && !draft.can_details());
        draft.index = true;
        draft.normalize();
        assert!(
            draft.can_topics() && !draft.can_details(),
            "the topics stay off until chosen again"
        );
        draft.topics = true;
        assert!(draft.can_details());
        let draft = Draft::new(&AnalysisOptions::default(), false);
        assert!(!draft.diarize && !draft.can_diarize());
        assert!(!draft.options().diarize);
        let no_model = mediaa_core::agent::ModelChoices::default();
        assert!(!draft.options().resolved(&no_model).diarize);
        let mut draft = Draft::new(&AnalysisOptions::default(), true);
        draft.diarize = true;
        draft.speakers = " 3 ".into();
        assert_eq!(draft.options().speakers, Some(3));
        draft.speakers = "abc".into();
        assert_eq!(draft.options().speakers, None);
        draft.speakers = "0".into();
        assert_eq!(draft.options().speakers, None);
        draft.speakers = "2".into();
        draft.diarize = false;
        assert_eq!(draft.options().speakers, None);
        let saved = AnalysisOptions {
            diarize: true,
            speakers: Some(4),
            index: true,
            topics: false,
            details: false,
        };
        assert_eq!(Draft::new(&saved, true).options(), saved);
    }

    #[test]
    fn availability_follows_the_data() {
        let none = availability(&Readiness::default(), None);
        assert_eq!(none.pane, Pane::Start);
        assert!(!none.chat && !none.search && !none.topics && !none.compact);
        for status in [
            TaskStatus::Queued,
            TaskStatus::Running,
            TaskStatus::Waiting,
            TaskStatus::Paused,
            TaskStatus::Failed,
            TaskStatus::Cancelled,
        ] {
            assert_eq!(
                availability(&Readiness::default(), Some(status)).pane,
                Pane::Progress,
                "{status:?}"
            );
        }
        assert_eq!(
            availability(&Readiness::default(), Some(TaskStatus::Done)).pane,
            Pane::NoSpeech
        );
        let replies = ready(true, false, false, false, false);
        let running = availability(&replies, Some(TaskStatus::Running));
        assert_eq!(running.pane, Pane::Replies);
        assert!(running.chat && running.compact && !running.search && !running.topics);
        assert!(
            availability(&replies, Some(TaskStatus::Failed)).compact,
            "a failure stays visible"
        );
        assert!(!availability(&replies, Some(TaskStatus::Done)).compact);
        assert!(!availability(&replies, Some(TaskStatus::Cancelled)).compact);
        assert!(!availability(&replies, None).compact);
        let indexed = availability(&ready(true, false, true, false, false), None);
        assert!(indexed.search && !indexed.topics);
        let topics = availability(&ready(true, false, true, true, false), None);
        assert!(topics.topics);
        assert!(
            !availability(
                &ready(false, false, false, true, true),
                Some(TaskStatus::Running)
            )
            .topics
        );
    }

    #[test]
    fn time_is_said_in_minutes() {
        assert_eq!(left(None), Left::Unknown);
        assert_eq!(left(Some(Duration::ZERO)), Left::Under);
        assert_eq!(left(Some(Duration::from_secs(59))), Left::Under);
        assert_eq!(left(Some(Duration::from_secs(60))), Left::Minutes(1));
        assert_eq!(left(Some(Duration::from_secs(89))), Left::Minutes(1));
        assert_eq!(left(Some(Duration::from_secs(90))), Left::Minutes(2));
        assert_eq!(
            left(Some(Duration::from_secs(4 * 60 + 10))),
            Left::Minutes(4)
        );
        let texts = [
            left_text(Left::Unknown),
            left_text(Left::Under),
            left_text(Left::Minutes(4)),
        ];
        assert!(texts[2].contains('4'));
        assert!(texts[0] != texts[1] && texts[1] != texts[2]);
        assert_eq!(clock(Duration::from_secs(0)), "0:00");
        assert_eq!(clock(Duration::from_secs(83)), "1:23");
        assert_eq!(clock(Duration::from_secs(12 * 60 + 5)), "12:05");
        assert_eq!(clock(Duration::from_secs(3723)), "1:02:03");
    }

    fn task(id: i64, kind: &str, status: TaskStatus, parent: Option<i64>) -> Task {
        Task {
            id,
            kind: kind.into(),
            title: kind.into(),
            input: serde_json::Value::Null,
            priority: 0,
            status,
            cursor: serde_json::Value::Null,
            partial: serde_json::Value::Null,
            result: None,
            progress: 0.0,
            error: None,
            note: None,
            attempts: 0,
            created_ms: 0,
            updated_ms: 0,
            parent,
            scope: Some("f".into()),
            inline: false,
            started_ms: None,
            finished_ms: None,
            stats: Default::default(),
            detail: serde_json::Value::Null,
            units: 0.0,
            resource: None,
            memory: 0,
            model: None,
            role: Role::Work,
            gen: 0,
            yield_cont: false,
            heartbeat_ms: None,
            stop_intent: None,
        }
    }

    fn state_of(rows: &[StageRow], stage: Stage) -> RowState {
        rows.iter()
            .find(|r| r.stage == stage)
            .unwrap()
            .state
            .clone()
    }

    #[test]
    fn rows_follow_the_tasks_of_the_analysis() {
        let analysis = task(1, "analysis", TaskStatus::Running, None);
        let mut transcribe = task(2, "transcribe", TaskStatus::Running, Some(1));
        transcribe.progress = 0.4;
        transcribe.started_ms = Some(1_000);
        transcribe.units = 10.0;
        let index = task(3, "index", TaskStatus::Queued, Some(1));
        let mut topics = task(4, TOPICS_KIND, TaskStatus::Waiting, Some(1));
        topics.input = json!({ "options": { "details": true } });
        let tree = vec![analysis.clone(), transcribe, index, topics];
        let stats = [KindStat {
            kind: "transcribe".into(),
            runs: 1,
            ms: 60_000,
            units: 10.0,
        }];
        let rows = super::rows(&analysis, &tree, &stats, &Readiness::default(), 31_000);
        assert_eq!(rows.iter().map(|r| r.stage).collect::<Vec<_>>(), Stage::ALL);
        let first = &rows[0];
        assert_eq!(first.state, RowState::Running);
        assert_eq!(first.elapsed, Some(Duration::from_secs(30)));
        let left_s = first.left.unwrap().as_secs_f64();
        assert!((left_s - 45.0).abs() < 0.01, "30 s at 40 %: {left_s}");
        assert_eq!(
            state_of(&rows, Stage::Diarize),
            RowState::Off,
            "not part of this analysis"
        );
        assert_eq!(state_of(&rows, Stage::Index), RowState::Waiting);
        assert_eq!(state_of(&rows, Stage::Topics), RowState::Waiting);
        assert_eq!(
            state_of(&rows, Stage::Details),
            RowState::Waiting,
            "the merge will add them"
        );
        assert_eq!(current(&rows).unwrap().stage, Stage::Transcribe);
    }

    #[test]
    fn rows_of_earlier_data_failures_and_counts() {
        let analysis = task(1, "analysis", TaskStatus::Running, None);
        let index = task(2, "index", TaskStatus::Done, Some(1));
        let topics = task(3, TOPICS_KIND, TaskStatus::Running, Some(1));
        let plan_task = task(4, PLAN_KIND, TaskStatus::Done, Some(3));
        let tree = vec![
            analysis.clone(),
            index,
            topics,
            plan_task,
            task(5, DIGEST_KIND, TaskStatus::Done, Some(3)),
            task(6, DIGEST_KIND, TaskStatus::Done, Some(3)),
            task(7, DIGEST_KIND, TaskStatus::Running, Some(3)),
        ];
        let have = ready(true, true, true, false, false);
        let rows = super::rows(&analysis, &tree, &[], &have, 0);
        assert_eq!(state_of(&rows, Stage::Transcribe), RowState::Done);
        assert_eq!(state_of(&rows, Stage::Diarize), RowState::Done);
        assert_eq!(state_of(&rows, Stage::Index), RowState::Done);
        assert_eq!(state_of(&rows, Stage::Details), RowState::Off);
        let topics = rows.iter().find(|r| r.stage == Stage::Topics).unwrap();
        assert_eq!(
            (topics.state.clone(), topics.count),
            (RowState::Running, Some((2, 3)))
        );
        assert!(
            row_name(topics).contains("2") && row_name(topics).contains('3'),
            "{}",
            row_name(topics)
        );
        assert_eq!(
            compact_title(TaskStatus::Running, &rows, Left::Minutes(4))
                .matches('4')
                .count(),
            1
        );
        assert!(compact_title(TaskStatus::Running, &rows, Left::Minutes(4))
            .contains(&stage_name(Stage::Topics).to_lowercase()));

        let failed_analysis = task(1, "analysis", TaskStatus::Failed, None);
        let mut index = task(2, "index", TaskStatus::Failed, Some(1));
        index.error = Some("нет модели".into());
        let topics = task(3, TOPICS_KIND, TaskStatus::Queued, Some(1));
        let tree = vec![failed_analysis.clone(), index, topics];
        let rows = super::rows(
            &failed_analysis,
            &tree,
            &[],
            &ready(true, false, false, false, false),
            0,
        );
        assert_eq!(
            state_of(&rows, Stage::Index),
            RowState::Failed("нет модели".into())
        );
        assert_eq!(
            rows.iter().find(|r| r.stage == Stage::Index).unwrap().task,
            Some(2),
            "the retry resumes this task"
        );
        assert_eq!(state_of(&rows, Stage::Topics), RowState::Off);
        assert!(compact_title(TaskStatus::Failed, &rows, Left::Unknown)
            .contains(&stage_name(Stage::Index).to_lowercase()));
    }

    #[test]
    fn row_texts_for_each_state() {
        let row = |state: RowState, elapsed: Option<u64>, left: Option<u64>| StageRow {
            stage: Stage::Index,
            state,
            progress: 0.5,
            elapsed: elapsed.map(Duration::from_secs),
            left: left.map(Duration::from_secs),
            count: None,
            task: None,
        };
        let running = row_time_text(&row(RowState::Running, Some(83), Some(240)));
        assert!(
            running.contains("1:23") && running.contains('4'),
            "{running}"
        );
        let no_estimate = row_time_text(&row(RowState::Running, Some(5), None));
        assert!(
            no_estimate.contains("0:05") && !no_estimate.contains('·'),
            "{no_estimate}"
        );
        let short = row_time_text(&row(RowState::Running, Some(70), Some(20)));
        assert!(short.contains(&left_text(Left::Under)), "{short}");
        assert_eq!(row_time_text(&row(RowState::Done, Some(125), None)), "2:05");
        assert_eq!(row_time_text(&row(RowState::Done, None, None)), "");
        assert_eq!(
            row_time_text(&row(RowState::Failed("x".into()), Some(1), None)),
            "",
            "the failed row shows the button"
        );
        let texts: Vec<String> = [
            RowState::Waiting,
            RowState::Paused,
            RowState::Cancelled,
            RowState::Off,
        ]
        .into_iter()
        .map(|s| row_time_text(&row(s, None, None)))
        .collect();
        for (i, a) in texts.iter().enumerate() {
            assert!(!a.is_empty());
            for b in &texts[i + 1..] {
                assert_ne!(a, b);
            }
        }
        assert_eq!(
            total_title(TaskStatus::Running, Left::Minutes(4)),
            total_title(TaskStatus::Queued, Left::Minutes(4))
        );
        assert!(total_title(TaskStatus::Running, Left::Minutes(4)).contains('4'));
        assert_ne!(
            total_title(TaskStatus::Failed, Left::Unknown),
            total_title(TaskStatus::Cancelled, Left::Unknown)
        );
    }

    #[test]
    fn the_row_columns_share_the_header_geometry() {
        let rect = Rect::from_min_size(
            egui::pos2(100.0, 200.0),
            egui::vec2(480.0, topics::HEADER_H),
        );
        let h = topics::header(rect);
        let g = row_geom(rect);
        assert_eq!(g.icon, h.icon, "the icon is the header's icon");
        assert_eq!(
            g.name.left(),
            h.text.left(),
            "the name starts where the header's text does"
        );
        assert_eq!(
            g.time.right(),
            h.caret.right(),
            "the right text ends at the caret column"
        );
        assert_eq!(g.bar.center().y, rect.center().y);
        assert!((g.bar.left() - g.name.right() - COL_GAP).abs() < 1e-6);
        assert!((g.time.left() - g.bar.right() - COL_GAP).abs() < 1e-6);
        assert_eq!((g.bar.width(), g.time.width()), (BAR_W, TIME_W));
        let narrow = row_geom(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(120.0, topics::HEADER_H),
        ));
        for r in [narrow.name, narrow.bar, narrow.time] {
            assert!(r.width() >= 0.0, "{r:?}");
        }
    }

    #[test]
    fn a_running_stage_leads_the_compact_line_and_an_empty_error_has_no_bare_dash() {
        let running = StageRow {
            stage: Stage::Topics,
            state: RowState::Running,
            progress: 0.4,
            elapsed: Some(Duration::from_secs(10)),
            left: Some(Duration::from_secs(240)),
            count: Some((2, 5)),
            task: Some(3),
        };
        let title = compact_title(
            TaskStatus::Failed,
            std::slice::from_ref(&running),
            Left::Minutes(4),
        );
        assert_eq!(
            title,
            compact_title(
                TaskStatus::Running,
                std::slice::from_ref(&running),
                Left::Minutes(4)
            )
        );
        assert!(
            shows_spinner(TaskStatus::Failed, std::slice::from_ref(&running)),
            "a restarted stage spins"
        );

        let failed = StageRow {
            stage: Stage::Index,
            state: RowState::Failed("  ".into()),
            progress: 0.0,
            elapsed: None,
            left: None,
            count: None,
            task: Some(2),
        };
        let title = compact_title(TaskStatus::Failed, &[failed], Left::Unknown);
        let bare = t!("analysis.compact_failed", stage = "").to_string();
        assert_ne!(title, bare);
        assert!(!title.trim_end().ends_with('—'), "{title}");
        assert!(
            title.contains(&stage_name(Stage::Index).to_lowercase()),
            "{title}"
        );

        let explained = StageRow {
            stage: Stage::Index,
            state: RowState::Failed("нет модели".into()),
            progress: 0.0,
            elapsed: None,
            left: None,
            count: None,
            task: Some(2),
        };
        let title = compact_title(TaskStatus::Failed, &[explained], Left::Unknown);
        assert!(
            title.contains("нет модели")
                || title.contains(&stage_name(Stage::Index).to_lowercase()),
            "{title}"
        );
        assert_eq!(
            compact_title(TaskStatus::Failed, &[], Left::Unknown),
            t!("analysis.total_failed").to_string()
        );
        assert_eq!(short_reason("  \nboom\n"), "boom");
        assert!(short_reason(&"x".repeat(200)).ends_with('…'));
        assert!(!shows_spinner(TaskStatus::Failed, &[]));
        assert!(shows_spinner(TaskStatus::Queued, &[]));
    }

    #[test]
    fn topics_and_the_breakdown_count_as_running() {
        let digest = task(5, DIGEST_KIND, TaskStatus::Running, Some(3));
        assert!(topics_running(&[digest]));
        assert!(!topics_running(&[task(
            2,
            "index",
            TaskStatus::Running,
            None
        )]));
        assert!(!topics_running(&[task(
            6,
            DETAIL_KIND,
            TaskStatus::Waiting,
            None
        )]));
        assert!(topics_running(&[task(
            7,
            DETAIL_KIND,
            TaskStatus::Running,
            None
        )]));
    }
}
