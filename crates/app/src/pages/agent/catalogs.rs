//! Catalogs: the virtual tree of the Agent section in a resizable side bar (folders and files
//! with virtual names and colours, drag and drop, context menus) and the files opened from it
//! in tabs (`egui_dock`), like an editor: many tabs, scrolling, close / close others / close all.

use crate::app::App;
use crate::theme;

use eframe::egui::{self, Color32, RichText};
use egui_dock::{DockArea, DockState, TabViewer};
use egui_ltreeview::{Action, DirPosition, NodeBuilder, TreeView, TreeViewBuilder};
use egui_phosphor::regular as icon;
use super::super::library::library_dir;
use crossbeam_channel::Receiver;
use mediaa_agent::pipeline::{self, Stage};
use mediaa_agent::search::{self, FileHit, Source, Trace};
use mediaa_core::library::human_duration;
use mediaa_core::transcript;
use mediaa_core::catalogs::{ai_dir, Catalogs, NodeId, NodeKind};
use mediaa_core::agent::AnalysisOptions;
use mediaa_core::queue::Queue;

use mediaa_core::MediaKind;
use super::chat::Chat;
use super::studio::{Studio, StudioAction};
use rust_i18n::t;
use std::cell::RefCell;
use std::path::{Path, PathBuf};

/// Hidden root of the tree (database ids start at 1): dropping onto it means the top level.
const ROOT: NodeId = 0;

/// Colours offered for folders and files (`0xRRGGBB`).
pub const COLORS: [u32; 8] = [0xe5484d, 0xf76b15, 0xffc53d, 0x46a758, 0x12a594, 0x3e63dd, 0x8e4ec6, 0xd6409f];


fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// What the tree asked for (applied after it is drawn).
enum Cmd {
    NewFolder(Option<NodeId>),
    AddFiles(Option<NodeId>, bool),
    Rename(NodeId),
    Color(NodeId, Option<u32>),
    Delete(NodeId),
    Open(NodeId),
    /// Open a file at a time (a search hit).
    OpenAt(NodeId, f64),
    /// The chat over a folder's recordings.
    FolderChat(NodeId),
    Reveal(PathBuf),
}

pub struct CatalogsUi {
    /// The catalogs of the library's `.ai` folder (reopened when the library folder changes).
    store: Option<(PathBuf, Result<Catalogs, String>)>,
    selected: Option<NodeId>,
    dock: DockState<Tab>,
    rename: Option<(NodeId, String)>,
    confirm_delete: Option<NodeId>,
    dev_opened: bool,
    search: Search,
}

impl Default for CatalogsUi {
    fn default() -> Self {
        Self { store: None, selected: None, dock: DockState::new(Vec::new()), rename: None, confirm_delete: None, dev_opened: false, search: Search::default() }
    }
}

impl CatalogsUi {
    fn catalogs(&mut self, media_dir: &Path) -> Result<&mut Catalogs, String> {
        let dir = ai_dir(media_dir);
        if self.store.as_ref().is_none_or(|(d, _)| *d != dir) {
            let opened = Catalogs::open(&dir).map_err(|e| format!("{e:#}"));
            self.store = Some((dir, opened));
            self.dock = DockState::new(Vec::new());
            self.selected = None;
        }
        match self.store.as_mut() {
            Some((_, Ok(c))) => Ok(c),
            Some((_, Err(e))) => Err(e.clone()),
            None => unreachable!("opened above"),
        }
    }

    /// Folder new items go into: the selected folder, the selected file's folder or the top.
    fn target_folder(&self, cat: &Catalogs) -> Option<NodeId> {
        let n = cat.node(self.selected?)?;
        if n.is_folder() {
            Some(n.id)
        } else {
            n.parent
        }
    }
}

// ------------------------------------------------------------------ search

/// Hits shown for a catalog search.
const CATALOG_HITS: usize = 12;
/// Characters of a hit's text shown in the side bar.
const HIT_CHARS: usize = 240;

type Found = (Vec<(NodeId, FileHit)>, Trace);

/// The search over the recordings of the selected folder (or of all catalogs).
#[derive(Default)]
struct Search {
    text: String,
    running: Option<Receiver<Result<Found, String>>>,
    /// Results of the last search (`None`: the tree is shown).
    results: Option<Result<Found, String>>,
}

