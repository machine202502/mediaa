mod cleanup;
mod grid;
mod hover_preview;
pub(crate) mod player;
pub(crate) mod thumbs;
mod viewer;

use crate::app::App;
use crate::jobs::Job;
use crate::pages::header;
use crate::theme;
use chrono::Datelike;
use cleanup::{
    cleanup_old_selected, cleanup_selected, plan_cleanup, CleanupFailure, CleanupItem,
    CleanupOlder, CleanupPlan, CleanupReport,
};
use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;
#[cfg(test)]
use grid::cover_uv;
use grid::{grid_columns, kind_filter, library_tile, toolbar_tools, TOOLBAR_ONE_LINE};
use hover_preview::Previews;
use mediaa_core::library::{self, human_duration, human_size, Details, KindFilter, LibraryEntry};
use mediaa_core::{naming, MediaKind, Settings};
use mediaa_platform::files;
use mediaa_storage::library_index::{self, LibraryIndex, LibraryWatcher, Query, Sort};
use rust_i18n::t;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use thumbs::Thumbnails;
use viewer::{Viewer, ViewerAction};

const STRIP_PAD: i8 = 8;
const MIN_TILE: f32 = 200.0;
const TILE_BORDER: f32 = 1.0;
const GAP: f32 = 10.0;
const MEDIA_ASPECT: f32 = 0.62;
const SCRUB_ZONE: f32 = 26.0;
const CHUNK: usize = 300;
const RESCAN_EVERY: Duration = Duration::from_secs(60);

#[cfg(test)]
type TestTrash = Arc<dyn Fn(&Path) -> anyhow::Result<()> + Send + Sync>;

pub struct LibraryController {
    index: Option<LibraryIndex>,
    load_job: Option<Job<(PathBuf, LibraryIndex)>>,
    watcher: Option<LibraryWatcher>,
    watcher_retry: Instant,
    scan_job: Option<Job<Vec<mediaa_core::library::LibraryEntry>>>,
    changed_while_scanning: Vec<PathBuf>,
    last_scan: Instant,
    query: Query,
    hits: Option<Arc<HitList>>,
    limit: usize,
    confirm_delete: Option<PathBuf>,
    confirm_delete_many: Option<Vec<PathBuf>>,
    confirm_cleanup: Option<CleanupPlan>,
    removal_job: Option<RemovalJob>,
    renaming: Option<(PathBuf, String)>,
    renaming_many: Option<(Vec<PathBuf>, String)>,
    selected: Vec<PathBuf>,
    anchor: Option<PathBuf>,
    last_query: Query,
    hovered: Option<PathBuf>,
    hover_next: Option<PathBuf>,
    previews_enabled: bool,
    empty_cache_purged: bool,
    prewarm_done: bool,
    previews: Previews,
    viewer: Option<Viewer>,
    dev_viewed: bool,
    #[cfg(test)]
    trash: Option<TestTrash>,
}

struct HitList {
    version: u64,
    query: Query,
    limit: usize,
    indices: Arc<[usize]>,
    entries: Arc<[LibraryEntry]>,
}

enum RemovalRequest {
    Selected { item: CleanupItem, success: String },
    SelectedMany(Vec<CleanupItem>),
    OlderThan(CleanupPlan),
}

struct RemovalJob {
    job: Job<CleanupReport>,
    success: String,
}

impl Default for LibraryController {
    fn default() -> Self {
        Self {
            index: None,
            load_job: None,
            watcher: None,
            watcher_retry: Instant::now(),
            scan_job: None,
            changed_while_scanning: Vec::new(),
            last_scan: Instant::now(),
            query: Query::default(),
            hits: None,
            limit: CHUNK,
            confirm_delete: None,
            confirm_delete_many: None,
            confirm_cleanup: None,
            removal_job: None,
            renaming: None,
            renaming_many: None,
            selected: Vec::new(),
            anchor: None,
            last_query: Query::default(),
            hovered: None,
            hover_next: None,
            previews_enabled: true,
            empty_cache_purged: false,
            prewarm_done: false,
            previews: Previews::default(),
            viewer: None,
            dev_viewed: false,
            #[cfg(test)]
            trash: None,
        }
    }
}

