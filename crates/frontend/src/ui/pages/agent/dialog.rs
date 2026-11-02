use super::analysis::{
    self, CardView, Command, ConfirmWindow, Offer, OptionsWindow, Outcome, Pane, RowState,
    StageHit, StageRow,
};
use super::composer;
use super::studio::{self, panel_insets, StudioAction};
use super::task_details;
use super::topics;
use crate::features::agent::requests as request_buffer;
use crate::features::agent::view as queue;
use crate::features::agent::view::{estimate_tree, KindStat, Status as TaskStatus, Task, TaskId};
use crate::features::library::player::Player;
use crate::ui::theme;
use eframe::egui::{
    self, text::LayoutJob, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke,
    TextFormat,
};
use egui_phosphor::regular as icon;
use mediaa_core::agent::AnalysisOptions;
use mediaa_core::analysis::{readiness, Readiness, Stage};
use mediaa_core::library::human_duration;
use mediaa_core::outline::Chapter;
use mediaa_core::retrieval::Hit;
use mediaa_core::transcript::{self, NameSource, Reply, SpeakerName, Transcript};
use mediaa_protocol::backend::AgentResponseStateWire;
use rust_i18n::t;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const ACTIVE_REFRESH: Duration = Duration::from_secs(1);
const IDLE_REFRESH: Duration = Duration::from_secs(5);

const SEARCH_CLEAR: f32 = 24.0;
const SEARCH_GAP: f32 = 8.0;
const SEARCH_PAD: f32 = 12.0;

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

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

pub fn task_of<'a>(tasks: &'a [Task], file_id: &str) -> Option<&'a Task> {
    tasks
        .iter()
        .filter(|t| {
            t.parent.is_none()
                && t.kind == mediaa_core::analysis::ANALYSIS_KIND
                && t.scope.as_deref() == Some(file_id)
        })
        .max_by_key(|t| t.id)
}

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

pub struct Chrome {
    pub sync: bool,
    pub maximized: bool,
}

pub fn should_follow(sync: bool, changed: bool, visible: bool, jump: bool) -> bool {
    sync && (jump || (changed && !visible))
}

pub fn click_seeks(sync: bool) -> bool {
    sync
}