/// Files the search covers: the selected folder's subtree, else every file; the folder's name.
fn scope(cat: &Catalogs, selected: Option<NodeId>) -> (Option<String>, Vec<(NodeId, PathBuf)>) {
    let folder = selected.and_then(|id| cat.node(id)).filter(|n| n.is_folder());
    let ids: Vec<NodeId> = match folder {
        Some(f) => cat.subtree(f.id),
        None => cat.nodes().iter().map(|n| n.id).collect(),
    };
    let files = ids.into_iter().filter_map(|id| cat.file_path(id).map(|p| (id, p.to_path_buf()))).collect();
    (folder.map(|f| f.name.clone()), files)
}

/// `text` cut to about `max` characters at a word end.
fn shortened(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head))
}

/// The search field and, after a search, the hits; returns whether hits are shown instead of
/// the tree.
fn search_ui(ui: &mut egui::Ui, search: &mut Search, cat: &Catalogs, selected: Option<NodeId>, services: Option<&super::Services>, cmds: &RefCell<Vec<Cmd>>) -> bool {
    let p = theme::palette(ui);
    let (folder, files) = scope(cat, selected);
    let mut start = false;
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::MAGNIFYING_GLASS).color(p.text_weak));
        let shown = search.results.is_some() || search.running.is_some();
        let width = ui.available_width() - if shown { 30.0 } else { 0.0 };
        let hint = match &folder {
            Some(name) => t!("agent.search_folder", name = name).to_string(),
            None => t!("agent.search_all").to_string(),
        };
        let edit = ui.add(egui::TextEdit::singleline(&mut search.text).hint_text(hint).desired_width(width));
        start = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !search.text.trim().is_empty();
        if shown && theme::icon_button(ui, icon::X, &t!("studio.search_close")).clicked() {
            *search = Search::default();
        }
    });
    ui.add_space(4.0);
    if let (true, Some(s)) = (start, services.cloned()) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let query = search.text.trim().to_string();
        let ctx = ui.ctx().clone();
        let _ = std::thread::Builder::new().name("catalog-search".into()).spawn(move || {
            let sources: Vec<Source> = files.iter().filter_map(|(_, p)| transcript::file_id(p).ok().map(|id| Source { id, path: p.clone() })).collect();
            let settings = s.settings.read().expect("settings lock").clone();
            let found = search::search_files(s.engine.as_ref(), &s.slots, &settings, &s.ai_dir, &sources, &query, CATALOG_HITS, true)
                .map(|(hits, trace)| {
                    let hits = hits.into_iter().filter_map(|h| files.iter().find(|(_, p)| *p == h.path).map(|(id, _)| (*id, h))).collect();
                    (hits, trace)
                })
                .map_err(|e| format!("{e:#}"));
            let _ = tx.send(found);
            ctx.request_repaint();
        });
        search.running = Some(rx);
        search.results = None;
    }
    if let Some(rx) = &search.running {
        match rx.try_recv() {
            Ok(r) => {
                search.results = Some(r);
                search.running = None;
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
    let Some(results) = &search.results else { return false };
    match results {
        Err(e) => {
            ui.colored_label(p.danger, e);
        }
        Ok((hits, trace)) => {
            if trace.not_indexed > 0 {
                theme::hint(ui, t!("agent.search_not_indexed", n = trace.not_indexed, all = trace.files));
            }
            if hits.is_empty() {
                theme::hint(ui, t!("studio.nothing_found"));
            }
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for (node, h) in hits {
                    let name = cat.node(*node).map_or_else(|| h.path.file_name().unwrap_or_default().to_string_lossy().into_owned(), |n| n.name.clone());
                    let card = theme::card(ui).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(human_duration(h.hit.chunk.start)).font(egui::FontId::monospace(11.0)).color(p.accent));
                            ui.add(egui::Label::new(RichText::new(name).strong().size(12.5)).truncate());
                        });
                        ui.add(egui::Label::new(RichText::new(shortened(&h.hit.chunk.text, HIT_CHARS)).size(12.5)).wrap());
                    });
                    let resp = ui.interact(card.response.rect, ui.id().with(("catalog-hit", *node, h.hit.id)), egui::Sense::click());
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if resp.on_hover_text(t!("studio.go_there")).clicked() {
                        cmds.borrow_mut().push(Cmd::OpenAt(*node, h.hit.chunk.start));
                    }
                    ui.add_space(4.0);
                }
            });
        }
    }
    true
}

// ------------------------------------------------------------------ side bar (tree)