pub fn library_dir(settings: &Settings) -> PathBuf {
    mediaa_core::dev::library_dir().unwrap_or_else(|| settings.media_dir.clone())
}

impl LibraryController {
    #[cfg(test)]
    pub(crate) fn set_test_entries(&mut self, dir: &Path) {
        let mut index = LibraryIndex::load(dir);
        index.apply_scan(library_index::scan_dir(dir));
        self.index = Some(index);
        self.hits = None;
        self.last_scan = Instant::now();
    }

    #[cfg(test)]
    pub(crate) fn set_test_trash(&mut self) {
        self.trash = Some(Arc::new(|path| {
            std::fs::remove_file(path)?;
            Ok(())
        }));
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.selected.len()
    }

    fn select_click(&mut self, path: &Path, ctrl: bool, shift: bool) {
        if shift {
            let entries: &[LibraryEntry] = self.hits.as_ref().map_or(&[], |h| &h.entries);
            let anchor = self.anchor.clone().unwrap_or_else(|| path.to_path_buf());
            let (Some(a), Some(b)) = (
                entries.iter().position(|e| e.path == anchor),
                entries.iter().position(|e| e.path == path),
            ) else {
                self.selected = vec![path.to_path_buf()];
                self.anchor = Some(path.to_path_buf());
                return;
            };
            let (from, to) = if a <= b { (a, b) } else { (b, a) };
            self.selected = entries[from..=to].iter().map(|e| e.path.clone()).collect();
            return;
        }
        if ctrl {
            if let Some(index) = self.selected.iter().position(|p| p == path) {
                self.selected.remove(index);
            } else {
                self.selected.push(path.to_path_buf());
            }
            self.anchor = Some(path.to_path_buf());
            return;
        }
        self.selected = vec![path.to_path_buf()];
        self.anchor = Some(path.to_path_buf());
    }

    pub(crate) fn toggle_selected(&mut self, path: &Path) {
        if let Some(index) = self.selected.iter().position(|p| p == path) {
            self.selected.remove(index);
        } else {
            self.selected.push(path.to_path_buf());
        }
        self.anchor = Some(path.to_path_buf());
    }

    fn select_day(&mut self, entries: &[&LibraryEntry]) {
        let all_selected = !entries.is_empty()
            && entries
                .iter()
                .all(|entry| self.selected.iter().any(|path| path == &entry.path));
        if all_selected {
            let paths: Vec<&PathBuf> = entries.iter().map(|entry| &entry.path).collect();
            self.selected.retain(|path| !paths.contains(&path));
            self.anchor = None;
        } else {
            for entry in entries {
                if !self.selected.contains(&entry.path) {
                    self.selected.push(entry.path.clone());
                }
            }
            self.anchor = entries.last().map(|entry| entry.path.clone());
        }
    }

    fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    pub(crate) fn rename_selected(&mut self, old: &Path, new: &Path) {
        for path in &mut self.selected {
            if path == old {
                *path = new.to_path_buf();
            }
        }
        if self.anchor.as_deref() == Some(old) {
            self.anchor = Some(new.to_path_buf());
        }
    }

    pub(crate) fn remove_selected(&mut self, removed: &[PathBuf]) {
        self.selected.retain(|path| !removed.contains(path));
        if self
            .anchor
            .as_ref()
            .is_some_and(|path| removed.contains(path))
        {
            self.anchor = None;
        }
    }
    #[cfg(test)]
    pub(crate) fn selected_for_test(&self) -> Vec<PathBuf> {
        self.selected.clone()
    }

    #[cfg(test)]
    pub(crate) fn select_for_test(&mut self, paths: Vec<PathBuf>) {
        self.selected = paths;
        self.anchor = self.selected.last().cloned();
    }

