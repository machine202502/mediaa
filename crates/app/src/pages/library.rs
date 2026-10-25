mod cleanup;
mod grid;

use super::header;
use crate::app::tasks::{TaskId, TaskKind, TaskSupervisor};
use crate::app::App;
use crate::hover_preview::Previews;
use crate::theme;
use crate::viewer::{Viewer, ViewerAction};
use chrono::Datelike;
use cleanup::{
    cleanup_old_selected, cleanup_plan, cleanup_selected, CleanupFailure, CleanupItem,
    CleanupOlder, CleanupPlan, CleanupReport,
};
use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;
#[cfg(test)]
use grid::cover_uv;
use grid::{grid_columns, kind_filter, tile, toolbar_tools, TOOLBAR_ONE_LINE};
use mediaa_core::library::{self, human_duration, human_size, Details, KindFilter, LibraryEntry};
use mediaa_core::{naming, MediaKind, Settings};
use mediaa_platform::files;
use mediaa_storage::library_index::{self, LibraryIndex, LibraryWatcher, Query, Sort};
use rust_i18n::t;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const STRIP_PAD: i8 = 8;
const MIN_TILE: f32 = 200.0;
const TILE_BORDER: f32 = 1.0;
const GAP: f32 = 10.0;
const MEDIA_ASPECT: f32 = 0.62;
const SCRUB_ZONE: f32 = 26.0;
const CHUNK: usize = 300;
const RESCAN_EVERY: Duration = Duration::from_secs(60);

pub struct LibraryUi {
    index: Option<LibraryIndex>,
    load: Option<Receiver<(PathBuf, LibraryIndex)>>,
    load_task: Option<TaskId>,
    watcher: Option<LibraryWatcher>,
    watcher_retry: Instant,
    scan: Option<crossbeam_channel::Receiver<Vec<mediaa_core::library::LibraryEntry>>>,
    scan_task: Option<TaskId>,
    changed_while_scanning: Vec<PathBuf>,
    last_scan: Instant,
    query: Query,
    hits: Option<Arc<HitList>>,
    limit: usize,
    confirm_delete: Option<PathBuf>,
    confirm_cleanup: Option<CleanupPlan>,
    cleanup_job: Option<Receiver<(CleanupPlan, CleanupReport)>>,
    cleanup_task: Option<TaskId>,
    cleanup_success: Option<String>,
    renaming: Option<(PathBuf, String)>,
    hovered: Option<PathBuf>,
    hover_next: Option<PathBuf>,
    previews: Previews,
    viewer: Option<Viewer>,
    dev_viewed: bool,
}

struct HitList {
    version: u64,
    query: Query,
    limit: usize,
    indices: Arc<[usize]>,
    entries: Arc<[LibraryEntry]>,
}

impl Default for LibraryUi {
    fn default() -> Self {
        Self {
            index: None,
            load: None,
            load_task: None,
            watcher: None,
            watcher_retry: Instant::now(),
            scan: None,
            scan_task: None,
            changed_while_scanning: Vec::new(),
            last_scan: Instant::now(),
            query: Query::default(),
            hits: None,
            limit: CHUNK,
            confirm_delete: None,
            confirm_cleanup: None,
            cleanup_job: None,
            cleanup_task: None,
            cleanup_success: None,
            renaming: None,
            hovered: None,
            hover_next: None,
            previews: Previews::default(),
            viewer: None,
            dev_viewed: false,
        }
    }
}

pub fn library_dir(settings: &Settings) -> PathBuf {
    mediaa_core::dev::library_dir().unwrap_or_else(|| settings.media_dir.clone())
}

impl LibraryUi {
    pub fn viewer_open(&self) -> bool {
        self.viewer.is_some()
    }