pub fn sidebar(app: &mut App, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let media_dir = super::super::library::library_dir(&app.settings);
    let cmds: RefCell<Vec<Cmd>> = RefCell::default();
    ui.horizontal(|ui| {
        ui.label(RichText::new(t!("agent.catalogs")).strong());
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let ui_state = &app.agent.catalogs;
            let target = match ui_state.store.as_ref() {
                Some((_, Ok(c))) => ui_state.target_folder(c),
                _ => None,
            };
            if theme::icon_button(ui, icon::LINK, &t!("agent.add_links")).clicked() {
                cmds.borrow_mut().push(Cmd::AddFiles(target, false));
            }
            if theme::icon_button(ui, icon::FILE_PLUS, &t!("agent.add_copies")).clicked() {
                cmds.borrow_mut().push(Cmd::AddFiles(target, true));
            }
            if theme::icon_button(ui, icon::FOLDER_PLUS, &t!("agent.new_folder")).clicked() {
                cmds.borrow_mut().push(Cmd::NewFolder(target));
            }
        });
    });
    ui.add_space(4.0);

    if let Err(e) = app.agent.catalogs.catalogs(&media_dir) {
        ui.colored_label(p.danger, e);
        return;
    }
    let services = app.agent.services();
    let CatalogsUi { store, selected, search, .. } = &mut app.agent.catalogs;
    let Some((_, Ok(cat))) = store.as_mut() else { return };
    if search_ui(ui, search, cat, *selected, services.as_ref(), &cmds) {
        for cmd in cmds.into_inner() {
            run(app, cmd);
        }
        return;
    }
    let (_, actions) = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let tree = TreeView::new(egui::Id::new("catalog-tree-view")).fallback_context_menu(|ui, _| {
                folder_menu(ui, None, &cmds);
            });
            tree.show(ui, |b| {
                b.node(NodeBuilder::dir(ROOT).flatten(true));
                build(b, cat, None, &cmds, None, p.text);
                b.close_dir();
            })
        })
        .inner;
    if cat.children(None).is_empty() {
        theme::hint(ui, t!("agent.catalogs_empty"));
    }

    for a in actions {
        match a {
            Action::SetSelected(sel) => *selected = sel.first().copied().filter(|&n| n != ROOT),
            Action::Move(dnd) => {
                let parent = (dnd.target != ROOT).then_some(dnd.target);
                let siblings: Vec<NodeId> = cat.children(parent).iter().map(|n| n.id).collect();
                let mut index = match dnd.position {
                    DirPosition::First => 0,
                    DirPosition::Last => siblings.len(),
                    DirPosition::Before(n) => siblings.iter().position(|&s| s == n).unwrap_or(siblings.len()),
                    DirPosition::After(n) => siblings.iter().position(|&s| s == n).map_or(siblings.len(), |i| i + 1),
                };
                for id in dnd.source.into_iter().filter(|&n| n != ROOT) {
                    // Moving down within the same folder: the node's old slot no longer counts.
                    let old = siblings.iter().position(|&s| s == id);
                    let at = if old.is_some_and(|o| o < index) { index - 1 } else { index };
                    match cat.move_node(id, parent, at) {
                        Ok(()) => index = at + 1,
                        Err(e) => {
                            app.toasts.error(format!("{e:#}"));
                        }
                    }
                }
            }
            Action::Activate(act) => cmds.borrow_mut().extend(act.selected.into_iter().map(Cmd::Open)),
            Action::Drag(dnd) => {
                // A folder cannot be dropped into itself: hide the marker.
                let target = dnd.target;
                if target != ROOT && dnd.source.iter().any(|&s| cat.subtree(s).contains(&target)) {
                    dnd.remove_drop_marker(ui);
                }
            }
            _ => {}
        }
    }
    for cmd in cmds.into_inner() {
        run(app, cmd);
    }
}

/// Adds the children of `parent` (and below) to the tree.
/// A tree label cut with "…" when the side bar is narrower than it.
fn truncated(text: RichText) -> impl FnMut(&mut egui::Ui) {
    move |ui| {
        ui.add(egui::Label::new(text.clone()).selectable(false).truncate());
    }
}

fn file_icon(path: &Path) -> &'static str {
    match MediaKind::from_path(path) {
        Some(MediaKind::Video) => icon::FILE_VIDEO,
        _ => icon::FILE_AUDIO,
    }
}