    #[cfg(test)]
    pub(crate) fn begin_bulk_rename_for_test(&mut self, paths: Vec<PathBuf>) {
        self.renaming_many = Some((paths, String::new()));
    }

    #[cfg(test)]
    pub(crate) fn bulk_rename_title_for_test(&self) -> Option<&str> {
        self.renaming_many.as_ref().map(|(_, title)| title.as_str())
    }

    pub(crate) fn delete_many(&mut self, paths: Vec<PathBuf>) -> anyhow::Result<()> {
        let items = paths
            .into_iter()
            .map(|path| {
                let known = self.index.as_ref().and_then(|index| {
                    index
                        .entries()
                        .iter()
                        .find(|entry| entry.path == path)
                        .map(|entry| (entry.size, entry.modified))
                });
                let metadata = std::fs::metadata(&path).ok();
                CleanupItem {
                    size: known
                        .map(|(size, _)| size)
                        .or_else(|| metadata.as_ref().map(|m| m.len()))
                        .unwrap_or(0),
                    modified: known
                        .map(|(_, modified)| modified)
                        .or_else(|| metadata.and_then(|m| m.modified().ok())),
                    path,
                }
            })
            .collect();
        self.start_removal(RemovalRequest::SelectedMany(items))
    }

    pub fn viewer_open(&self) -> bool {
        self.viewer.is_some()
    }

    pub(crate) fn refresh(&mut self, settings: &Settings) {
        let dir = library_dir(settings);
        if self.scan_job.is_some() {
            if self.index.as_ref().is_some_and(|index| index.dir() == dir) {
                return;
            }
            if let Some(job) = self.scan_job.take() {
                job.cancel();
            }
        }
        self.prewarm_done = false;
        if self.index.as_ref().is_none_or(|i| i.dir() != dir) {
            if self.load_job.is_none() {
                let worker_dir = dir.clone();
                match Job::spawn("library-load", Arc::default(), move || {
                    (worker_dir.clone(), LibraryIndex::load(&worker_dir))
                }) {
                    Ok(job) => self.load_job = Some(job),
                    Err(error) => log::warn!("library load thread: {error}"),
                }
            }
            self.watcher = None;
            self.watcher_retry = Instant::now();
            return;
        }
        let worker_dir = dir.clone();
        let spawned = Job::spawn("library-scan", Arc::default(), move || {
            library_index::scan_dir(&worker_dir)
        });
        self.scan_job = match spawned {
            Ok(job) => Some(job),
            Err(error) => {
                log::warn!("library scan thread: {error}");
                None
            }
        };
        self.changed_while_scanning.clear();
        self.last_scan = Instant::now();
    }

    pub(crate) fn poll_background(
        &mut self,
        settings: &Settings,
        ctx: &egui::Context,
        thumbs: &mut Thumbnails,
    ) {
        if let Some((dir, names)) = self.poll(settings, ctx) {
            self.previews.retain(&dir, &names);
            thumbs.retain(&dir, names);
            self.prewarm_done = false;
        }
        if !self.prewarm_done {
            if let Some(index) = self.index.as_ref() {
                thumbs.prewarm(index.entries());
                self.prewarm_done = true;
            }
        }
    }

    pub fn files_changed(&mut self, paths: &[PathBuf]) {
        self.prewarm_done = false;
        if let Some(index) = self.index.as_mut() {
            index.refresh_paths(paths);
        }
        if self.scan_job.is_some() {
            self.changed_while_scanning.extend_from_slice(paths);
        }
    }