fn word_on(now: f64, start: f64, end: f64) -> bool {
    now >= start && now < end.max(start + 0.15)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpeakerFact {
    pub id: u32,
    pub spoken: f64,
    pub first: f64,
}

pub fn speaker_facts(replies: &[Reply]) -> Vec<SpeakerFact> {
    let mut map: BTreeMap<u32, (f64, f64)> = BTreeMap::new();
    for reply in replies {
        let Some(id) = reply.speaker else { continue };
        let spoken = (reply.end - reply.start).max(0.0);
        map.entry(id)
            .and_modify(|(total, first)| {
                *total += spoken;
                *first = first.min(reply.start);
            })
            .or_insert((spoken, reply.start));
    }
    map.into_iter()
        .map(|(id, (spoken, first))| SpeakerFact { id, spoken, first })
        .collect()
}

pub fn auto_evidence(info: &SpeakerName) -> Option<String> {
    if info.source != NameSource::Auto || info.evidence.is_empty() {
        return None;
    }
    Some(
        info.evidence
            .iter()
            .map(|e| {
                format!(
                    "[{}] {}",
                    analysis::clock(Duration::from_secs_f64(e.start.max(0.0))),
                    e.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub fn apply_names(
    current: &BTreeMap<u32, String>,
    draft: &[(u32, String)],
) -> BTreeMap<u32, String> {
    let mut names = current.clone();
    for (id, raw) in draft {
        match raw.trim() {
            "" => {
                names.remove(id);
            }
            name => {
                names.insert(*id, name.to_string());
            }
        }
    }
    names
}

const SEARCH_HITS: usize = 8;
const SYNC_PILL: f32 = 28.0;
const SYNC_GAP: f32 = 6.0;

fn sync_switch(ui: &mut egui::Ui, rect: Rect, on: bool, p: theme::Palette) -> egui::Response {
    let resp = ui.interact(rect, ui.id().with("dialog-sync"), Sense::click());
    if ui.is_rect_visible(rect) {
        let color = if on { p.text } else { p.text_weak };
        let galley = ui.fonts_mut(|f| {
            f.layout_no_wrap(
                t!("studio.sync").to_string(),
                FontId::proportional(12.0),
                color,
            )
        });
        ui.painter().galley(
            egui::pos2(rect.left(), rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
        let pill = Rect::from_center_size(
            egui::pos2(rect.right() - SYNC_PILL / 2.0, rect.center().y),
            egui::vec2(SYNC_PILL, 16.0),
        );
        ui.painter().rect_filled(
            pill,
            CornerRadius::same(8),
            if on { p.accent } else { p.border },
        );
        let knob_x = if on {
            pill.right() - 8.0
        } else {
            pill.left() + 8.0
        };
        ui.painter().circle_filled(
            egui::pos2(knob_x, pill.center().y),
            5.0,
            if on { Color32::WHITE } else { p.text_weak },
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    studio::tip(&resp, &t!("studio.sync"));
    resp
}

#[derive(Default)]
struct Search {
    text: String,
    request: Option<u64>,
    hits: Option<Result<Vec<Hit>, String>>,
}

fn hits_from_payload(payload: &serde_json::Value) -> anyhow::Result<Vec<Hit>> {
    let hits = payload["hits"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("search payload has no hits"))?;
    hits.iter()
        .map(|hit| {
            let chunk = &hit["chunk"];
            Ok(Hit {
                id: hit["id"].as_i64().unwrap_or_default(),
                score: hit["score"].as_f64().unwrap_or_default() as f32,
                chunk: mediaa_core::retrieval::Chunk {
                    start: chunk["start"].as_f64().unwrap_or_default(),
                    end: chunk["end"].as_f64().unwrap_or_default(),
                    speakers: chunk["speakers"]
                        .as_array()
                        .map(|speakers| {
                            speakers
                                .iter()
                                .filter_map(serde_json::Value::as_u64)
                                .map(|speaker| speaker as u32)
                                .collect()
                        })
                        .unwrap_or_default(),
                    text: chunk["text"].as_str().unwrap_or_default().to_string(),
                },
            })
        })
        .collect()
}

fn latest_names<'a>(tasks: &'a [Task], file_id: &str) -> Option<&'a Task> {
    tasks
        .iter()
        .filter(|task| {
            task.parent.is_none()
                && task.kind == "speaker-names"
                && task.scope.as_deref() == Some(file_id)
        })
        .max_by_key(|task| task.id)
}

pub struct Dialog {
    path: PathBuf,
    ai_dir: PathBuf,
    file_id: Option<String>,
    transcript: Option<Transcript>,
    replies: Vec<Reply>,
    chapters: Vec<Chapter>,
    names: BTreeMap<u32, String>,
    name_info: BTreeMap<u32, SpeakerName>,
    names_task: Option<Task>,
    task: Option<Task>,
    read: Option<Instant>,
    naming: Option<(u32, String)>,
    playing: Option<usize>,
    scroll_to: Option<usize>,
    ready: Readiness,
    tree: Vec<Task>,
    stats: Vec<KindStat>,
    reading: Option<usize>,
    asked: HashSet<usize>,
    compact_open: bool,
    options: Option<OptionsWindow>,
    confirm: Option<ConfirmWindow>,
    details: Option<TaskId>,
    details_log: String,
    details_copied: bool,
    search: Search,
    name_draft: Option<Vec<(u32, String)>>,
    name_focus: bool,
    saw_sync: bool,
    jump_play: bool,
}

impl Dialog {
    pub fn new(path: &Path, ai_dir: &Path) -> Dialog {
        let file_id = transcript::file_id(path)
            .map_err(|e| log::warn!("{}: {e:#}", path.display()))
            .ok();
        Dialog {
            path: path.to_path_buf(),
            ai_dir: ai_dir.to_path_buf(),
            file_id,
            transcript: None,
            replies: Vec::new(),
            chapters: Vec::new(),
            names: BTreeMap::new(),
            name_info: BTreeMap::new(),
            names_task: None,
            task: None,
            read: None,
            naming: None,
            playing: None,
            scroll_to: None,
            ready: Readiness::default(),
            tree: Vec::new(),
            stats: Vec::new(),
            reading: None,
            asked: HashSet::new(),
            compact_open: false,
            options: None,
            confirm: None,
            details: None,
            details_log: String::new(),
            details_copied: false,
            search: Search::default(),
            name_draft: None,
            name_focus: false,
            saw_sync: true,
            jump_play: false,
        }
    }

    pub fn has_speakers(&self) -> bool {
        self.replies.iter().any(|reply| reply.speaker.is_some())
    }

    pub fn open_names(&mut self) {
        if self.name_draft.is_some() || !self.has_speakers() {
            return;
        }
        self.name_draft = Some(
            speaker_facts(&self.replies)
                .into_iter()
                .map(|fact| {
                    (
                        fact.id,
                        self.names.get(&fact.id).cloned().unwrap_or_default(),
                    )
                })
                .collect(),
        );
        self.name_focus = true;
    }

    pub fn blocks_escape(&self) -> bool {
        self.name_draft.is_some()
            || self.naming.is_some()
            || self.options.is_some()
            || self.confirm.is_some()
            || self.details.is_some()
    }

    fn active(&self) -> bool {
        self.task.as_ref().is_some_and(|t| !t.status.is_finished())
    }

    pub fn has_replies(&self) -> bool {
        self.ready.replies
    }

    fn refresh(&mut self) {
        let every = if self.active() {
            ACTIVE_REFRESH
        } else {
            IDLE_REFRESH
        };
        if self.read.is_some_and(|r| r.elapsed() < every) {
            return;
        }
        self.read = Some(Instant::now());
        let id = self.file_id.clone();
        if let (Some(snapshot), Some(id)) = (super::live_buffer::snapshot(), id) {
            let tasks: Vec<Task> = queue::tasks(&snapshot);
            self.task = task_of(&tasks, &id).cloned();
            self.tree = self
                .task
                .as_ref()
                .map(|task| analysis::descendants(&tasks, task.id))
                .unwrap_or_default();
            self.names_task = latest_names(&tasks, &id).cloned();
            self.stats = queue::kind_stats(&snapshot);
        }
        let Some(id) = &self.file_id else { return };
        match readiness(&self.ai_dir, id) {
            Ok(ready) => self.ready = ready,
            Err(e) => log::warn!("readiness: {e:#}"),
        }
        if self.transcript.is_none() && Transcript::exists(&self.ai_dir, id) {
            self.transcript = Transcript::open(&self.ai_dir, id)
                .map_err(|e| log::warn!("transcript: {e:#}"))
                .ok();
        }
        if let Some(t) = &self.transcript {
            match (t.dialog(), t.speaker_names(), t.speaker_entries()) {
                (Ok(r), Ok(n), Ok(info)) => {
                    self.replies = r;
                    self.names = n;
                    self.name_info = info;
                    self.chapters = if self.ready.topics {
                        t.chapters().unwrap_or_default()
                    } else {
                        Vec::new()
                    };
                }
                (Err(e), _, _) | (_, Err(e), _) | (_, _, Err(e)) => log::warn!("transcript: {e:#}"),
            }
        }
    }

    pub fn roster(&self) -> super::mentions::Roster {
        super::mentions::Roster::recording(
            &self.names,
            self.replies.iter().filter_map(|r| r.speaker),
        )
    }

    pub fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    fn speaker_name(&self, speaker: u32) -> String {
        self.names
            .get(&speaker)
            .cloned()
            .unwrap_or_else(|| t!("studio.speaker", n = speaker + 1).into())
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        rect: Rect,
        player: &mut Player,
        agent: &mediaa_core::agent::AgentSettings,
        p: theme::Palette,
        chrome: &mut Chrome,
    ) -> Option<StudioAction> {
        self.refresh();
        ui.ctx().request_repaint_after(if self.active() {
            ACTIVE_REFRESH
        } else {
            IDLE_REFRESH
        });
        let sync_text = ui
            .fonts_mut(|f| {
                f.layout_no_wrap(
                    t!("studio.sync").to_string(),
                    FontId::proportional(12.0),
                    Color32::PLACEHOLDER,
                )
            })
            .size()
            .x;
        let sync_w = SYNC_PILL + SYNC_GAP + sync_text;
        let left = 4.0 + theme::ICON_BUTTON + 8.0;
        let right = 4.0 + theme::ICON_BUTTON + 8.0 + sync_w + 8.0;
        let body = panel_insets(ui, rect, &t!("studio.dialog"), left, right, p);
        let mut action = None;
        let (glyph, maximize_tip) = if chrome.maximized {
            (icon::ARROWS_IN, t!("studio.restore"))
        } else {
            (icon::ARROWS_OUT, t!("studio.maximize"))
        };
        if studio::edge_button(ui, rect, false, glyph, &maximize_tip, "dialog").clicked() {
            chrome.maximized = !chrome.maximized;
        }
        let (defaults, model, names_model) = (
            agent.analysis,
            agent
                .models
                .speech_recognition
                .as_ref()
                .is_some_and(|source| source.diarize),
            agent.models.agent.is_some(),
        );
        let av = analysis::availability(&self.ready, self.task.as_ref().map(|t| t.status));
        if !av.topics {
            self.reading = None;
        }
        let menu = Rect::from_min_max(
            egui::pos2(rect.max.x - 4.0 - theme::ICON_BUTTON, rect.min.y),
            egui::pos2(rect.max.x - 4.0, rect.min.y + studio::PANE_HEAD),
        );
        let sync_rect = Rect::from_min_max(
            egui::pos2(menu.min.x - 8.0 - sync_w, rect.min.y),
            egui::pos2(menu.min.x - 8.0, rect.min.y + studio::PANE_HEAD),
        );
        if sync_switch(ui, sync_rect, chrome.sync, p).clicked() {
            chrome.sync = !chrome.sync;
        }
        if self.task.is_some() || av.pane == Pane::Replies {
            let rows = if self.active() {
                self.analysis_rows()
                    .map(|(_, rows, _)| rows)
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let items = analysis::menu(
                &self.ready,
                &rows,
                self.active(),
                model,
                self.names_task.as_ref(),
                names_model,
            );
            let command = ui
                .scope_builder(egui::UiBuilder::new().max_rect(menu), |ui| {
                    ui.centered_and_justified(|ui| {
                        let button = theme::icon_face(
                            ui,
                            icon::DOTS_THREE_CIRCLE,
                            &t!("analysis.menu"),
                            theme::ICON_BUTTON,
                        );
                        studio::tip(&button, &t!("analysis.menu"));
                        analysis::menu_popup(&button, &items, p)
                    })
                    .inner
                })
                .inner;
            match command {
                Some(Command::Stop) => {
                    action = self.task.as_ref().map(|t| StudioAction::Stop(t.id))
                }
                Some(Command::StopNames) => {
                    action = self.names_task.as_ref().map(|t| StudioAction::Stop(t.id))
                }
                Some(Command::Names) => action = Some(StudioAction::FindNames(self.path.clone())),
                Some(Command::Analyse) => {
                    self.options = Some(OptionsWindow::new(true, &defaults, model))
                }
                Some(Command::Stage(stage)) => {
                    let add = analysis::offer(stage, &self.ready, model) == Offer::Add;
                    self.confirm = Some(ConfirmWindow::new(stage, add, &defaults));
                }
                None => {}
            }
        }
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(12.0, 8.0))),
            |ui| {
                match av.pane {
                    Pane::Start | Pane::NoSpeech => {
                        if self.empty(ui, p) {
                            self.options = Some(OptionsWindow::new(false, &defaults, model));
                        }
                        return;
                    }
                    Pane::Progress => {
                        action = self.progress_ui(ui, p);
                        if !self.active() {
                            ui.add_space(6.0);
                            if self.empty(ui, p) {
                                self.options = Some(OptionsWindow::new(false, &defaults, model));
                            }
                        }
                        return;
                    }
                    Pane::Replies => {}
                }
                if av.compact {
                    if let Some(a) = self.compact_ui(ui, p) {
                        action = Some(a);
                    }
                    ui.add_space(6.0);
                }
                if av.topics && !self.chapters.is_empty() {
                    let roster = self.roster();
                    if self.reading.is_some() {
                        let building = self.details_building();
                        let spin = analysis::topics_running(&self.tree);
                        if let Some(topic) = topics::reading(
                            ui,
                            &self.chapters,
                            &mut self.reading,
                            player,
                            building,
                            spin,
                            &roster,
                            p,
                        ) {
                            self.hurry_detail(topic);
                        }
                        if self.reading.is_none() {
                            self.asked.clear();
                        }
                        return;
                    }
                    let spin = analysis::topics_running(&self.tree);
                    if let Some(start) = topics::block(
                        ui,
                        &self.chapters,
                        player.position(),
                        body.height() * 0.4,
                        &mut self.reading,
                        spin,
                        &roster,
                        p,
                    ) {
                        if click_seeks(chrome.sync) {
                            player.seek(start);
                            self.scroll_to = self
                                .replies
                                .iter()
                                .position(|r| r.end > start + 0.001)
                                .or(Some(0));
                            self.search.hits = None;
                        }
                    }
                    ui.add_space(6.0);
                }
                if self.search_ui(ui, player, av.search, chrome.sync, p) {
                    return;
                }
                self.replies_ui(ui, player, chrome.sync, p);
            },
        );
        self.names_window(ui);
        if let Some(a) = self.windows(ui.ctx(), &defaults, model, p) {
            action = Some(a);
        }
        action
    }

    fn open_details(&mut self, id: TaskId) {
        if self.details == Some(id) {
            return;
        }
        let around = self
            .tree
            .iter()
            .find(|t| t.id == id)
            .and_then(|t| t.finished_ms)
            .and_then(task_details::local_time);
        self.details_log = task_details::log_excerpt(&task_details::read_program_log(), id, around);
        self.details = Some(id);
        self.details_copied = false;
    }

    fn take_stage(&mut self, hit: Option<StageHit>) -> Option<StudioAction> {
        match hit {
            Some(StageHit::Retry(id)) => Some(StudioAction::ResumeTask(id)),
            Some(StageHit::Details(id)) => {
                self.open_details(id);
                None
            }
            None => None,
        }
    }

    fn windows(
        &mut self,
        ctx: &egui::Context,
        defaults: &AnalysisOptions,
        model: bool,
        p: theme::Palette,
    ) -> Option<StudioAction> {
        let mut action = None;
        if let Some(window) = &mut self.options {
            match window.show(ctx, egui::Id::new(("analysis-options", &self.path)), p) {
                Outcome::Open => {}
                Outcome::Cancel => self.options = None,
                Outcome::Run(options) => {
                    let from = window.restart.then_some(Stage::Transcribe);
                    action = Some(StudioAction::Analyse {
                        path: self.path.clone(),
                        from,
                        options,
                        remember: true,
                    });
                    self.options = None;
                }
            }
        }
        if let Some(window) = &mut self.confirm {
            let plan = window.plan(&self.ready, defaults, model);
            match window.show(
                ctx,
                egui::Id::new(("analysis-confirm", &self.path)),
                &plan,
                p,
            ) {
                Outcome::Open => {}
                Outcome::Cancel => self.confirm = None,
                Outcome::Run(plan) => {
                    action = Some(StudioAction::Analyse {
                        path: self.path.clone(),
                        from: Some(window.stage),
                        options: plan.options,
                        remember: false,
                    });
                    self.confirm = None;
                }
            }
        }
        if let Some(a) = self.show_details(ctx) {
            action = Some(a);
        }
        action
    }

    fn show_details(&mut self, ctx: &egui::Context) -> Option<StudioAction> {
        let id = self.details?;
        let Some(task) = self.tree.iter().find(|t| t.id == id).cloned() else {
            self.details = None;
            return None;
        };
        let stats = super::queue::subtree_stats(&task, &self.tree);
        let roster = task
            .scope
            .as_deref()
            .map(|id| super::mentions::Roster::of_file(&self.ai_dir, id))
            .unwrap_or_else(|| self.roster());
        match task_details::show(
            ctx,
            &task,
            &stats,
            now_ms(),
            &self.details_log,
            &mut self.details_copied,
            &roster,
        ) {
            Some(task_details::Cmd::Close) => {
                self.details = None;
                None
            }
            Some(task_details::Cmd::CopyFailed(e)) => {
                log::warn!("task details: {e}");
                None
            }
            None => None,
        }
    }

    fn analysis_rows(&self) -> Option<(TaskStatus, Vec<StageRow>, analysis::Left)> {
        let task = self.task.as_ref()?;
        let now = now_ms();
        let rows = analysis::rows(task, &self.tree, &self.stats, &self.ready, now);
        let total = analysis::left(estimate_tree(task, &self.tree, now, &self.stats));
        Some((task.status, rows, total))
    }

    fn details_building(&self) -> bool {
        self.analysis_rows().is_some_and(|(_, rows, _)| {
            rows.iter().any(|r| {
                r.stage == Stage::Details
                    && matches!(r.state, RowState::Running | RowState::Waiting)
            })
        })
    }

    fn hurry_detail(&mut self, topic: usize) {
        if !self.asked.insert(topic) {
            return;
        }
        if let Some(id) = &self.file_id {
            request_buffer::start(
                "prioritize_detail",
                serde_json::json!({ "store": id, "topic": topic }),
            );
        }
    }

    fn progress_ui(&mut self, ui: &mut egui::Ui, p: theme::Palette) -> Option<StudioAction> {
        let (status, rows, total) = self.analysis_rows()?;
        let title = analysis::total_title(status, total);
        let view = CardView {
            title: &title,
            rows: &rows,
            running: analysis::shows_spinner(status, &rows),
            failed: status == TaskStatus::Failed,
        };
        self.take_stage(analysis::card(ui, &view, None, p))
    }

    fn compact_ui(&mut self, ui: &mut egui::Ui, p: theme::Palette) -> Option<StudioAction> {
        let (status, rows, total) = self.analysis_rows()?;
        let title = analysis::compact_title(status, &rows, total);
        let view = CardView {
            title: &title,
            rows: &rows,
            running: analysis::shows_spinner(status, &rows),
            failed: status == TaskStatus::Failed,
        };
        let hit = analysis::card(ui, &view, Some(&mut self.compact_open), p);
        self.take_stage(hit)
    }

    fn empty(&self, ui: &mut egui::Ui, p: theme::Palette) -> bool {
        let mut pressed = false;
        ui.vertical_centered(|ui| {
            ui.add_space((ui.available_height() / 2.0 - 70.0).max(0.0));
            ui.label(RichText::new(icon::CHAT_TEXT).size(28.0).color(p.text_weak));
            ui.add_space(4.0);
            let done = self
                .task
                .as_ref()
                .is_some_and(|t| t.status == TaskStatus::Done);
            theme::hint(
                ui,
                if done {
                    t!("studio.no_speech")
                } else {
                    t!("studio.no_transcript")
                },
            );
            ui.add_space(8.0);
            pressed = ui
                .add(theme::primary(ui, icon::WAVEFORM, &t!("studio.transcribe")))
                .clicked();
        });
        pressed
    }

    fn search_ui(
        &mut self,
        ui: &mut egui::Ui,
        player: &mut Player,
        available: bool,
        sync: bool,
        p: theme::Palette,
    ) -> bool {
        if !available {
            if self
                .task
                .as_ref()
                .is_none_or(|t| t.status == TaskStatus::Done)
            {
                theme::hint(ui, t!("studio.search_soon"));
                ui.add_space(4.0);
            }
            return false;
        }
        let (card, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), topics::HEADER_H),
            Sense::hover(),
        );
        ui.painter().rect(
            card,
            CornerRadius::same(8),
            p.card,
            Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            egui::pos2(card.left() + topics::HEADER_ICON_X, card.center().y),
            egui::Align2::LEFT_CENTER,
            icon::MAGNIFYING_GLASS,
            FontId::proportional(14.0),
            p.accent,
        );
        let clear = self.search.hits.is_some() || !self.search.text.is_empty();
        let clear_slot = Rect::from_center_size(
            egui::pos2(
                card.right() - SEARCH_PAD - SEARCH_CLEAR / 2.0,
                card.center().y,
            ),
            egui::vec2(SEARCH_CLEAR, SEARCH_CLEAR),
        );
        let field = Rect::from_min_max(
            egui::pos2(card.left() + topics::HEADER_TEXT_X, card.top() + 2.0),
            egui::pos2(clear_slot.left() - SEARCH_GAP, card.bottom() - 2.0),
        );
        let edit = ui
            .scope_builder(
                egui::UiBuilder::new()
                    .max_rect(field)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.search.text)
                            .hint_text(t!("studio.search_hint"))
                            .desired_width(field.width())
                            .margin(Margin::ZERO)
                            .frame(egui::Frame::NONE),
                    )
                },
            )
            .inner;
        let start = edit.lost_focus()
            && ui.input(|i| i.key_pressed(egui::Key::Enter))
            && !self.search.text.trim().is_empty();
        if clear {
            let close = ui
                .scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(clear_slot)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        theme::icon_button_sized(
                            ui,
                            icon::X,
                            &t!("studio.search_close"),
                            SEARCH_CLEAR,
                        )
                    },
                )
                .inner;
            if close.clicked() {
                self.search = Search::default();
            }
        }
        ui.add_space(4.0);
        if let Some(request) = self.search.request {
            match request_buffer::take(request) {
                Some(AgentResponseStateWire::Done { payload }) => {
                    self.search.request = None;
                    self.search.hits =
                        Some(hits_from_payload(&payload).map_err(|error| format!("{error:#}")));
                }
                Some(AgentResponseStateWire::Failed { error }) => {
                    self.search.request = None;
                    self.search.hits = Some(Err(error));
                }
                _ => {}
            }
        }
        if start {
            if let Some(id) = self.file_id.clone() {
                self.search.request = Some(request_buffer::start(
                    "search",
                    serde_json::json!({
                        "store": id,
                        "query": self.search.text.trim(),
                        "limit": SEARCH_HITS,
                    }),
                ));
                self.search.hits = None;
            }
        }
        if self.search.request.is_some() && self.search.hits.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                theme::hint(ui, t!("studio.searching"));
            });
            return true;
        }
        let Some(hits) = &self.search.hits else {
            return false;
        };
        let mut seek = None;
        match hits {
            Err(e) => {
                ui.colored_label(p.danger, format!("{e:#}"));
            }
            Ok(hits) if hits.is_empty() => theme::hint(ui, t!("studio.nothing_found")),
            Ok(hits) => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for h in hits {
                            let card = theme::card(ui).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(format!(
                                            "{} – {}",
                                            human_duration(h.chunk.start),
                                            human_duration(h.chunk.end)
                                        ))
                                        .font(FontId::monospace(11.5))
                                        .color(p.accent),
                                    );
                                    for s in &h.chunk.speakers {
                                        ui.label(
                                            RichText::new(self.speaker_name(*s))
                                                .color(speaker_color(*s))
                                                .size(12.0),
                                        );
                                    }
                                });
                                ui.add(
                                    egui::Label::new(RichText::new(&h.chunk.text).size(13.0))
                                        .wrap(),
                                );
                            });
                            let resp = ui.interact(
                                card.response.rect,
                                ui.id().with(("hit", h.id)),
                                Sense::click(),
                            );
                            if click_seeks(sync) && resp.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if click_seeks(sync) {
                                let roster = self.roster();
                                topics::text_tip(&resp, &t!("studio.go_there"), &roster);
                            }
                            if click_seeks(sync) && resp.clicked() {
                                seek = Some(h.chunk.start);
                            }
                            ui.add_space(4.0);
                        }
                    });
            }
        }
        if click_seeks(sync) {
            if let Some(t) = seek {
                player.seek(t);
                self.search.hits = None;
            }
        }
        self.search.hits.is_some()
    }

    fn replies_ui(
        &mut self,
        ui: &mut egui::Ui,
        player: &mut Player,
        sync: bool,
        p: theme::Palette,
    ) {
        if sync && !self.saw_sync {
            self.jump_play = true;
        }
        self.saw_sync = sync;
        let now = player.position();
        let playing = self
            .replies
            .iter()
            .position(|r| now >= r.start && now < r.end + 0.3);
        let changed = playing != self.playing;
        self.playing = playing;
        let jump = self.jump_play;
        self.jump_play = false;
        let mut seek = None;
        let mut name_click = None;
        let scroll_to = self.scroll_to.take().filter(|_| sync);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (i, reply) in self.replies.iter().enumerate() {
                    let current = sync && playing == Some(i);
                    let fill = if current {
                        p.accent_soft
                    } else {
                        Color32::TRANSPARENT
                    };
                    let mut slots = None;
                    let row = egui::Frame::new()
                        .fill(fill)
                        .corner_radius(CornerRadius::same(6))
                        .inner_margin(Margin::symmetric(4, 2))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if let Some(s) = reply.speaker {
                                    let color = speaker_color(s);
                                    let chip = ui.add(
                                        egui::Button::new(
                                            RichText::new(self.speaker_name(s))
                                                .color(color)
                                                .strong()
                                                .size(12.5),
                                        )
                                        .frame(false),
                                    );
                                    if chip.on_hover_text(t!("studio.rename_speaker")).clicked() {
                                        name_click = Some(s);
                                    }
                                }
                                let time = ui.add(
                                    egui::Button::new(
                                        RichText::new(human_duration(reply.start))
                                            .font(FontId::monospace(11.5))
                                            .color(p.text_weak),
                                    )
                                    .frame(false),
                                );
                                if click_seeks(sync) && time.clicked() {
                                    seek = Some(reply.start);
                                }
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let copy = ui.allocate_exact_size(
                                            egui::vec2(theme::ICON_BUTTON, theme::ICON_BUTTON),
                                            Sense::click(),
                                        );
                                        let reply_btn = ui.allocate_exact_size(
                                            egui::vec2(theme::ICON_BUTTON, theme::ICON_BUTTON),
                                            Sense::click(),
                                        );
                                        slots = Some((reply_btn, copy));
                                    },
                                );
                            });
                            if let Some(t) = reply_text(
                                ui,
                                reply,
                                if current { Some(now) } else { None },
                                click_seeks(sync),
                                p,
                            ) {
                                seek = Some(t);
                            }
                        });
                    if let Some(((reply_rect, reply_resp), (copy_rect, copy_resp))) = slots {
                        if row.response.hovered() {
                            let color = p.text;
                            ui.painter().text(
                                reply_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                icon::ARROW_BEND_UP_LEFT,
                                FontId::proportional(14.0),
                                color,
                            );
                            ui.painter().text(
                                copy_rect.center(),
                                egui::Align2::CENTER_CENTER,
                                icon::COPY,
                                FontId::proportional(14.0),
                                color,
                            );
                        }
                        if reply_resp.hovered() {
                            studio::tip(&reply_resp, &t!("chat.reply"));
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if copy_resp.hovered() {
                            studio::tip(&copy_resp, &t!("chat.copy"));
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if reply_resp.clicked() {
                            let shown = reply
                                .speaker
                                .map(|s| self.speaker_name(s))
                                .unwrap_or_default();
                            let quote = composer::Quote::from_reply(
                                reply.speaker,
                                &shown,
                                reply.start,
                                reply.end,
                                &reply.text(),
                            );
                            let key = self.path.to_string_lossy().into_owned();
                            ui.ctx()
                                .data_mut(|d| d.insert_temp(composer::quote_id(&key), quote));
                        }
                        if copy_resp.clicked() {
                            ui.ctx().copy_text(reply.text());
                        }
                    }
                    let visible = ui.clip_rect().contains_rect(row.response.rect);
                    if should_follow(
                        sync,
                        changed && playing == Some(i),
                        visible,
                        jump && playing == Some(i),
                    ) || scroll_to == Some(i)
                    {
                        row.response.scroll_to_me(Some(egui::Align::Center));
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

    fn names_window(&mut self, ui: &mut egui::Ui) {
        if self.name_draft.is_none() {
            return;
        }
        let facts = speaker_facts(&self.replies);
        if let Some(draft) = &mut self.name_draft {
            for fact in &facts {
                if !draft.iter().any(|(id, _)| *id == fact.id) {
                    draft.push((fact.id, String::new()));
                }
            }
            draft.retain(|(id, _)| facts.iter().any(|fact| fact.id == *id));
        }
        let mut save = false;
        let mut cancel = false;
        let focus = std::mem::take(&mut self.name_focus);
        let ctx = ui.ctx().clone();
        let mut dismissed = false;
        theme::with_compact_title(&ctx, || {
            let modal =
                egui::Modal::new(egui::Id::new(("speaker-names", &self.path))).show(&ctx, |ui| {
                    ui.set_width(460.0);
                    theme::popup_title(ui, &t!("studio.names"));
                    let p = theme::palette(ui);
                    let draft = self.name_draft.as_mut().expect("the dialog is open");
                    let mut pending_focus = focus;
                    for fact in &facts {
                        let Some(pos) = draft.iter().position(|(id, _)| *id == fact.id) else {
                            continue;
                        };
                        let (id, spoken, first_at) = (fact.id, fact.spoken, fact.first);
                        let focus_this = pending_focus;
                        pending_focus = false;
                        ui.horizontal(|ui| {
                            let label = t!("studio.speaker", n = id + 1);
                            ui.add_sized(
                                [140.0, 22.0],
                                egui::Label::new(
                                    RichText::new(label.to_string())
                                        .color(speaker_color(id))
                                        .strong()
                                        .size(12.5),
                                ),
                            );
                            let edit = ui.add(
                                egui::TextEdit::singleline(&mut draft[pos].1)
                                    .desired_width(150.0)
                                    .hint_text(t!("studio.speaker", n = id + 1)),
                            );
                            if focus_this {
                                edit.request_focus();
                            }
                            if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                save = true;
                            }
                            ui.label(
                                RichText::new(t!(
                                    "studio.names_meta",
                                    spoken = human_duration(spoken),
                                    first = human_duration(first_at)
                                ))
                                .size(11.5)
                                .color(p.text_weak),
                            );
                            if let Some(evidence) = self.name_info.get(&id).and_then(auto_evidence)
                            {
                                let mark = ui.label(
                                    RichText::new(t!("studio.names_auto"))
                                        .size(11.0)
                                        .color(p.success),
                                );
                                studio::tip(&mark, &evidence);
                            }
                        });
                    }
                    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                        save = true;
                    }
                    if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
                        cancel = true;
                    }
                    ui.add_space(8.0);
                    theme::centered_row(ui, |ui| {
                        if ui
                            .add(theme::primary(ui, icon::CHECK, &t!("studio.save_name")))
                            .clicked()
                        {
                            save = true;
                        }
                        if ui
                            .add(theme::secondary(icon::X, &t!("common.cancel")))
                            .clicked()
                        {
                            cancel = true;
                        }
                    });
                });
            dismissed = modal.should_close();
        });
        if dismissed {
            cancel = true;
        }
        if save && !cancel {
            self.save_names();
        } else if cancel {
            self.name_draft = None;
        }
    }

    fn save_names(&mut self) {
        let Some(draft) = self.name_draft.take() else {
            return;
        };
        if let Some(transcript) = &self.transcript {
            for (id, name) in &draft {
                if let Err(e) = transcript.rename_speaker(*id, name) {
                    log::warn!("transcript: {e:#}");
                }
            }
        }
        self.names = apply_names(&self.names, &draft);
    }

    fn naming_window(&mut self, ui: &mut egui::Ui) {
        let Some((speaker, name)) = &mut self.naming else {
            return;
        };
        let speaker = *speaker;
        let mut name = name.clone();
        let mut done = false;
        let ctx = ui.ctx().clone();
        let mut dismissed = false;
        theme::with_compact_title(&ctx, || {
            let modal =
                egui::Modal::new(egui::Id::new(("speaker-name", &self.path))).show(&ctx, |ui| {
                    ui.set_width(280.0);
                    theme::popup_title(ui, &t!("studio.rename_speaker"));
                    let edit = ui.add(
                        egui::TextEdit::singleline(&mut name)
                            .desired_width(240.0)
                            .hint_text(t!("studio.speaker", n = speaker + 1)),
                    );
                    edit.request_focus();
                    if edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        done = true;
                    }
                    theme::centered_row_sized(ui, 120.0, |ui| {
                        if ui
                            .add(theme::primary(ui, icon::CHECK, &t!("studio.save_name")))
                            .clicked()
                        {
                            done = true;
                        }
                    });
                });
            dismissed = modal.should_close();
        });
        if dismissed {
            self.naming = None;
        }
        if done {
            if let Some(t) = &self.transcript {
                if let Err(e) = t.rename_speaker(speaker, &name) {
                    log::warn!("transcript: {e:#}");
                }
            }
            self.read = None;
            self.naming = None;
        }
    }
}

fn reply_text(
    ui: &mut egui::Ui,
    reply: &Reply,
    now: Option<f64>,
    seekable: bool,
    p: theme::Palette,
) -> Option<f64> {
    let mut job = LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let plain = TextFormat {
        font_id: FontId::proportional(14.0),
        color: p.text,
        ..Default::default()
    };
    let lit = TextFormat {
        background: p.accent_soft,
        color: p.accent,
        ..plain.clone()
    };
    for (i, w) in reply.words.iter().enumerate() {
        let on = now.is_some_and(|t| word_on(t, w.start, w.end));
        job.append(&w.text, 0.0, if on { lit.clone() } else { plain.clone() });
        if i + 1 < reply.words.len() {
            job.append(" ", 0.0, plain.clone());
        }
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let (rect, resp) = ui.allocate_exact_size(galley.size(), Sense::click());
    ui.painter().galley(rect.min, galley.clone(), p.text);
    if seekable && resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if !seekable {
        return None;
    }
    let pos = resp.interact_pointer_pos().filter(|_| resp.clicked())?;
    let cursor = galley.cursor_from_pos(pos - rect.min);
    word_at(reply, cursor.index.0).map(|i| reply.words[i].start)
}