/// `inherited`: the colour of the nearest coloured folder above (uncoloured nodes take it).
fn build(b: &mut TreeViewBuilder<'_, NodeId>, cat: &Catalogs, parent: Option<NodeId>, cmds: &RefCell<Vec<Cmd>>, inherited: Option<u32>, text: Color32) {
    for n in cat.children(parent) {
        let own = n.color.or(inherited);
        let color = own.map_or(text, rgb);
        let id = n.id;
        match &n.kind {
            NodeKind::Folder => {
                let label = RichText::new(format!("{}  {}", icon::FOLDER, n.name)).color(color);
                b.node(NodeBuilder::dir(id).label_ui(truncated(label)).context_menu(move |ui| folder_menu(ui, Some(id), cmds)));
                build(b, cat, Some(id), cmds, own, text);
                b.close_dir();
            }
            NodeKind::File { path, hard } => {
                let ic = file_icon(path);
                // Links are marked: the file lives elsewhere and stays when the node goes.
                let mark = if *hard { "" } else { "  ↗" };
                let label = RichText::new(format!("{ic}  {}{mark}", n.name)).color(color);
                let path = path.clone();
                b.node(NodeBuilder::leaf(id).label_ui(truncated(label)).context_menu(move |ui| file_menu(ui, id, &path, cmds)));
            }
        }
    }
}

fn item(ui: &mut egui::Ui, cmds: &RefCell<Vec<Cmd>>, ic: &str, label: &str, danger: bool, cmd: impl FnOnce() -> Cmd) {
    if theme::menu_item(ui, ic, label, danger).clicked() {
        cmds.borrow_mut().push(cmd());
        ui.close();
    }
}

fn color_row(ui: &mut egui::Ui, id: NodeId, cmds: &RefCell<Vec<Cmd>>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        ui.add_space(6.0);
        for c in COLORS.map(Some).into_iter().chain([None]) {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let r = if resp.hovered() { 9.0 } else { 7.0 };
            match c {
                Some(c) => {
                    ui.painter().circle_filled(rect.center(), r, rgb(c));
                }
                None => {
                    ui.painter().circle_stroke(rect.center(), r - 1.0, egui::Stroke::new(1.5, theme::palette(ui).text_weak));
                    resp.clone().on_hover_text(t!("agent.no_color"));
                }
            }
            if resp.clicked() {
                cmds.borrow_mut().push(Cmd::Color(id, c));
                ui.close();
            }
        }
    });
}

fn folder_menu(ui: &mut egui::Ui, id: Option<NodeId>, cmds: &RefCell<Vec<Cmd>>) {
    ui.set_min_width(230.0);
    item(ui, cmds, icon::FOLDER_PLUS, &t!("agent.new_folder"), false, || Cmd::NewFolder(id));
    item(ui, cmds, icon::LINK, &t!("agent.add_links"), false, || Cmd::AddFiles(id, false));
    item(ui, cmds, icon::FILE_PLUS, &t!("agent.add_copies"), false, || Cmd::AddFiles(id, true));
    if let Some(id) = id {
        item(ui, cmds, icon::CHATS_CIRCLE, &t!("agent.folder_chat"), false, || Cmd::FolderChat(id));
        ui.separator();
        item(ui, cmds, icon::PENCIL_SIMPLE, &t!("agent.rename"), false, || Cmd::Rename(id));
        color_row(ui, id, cmds);
        ui.separator();
        item(ui, cmds, icon::TRASH, &t!("agent.delete"), true, || Cmd::Delete(id));
    }
}

fn file_menu(ui: &mut egui::Ui, id: NodeId, path: &Path, cmds: &RefCell<Vec<Cmd>>) {
    ui.set_min_width(230.0);
    item(ui, cmds, icon::ARROW_SQUARE_OUT, &t!("agent.open"), false, || Cmd::Open(id));
    item(ui, cmds, icon::FOLDER_OPEN, &t!("lib.reveal"), false, || Cmd::Reveal(path.to_path_buf()));
    ui.separator();
    item(ui, cmds, icon::PENCIL_SIMPLE, &t!("agent.rename"), false, || Cmd::Rename(id));
    color_row(ui, id, cmds);
    ui.separator();
    item(ui, cmds, icon::TRASH, &t!("agent.delete"), true, || Cmd::Delete(id));
}