    pub(crate) fn refresh(&mut self, settings: &Settings, tasks: &mut TaskSupervisor) {
        let dir = library_dir(settings);
        if self.scan.is_some() {
            if self.index.as_ref().is_some_and(|index| index.dir() == dir) {
                return;
            }
            if let Some(task) = self.scan_task.take() {
                tasks.detach(task);
            }
            self.scan = None;
        }
        if self.index.as_ref().is_none_or(|i| i.dir() != dir) {
            if self.load.is_none() {
                let (tx, rx) = crossbeam_channel::bounded(1);
                match tasks.spawn(TaskKind::LibraryLoad, "library-load", None, move || {
                    let index = LibraryIndex::load(&dir);
                    let _ = tx.send((dir, index));
                }) {
                    Ok(task) => {
                        self.load = Some(rx);
                        self.load_task = Some(task);
                    }
                    Err(error) => log::warn!("library load thread: {error}"),
                }
            }
            self.watcher = None;
            self.watcher_retry = Instant::now();
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let spawned = tasks.spawn(TaskKind::LibraryScan, "library-scan", None, move || {
            let _ = tx.send(library_index::scan_dir(&dir));
        });
        self.scan = match spawned {
            Ok(task) => {
                self.scan_task = Some(task);
                Some(rx)
            }
            Err(error) => {
                log::warn!("library scan thread: {error}");
                None
            }
        };
        self.changed_while_scanning.clear();
        self.last_scan = Instant::now();
    }

    pub fn files_changed(&mut self, paths: &[PathBuf]) {
        if let Some(index) = self.index.as_mut() {
            index.refresh_paths(paths);
        }
        if self.scan.is_some() {
            self.changed_while_scanning.extend_from_slice(paths);
        }
    }

    fn poll(
        &mut self,
        settings: &Settings,
        tasks: &mut TaskSupervisor,
        ctx: &egui::Context,
    ) -> Option<(PathBuf, HashMap<String, SystemTime>)> {
        if self
            .index
            .as_ref()
            .is_none_or(|i| i.dir() != library_dir(settings))
        {
            if let Some(load) = &self.load {
                match load.try_recv() {
                    Ok((dir, index)) => {
                        log::debug!("library index loaded from {}", dir.display());
                        self.index = Some(index);
                        self.load = None;
                        if let Some(task) = self.load_task.take() {
                            tasks.finish(task);
                        }
                        self.refresh(settings, tasks);
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                    Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        self.load = None;
                        if let Some(task) = self.load_task.take() {
                            tasks.finish(task);
                        }
                        log::warn!("library index loader stopped");
                    }
                }
                return None;
            }
            self.refresh(settings, tasks);
        }
        let index = self.index.as_mut()?;
        index.flush_cache();
        let mut scanned = None;
        if self.watcher.is_none() && Instant::now() >= self.watcher_retry {
            let c = ctx.clone();
            self.watcher = LibraryWatcher::new(index.dir(), move || c.request_repaint())
                .map_err(|e| log::warn!("library watcher: {e:#}"))
                .ok();
            if self.watcher.is_none() {
                self.watcher_retry = Instant::now() + RESCAN_EVERY;
            }
        }
        let mut rescan = false;
        if let Some(w) = &self.watcher {
            let c = w.changes();
            index.refresh_paths(&c.paths);
            if self.scan.is_some() {
                self.changed_while_scanning.extend(c.paths);
            }
            rescan = c.rescan;
        }
        if (rescan || self.last_scan.elapsed() > RESCAN_EVERY) && self.scan.is_none() {
            if rescan {
                self.watcher = None;
            }
            self.refresh(settings, tasks);
            return None;
        }
        let index = self.index.as_mut()?;
        if let Some(rx) = &self.scan {
            match rx.try_recv() {
                Ok(listing) => {
                    index.apply_scan(listing);
                    index.refresh_paths(&std::mem::take(&mut self.changed_while_scanning));
                    self.scan = None;
                    if let Some(task) = self.scan_task.take() {
                        tasks.finish(task);
                    }
                    scanned = Some((
                        index.dir().to_path_buf(),
                        index
                            .entries()
                            .iter()
                            .map(|entry| (entry.name.clone(), entry.modified))
                            .collect(),
                    ));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50))
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.scan = None;
                    if let Some(task) = self.scan_task.take() {
                        tasks.finish(task);
                    }
                }
            }
        }
        scanned
    }

    fn hits(&mut self) -> Option<Arc<HitList>> {
        let index = self.index.as_ref()?;
        let query_changed = self
            .hits
            .as_ref()
            .is_none_or(|hits| hits.query != self.query);
        if query_changed {
            self.limit = CHUNK;
        }
        let rebuild = self.hits.as_ref().is_none_or(|hits| {
            hits.version != index.version() || hits.query != self.query || hits.limit != self.limit
        });
        if !rebuild {
            return self.hits.clone();
        }
        let indices: Arc<[usize]> = if query_changed
            || self
                .hits
                .as_ref()
                .is_none_or(|hits| hits.version != index.version())
        {
            index.query(&self.query).into()
        } else {
            self.hits
                .as_ref()
                .map(|hits| hits.indices.clone())
                .unwrap_or_default()
        };
        let entries: Arc<[LibraryEntry]> = indices
            .iter()
            .take(self.limit)
            .filter_map(|&entry| index.entries().get(entry).cloned())
            .collect::<Vec<_>>()
            .into();
        self.hits = Some(Arc::new(HitList {
            version: index.version(),
            query: self.query.clone(),
            limit: self.limit,
            indices,
            entries,
        }));
        self.hits.clone()
    }

    fn set_details(&mut self, path: &Path, details: Details) {
        if let Some(i) = self.index.as_mut() {
            i.set_details(path, details);
        }
    }

    pub fn flush_details(&mut self) {
        if let Some(index) = self.index.as_mut() {
            index.flush_details();
        }
    }

    pub fn flush(&mut self) {
        if let Some(index) = self.index.as_mut() {
            index.flush();
        }
    }

    fn cleanup_plan(&self, older: CleanupOlder) -> CleanupPlan {
        let entries = self
            .index
            .as_ref()
            .map(LibraryIndex::entries)
            .unwrap_or_default();
        cleanup_plan(entries, older, SystemTime::now())
    }

    fn ask_cleanup(&mut self, older: CleanupOlder) {
        self.confirm_cleanup = Some(self.cleanup_plan(older));
    }

    fn prepare_for_removal(&mut self, items: &[CleanupItem]) {
        self.previews.stop();
        if self
            .viewer
            .as_ref()
            .is_some_and(|viewer| items.iter().any(|item| item.path == viewer.path()))
        {
            self.viewer = None;
        }
    }

    pub fn cleanup_active(&self) -> bool {
        self.cleanup_job.is_some()
    }

    fn start_cleanup(
        &mut self,
        plan: &CleanupPlan,
        tasks: &mut TaskSupervisor,
    ) -> anyhow::Result<()> {
        if self.cleanup_active() {
            anyhow::bail!("cleanup is already running");
        }
        self.prepare_for_removal(&plan.items);
        let plan = plan.clone();
        let worker_plan = plan.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let task = tasks.spawn(
            TaskKind::LibraryCleanup,
            "library-cleanup",
            None,
            move || {
                let report = cleanup_old_selected(
                    &worker_plan.items,
                    worker_plan.older,
                    SystemTime::now(),
                    files::move_to_trash,
                );
                let _ = tx.send((worker_plan, report));
            },
        )?;
        self.cleanup_job = Some(rx);
        self.cleanup_task = Some(task);
        self.cleanup_success = None;
        Ok(())
    }

    fn start_delete(&mut self, path: PathBuf, tasks: &mut TaskSupervisor) -> anyhow::Result<()> {
        if self.cleanup_active() {
            anyhow::bail!("cleanup is already running");
        }
        let item = CleanupItem { path, size: 0 };
        self.prepare_for_removal(std::slice::from_ref(&item));
        let worker_item = item.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let task = tasks.spawn(
            TaskKind::LibraryCleanup,
            "library-delete",
            None,
            move || {
                let items = [worker_item];
                let report = cleanup_selected(&items, files::move_to_trash);
                let plan = CleanupPlan {
                    older: CleanupOlder::Week,
                    items: items.to_vec(),
                };
                let _ = tx.send((plan, report));
            },
        )?;
        self.cleanup_job = Some(rx);
        self.cleanup_task = Some(task);
        self.cleanup_success = Some(
            t!(
                "lib.deleted",
                name = item.path.file_name().unwrap_or_default().to_string_lossy()
            )
            .into_owned(),
        );
        Ok(())
    }

    fn poll_cleanup(
        &mut self,
        tasks: &mut TaskSupervisor,
        ctx: &egui::Context,
    ) -> Option<(CleanupPlan, CleanupReport, Option<String>)> {
        let rx = self.cleanup_job.as_ref()?;
        match rx.try_recv() {
            Ok(result) => {
                self.cleanup_job = None;
                if let Some(task) = self.cleanup_task.take() {
                    tasks.finish(task);
                }
                let success = self.cleanup_success.take();
                Some((result.0, result.1, success))
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                None
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.cleanup_job = None;
                if let Some(task) = self.cleanup_task.take() {
                    tasks.finish(task);
                }
                let success = self.cleanup_success.take();
                Some((
                    CleanupPlan {
                        older: CleanupOlder::Week,
                        items: Vec::new(),
                    },
                    CleanupReport {
                        failures: vec![CleanupFailure {
                            path: PathBuf::new(),
                            error: "cleanup worker stopped".into(),
                        }],
                        ..CleanupReport::default()
                    },
                    success,
                ))
            }
        }
    }
}

