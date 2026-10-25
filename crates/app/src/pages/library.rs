use super::header;
use crate::app::App;
use crate::hover_preview::Previews;
use crate::theme;
use crate::viewer::{Viewer, ViewerAction};
use chrono::Datelike;
use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;
use mediaa_core::library::{self, human_duration, human_size, Details, KindFilter, LibraryEntry};
use mediaa_core::library_index::{self, LibraryIndex, LibraryWatcher, Query, Sort};
use mediaa_core::{naming, MediaKind, Settings};
use mediaa_platform::{clipboard, files};
use rust_i18n::t;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

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
    watcher: Option<LibraryWatcher>,
    scan: Option<crossbeam_channel::Receiver<Vec<mediaa_core::library::LibraryEntry>>>,
    changed_while_scanning: Vec<PathBuf>,
    last_scan: Instant,
    query: Query,
    hits: Option<(u64, Query, Vec<usize>)>,
    limit: usize,
    confirm_delete: Option<PathBuf>,
    confirm_cleanup: Option<CleanupPlan>,
    renaming: Option<(PathBuf, String)>,
    hovered: Option<PathBuf>,
    hover_next: Option<PathBuf>,
    previews: Previews,
    viewer: Option<Viewer>,
    dev_viewed: bool,
}

impl Default for LibraryUi {
    fn default() -> Self {
        Self {
            index: None,
            watcher: None,
            scan: None,
            changed_while_scanning: Vec::new(),
            last_scan: Instant::now(),
            query: Query::default(),
            hits: None,
            limit: CHUNK,
            confirm_delete: None,
            confirm_cleanup: None,
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
    std::env::var_os("MEDIAA_DEV_LIBRARY").map_or_else(|| settings.media_dir.clone(), PathBuf::from)
}

impl LibraryUi {
    pub fn viewer_open(&self) -> bool {
        self.viewer.is_some()
    }

    pub fn refresh(&mut self, settings: &Settings) {
        let dir = library_dir(settings);
        if self.index.as_ref().is_none_or(|i| i.dir() != dir) {
            self.index = Some(LibraryIndex::load(&dir));
            self.watcher = None;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        std::thread::Builder::new()
            .name("library-scan".into())
            .spawn(move || {
                let _ = tx.send(library_index::scan_dir(&dir));
            })
            .expect("spawn library scan");
        self.scan = Some(rx);
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
        ctx: &egui::Context,
    ) -> Option<(PathBuf, HashSet<String>)> {
        if self
            .index
            .as_ref()
            .is_none_or(|i| i.dir() != library_dir(settings))
        {
            self.refresh(settings);
        }
        let index = self.index.as_mut()?;
        let mut scanned = None;
        if self.watcher.is_none() {
            let c = ctx.clone();
            self.watcher = LibraryWatcher::new(index.dir(), move || c.request_repaint())
                .map_err(|e| log::warn!("library watcher: {e:#}"))
                .ok();
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
            self.refresh(settings);
            return None;
        }
        let index = self.index.as_mut()?;
        if let Some(rx) = &self.scan {
            match rx.try_recv() {
                Ok(listing) => {
                    index.apply_scan(listing);
                    index.refresh_paths(&std::mem::take(&mut self.changed_while_scanning));
                    self.scan = None;
                    scanned = Some((
                        index.dir().to_path_buf(),
                        index.entries().iter().map(|e| e.name.clone()).collect(),
                    ));
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    ctx.request_repaint_after(Duration::from_millis(50))
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => self.scan = None,
            }
        }
        scanned
    }

    fn hits(&mut self) -> Vec<usize> {
        let Some(index) = self.index.as_ref() else {
            return Vec::new();
        };
        let fresh = self
            .hits
            .as_ref()
            .is_some_and(|(v, q, _)| *v == index.version() && *q == self.query);
        if !fresh {
            if self.hits.as_ref().is_none_or(|(_, q, _)| *q != self.query) {
                self.limit = CHUNK;
            }
            self.hits = Some((
                index.version(),
                self.query.clone(),
                index.query(&self.query),
            ));
        }
        self.hits
            .as_ref()
            .map(|(_, _, h)| h.clone())
            .unwrap_or_default()
    }

    fn set_details(&mut self, path: &Path, details: Details) {
        if let Some(i) = self.index.as_mut() {
            i.set_details(path, details);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CleanupOlder {
    Week,
    Month,
}

impl CleanupOlder {
    fn days(self) -> u64 {
        match self {
            CleanupOlder::Week => 7,
            CleanupOlder::Month => 30,
        }
    }

    fn cutoff(self, now: SystemTime) -> SystemTime {
        now.checked_sub(Duration::from_secs(self.days() * 24 * 60 * 60))
            .unwrap_or(UNIX_EPOCH)
    }

    fn label(self) -> String {
        match self {
            CleanupOlder::Week => t!("lib.cleanup_week"),
            CleanupOlder::Month => t!("lib.cleanup_month"),
        }
        .into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CleanupItem {
    path: PathBuf,
    size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct CleanupPlan {
    older: CleanupOlder,
    items: Vec<CleanupItem>,
}

impl CleanupPlan {
    fn bytes(&self) -> u64 {
        self.items.iter().map(|item| item.size).sum()
    }
}

#[derive(Debug)]
struct RemovedItem {
    path: PathBuf,
    size: u64,
    proxy: PathBuf,
}

#[derive(Debug)]
struct CleanupFailure {
    path: PathBuf,
    error: String,
}

#[derive(Debug, Default)]
struct CleanupReport {
    removed: Vec<RemovedItem>,
    failures: Vec<CleanupFailure>,
}

impl CleanupReport {
    fn bytes(&self) -> u64 {
        self.removed.iter().map(|item| item.size).sum()
    }
}

fn cleanup_items(
    entries: &[LibraryEntry],
    older: CleanupOlder,
    now: SystemTime,
) -> Vec<CleanupItem> {
    let cutoff = older.cutoff(now);
    entries
        .iter()
        .filter(|entry| entry.modified < cutoff)
        .map(|entry| CleanupItem {
            path: entry.path.clone(),
            size: entry.size,
        })
        .collect()
}

fn cleanup_plan(entries: &[LibraryEntry], older: CleanupOlder, now: SystemTime) -> CleanupPlan {
    CleanupPlan {
        older,
        items: cleanup_items(entries, older, now),
    }
}

fn cleanup_selected(
    items: &[CleanupItem],
    mut move_to_trash: impl FnMut(&Path) -> anyhow::Result<()>,
) -> CleanupReport {
    let mut report = CleanupReport::default();
    for item in items {
        let proxy = mediaa_media::preview_cache::proxy_path(&item.path);
        match move_to_trash(&item.path) {
            Ok(()) => {
                report.removed.push(RemovedItem {
                    path: item.path.clone(),
                    size: item.size,
                    proxy: proxy.clone(),
                });
                if let Err(error) = std::fs::remove_file(&proxy) {
                    if error.kind() != std::io::ErrorKind::NotFound {
                        report.failures.push(CleanupFailure {
                            path: item.path.clone(),
                            error: format!("preview: {error}"),
                        });
                    }
                }
            }
            Err(error) => report.failures.push(CleanupFailure {
                path: item.path.clone(),
                error: format!("{error:#}"),
            }),
        }
    }
    report
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
    if let Some((dir, names)) = app.lib.poll(&app.settings, ui.ctx()) {
        app.thumbs.retain(&dir, names);
    }

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
    if hits.is_empty() {
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
    let shown: Vec<LibraryEntry> = {
        let entries = app
            .lib
            .index
            .as_ref()
            .map(LibraryIndex::entries)
            .unwrap_or_default();
        hits.iter()
            .take(app.lib.limit)
            .map(|&i| entries[i].clone())
            .collect()
    };

    if !app.lib.dev_viewed {
        app.lib.dev_viewed = true;
        if let Some(e) = std::env::var("MEDIAA_DEV_VIEW")
            .ok()
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
    if hits.len() > app.lib.limit {
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
        run(app, a);
    }
    app.lib.hovered = app.lib.hover_next.take();
    if let Some(v) = app.lib.viewer.as_mut() {
        match v.show(ui.ctx()) {
            Some(ViewerAction::Close) => app.lib.viewer = None,
            Some(ViewerAction::OpenFile(p)) => {
                app.lib.viewer = None;
                run(app, Action::Open(p));
            }
            None => {}
        }
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

fn cover_uv(tex: egui::Vec2, target: egui::Vec2) -> Rect {
    let (ta, a) = (tex.x / tex.y.max(1.0), target.x / target.y.max(1.0));
    if ta > a {
        let w = a / ta;
        Rect::from_min_max(
            egui::pos2((1.0 - w) / 2.0, 0.0),
            egui::pos2((1.0 + w) / 2.0, 1.0),
        )
    } else {
        let h = ta / a;
        Rect::from_min_max(
            egui::pos2(0.0, (1.0 - h) / 2.0),
            egui::pos2(1.0, (1.0 + h) / 2.0),
        )
    }
}

const TOOLBAR_ONE_LINE: f32 = 1380.0;

fn kind_filter(app: &mut App, ui: &mut egui::Ui) {
    theme::segmented(
        ui,
        &mut app.lib.query.kind,
        &[
            (KindFilter::All, t!("lib.all").into()),
            (KindFilter::Only(MediaKind::Image), t!("lib.images").into()),
            (KindFilter::Only(MediaKind::Video), t!("lib.videos").into()),
            (
                KindFilter::Only(MediaKind::Animation),
                t!("lib.animations").into(),
            ),
            (KindFilter::Only(MediaKind::Audio), t!("lib.audio").into()),
        ],
    );
}

fn toolbar_tools(app: &mut App, ui: &mut egui::Ui, total: usize, view_w: f32) {
    ui.add(
        egui::TextEdit::singleline(&mut app.lib.query.search)
            .hint_text(format!("{}  {}", icon::MAGNIFYING_GLASS, t!("lib.search")))
            .desired_width((view_w - 260.0).clamp(120.0, 220.0)),
    )
    .on_hover_text(t!("lib.search_help"));
    let sort = &mut app.lib.query.sort;
    egui::ComboBox::from_id_salt("lib-sort")
        .width(150.0)
        .selected_text(sort_label(*sort))
        .show_ui(ui, |ui| {
            for s in Sort::ALL {
                ui.selectable_value(sort, s, sort_label(s));
            }
        });
    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked() {
        app.lib.refresh(&app.settings);
    }
    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("lib.open_folder")).clicked() {
        let dir = library_dir(&app.settings);
        let _ = std::fs::create_dir_all(&dir);
        app.open_path(&dir);
    }
    cleanup_button(app, ui, CleanupOlder::Week);
    cleanup_button(app, ui, CleanupOlder::Month);
    theme::hint(ui, t!("lib.count", n = total));
}

fn cleanup_button(app: &mut App, ui: &mut egui::Ui, older: CleanupOlder) {
    let plan = app.lib.cleanup_plan(older);
    let count = plan.items.len();
    let tooltip = if count == 0 {
        t!("lib.cleanup_empty")
    } else {
        t!(
            "lib.cleanup_available",
            count = count,
            size = human_size(plan.bytes())
        )
    };
    let button = ui.add_enabled(
        count > 0,
        theme::secondary(icon::CALENDAR_X, &older.label()),
    );
    if button.on_hover_text(tooltip).clicked() {
        app.lib.ask_cleanup(older);
    }
}

fn grid_columns(avail: f32) -> (usize, f32) {
    let cols = (((avail + GAP) / (MIN_TILE + GAP)).floor() as usize).max(1);
    let tile_w = ((avail - GAP * (cols - 1) as f32) / cols as f32)
        .floor()
        .max(1.0);
    (cols, tile_w)
}

fn tile(
    app: &mut App,
    ui: &mut egui::Ui,
    e: &LibraryEntry,
    width: f32,
    previewing: &mut bool,
) -> Option<Action> {
    let width = width - 2.0 * TILE_BORDER;
    let p = theme::palette(ui);
    let mut action = None;
    let mut media_resp = None;
    let mut name_resp: Option<egui::Response> = None;
    let tile_rect = Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(width, width * MEDIA_ASPECT + 70.0),
    );
    let data = if ui.is_rect_visible(tile_rect) {
        app.thumbs.get(e, e.details.is_none())
    } else {
        Default::default()
    };
    let info: Option<Details> = e.details.or_else(|| data.info.as_ref().map(details_of));
    if e.details.is_none() {
        if let Some(d) = info {
            app.lib.set_details(&e.path, d);
        }
    }
    let hovered = app.lib.hovered.as_ref() == Some(&e.path);
    let shown = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(TILE_BORDER, p.border))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.set_max_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let size = egui::vec2(width, (width * MEDIA_ASPECT).round());
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
                media_resp = Some(resp.clone());
                let painter = ui.painter_at(rect);
                let top = CornerRadius {
                    nw: 10,
                    ne: 10,
                    sw: 0,
                    se: 0,
                };
                painter.rect_filled(rect, top, p.card_hover);

                let playable = matches!(e.kind, MediaKind::Video | MediaKind::Animation);
                let dev = std::env::var("MEDIAA_DEV_PREVIEW")
                    .is_ok_and(|n| !n.is_empty() && e.name.contains(&n));
                let active = resp.hovered() || resp.dragged() || dev;
                let mut texture = data.texture.clone();
                let mut timeline = None;
                let mut playing_at = None;
                let mut loading = false;
                if playable && active {
                    *previewing = true;
                    let pv = app.lib.previews.play(
                        ui.ctx(),
                        &e.path,
                        info.and_then(|i| i.duration_secs),
                    );
                    let scrub = Rect::from_min_max(
                        egui::pos2(rect.left(), rect.bottom() - SCRUB_ZONE),
                        rect.max,
                    );
                    if resp.is_pointer_button_down_on() {
                        let origin = ui.input(|i| i.pointer.press_origin());
                        if let (Some(o), Some(now)) = (origin, resp.interact_pointer_pos()) {
                            if scrub.contains(o) {
                                let frac = ((now.x - rect.left() - 8.0) / (rect.width() - 16.0))
                                    .clamp(0.0, 1.0);
                                pv.seek(f64::from(frac) * pv.length());
                            }
                        }
                    }
                    if let Some(t) = pv.frame() {
                        texture = Some(t.clone());
                    }
                    loading = pv.is_loading();
                    playing_at = Some(pv.position());
                    timeline = Some((pv.position() / pv.length()) as f32);
                }

                match &texture {
                    Some(t) => {
                        let uv = cover_uv(t.size_vec2(), rect.size());
                        egui::Image::new((t.id(), rect.size()))
                            .uv(uv)
                            .corner_radius(top)
                            .paint_at(ui, rect);
                    }
                    None => {
                        let ic = match e.kind {
                            MediaKind::Image => icon::IMAGE,
                            MediaKind::Video => icon::FILE_VIDEO,
                            MediaKind::Animation => icon::GIF,
                            MediaKind::Audio => icon::MUSIC_NOTES,
                        };
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            ic,
                            FontId::proportional(32.0),
                            p.text_weak,
                        );
                    }
                }
                if loading {
                    let spinner = Rect::from_center_size(rect.center(), egui::vec2(28.0, 28.0));
                    egui::Spinner::new()
                        .color(Color32::WHITE)
                        .paint_at(ui, spinner);
                }

                let pad = 7.0;
                overlay_badge(
                    &painter,
                    rect.left_top() + egui::vec2(pad, pad),
                    egui::Align2::LEFT_TOP,
                    e.extension(),
                );
                overlay_badge(
                    &painter,
                    rect.right_top() + egui::vec2(-pad, pad),
                    egui::Align2::RIGHT_TOP,
                    human_size(e.size),
                );
                let bottom = rect.bottom() - pad - if timeline.is_some() { 14.0 } else { 0.0 };
                if let Some(i) = info.filter(|i| i.width > 0) {
                    overlay_badge(
                        &painter,
                        egui::pos2(rect.left() + pad, bottom),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{}×{}", i.width, i.height),
                    );
                }
                if e.kind != MediaKind::Image {
                    let mut text = String::from(icon::PLAY);
                    if let Some(d) = playing_at
                        .map(f64::floor)
                        .or_else(|| info.and_then(|i| i.duration_secs))
                    {
                        text = format!("{text} {}", human_duration(d));
                    }
                    if info.is_some_and(|i| i.audio) {
                        text = format!("{text}  {}", icon::SPEAKER_HIGH);
                    }
                    overlay_badge(
                        &painter,
                        egui::pos2(rect.right() - pad, bottom),
                        egui::Align2::RIGHT_BOTTOM,
                        text,
                    );
                }
                if let Some(frac) = timeline {
                    let y = rect.bottom() - 9.0;
                    let (l, r) = (rect.left() + 8.0, rect.right() - 8.0);
                    let track = Rect::from_min_max(egui::pos2(l, y - 2.0), egui::pos2(r, y + 2.0));
                    painter.rect_filled(
                        track,
                        CornerRadius::same(2),
                        Color32::from_white_alpha(90),
                    );
                    let x = l + (r - l) * frac;
                    painter.rect_filled(
                        Rect::from_min_max(track.min, egui::pos2(x, track.max.y)),
                        CornerRadius::same(2),
                        p.accent,
                    );
                    painter.circle_filled(egui::pos2(x, y), 6.0, Color32::WHITE);
                    painter.circle_stroke(egui::pos2(x, y), 6.0, Stroke::new(1.5, p.accent));
                }
                if resp.double_clicked() {
                    action = Some(Action::View(e.path.clone(), e.kind, info));
                }

                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(STRIP_PAD, 6))
                    .show(ui, |ui| {
                        let inner = width - 2.0 * f32::from(STRIP_PAD);
                        ui.set_width(inner);
                        ui.set_max_width(inner);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let label = if hovered {
                            RichText::new(&e.name).size(12.5).color(p.text_weak)
                        } else {
                            RichText::new(&e.title).size(12.5)
                        };
                        name_resp =
                            Some(ui.add(egui::Label::new(label).truncate().sense(Sense::click())));
                        if let Some(a) = tools_strip(ui, &tools(e)) {
                            action = Some(a);
                        }
                    });
            })
        });
    if let Some(target) = media_resp
        .into_iter()
        .chain(name_resp)
        .reduce(|a, b| a.union(b))
    {
        if let Some(a) = tools_menu(egui::Popup::context_menu(&target), &tools(e)) {
            action = Some(a);
        }
    }
    if shown.response.contains_pointer() {
        app.lib.hover_next = Some(e.path.clone());
    }
    action
}

fn run(app: &mut App, action: Action) {
    let res: anyhow::Result<Option<String>> = match action {
        Action::Open(p) => files::open(&p).map(|_| None),
        Action::View(p, kind, info) => {
            app.lib.previews.stop();
            app.lib.viewer = Some(Viewer::open(
                &p,
                kind,
                info.and_then(|d| d.duration_secs),
                info.is_some_and(|d| d.audio),
            ));
            Ok(None)
        }
        Action::Reveal(p) => files::reveal(&p).map(|_| None),
        Action::CopyFile(p) => {
            clipboard::copy_files(&[&p]).map(|_| Some(t!("lib.copied_file").into()))
        }
        Action::CopyImage(p) => image::open(&p)
            .map_err(anyhow::Error::from)
            .and_then(|img| clipboard::copy_image(&mediaa_core::Frame::from_image(img.to_rgba8())))
            .map(|_| Some(t!("lib.copied_image").into())),
        Action::CopyPath(p) => {
            clipboard::copy_text(&p.to_string_lossy()).map(|_| Some(t!("lib.copied_path").into()))
        }
        Action::CopyLink(url) => {
            clipboard::copy_text(&url).map(|_| Some(t!("lib.copied_link").into()))
        }
        Action::Convert(p) => {
            app.open_in_convert(p);
            Ok(None)
        }
        Action::AskDelete(p) => {
            app.lib.confirm_delete = Some(p);
            Ok(None)
        }
        Action::AskRename(p, title) => {
            app.lib.renaming = Some((p, title));
            Ok(None)
        }
    };
    match res {
        Ok(Some(msg)) => app.success(msg),
        Ok(None) => {}
        Err(e) => app.error(format!("{e:#}")),
    }
}

fn delete_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(path) = app.lib.confirm_delete.clone() else {
        return;
    };
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("confirm-delete")).show(ctx, |ui| {
        ui.set_width(340.0);
        theme::popup_title(ui, &t!("lib.delete_title"));
        ui.label(t!("lib.delete_text", name = name));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add(theme::danger(ui, icon::TRASH, &t!("lib.delete_confirm")))
                .clicked()
            {
                decision = Some(true);
            }
            if ui
                .add(theme::secondary(icon::X, &t!("lib.delete_keep")))
                .clicked()
            {
                decision = Some(false);
            }
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => {
            app.lib.confirm_delete = None;
            let size = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or_default();
            let report = perform_cleanup(
                app,
                &[CleanupItem {
                    path: path.clone(),
                    size,
                }],
            );
            finish_cleanup(app, report, t!("lib.deleted", name = name).into());
        }
        Some(false) => app.lib.confirm_delete = None,
        None => {}
    }
}

fn cleanup_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(plan) = app.lib.confirm_cleanup.clone() else {
        return;
    };
    match cleanup_modal(ctx, &plan) {
        Some(true) => {
            app.lib.confirm_cleanup = None;
            let report = perform_cleanup(app, &plan.items);
            finish_cleanup(
                app,
                report,
                t!(
                    "lib.cleanup_done",
                    count = plan.items.len(),
                    size = human_size(plan.bytes())
                )
                .into(),
            );
        }
        Some(false) => app.lib.confirm_cleanup = None,
        None => {}
    }
}

fn cleanup_modal(ctx: &egui::Context, plan: &CleanupPlan) -> Option<bool> {
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("confirm-cleanup")).show(ctx, |ui| {
        ui.set_width(410.0);
        theme::popup_title(ui, &plan.older.label());
        ui.label(t!(
            "lib.cleanup_text",
            count = plan.items.len(),
            size = human_size(plan.bytes())
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !plan.items.is_empty(),
                    theme::danger(ui, icon::CALENDAR_X, &t!("lib.cleanup_confirm")),
                )
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
    decision
}

fn perform_cleanup(app: &mut App, items: &[CleanupItem]) -> CleanupReport {
    app.lib.prepare_for_removal(items);
    let report = cleanup_selected(items, files::move_to_trash);
    for item in &report.removed {
        app.thumbs.forget(&item.path, item.proxy.clone());
    }
    let removed: Vec<PathBuf> = report
        .removed
        .iter()
        .map(|item| item.path.clone())
        .collect();
    if !removed.is_empty() {
        app.lib.files_changed(&removed);
    }
    report
}

fn finish_cleanup(app: &mut App, report: CleanupReport, success: String) {
    let details = report
        .failures
        .iter()
        .map(|failure| format!("{}: {}", failure.path.display(), failure.error))
        .collect::<Vec<_>>()
        .join("\n");
    if report.removed.is_empty() {
        app.error(t!(
            "lib.cleanup_failed",
            failed = report.failures.len(),
            details = details
        ));
    } else if report.failures.is_empty() {
        app.success(success);
    } else {
        app.warning(t!(
            "lib.cleanup_partial",
            count = report.removed.len(),
            size = human_size(report.bytes()),
            failed = report.failures.len(),
            details = details
        ));
    }
}

fn rename_dialog(app: &mut App, ctx: &egui::Context) {
    let Some((path, mut title)) = app.lib.renaming.clone() else {
        return;
    };
    let Some(current) = naming::parse_path(&path) else {
        app.lib.renaming = None;
        return;
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_string();
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("rename-media")).show(ctx, |ui| {
        ui.set_width(380.0);
        theme::popup_title(ui, &t!("lib.rename_title"));
        let edit = ui.add(egui::TextEdit::singleline(&mut title).desired_width(f32::INFINITY));
        if !edit.has_focus() && ui.memory(|m| m.focused().is_none()) {
            edit.request_focus();
        }
        let preview = naming::MediaName {
            title: naming::sanitize(&title, naming::MAX_TITLE),
            ..current.clone()
        };
        theme::hint(
            ui,
            t!(
                "lib.rename_file",
                name = format!("{}.{ext}", preview.stem())
            ),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add(theme::primary(ui, icon::CHECK, &t!("lib.rename_confirm")))
                .clicked()
                || enter
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
    match decision {
        Some(true) => {
            app.lib.renaming = None;
            let proxy = mediaa_media::preview_cache::proxy_path(&path);
            match naming::rename(&path, &title) {
                Ok(new) if new != path => {
                    app.thumbs.forget(&path, proxy);
                    app.lib.files_changed(&[path.clone(), new.clone()]);
                    app.success(t!(
                        "lib.renamed",
                        name = new.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                Ok(_) => {}
                Err(e) => app.error(format!("{e:#}")),
            }
        }
        Some(false) => app.lib.renaming = None,
        None => app.lib.renaming = Some((path, title)),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        cleanup_modal, cleanup_plan, cleanup_selected, cover_uv, grid_columns, tools, CleanupItem,
        CleanupOlder, GAP, MAIN_TOOLS, MIN_TILE,
    };
    use eframe::egui::vec2;
    use mediaa_core::library::LibraryEntry;
    use mediaa_core::library_index::LibraryIndex;
    use std::time::{Duration, SystemTime};

    fn entry(path: std::path::PathBuf, modified: SystemTime) -> LibraryEntry {
        LibraryEntry {
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            title: "Recording".into(),
            path,
            kind: mediaa_core::MediaKind::Video,
            size: 1024,
            modified,
            taken: chrono::NaiveDateTime::default(),
            details: None,
        }
    }

    #[test]
    fn more_button_opens_the_menu_and_its_items_act() {
        use super::{tools_strip, Action};
        use egui_kittest::kittest::Queryable;
        use mediaa_core::library::LibraryEntry;
        let e = LibraryEntry {
            path: "/m/A_2026-01-01_10-00-00.png".into(),
            name: "A_2026-01-01_10-00-00.png".into(),
            title: "A".into(),
            kind: mediaa_core::MediaKind::Image,
            size: 1,
            modified: std::time::SystemTime::UNIX_EPOCH,
            taken: chrono::NaiveDateTime::default(),
            details: None,
        };
        let list = tools(&e);
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui, chosen: &mut Option<Action>| {
                ui.set_width(300.0);
                if let Some(a) = tools_strip(ui, &list) {
                    *chosen = Some(a);
                }
            },
            None,
        );
        let rename = rust_i18n::t!("lib.rename").to_string();
        assert!(
            harness.query_by_label(&rename).is_none(),
            "menu closed at first"
        );
        harness.get_by_label(&rust_i18n::t!("lib.more")).click();
        harness.run();
        assert!(
            harness.query_by_label(&rename).is_some(),
            "⋯ opens the menu"
        );
        harness.get_by_label(&rename).click();
        harness.run();
        assert!(
            matches!(harness.state(), Some(Action::AskRename(_, t)) if t == "A"),
            "the item's action is returned"
        );
        assert!(
            harness.query_by_label(&rename).is_none(),
            "menu closes after a choice"
        );
        harness.get_by_label(&rust_i18n::t!("lib.reveal")).click();
        harness.run();
        assert!(matches!(harness.state(), Some(Action::Reveal(_))));
    }

    #[test]
    fn every_kind_gets_all_tools_and_few_main_ones() {
        use mediaa_core::library::LibraryEntry;
        use mediaa_core::MediaKind;
        for kind in [
            MediaKind::Image,
            MediaKind::Video,
            MediaKind::Animation,
            MediaKind::Audio,
        ] {
            let e = LibraryEntry {
                path: "/m/A_2026-01-01_10-00-00.png".into(),
                name: "A_2026-01-01_10-00-00.png".into(),
                title: "A".into(),
                kind,
                size: 1,
                modified: std::time::SystemTime::UNIX_EPOCH,
                taken: chrono::NaiveDateTime::default(),
                details: None,
            };
            let list = tools(&e);
            assert!(
                list.iter().filter(|t| t.main).count() <= MAIN_TOOLS,
                "{kind:?}: main tools fit the tile"
            );
            assert!(
                list.last().is_some_and(|t| t.danger)
                    && list.iter().filter(|t| t.danger).count() == 1,
                "delete last"
            );
            assert_eq!(
                list.len(),
                if kind == MediaKind::Image { 9 } else { 8 },
                "{kind:?}"
            );
        }
    }

    #[test]
    fn grid_rows_always_fit_the_window() {
        for avail in [
            120.0_f32,
            250.0,
            MIN_TILE,
            MIN_TILE + 1.0,
            600.0,
            1000.0,
            1333.0,
            1999.0,
            3000.0,
        ] {
            let (cols, w) = grid_columns(avail);
            let row = cols as f32 * w + GAP * (cols - 1) as f32;
            assert!(row <= avail.max(1.0), "{avail}: {cols} × {w} = {row}");
            assert!(
                cols == 1 || w >= MIN_TILE,
                "{avail}: tiles never narrower than the minimum"
            );
            assert!(
                avail - row < cols as f32 + 1.0,
                "{avail}: row filled ({row})"
            );
        }
        assert_eq!(grid_columns(1000.0).0, 4);
    }

    #[test]
    fn cover_crops_the_longer_side_symmetrically() {
        let wide = cover_uv(vec2(200.0, 100.0), vec2(100.0, 100.0));
        assert_eq!(
            (wide.min.x, wide.max.x, wide.min.y, wide.max.y),
            (0.25, 0.75, 0.0, 1.0)
        );
        let tall = cover_uv(vec2(100.0, 200.0), vec2(100.0, 100.0));
        assert_eq!(
            (tall.min.y, tall.max.y, tall.min.x, tall.max.x),
            (0.25, 0.75, 0.0, 1.0)
        );
        let same = cover_uv(vec2(160.0, 90.0), vec2(320.0, 180.0));
        assert_eq!((same.min.x, same.max.x), (0.0, 1.0));
    }

    #[test]
    fn cleanup_thresholds_use_strict_seven_and_thirty_day_boundaries() {
        let now = SystemTime::now();
        let days = |n: u64| {
            now.checked_sub(Duration::from_secs(n * 24 * 60 * 60))
                .unwrap()
        };
        let entries = [
            entry(
                "fresh.png".into(),
                now.checked_sub(Duration::from_secs(6 * 24 * 60 * 60))
                    .unwrap(),
            ),
            entry("week.png".into(), days(7)),
            entry("older-week.png".into(), days(8)),
            entry("month.png".into(), days(30)),
            entry("older-month.png".into(), days(31)),
        ];
        let week = cleanup_plan(&entries, CleanupOlder::Week, now);
        let month = cleanup_plan(&entries, CleanupOlder::Month, now);
        assert_eq!(
            week.items
                .iter()
                .map(|i| i.path.as_path())
                .collect::<Vec<_>>(),
            ["older-week.png", "month.png", "older-month.png"]
        );
        assert_eq!(
            month
                .items
                .iter()
                .map(|i| i.path.as_path())
                .collect::<Vec<_>>(),
            ["older-month.png"]
        );
    }

    #[test]
    fn cleanup_continues_after_a_failed_move_and_removes_ready_proxies() {
        let dir = tempfile::tempdir().unwrap();
        let items: Vec<CleanupItem> = ["first.webp", "busy.webp", "last.webp"]
            .into_iter()
            .map(|name| CleanupItem {
                path: dir.path().join(name),
                size: 10,
            })
            .collect();
        for item in &items {
            std::fs::write(&item.path, b"media").unwrap();
            let proxy = mediaa_media::preview_cache::proxy_path(&item.path);
            std::fs::create_dir_all(proxy.parent().unwrap()).unwrap();
            std::fs::write(proxy, b"proxy").unwrap();
        }
        let report = cleanup_selected(&items, |path| {
            if path.file_name().is_some_and(|name| name == "busy.webp") {
                anyhow::bail!("file is busy")
            }
            std::fs::remove_file(path)?;
            Ok(())
        });
        assert_eq!(report.removed.len(), 2);
        assert_eq!(report.failures.len(), 1);
        assert!(!items[0].path.exists());
        assert!(items[1].path.exists());
        assert!(!items[2].path.exists());
        assert!(!mediaa_media::preview_cache::proxy_path(&items[0].path).exists());
        assert!(mediaa_media::preview_cache::proxy_path(&items[1].path).exists());
        assert!(!mediaa_media::preview_cache::proxy_path(&items[2].path).exists());
    }

    #[test]
    fn cleanup_removes_the_library_index_rows() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("Old_2026-01-01_00-00-00.webp");
        std::fs::write(&media, b"media").unwrap();
        let old = SystemTime::now()
            .checked_sub(Duration::from_secs(31 * 24 * 60 * 60))
            .unwrap();
        std::fs::File::options()
            .write(true)
            .open(&media)
            .unwrap()
            .set_modified(old)
            .unwrap();
        let mut index = LibraryIndex::load(dir.path());
        index.apply_scan(mediaa_core::library_index::scan_dir(dir.path()));
        assert_eq!(index.len(), 1);
        let plan = cleanup_plan(index.entries(), CleanupOlder::Month, SystemTime::now());
        let report = cleanup_selected(&plan.items, |path| {
            std::fs::remove_file(path)?;
            Ok(())
        });
        let removed = report
            .removed
            .iter()
            .map(|item| item.path.clone())
            .collect::<Vec<_>>();
        index.refresh_paths(&removed);
        assert!(index.is_empty());
        assert!(LibraryIndex::load(dir.path()).is_empty());
    }

    #[test]
    fn cleanup_modal_shows_the_scope_and_can_be_cancelled() {
        use egui_kittest::kittest::Queryable;
        if let Ok(locale) = std::env::var("MEDIAA_LIBRARY_MODAL_LOCALE") {
            rust_i18n::set_locale(&locale);
        }
        let plan = cleanup_plan(
            &[entry(
                "old.webp".into(),
                SystemTime::now()
                    .checked_sub(Duration::from_secs(31 * 24 * 60 * 60))
                    .unwrap(),
            )],
            CleanupOlder::Month,
            SystemTime::now(),
        );
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui: &mut eframe::egui::Ui, decision: &mut Option<bool>| {
                if let Some(value) = cleanup_modal(ui.ctx(), &plan) {
                    *decision = Some(value);
                }
            },
            None,
        );
        crate::theme::install(&harness.ctx);
        harness.run();
        assert!(harness
            .query_by_label(&rust_i18n::t!("lib.cleanup_month"))
            .is_some());
        assert!(harness
            .query_by_label(&rust_i18n::t!(
                "lib.cleanup_text",
                count = 1,
                size = "1.0 KB"
            ))
            .is_some());
        if let Ok(path) = std::env::var("MEDIAA_LIBRARY_MODAL_SNAPSHOT") {
            harness.render().unwrap().save(path).unwrap();
        }
        harness
            .get_by_label_contains(&rust_i18n::t!("common.cancel"))
            .click();
        harness.run();
        assert_eq!(harness.state(), &Some(false));
    }
}