fn run(app: &mut App, cmd: Cmd) {
    let media_dir = super::super::library::library_dir(&app.settings);
    let state = &mut app.agent.catalogs;
    let Ok(cat) = state.catalogs(&media_dir) else { return };
    let result: anyhow::Result<()> = match cmd {
        Cmd::NewFolder(parent) => cat.create_folder(parent, &t!("agent.new_folder_name")).map(|id| {
            state.rename = Some((id, t!("agent.new_folder_name").into()));
        }),
        Cmd::AddFiles(parent, hard) => {
            let mut res = Ok(());
            if let Some(files) = rfd::FileDialog::new().add_filter(t!("agent.catalog_files"), &mediaa_core::catalogs::accepted_extensions()).pick_files() {
                for f in files {
                    // Video only with sound: the agent works with what is said.
                    let added = match mediaa_media::info::has_sound(&f) {
                        Ok(true) => cat.add_file(parent, &f, hard).map(|_| ()),
                        Ok(false) => Err(anyhow::anyhow!("{}", t!("agent.no_sound", name = f.file_name().unwrap_or_default().to_string_lossy()))),
                        Err(e) => Err(e),
                    };
                    if let Err(e) = added {
                        res = Err(e);
                    }
                }
            }
            res
        }
        Cmd::Rename(id) => {
            state.rename = cat.node(id).map(|n| (id, n.name.clone()));
            Ok(())
        }
        Cmd::Color(id, c) => cat.set_color(id, c),
        Cmd::Delete(id) => {
            state.confirm_delete = Some(id);
            Ok(())
        }
        Cmd::Open(id) => {
            open_tab(state, id);
            Ok(())
        }
        Cmd::FolderChat(id) => {
            open_folder_chat(state, id);
            Ok(())
        }
        Cmd::OpenAt(id, t) => {
            open_tab(state, id);
            if let Some((_, tab)) = state.dock.iter_all_tabs_mut().find(|(_, tab)| tab.node == id) {
                if let Body::Studio(s) = &mut tab.body {
                    s.seek(t);
                }
            }
            Ok(())
        }
        Cmd::Reveal(path) => mediaa_platform::files::reveal(&path),
    };
    if let Err(e) = result {
        app.error(format!("{e:#}"));
    }
}

/// Rename and delete confirmations.
pub fn dialogs(app: &mut App, ctx: &egui::Context) {
    let media_dir = super::super::library::library_dir(&app.settings);
    if let Some((id, mut name)) = app.agent.catalogs.rename.clone() {
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new("catalog-rename")).show(ctx, |ui| {
            ui.set_width(360.0);
            theme::popup_title(ui, &t!("agent.rename"));
            let edit = ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
            if !edit.has_focus() && ui.memory(|m| m.focused().is_none()) {
                edit.request_focus();
            }
            theme::hint(ui, t!("agent.rename_hint"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                if ui.add(theme::primary(ui, icon::CHECK, &t!("lib.rename_confirm"))).clicked() || enter {
                    decision = Some(true);
                }
                if ui.add(theme::secondary(icon::X, &t!("common.cancel"))).clicked() {
                    decision = Some(false);
                }
            });
        });
        app.agent.catalogs.rename = Some((id, name.clone()));
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        if let Some(ok) = decision {
            app.agent.catalogs.rename = None;
            if ok {
                if let Ok(cat) = app.agent.catalogs.catalogs(&media_dir) {
                    if let Err(e) = cat.rename(id, &name) {
                        app.error(format!("{e:#}"));
                    }
                }
            }
        }
    }

    if let Some(id) = app.agent.catalogs.confirm_delete {
        let Ok(cat) = app.agent.catalogs.catalogs(&media_dir) else { return };
        let Some(node) = cat.node(id) else {
            app.agent.catalogs.confirm_delete = None;
            return;
        };
        let name = node.name.clone();
        let copies = cat.subtree(id).iter().filter(|&&n| matches!(cat.node(n).map(|n| &n.kind), Some(NodeKind::File { hard: true, .. }))).count();
        let mut decision = None;
        let modal = egui::Modal::new(egui::Id::new("catalog-delete")).show(ctx, |ui| {
            ui.set_width(380.0);
            theme::popup_title(ui, &t!("agent.delete_title"));
            ui.label(t!("agent.delete_text", name = name));
            if copies > 0 {
                ui.colored_label(theme::palette(ui).danger, t!("agent.delete_copies", n = copies));
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.add(theme::danger(ui, icon::TRASH, &t!("agent.delete"))).clicked() {
                    decision = Some(true);
                }
                if ui.add(theme::secondary(icon::X, &t!("common.cancel"))).clicked() {
                    decision = Some(false);
                }
            });
        });
        if modal.should_close() && decision.is_none() {
            decision = Some(false);
        }
        if let Some(ok) = decision {
            let removed = cat.subtree(id);
            let deleted = if ok { Some(cat.delete(id)) } else { None };
            let state = &mut app.agent.catalogs;
            state.confirm_delete = None;
            match deleted {
                Some(Ok(())) => state.dock.retain_tabs(|t| !removed.contains(&t.node)),
                Some(Err(e)) => app.error(format!("{e:#}")),
                None => {}
            }
        }
    }
}

// ------------------------------------------------------------------ tabs