    fn poll(
        &mut self,
        settings: &Settings,
        ctx: &egui::Context,
    ) -> Option<(PathBuf, HashMap<String, SystemTime>)> {
        if self
            .index
            .as_ref()
            .is_none_or(|i| i.dir() != library_dir(settings))
        {
            if let Some(job) = self.load_job.as_mut() {
                match job.poll() {
                    Some(Ok((dir, index))) => {
                        log::debug!("library index loaded from {}", dir.display());
                        self.index = Some(index);
                        self.load_job = None;
                        self.refresh(settings);
                    }
                    None => {
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                    Some(Err(error)) => {
                        self.load_job = None;
                        log::warn!("library index loader stopped: {error:#}");
                    }
                }
                return None;
            }
            self.refresh(settings);
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
            if self.scan_job.is_some() {
                self.changed_while_scanning.extend(c.paths);
            }
            rescan = c.rescan;
        }
        if (rescan || self.last_scan.elapsed() > RESCAN_EVERY) && self.scan_job.is_none() {
            if rescan {
                self.watcher = None;
            }
            self.refresh(settings);
            return None;
        }
        let index = self.index.as_mut()?;
        if let Some(job) = self.scan_job.as_mut() {
            match job.poll() {
                Some(Ok(listing)) => {
                    index.apply_scan(listing);
                    index.refresh_paths(&std::mem::take(&mut self.changed_while_scanning));
                    self.scan_job = None;
                    scanned = Some((
                        index.dir().to_path_buf(),
                        index
                            .entries()
                            .iter()
                            .map(|entry| (entry.name.clone(), entry.modified))
                            .collect(),
                    ));
                }
                None => ctx.request_repaint_after(Duration::from_millis(50)),
                Some(Err(error)) => {
                    self.scan_job = None;
                    log::warn!("library scan stopped: {error:#}");
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
        plan_cleanup(entries, older, SystemTime::now())
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
        self.removal_job.is_some()
    }

    fn start_cleanup(&mut self, plan: &CleanupPlan) -> anyhow::Result<()> {
        self.start_removal(RemovalRequest::OlderThan(plan.clone()))
    }

    fn start_delete(&mut self, path: PathBuf) -> anyhow::Result<()> {
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        let item = CleanupItem {
            path,
            size: 0,
            modified,
        };
        let success = t!(
            "lib.deleted",
            name = item.path.file_name().unwrap_or_default().to_string_lossy()
        )
        .into_owned();
        self.start_removal(RemovalRequest::Selected { item, success })
    }

    fn start_removal(&mut self, request: RemovalRequest) -> anyhow::Result<()> {
        if self.cleanup_active() {
            anyhow::bail!("cleanup is already running");
        }
        let (items, success) = match &request {
            RemovalRequest::Selected { item, success } => (vec![item.clone()], success.clone()),
            RemovalRequest::SelectedMany(items) => (
                items.clone(),
                t!(
                    "lib.deleted_many",
                    count = items.len(),
                    files = crate::i18n::files(items.len())
                )
                .into_owned(),
            ),
            RemovalRequest::OlderThan(plan) => (
                plan.items.clone(),
                t!(
                    "lib.cleanup_done",
                    count = plan.items.len(),
                    size = human_size(plan.bytes())
                )
                .into_owned(),
            ),
        };
        self.prepare_for_removal(&items);
        let worker_request = request;
        #[cfg(test)]
        let trash = self.trash.clone();
        let job = Job::spawn("library-removal", Arc::default(), move || {
            let delete = move |path: &Path| -> anyhow::Result<()> {
                if !path.exists() {
                    return Ok(());
                }
                #[cfg(test)]
                if let Some(trash) = &trash {
                    return trash(path);
                }
                files::move_to_trash(path)
            };
            match worker_request {
                RemovalRequest::Selected { item, .. } => cleanup_selected(&[item], delete),
                RemovalRequest::SelectedMany(items) => cleanup_selected(&items, delete),
                RemovalRequest::OlderThan(plan) => {
                    cleanup_old_selected(&plan.items, plan.older, SystemTime::now(), delete)
                }
            }
        })?;
        self.removal_job = Some(RemovalJob { job, success });
        Ok(())
    }

    fn poll_cleanup(&mut self, ctx: &egui::Context) -> Option<(CleanupReport, String)> {
        let job = self.removal_job.as_mut()?;
        let Some(result) = job.job.poll() else {
            ctx.request_repaint_after(Duration::from_millis(100));
            return None;
        };
        let job = self.removal_job.take().expect("removal job checked");
        Some((
            result.unwrap_or_else(|_| CleanupReport {
                failures: vec![CleanupFailure {
                    path: PathBuf::new(),
                    error: "cleanup worker stopped".into(),
                }],
                ..CleanupReport::default()
            }),
            job.success,
        ))
    }
}

#[derive(Clone)]
pub(crate) enum LibraryAction {
    Open(PathBuf),
    View(PathBuf, MediaKind, Option<Details>),
    Reveal(PathBuf),
    CopyFile(PathBuf),
    CopyFiles(Vec<PathBuf>),
    CopyImage(PathBuf),
    CopyPath(PathBuf),
    CopyPaths(Vec<PathBuf>),
    CopyLink(String),
    Convert(PathBuf),
    ConvertMany(Vec<PathBuf>),
    AskDelete(PathBuf),
    AskDeleteMany(Vec<PathBuf>),
    AskRename(PathBuf, String),
    AskRenameMany(Vec<PathBuf>),
    ToggleSelect(PathBuf),
    SetViewerVolume(f32),
    AskCleanup(CleanupPlan),
}

struct Tool {
    icon: &'static str,
    label: String,
    action: LibraryAction,
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
            LibraryAction::Open(p.clone()),
            true,
        ),
        tool(
            icon::FOLDER_OPEN,
            t!("lib.reveal"),
            LibraryAction::Reveal(p.clone()),
            true,
        ),
        tool(
            icon::COPY,
            t!("lib.copy_file"),
            LibraryAction::CopyFile(p.clone()),
            true,
        ),
    ];
    if e.kind == MediaKind::Image {
        list.push(tool(
            icon::IMAGE,
            t!("lib.copy_image"),
            LibraryAction::CopyImage(p.clone()),
            false,
        ));
    }
    list.extend([
        tool(
            icon::CLIPBOARD,
            t!("lib.copy_path"),
            LibraryAction::CopyPath(p.clone()),
            false,
        ),
        tool(
            icon::LINK,
            t!("lib.copy_link"),
            LibraryAction::CopyLink(e.file_url()),
            false,
        ),
        tool(
            icon::SHUFFLE,
            t!("lib.convert"),
            LibraryAction::Convert(p.clone()),
            true,
        ),
        tool(
            icon::PENCIL_SIMPLE,
            t!("lib.rename"),
            LibraryAction::AskRename(p.clone(), e.title.clone()),
            false,
        ),
        tool(
            icon::CHECK,
            t!("lib.select"),
            LibraryAction::ToggleSelect(p.clone()),
            false,
        ),
        Tool {
            danger: true,
            ..tool(
                icon::TRASH,
                t!("lib.delete"),
                LibraryAction::AskDelete(p.clone()),
                false,
            )
        },
    ]);
    list
}

fn tools_strip(ui: &mut egui::Ui, all: &[Tool]) -> (Option<LibraryAction>, bool) {
    let mut action = None;
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
        for t in all.iter().filter(|t| t.main) {
            if theme::icon_button(ui, t.icon, &t.label).clicked() {
                action = Some(t.action.clone());
                clicked = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let more = theme::icon_button(ui, icon::DOTS_THREE_CIRCLE, &t!("lib.more"));
            clicked |= more.clicked();
            if let Some(a) = tools_menu(egui::Popup::menu(&more), all) {
                action = Some(a);
                clicked = true;
            }
        });
    });
    (action, clicked)
}