#[derive(Clone)]
enum Action {
    Open(PathBuf),
    View(PathBuf, MediaKind, Option<Details>),
    Reveal(PathBuf),
    CopyFile(PathBuf),
    CopyImage(PathBuf),
    CopyPath(PathBuf),
    CopyLink(String),
    Convert(PathBuf),
    AskDelete(PathBuf),
    AskRename(PathBuf, String),
}

struct Tool {
    icon: &'static str,
    label: String,
    action: Action,
    main: bool,
    danger: bool,
}

const MAIN_TOOLS: usize = 4;
const _: () = assert!(
    MIN_TILE
        >= (MAIN_TOOLS + 1) as f32 * theme::ICON_BUTTON
            + 2.0 * STRIP_PAD as f32
            + 2.0 * TILE_BORDER,
    "the main tools and the menu button fit a minimal tile"
);

fn tools(e: &LibraryEntry) -> Vec<Tool> {
    let tool = |icon, label: std::borrow::Cow<'_, str>, action, main| Tool {
        icon,
        label: label.into_owned(),
        action,
        main,
        danger: false,
    };
    let p = &e.path;
    let mut list = vec![
        tool(
            icon::ARROW_SQUARE_OUT,
            t!("lib.open"),
            Action::Open(p.clone()),
            true,
        ),
        tool(
            icon::FOLDER_OPEN,
            t!("lib.reveal"),
            Action::Reveal(p.clone()),
            true,
        ),
        tool(
            icon::COPY,
            t!("lib.copy_file"),
            Action::CopyFile(p.clone()),
            true,
        ),
    ];
    if e.kind == MediaKind::Image {
        list.push(tool(
            icon::IMAGE,
            t!("lib.copy_image"),
            Action::CopyImage(p.clone()),
            false,
        ));
    }
    list.extend([
        tool(
            icon::CLIPBOARD,
            t!("lib.copy_path"),
            Action::CopyPath(p.clone()),
            false,
        ),
        tool(
            icon::LINK,
            t!("lib.copy_link"),
            Action::CopyLink(e.file_url()),
            false,
        ),
        tool(
            icon::SHUFFLE,
            t!("lib.convert"),
            Action::Convert(p.clone()),
            true,
        ),
        tool(
            icon::PENCIL_SIMPLE,
            t!("lib.rename"),
            Action::AskRename(p.clone(), e.title.clone()),
            false,
        ),
        Tool {
            danger: true,
            ..tool(
                icon::TRASH,
                t!("lib.delete"),
                Action::AskDelete(p.clone()),
                false,
            )
        },
    ]);
    list
}