/// A file opened from the tree.
pub struct Tab {
    node: NodeId,
    path: PathBuf,
    body: Body,
}

/// What a tab shows.
enum Body {
    Studio(Box<Studio>),
    /// The chat over a folder's recordings.
    Chat(Box<Chat>),
    /// The file cannot be opened: why.
    Missing(String),
}

fn open_tab(state: &mut CatalogsUi, id: NodeId) {
    let Some((ai, Ok(cat))) = state.store.as_ref() else { return };
    let Some(path) = cat.file_path(id).map(Path::to_path_buf) else { return };
    let ai = ai.clone();
    if let Some(at) = state.dock.find_tab_from(|t| t.node == id) {
        let _ = state.dock.set_active_tab(at);
        return;
    }
    let body = if path.exists() {
        Studio::open(&path, &ai).map_or_else(|e| Body::Missing(format!("{e:#}")), |s| Body::Studio(Box::new(s)))
    } else {
        Body::Missing(t!("agent.file_missing").into())
    };
    state.dock.push_to_focused_leaf(Tab { node: id, path, body });
}

/// Opens (or shows) the chat over the recordings of folder `id` and its subfolders.
fn open_folder_chat(state: &mut CatalogsUi, id: NodeId) {
    let Some((ai, Ok(cat))) = state.store.as_ref() else { return };
    if let Some(at) = state.dock.find_tab_from(|t| t.node == id) {
        let _ = state.dock.set_active_tab(at);
        return;
    }
    let (_, files) = scope(cat, Some(id));
    let chat = Chat::folder(id, files.into_iter().map(|(_, p)| p).collect(), ai);
    state.dock.push_to_focused_leaf(Tab { node: id, path: PathBuf::new(), body: Body::Chat(Box::new(chat)) });
}

enum TabCmd {
    CloseOthers(NodeId),
    Open(PathBuf),
    /// A recording at a time (a citation in a folder's chat).
    OpenAt(PathBuf, f64),
    Reveal(PathBuf),
    Analyse { path: PathBuf, from: Option<Stage>, options: AnalysisOptions, remember: bool },
    Stop(mediaa_core::queue::TaskId),
    ResumeTask(mediaa_core::queue::TaskId),
}

/// The width of every tab in the dock: short titles are padded, long ones are cut.
const TAB_WIDTH: f32 = 180.0;

/// The room a title gets inside [`TAB_WIDTH`]: the tab minus the tab button's 2×8 px text spacing
/// and the 24 px close button.
const TAB_TITLE_WIDTH: f32 = TAB_WIDTH - 16.0 - 24.0;

/// The full title of a tab: the catalog node's name (or the file name), with the folder chat icon.
fn tab_title(cat: &Catalogs, tab: &Tab) -> String {
    let name = cat.node(tab.node).map_or_else(|| tab.path.file_name().unwrap_or_default().to_string_lossy().into_owned(), |n| n.name.clone());
    match tab.body {
        Body::Chat(_) => format!("{}  {name}", icon::CHATS_CIRCLE),
        _ => name,
    }
}

/// One row of [`tab_title`], cut to [`TAB_TITLE_WIDTH`] by egui itself (its glyph widths) with `…`.
fn tab_title_job(full: &str) -> egui::text::LayoutJob {
    // The placeholder colour is replaced by egui_dock's active/inactive tab colour.
    let format = egui::TextFormat { font_id: egui::FontId::proportional(13.0), color: Color32::PLACEHOLDER, ..Default::default() };
    let mut job = egui::text::LayoutJob::single_section(full.to_owned(), format);
    job.wrap.max_width = TAB_TITLE_WIDTH;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    job
}