fn tools_menu(popup: egui::Popup<'_>, tools: &[Tool]) -> Option<LibraryAction> {
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

impl LibraryController {
    pub(crate) fn stop_previews(&mut self) {
        self.previews.stop();
    }

    pub(crate) fn ui(
        &mut self,
        settings: &Settings,
        thumbs: &mut Thumbnails,
        ui: &mut egui::Ui,
    ) -> Vec<LibraryAction> {
        self.previews_enabled = settings.animated_previews;
        if let Some(index) = self.index.as_ref() {
            let empty = index.is_empty();
            if empty && !self.empty_cache_purged {
                let dir = index.dir().to_path_buf();
                mediaa_media::preview_cache::clear_previews(&dir, true, true);
                thumbs.clear_cache(&dir);
                self.empty_cache_purged = true;
            } else if !empty {
                self.empty_cache_purged = false;
            }
        }
        let mut actions = Vec::new();
        if let Some((dir, names)) = self.poll(settings, ui.ctx()) {
            self.previews.retain(&dir, &names);
            thumbs.retain(&dir, names);
        }
        self.flush_details();

        let view_w = ui.available_width();
        header(ui, &t!("lib.title"), "");
        let total = self.index.as_ref().map_or(0, LibraryIndex::len);
        if view_w >= TOOLBAR_ONE_LINE {
            ui.horizontal(|ui| {
                kind_filter(self, ui);
                actions.extend(toolbar_tools(self, settings, ui, total, view_w));
            });
        } else {
            ui.horizontal(|ui| kind_filter(self, ui));
            ui.horizontal_wrapped(|ui| {
                actions.extend(toolbar_tools(self, settings, ui, total, view_w));
            });
        }
        if self.selected_count() > 0 {
            let paths = self.selected.clone();
            let label = t!("lib.selected", n = paths.len());
            let ctx = ui.ctx().clone();
            egui::Area::new(egui::Id::new("library-bulk-floating"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 54.0))
                .show(&ctx, |ui| {
                    let p = theme::palette(ui);
                    egui::Frame::new()
                        .fill(p.card)
                        .stroke(egui::Stroke::new(1.0, p.border))
                        .corner_radius(egui::CornerRadius::same(10))
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 4],
                            blur: 12,
                            spread: 0,
                            color: egui::Color32::from_black_alpha(48),
                        })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let button = ui.add(theme::primary(ui, icon::CHECK, &label));
                                if theme::icon_button(ui, icon::X, &t!("lib.clear_selection"))
                                    .clicked()
                                {
                                    self.clear_selection();
                                }
                                egui::Popup::menu(&button).show(|ui| {
                                    if theme::menu_item(
                                        ui,
                                        icon::COPY,
                                        &t!("lib.copy_files"),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        actions.push(LibraryAction::CopyFiles(paths.clone()));
                                        ui.close();
                                    }
                                    if theme::menu_item(
                                        ui,
                                        icon::CLIPBOARD,
                                        &t!("lib.copy_paths"),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        actions.push(LibraryAction::CopyPaths(paths.clone()));
                                        ui.close();
                                    }
                                    if theme::menu_item(
                                        ui,
                                        icon::SHUFFLE,
                                        &t!("lib.convert"),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        actions.push(LibraryAction::ConvertMany(paths.clone()));
                                        ui.close();
                                    }
                                    if theme::menu_item(
                                        ui,
                                        icon::PENCIL_SIMPLE,
                                        &t!("lib.rename"),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        actions.push(LibraryAction::AskRenameMany(paths.clone()));
                                        ui.close();
                                    }
                                    if theme::menu_item(
                                        ui,
                                        icon::X,
                                        &t!("lib.clear_selection"),
                                        false,
                                    )
                                    .clicked()
                                    {
                                        self.clear_selection();
                                        ui.close();
                                    }
                                    ui.separator();
                                    if theme::menu_item(ui, icon::TRASH, &t!("lib.delete"), true)
                                        .clicked()
                                    {
                                        actions.push(LibraryAction::AskDeleteMany(paths.clone()));
                                        ui.close();
                                    }
                                });
                            });
                        });
                });
        }
        ui.add_space(4.0);

        if self.last_query != self.query {
            self.clear_selection();
            self.last_query = self.query.clone();
        }
        let hits = self.hits();
        if hits.as_ref().is_none_or(|hits| hits.entries.is_empty()) {
            self.previews.stop();
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
            return actions;
        }
        let hits = hits.expect("non-empty hits were checked");
        let shown = &hits.entries;

        if !self.dev_viewed {
            self.dev_viewed = true;
            if let Some(e) = mediaa_core::dev::view_filter()
                .and_then(|n| shown.iter().find(|e| !n.is_empty() && e.name.contains(&n)))
            {
                let d = mediaa_media::preview::details(&e.path, e.kind)
                    .ok()
                    .map(|i| details_of(&i));
                self.viewer = Some(Viewer::open(
                    &e.path,
                    e.kind,
                    d.and_then(|d| d.duration_secs),
                    d.is_some_and(|d| d.audio),
                    settings.viewer_volume,
                ));
            }
        }

        let avail = view_w.min(ui.available_width());
        let (cols, tile_w) = grid_columns(avail);
        let refs: Vec<&LibraryEntry> = shown.iter().collect();
        let groups: Vec<(Option<chrono::NaiveDate>, Vec<&LibraryEntry>)> =
            if self.query.sort.by_date() {
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
                if date_divider(ui, &day_label(day)) {
                    self.select_day(&items);
                }
            }
            for row in items.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for e in row {
                        if let Some(a) = library_tile(self, thumbs, ui, e, tile_w, &mut previewing)
                        {
                            action = Some(a);
                        }
                    }
                });
                ui.add_space(GAP - ui.spacing().item_spacing.y);
            }
        }
        if hits.indices.len() > self.limit {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(avail, 40.0), Sense::hover());
            egui::Spinner::new().paint_at(
                ui,
                Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)),
            );
            if ui.is_rect_visible(rect) {
                self.limit += CHUNK;
                ui.ctx().request_repaint();
            }
        }
        if !previewing {
            self.previews.stop();
        }
        if let Some(a) = action {
            actions.push(a);
        }
        self.hovered = self.hover_next.take();
        let viewer_action = self
            .viewer
            .as_mut()
            .and_then(|viewer| viewer.show(ui.ctx()));
        match viewer_action {
            Some(ViewerAction::Close) => {
                if let Some(mut viewer) = self.viewer.take() {
                    viewer.cancel_pending();
                }
            }
            Some(ViewerAction::OpenFile(p)) => {
                if let Some(mut viewer) = self.viewer.take() {
                    viewer.cancel_pending();
                }
                actions.push(LibraryAction::Open(p));
            }
            Some(ViewerAction::Volume(volume)) => {
                actions.push(LibraryAction::SetViewerVolume(volume));
            }
            None => {}
        }
        self.flush_details();
        actions
    }
}

pub fn poll_cleanup(app: &mut App, ctx: &egui::Context) {
    if let Some((report, success)) = app.lib.poll_cleanup(ctx) {
        apply_cleanup_report(app, &report);
        finish_cleanup(app, report, success);
    }
}

pub(crate) fn dialogs(app: &mut App, ctx: &egui::Context) {
    delete_dialog(app, ctx);
    delete_many_dialog(app, ctx);
    cleanup_dialog(app, ctx);
    rename_dialog(app, ctx);
    rename_many_dialog(app, ctx);
}

pub(crate) fn apply_action(app: &mut App, ctx: &egui::Context, action: LibraryAction) {
    run_action(app, ctx, action);
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

fn date_divider(ui: &mut egui::Ui, text: &str) -> bool {
    let p = theme::palette(ui);
    ui.add_space(6.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), Sense::click());
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
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.add_space(2.0);
    response.clicked()
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
    apply_cleanup_report, cleanup_dialog, delete_dialog, delete_many_dialog, finish_cleanup,
    rename_dialog, rename_many_dialog, run_action,
};

#[cfg(test)]
mod tests;