fn tools_strip(ui: &mut egui::Ui, all: &[Tool]) -> Option<Action> {
    let mut action = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        for t in all.iter().filter(|t| t.main) {
            if theme::icon_button(ui, t.icon, &t.label).clicked() {
                action = Some(t.action.clone());
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let more = theme::icon_button(ui, icon::DOTS_THREE_CIRCLE, &t!("lib.more"));
            if let Some(a) = tools_menu(egui::Popup::menu(&more), all) {
                action = Some(a);
            }
        });
    });
    action
}

fn tools_menu(popup: egui::Popup<'_>, tools: &[Tool]) -> Option<Action> {
    popup
        .show(|ui| {
            let widest = tools
                .iter()
                .map(|t| {
                    ui.fonts_mut(|f| {
                        f.layout_no_wrap(
                            t.label.clone(),
                            egui::FontId::proportional(13.5),
                            Color32::WHITE,
                        )
                        .size()
                        .x
                    })
                })
                .fold(0.0, f32::max);
            ui.set_min_width(widest + 52.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut chosen = None;
            for t in tools {
                if t.danger {
                    ui.separator();
                }
                if theme::menu_item(ui, t.icon, &t.label, t.danger).clicked() {
                    chosen = Some(t.action.clone());
                }
            }
            chosen
        })
        .and_then(|r| r.inner)
}

fn sort_label(s: Sort) -> String {
    match s {
        Sort::Newest => t!("lib.sort_newest"),
        Sort::Oldest => t!("lib.sort_oldest"),
        Sort::Title => t!("lib.sort_title"),
        Sort::Largest => t!("lib.sort_largest"),
    }
    .into()
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    if let Some((dir, names)) = app.lib.poll(&app.settings, &mut app.tasks, ui.ctx()) {
        app.lib.previews.retain(&dir, &names);
        app.thumbs.retain(&dir, names);
    }
    app.lib.flush_details();

    let view_w = ui.available_width();
    header(ui, &t!("lib.title"), "");
    let total = app.lib.index.as_ref().map_or(0, LibraryIndex::len);
    if view_w >= TOOLBAR_ONE_LINE {
        ui.horizontal(|ui| {
            kind_filter(app, ui);
            toolbar_tools(app, ui, total, view_w);
        });
    } else {
        ui.horizontal(|ui| kind_filter(app, ui));
        ui.horizontal_wrapped(|ui| toolbar_tools(app, ui, total, view_w));
    }
    ui.add_space(4.0);
    delete_dialog(app, ui.ctx());
    cleanup_dialog(app, ui.ctx());
    rename_dialog(app, ui.ctx());

    let hits = app.lib.hits();
    if hits.as_ref().is_none_or(|hits| hits.entries.is_empty()) {
        app.lib.previews.stop();
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.vertical_centered(|ui| {
                ui.add_space(14.0);
                ui.label(
                    RichText::new(icon::IMAGES)
                        .size(32.0)
                        .color(theme::palette(ui).text_weak),
                );
                ui.label(if total == 0 {
                    t!("lib.empty")
                } else {
                    t!("lib.nothing_found")
                });
                ui.add_space(14.0);
            });
        });
        return;
    }
    let hits = hits.expect("non-empty hits were checked");
    let shown = &hits.entries;

    if !app.lib.dev_viewed {
        app.lib.dev_viewed = true;
        if let Some(e) = mediaa_core::dev::view_filter()
            .and_then(|n| shown.iter().find(|e| !n.is_empty() && e.name.contains(&n)))
        {
            let d = mediaa_media::preview::details(&e.path, e.kind)
                .ok()
                .map(|i| details_of(&i));
            app.lib.viewer = Some(Viewer::open(
                &e.path,
                e.kind,
                d.and_then(|d| d.duration_secs),
                d.is_some_and(|d| d.audio),
                &mut app.tasks,
            ));
        }
    }

    let avail = view_w.min(ui.available_width());
    let (cols, tile_w) = grid_columns(avail);
    let refs: Vec<&LibraryEntry> = shown.iter().collect();
    let groups: Vec<(Option<chrono::NaiveDate>, Vec<&LibraryEntry>)> =
        if app.lib.query.sort.by_date() {
            library::group_by_day(&refs)
                .into_iter()
                .map(|(d, l)| (Some(d), l))
                .collect()
        } else {
            vec![(None, refs)]
        };
    let mut action = None;
    let mut previewing = false;
    for (day, items) in groups {
        if let Some(day) = day {
            date_divider(ui, &day_label(day));
        }
        for row in items.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for e in row {
                    if let Some(a) = tile(app, ui, e, tile_w, &mut previewing) {
                        action = Some(a);
                    }
                }
            });
            ui.add_space(GAP - ui.spacing().item_spacing.y);
        }
    }
    if hits.indices.len() > app.lib.limit {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, 40.0), Sense::hover());
        egui::Spinner::new().paint_at(
            ui,
            Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)),
        );
        if ui.is_rect_visible(rect) {
            app.lib.limit += CHUNK;
            ui.ctx().request_repaint();
        }
    }
    if !previewing {
        app.lib.previews.stop();
    }
    if let Some(a) = action {
        run(app, ui.ctx(), a);
    }
    app.lib.hovered = app.lib.hover_next.take();
    let viewer_action = app
        .lib
        .viewer
        .as_mut()
        .and_then(|viewer| viewer.show(ui.ctx(), &mut app.tasks));
    match viewer_action {
        Some(ViewerAction::Close) => {
            if let Some(mut viewer) = app.lib.viewer.take() {
                viewer.cancel_pending(&mut app.tasks);
            }
        }
        Some(ViewerAction::OpenFile(p)) => {
            if let Some(mut viewer) = app.lib.viewer.take() {
                viewer.cancel_pending(&mut app.tasks);
            }
            run(app, ui.ctx(), Action::Open(p));
        }
        None => {}
    }
    app.lib.flush_details();
}