struct Tabs<'a> {
    cat: &'a Catalogs,
    services: Option<super::Services>,
    cmds: Vec<TabCmd>,
    /// For laying the title out ourselves: egui_dock discards a `LayoutJob`'s own wrapping.
    ctx: egui::Context,
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> egui::Id {
        egui::Id::new(("catalog-tab", tab.node))
    }

    fn title(&mut self, tab: &mut Tab) -> egui::WidgetText {
        let job = tab_title_job(&tab_title(self.cat, tab));
        egui::WidgetText::Galley(self.ctx.fonts_mut(|f| f.layout_job(job)))
    }

    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        response.clone().on_hover_text(tab_title(self.cat, tab));
    }

    fn ui(&mut self, ui: &mut egui::Ui, tab: &mut Tab) {
        let title = tab_title(self.cat, tab);
        match &mut tab.body {
            Body::Studio(studio) => match studio.ui(ui, &title, self.services.as_ref()) {
                Some(StudioAction::OpenAt(p, t)) => self.cmds.push(TabCmd::OpenAt(p, t)),
                Some(StudioAction::Open(p)) => self.cmds.push(TabCmd::Open(p)),
                Some(StudioAction::Reveal(p)) => self.cmds.push(TabCmd::Reveal(p)),
                Some(StudioAction::Analyse { path, from, options, remember }) => self.cmds.push(TabCmd::Analyse { path, from, options, remember }),
                Some(StudioAction::Stop(id)) => self.cmds.push(TabCmd::Stop(id)),
                Some(StudioAction::ResumeTask(id)) => self.cmds.push(TabCmd::ResumeTask(id)),
                None => {}
            },
            Body::Chat(chat) => {
                let full = ui.available_rect_before_wrap().shrink(6.0);
                if let Some(StudioAction::OpenAt(p, t)) = chat.ui(ui, full, None, self.services.as_ref(), theme::palette(ui)) {
                    self.cmds.push(TabCmd::OpenAt(p, t));
                }
                ui.allocate_rect(full, egui::Sense::hover());
            }
            Body::Missing(e) => {
                let p = theme::palette(ui);
                ui.vertical_centered(|ui| {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.label(RichText::new(icon::WARNING).size(48.0).color(p.text_weak));
                    theme::hint(ui, e.as_str());
                });
            }
        }
    }

    fn context_menu(&mut self, ui: &mut egui::Ui, tab: &mut Tab, _path: egui_dock::NodePath) {
        if theme::menu_item(ui, icon::X_SQUARE, &t!("agent.close_others"), false).clicked() {
            self.cmds.push(TabCmd::CloseOthers(tab.node));
            ui.close();
        }
    }

    fn allowed_in_windows(&self, _tab: &mut Tab) -> bool {
        false
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }
}

fn translations() -> egui_dock::Translations {
    let mut tr = egui_dock::Translations::english();
    tr.tab_context_menu.close_button = t!("agent.close").into();
    tr.leaf.close_all_button = t!("agent.close_all").into();
    tr.leaf.close_button_disabled_tooltip = String::new();
    tr
}

/// The open files; a hint while there are none.
pub fn tabs(app: &mut App, ui: &mut egui::Ui) {
    let media_dir = super::super::library::library_dir(&app.settings);
    if app.agent.catalogs.catalogs(&media_dir).is_err() {
        return;
    }
    // Dev: `MEDIAA_DEV_OPEN=<name part>` opens the first such file (or a folder's chat) once
    // (UI checks).
    if !app.agent.catalogs.dev_opened {
        app.agent.catalogs.dev_opened = true;
        let state = &mut app.agent.catalogs;
        let found = match (std::env::var("MEDIAA_DEV_OPEN"), state.store.as_ref()) {
            (Ok(part), Some((_, Ok(cat)))) => cat.nodes().iter().find(|n| n.name.contains(&part)).map(|n| (n.id, n.is_folder())),
            _ => None,
        };
        match found {
            Some((id, true)) => open_folder_chat(state, id),
            Some((id, false)) => open_tab(state, id),
            None => {}
        }
    }
    let services = app.agent.services();
    let CatalogsUi { store, dock, .. } = &mut app.agent.catalogs;
    let Some((_, Ok(cat))) = store.as_ref() else { return };
    // Tabs of nodes removed meanwhile go too.
    dock.retain_tabs(|t| cat.node(t.node).is_some());
    if dock.iter_all_tabs().next().is_none() {
        ui.centered_and_justified(|ui| theme::hint(ui, t!("agent.open_hint")));
        return;
    }
    let mut viewer = Tabs { cat, services, cmds: Vec::new(), ctx: ui.ctx().clone() };
    let mut style = egui_dock::Style::from_egui(ui.style());
    // No maximum width in egui_dock: the title is cut to the tab's width instead (see `tab_title_job`).
    style.tab.minimum_width = Some(TAB_WIDTH);
    *dock = std::mem::replace(dock, DockState::new(Vec::new())).with_translations(translations());
    DockArea::new(dock)
        .style(style)
        .show_leaf_close_all_buttons(true)
        .show_leaf_collapse_buttons(false)
        .show_inside(ui, &mut viewer);
    // Only the tabs in view play.
    for (_, tab) in dock.iter_all_tabs_mut() {
        if let Body::Studio(s) = &mut tab.body {
            if !s.drawn {
                s.pause();
            }
            s.drawn = false;
        }
    }
    for c in viewer.cmds {
        match c {
            TabCmd::CloseOthers(keep) => {
                let dock = &mut app.agent.catalogs.dock;
                dock.retain_tabs(|t| t.node == keep);
                // The tab the command came from is the one shown.
                if let Some(at) = dock.find_tab_from(|t| t.node == keep) {
                    let _ = dock.set_active_tab(at);
                    dock.set_focused_node_and_surface(at.node_path());
                }
            }
            TabCmd::Open(p) => app.open_path(&p),
            TabCmd::OpenAt(p, t) => {
                let node = match &app.agent.catalogs.store {
                    Some((_, Ok(cat))) => cat.nodes().iter().find(|n| cat.file_path(n.id) == Some(p.as_path())).map(|n| n.id),
                    _ => None,
                };
                if let Some(node) = node {
                    run(app, Cmd::OpenAt(node, t));
                }
            }
            TabCmd::Reveal(p) => {
                if let Err(e) = mediaa_platform::files::reveal(&p) {
                    app.error(format!("{e:#}"));
                }
            }
            TabCmd::Analyse { path, from, options, remember } => {
                if remember {
                    // The choice is the default for the next analysis.
                    app.settings.agent.analysis = options.enforced();
                    app.settings_changed();
                }
                if let Err(e) = analyse(app, &path, from, options) {
                    app.error(format!("{e:#}"));
                }
            }
            TabCmd::Stop(id) => {
                let ai = ai_dir(&library_dir(&app.settings));
                if let Err(e) = Queue::open(&ai).and_then(|q| q.cancel(id)) {
                    app.error(format!("{e:#}"));
                }
            }
            TabCmd::ResumeTask(id) => {
                let ai = ai_dir(&library_dir(&app.settings));
                if let Err(e) = Queue::open(&ai).and_then(|q| q.resume(id)) {
                    app.error(format!("{e:#}"));
                }
            }
        }
    }
}

/// Starts the analysis of `path` (or, with `from`, rebuilds it from that stage) with the stages of
/// `options` (what cannot run is switched off). It continues in the background; the studio shows
/// its progress.
fn analyse(app: &App, path: &Path, from: Option<Stage>, options: AnalysisOptions) -> anyhow::Result<()> {
    let agent = &app.settings.agent;
    anyhow::ensure!(agent.models.speech_recognition.is_some(), "{}", t!("studio.no_recognizer"));
    let options = options.resolved(&agent.models);
    let ai = ai_dir(&library_dir(&app.settings));
    match from {
        Some(from) => pipeline::restart(&ai, path, from, options)?,
        None => pipeline::start(&ai, path, options)?,
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_scope_is_the_selected_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut cat = Catalogs::open(&ai_dir(dir.path())).unwrap();
        let file = |name: &str| {
            let p = dir.path().join(name);
            std::fs::write(&p, b"x").unwrap();
            p
        };
        let work = cat.create_folder(None, "Работа").unwrap();
        let inner = cat.create_folder(Some(work), "Созвоны").unwrap();
        let a = cat.add_file(Some(inner), &file("a.mp3"), false).unwrap();
        let b = cat.add_file(None, &file("b.mp3"), false).unwrap();
        let (name, files) = scope(&cat, Some(work));
        assert_eq!(name.as_deref(), Some("Работа"));
        assert_eq!(files.iter().map(|(id, _)| *id).collect::<Vec<_>>(), [a], "the subtree");
        let (name, files) = scope(&cat, None);
        assert!(name.is_none());
        assert_eq!(files.len(), 2);
        // A selected file means everything.
        assert_eq!(scope(&cat, Some(b)).1.len(), 2);
    }

    #[test]
    fn hit_text_is_cut_at_a_word() {
        assert_eq!(shortened("коротко", 20), "коротко");
        assert_eq!(shortened("первое второе третье", 15), "первое второе…");
    }

    #[test]
    fn tab_title_job_is_one_row_cut_with_an_ellipsis() {
        let full = "очень длинное имя записи, которое не поместится в вкладку";
        let job = tab_title_job(full);
        assert_eq!(job.text, full, "the full title stays in the job");
        assert_eq!(job.wrap.max_width, TAB_TITLE_WIDTH);
        assert_eq!(job.wrap.max_rows, 1);
        assert!(job.wrap.break_anywhere);
        assert_eq!(job.wrap.overflow_character, Some('…'));
        assert_eq!(job.sections.len(), 1);
        assert_eq!(job.sections[0].format.color, Color32::PLACEHOLDER, "egui_dock colours the tab");
    }
}