pub fn poll_cleanup(app: &mut App, ctx: &egui::Context) {
    if let Some((plan, report, success)) = app.lib.poll_cleanup(&mut app.tasks, ctx) {
        apply_cleanup_report(app, &report);
        let success = success.unwrap_or_else(|| {
            t!(
                "lib.cleanup_done",
                count = plan.items.len(),
                size = human_size(plan.bytes())
            )
            .into()
        });
        finish_cleanup(app, report, success);
    }
}

fn details_of(i: &mediaa_media::info::MediaInfo) -> Details {
    Details {
        width: i.width,
        height: i.height,
        duration_secs: i.duration_secs(),
        audio: i.has_audio(),
    }
}

fn day_label(day: chrono::NaiveDate) -> String {
    let today = chrono::Local::now().date_naive();
    if day == today {
        return t!("lib.today").into();
    }
    if Some(day) == today.pred_opt() {
        return t!("lib.yesterday").into();
    }
    let months = t!("lib.months");
    let month = months
        .split(',')
        .nth(day.month0() as usize)
        .unwrap_or_default()
        .trim()
        .to_string();
    t!(
        "lib.date",
        day = day.day(),
        month = month,
        year = day.year()
    )
    .into()
}

fn date_divider(ui: &mut egui::Ui, text: &str) {
    let p = theme::palette(ui);
    ui.add_space(6.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), Sense::hover());
    let galley =
        ui.painter()
            .layout_no_wrap(text.to_string(), FontId::proportional(12.5), p.text_weak);
    let text_rect = Rect::from_center_size(rect.center(), galley.size());
    let stroke = Stroke::new(1.0, p.border);
    let y = rect.center().y;
    ui.painter()
        .hline(rect.left()..=text_rect.left() - 12.0, y, stroke);
    ui.painter()
        .hline(text_rect.right() + 12.0..=rect.right(), y, stroke);
    ui.painter().galley(text_rect.min, galley, p.text_weak);
    ui.add_space(2.0);
}

fn overlay_badge(painter: &egui::Painter, pos: Pos2, anchor: egui::Align2, text: String) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), Color32::WHITE);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = anchor.anchor_size(pos, size);
    painter.rect_filled(rect, CornerRadius::same(5), Color32::from_black_alpha(165));
    painter.galley(rect.min + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
}

mod actions;
#[cfg(test)]
use actions::cleanup_modal;
use actions::{
    apply_cleanup_report, cleanup_dialog, delete_dialog, finish_cleanup, rename_dialog, run,
};

#[cfg(test)]
mod tests;
