use super::{page_title, Ctx};
use crate::textures::Textures;
use crate::ui::theme;
use crate::ui_state::LibraryPageState;
use chrono::Datelike;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, Sense, Stroke};
use egui_phosphor::regular as icon;
use mediaa2_core::convert::ConversionOptions;
use mediaa2_core::formats::MediaKind;
use mediaa2_core::library::{file_url, human_duration, human_size, LibraryEntry};
use mediaa2_core::naming;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::query::LibrarySort;
use mediaa2_i18n::t;
use mediaa2_protocol::{
    ClientCommand, ConversionCommand, LibraryCommand, PreviewKind, PreviewState,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

const MIN_TILE: f32 = 200.0;
const TILE_GAP: f32 = 10.0;
const TILE_BORDER: f32 = 1.0;
const PAGE_LIMIT: u32 = 200;
const INFO_HEIGHT: f32 = 70.0;
const MEDIA_ASPECT: f32 = 0.62;
const STRIP_PAD: i8 = 8;
const TOOLBAR_ONE_LINE: f32 = 1380.0;

#[derive(Clone, Default)]
struct PendingDelete {
    paths: Vec<PathBuf>,
}

#[derive(Clone, Default)]
struct PendingRename {
    paths: Vec<PathBuf>,
    title: String,
}

#[derive(Clone, Copy)]
struct LoadRequest {
    offset: u32,
    entries: usize,
}

#[derive(Clone)]
enum CardAction {
    Viewer(PathBuf),
    Open(PathBuf),
    Reveal(PathBuf),
    CopyFile(PathBuf),
    CopyImage(PathBuf),
    CopyPath(PathBuf),
    CopyLink(PathBuf),
    Convert(PathBuf),
    Rename(PathBuf),
    ToggleSelect(PathBuf),
    Delete(PathBuf),
}

#[derive(Clone)]
struct Tool {
    icon: &'static str,
    label: String,
    action: CardAction,
    main: bool,
    danger: bool,
}

fn delete_id() -> egui::Id {
    egui::Id::new("mediaa2-library-delete")
}

fn rename_id() -> egui::Id {
    egui::Id::new("mediaa2-library-rename")
}

fn cleanup_id() -> egui::Id {
    egui::Id::new("mediaa2-library-cleanup")
}

fn load_id() -> egui::Id {
    egui::Id::new("mediaa2-library-load")
}

fn anchor_id() -> egui::Id {
    egui::Id::new("mediaa2-library-anchor")
}

struct CardCtx<'a> {
    previews: &'a HashMap<PathBuf, PreviewState>,
    hover_previews: &'a HashMap<PathBuf, PreviewState>,
    textures: &'a mut Textures,
    library: &'a mut LibraryPageState,
    entries: &'a [LibraryEntry],
    anchor: &'a mut Option<PathBuf>,
    requests: &'a mut Vec<(PathBuf, PreviewKind)>,
    hover_requests: &'a mut Vec<PathBuf>,
}

#[cfg(test)]
pub(crate) fn seed_delete(ctx: &egui::Context, paths: Vec<PathBuf>) {
    ctx.data_mut(|data| data.insert_temp(delete_id(), PendingDelete { paths }));
}

#[cfg(test)]
pub(crate) fn seed_rename(ctx: &egui::Context, paths: Vec<PathBuf>, title: String) {
    ctx.data_mut(|data| {
        data.insert_temp(rename_id(), PendingRename { paths, title });
    });
}

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let view_w = ui.available_width();
    page_title(ui, &t!("lib.title"), "");
    let mut pending_delete = ui
        .ctx()
        .data_mut(|data| data.get_temp::<PendingDelete>(delete_id()))
        .unwrap_or_default();
    let mut pending_rename = ui
        .ctx()
        .data_mut(|data| data.get_temp::<PendingRename>(rename_id()))
        .unwrap_or_default();
    let mut anchor = ui
        .ctx()
        .data_mut(|data| data.get_temp::<PathBuf>(anchor_id()));
    let query_before = ctx.ui.library.query.clone();

    toolbar(ctx, ui, view_w);
    if ctx.ui.library.query != query_before {
        ctx.ui.library.clear();
        anchor = None;
    }
    command_error(ctx, ui);
    selection_bar(ctx, ui, &mut pending_delete, &mut pending_rename);

    if ctx.view.library.entries.is_empty() {
        empty_state(ctx, ui);
    } else {
        grid(
            ctx,
            ui,
            &mut pending_delete,
            &mut pending_rename,
            &mut anchor,
        );
        pagination(ctx, ui);
    }

    delete_modal(ctx, ui, &mut pending_delete, &mut anchor);
    rename_modal(ctx, ui, &mut pending_rename);
    cleanup_modal(ctx, ui);
    ui.ctx()
        .data_mut(|data| data.insert_temp(delete_id(), pending_delete.clone()));
    ui.ctx()
        .data_mut(|data| data.insert_temp(rename_id(), pending_rename.clone()));
    if let Some(path) = anchor.clone() {
        ui.ctx()
            .data_mut(|data| data.insert_temp(anchor_id(), path));
    } else {
        ui.ctx()
            .data_mut(|data| data.remove_temp::<PathBuf>(anchor_id()));
    }
    super::viewer::show(ctx, ui);
}

fn toolbar(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, view_w: f32) {
    let before_search = ctx.ui.library.query.search.clone();
    let before_kind = ctx.ui.library.query.kind;
    let before_sort = ctx.ui.library.query.sort;

    let mut search = ctx.ui.library.query.search.clone();
    let mut kind = ctx.ui.library.query.kind;
    let mut sort = ctx.ui.library.query.sort;
    let mut refresh = false;
    let mut open_folder = false;
    let mut cleanup = None;
    let total = ctx.view.library.total;

    let kind_options = vec![
        (None, t!("lib.all").to_string()),
        (Some(MediaKind::Image), t!("lib.images").to_string()),
        (Some(MediaKind::Video), t!("lib.videos").to_string()),
        (Some(MediaKind::Animation), t!("lib.animations").to_string()),
        (Some(MediaKind::Audio), t!("lib.audio").to_string()),
    ];

    if view_w >= TOOLBAR_ONE_LINE {
        ui.horizontal(|ui| {
            theme::segmented(ui, &mut kind, &kind_options);
            toolbar_controls(
                ui,
                &mut search,
                &mut sort,
                &mut refresh,
                &mut open_folder,
                &mut cleanup,
                total,
                view_w,
            );
        });
    } else {
        ui.horizontal(|ui| theme::segmented(ui, &mut kind, &kind_options));
        ui.horizontal_wrapped(|ui| {
            toolbar_controls(
                ui,
                &mut search,
                &mut sort,
                &mut refresh,
                &mut open_folder,
                &mut cleanup,
                total,
                view_w,
            );
        });
    }

    ctx.ui.library.query.search = search;
    ctx.ui.library.query.kind = kind;
    ctx.ui.library.query.sort = sort;

    if refresh {
        ctx.ui.library.query.offset = 0;
        ctx.actions
            .send(ClientCommand::Library(LibraryCommand::Refresh));
    }
    if open_folder {
        open_media_folder(ctx);
    }
    if let Some(days) = cleanup {
        ui.ctx()
            .data_mut(|data| data.insert_temp(cleanup_id(), days));
    }
    if ctx.ui.library.query.search != before_search
        || ctx.ui.library.query.kind != before_kind
        || ctx.ui.library.query.sort != before_sort
    {
        request_page(ctx, 0);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the toolbar controls mirror the old library layout"
)]
fn toolbar_controls(
    ui: &mut egui::Ui,
    search: &mut String,
    sort: &mut LibrarySort,
    refresh: &mut bool,
    open_folder: &mut bool,
    cleanup: &mut Option<u32>,
    total: u32,
    view_w: f32,
) {
    let sort_options = [
        (LibrarySort::Newest, t!("lib.sort_newest").to_string()),
        (LibrarySort::Oldest, t!("lib.sort_oldest").to_string()),
        (LibrarySort::Title, t!("lib.sort_title").to_string()),
        (LibrarySort::Largest, t!("lib.sort_largest").to_string()),
    ];

    ui.add(
        egui::TextEdit::singleline(search)
            .hint_text(format!("{}  {}", icon::MAGNIFYING_GLASS, t!("lib.search")))
            .desired_width((view_w - 260.0).clamp(120.0, 220.0)),
    )
    .on_hover_text(t!("lib.search_help"));
    theme::combo_box("library-sort")
        .width(150.0)
        .selected_text(sort_label(*sort))
        .show_ui(ui, |ui| {
            for (value, label) in &sort_options {
                ui.selectable_value(sort, *value, label.as_str());
            }
        });
    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked() {
        *refresh = true;
    }
    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("lib.open_folder")).clicked() {
        *open_folder = true;
    }
    theme::hint(ui, t!("lib.count", n = total));
    if ui
        .add(theme::secondary(icon::CALENDAR_X, &t!("lib.cleanup_week")))
        .clicked()
    {
        *cleanup = Some(7);
    }
    if ui
        .add(theme::secondary(icon::CALENDAR_X, &t!("lib.cleanup_month")))
        .clicked()
    {
        *cleanup = Some(30);
    }
}

fn sort_label(sort: LibrarySort) -> String {
    match sort {
        LibrarySort::Newest => t!("lib.sort_newest").to_string(),
        LibrarySort::Oldest => t!("lib.sort_oldest").to_string(),
        LibrarySort::Title => t!("lib.sort_title").to_string(),
        LibrarySort::Largest => t!("lib.sort_largest").to_string(),
    }
}

fn request_page(ctx: &mut Ctx<'_>, offset: u32) {
    ctx.ui.library.query.offset = offset;
    ctx.ui.library.query.limit = PAGE_LIMIT;
    let query = ctx.ui.library.query.clone();
    ctx.actions
        .send(ClientCommand::Library(LibraryCommand::Query(query)));
}

fn command_error(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let Some(error) = ctx.command_error else {
        return;
    };
    let p = theme::palette(ui);
    ui.label(egui::RichText::new(error).size(12.0).color(p.danger));
}

fn open_media_folder(ctx: &mut Ctx<'_>) {
    let dir = ctx.view.settings().media_dir.clone();
    if let Err(error) = mediaa2_platform::files::open(&dir) {
        report(ctx, error);
    }
}

fn report(ctx: &mut Ctx<'_>, error: anyhow::Error) {
    ctx.actions.notice(
        NoticeLevel::Error,
        t!("notice.error").to_string(),
        format!("{error:#}"),
    );
}

fn empty_state(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let p = theme::palette(ui);
    let text = if ctx.view.library.total == 0 {
        t!("lib.empty").to_string()
    } else {
        t!("lib.nothing_found").to_string()
    };
    ui.add_space(4.0);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.vertical_centered(|ui| {
            ui.add_space(14.0);
            ui.label(
                egui::RichText::new(icon::IMAGES)
                    .size(32.0)
                    .color(p.text_weak),
            );
            ui.label(egui::RichText::new(text).size(13.0));
            ui.add_space(14.0);
        });
    });
}

fn selection_bar(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    pending_delete: &mut PendingDelete,
    pending_rename: &mut PendingRename,
) {
    let count = ctx.ui.library.selection.len();
    if count == 0 {
        return;
    }
    let paths: Vec<PathBuf> = ctx.ui.library.selection.iter().cloned().collect();
    let p = theme::palette(ui);
    let label = t!("lib.selected", n = count).to_string();
    let mut convert = false;
    let mut confirm = false;
    let mut clear = false;
    let mut copy_files = false;
    let mut copy_paths = false;
    let mut rename = false;
    let gui = ui.ctx().clone();

    egui::Area::new(egui::Id::new("mediaa2-library-selection"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 62.0))
        .show(&gui, |ui| {
            egui::Frame::new()
                .fill(p.card)
                .stroke(Stroke::new(1.0, p.border))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::symmetric(6, 4))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 12,
                    spread: 0,
                    color: Color32::from_black_alpha(48),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let button = ui.add(theme::primary(ui, icon::CHECK, &label));
                        egui::Popup::menu(&button).show(|ui| {
                            ui.set_min_width(190.0);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            if theme::menu_item(ui, icon::COPY, &t!("lib.copy_files"), false)
                                .clicked()
                            {
                                copy_files = true;
                            }
                            if theme::menu_item(ui, icon::CLIPBOARD, &t!("lib.copy_paths"), false)
                                .clicked()
                            {
                                copy_paths = true;
                            }
                            if theme::menu_item(ui, icon::SHUFFLE, &t!("lib.convert"), false)
                                .clicked()
                            {
                                convert = true;
                            }
                            if theme::menu_item(ui, icon::PENCIL_SIMPLE, &t!("lib.rename"), false)
                                .clicked()
                            {
                                rename = true;
                            }
                            if theme::menu_item(ui, icon::X, &t!("lib.clear_selection"), false)
                                .clicked()
                            {
                                clear = true;
                            }
                            ui.separator();
                            if theme::menu_item(ui, icon::TRASH, &t!("lib.delete"), true).clicked()
                            {
                                confirm = true;
                            }
                        });
                        if theme::icon_button(ui, icon::X, &t!("lib.clear_selection")).clicked() {
                            clear = true;
                        }
                    });
                });
        });

    if copy_files {
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        match mediaa2_platform::clipboard::copy_files(&refs) {
            Ok(()) => ctx.actions.notice(
                NoticeLevel::Success,
                t!("lib.copied_file").to_string(),
                String::new(),
            ),
            Err(error) => report(ctx, error),
        }
    }
    if copy_paths {
        let text = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("\n");
        match mediaa2_platform::clipboard::copy_text(&text) {
            Ok(()) => ctx.actions.notice(
                NoticeLevel::Success,
                t!("lib.copied_path").to_string(),
                String::new(),
            ),
            Err(error) => report(ctx, error),
        }
    }
    if convert {
        start_conversion(ctx, paths.clone());
    }
    if rename {
        pending_rename.paths = paths.clone();
        pending_rename.title = String::new();
    }
    if confirm {
        pending_delete.paths = paths;
    }
    if clear {
        ctx.ui.library.clear();
    }
}

fn start_conversion(ctx: &mut Ctx<'_>, inputs: Vec<PathBuf>) {
    if inputs.is_empty() {
        return;
    }
    if ctx.view.conversion_busy() {
        ctx.actions.notice(
            NoticeLevel::Warning,
            t!("convert.busy").to_string(),
            String::new(),
        );
        return;
    }
    let output_dir = if ctx.ui.convert.output_dir.as_os_str().is_empty() {
        ctx.view.settings().convert_dir()
    } else {
        ctx.ui.convert.output_dir.clone()
    };
    ctx.actions
        .send(ClientCommand::Conversion(ConversionCommand::Start {
            inputs,
            target: ConversionOptions::default(),
            output_dir,
        }));
}

fn grid(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    pending_delete: &mut PendingDelete,
    pending_rename: &mut PendingRename,
    anchor: &mut Option<PathBuf>,
) {
    let (columns, tile_width) = grid_columns(ui.available_width());
    let entries = &ctx.view.library.entries;
    let previews = &ctx.view.previews;
    let hover_previews = &ctx.view.hover_previews;
    let textures = &mut ctx.textures;
    let library = &mut ctx.ui.library;
    let mut actions = Vec::new();
    let mut requests: Vec<(PathBuf, PreviewKind)> = Vec::new();
    let mut hover_requests: Vec<PathBuf> = Vec::new();
    let refs: Vec<&LibraryEntry> = entries.iter().collect();
    let groups: Vec<(Option<chrono::NaiveDate>, Vec<&LibraryEntry>)> =
        if by_date(library.query.sort) {
            group_entries(&refs)
                .into_iter()
                .map(|(day, list)| (Some(day), list))
                .collect()
        } else {
            vec![(None, refs)]
        };

    for (day, items) in groups {
        if let Some(day) = day {
            if date_divider(ui, &day_label(day)) {
                select_day(library, &items, anchor);
            }
        }
        for row in items.chunks(columns) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = TILE_GAP;
                for entry in row {
                    let mut card_ctx = CardCtx {
                        previews,
                        hover_previews,
                        textures,
                        library,
                        entries,
                        anchor,
                        requests: &mut requests,
                        hover_requests: &mut hover_requests,
                    };
                    if let Some(action) = card(&mut card_ctx, ui, entry, tile_width) {
                        actions.push(action);
                    }
                }
            });
            ui.add_space(TILE_GAP - ui.spacing().item_spacing.y);
        }
    }

    if !requests.is_empty() {
        let key = egui::Id::new("mediaa2-library-preview-requests");
        let mut requested: HashSet<PathBuf> =
            ui.data_mut(|data| data.get_temp(key).unwrap_or_default());
        for (path, kind) in requests {
            if requested.insert(path.clone()) {
                ctx.actions.send(ClientCommand::Preview { path, kind });
            }
        }
        ui.data_mut(|data| data.insert_temp(key, requested));
    }

    if !hover_requests.is_empty() {
        let key = egui::Id::new("mediaa2-library-hover-requests");
        let mut requested: HashSet<PathBuf> =
            ui.data_mut(|data| data.get_temp(key).unwrap_or_default());
        for path in hover_requests {
            if requested.insert(path.clone()) {
                ctx.actions.send(ClientCommand::Preview {
                    path,
                    kind: PreviewKind::Hover,
                });
            }
        }
        ui.data_mut(|data| data.insert_temp(key, requested));
    }

    for action in actions {
        apply_card_action(ctx, action, pending_delete, pending_rename);
    }
}

fn by_date(sort: LibrarySort) -> bool {
    matches!(sort, LibrarySort::Newest | LibrarySort::Oldest)
}

fn group_entries<'a>(
    entries: &[&'a LibraryEntry],
) -> Vec<(chrono::NaiveDate, Vec<&'a LibraryEntry>)> {
    let mut groups: Vec<(chrono::NaiveDate, Vec<&LibraryEntry>)> = Vec::new();
    for entry in entries {
        let day = entry.taken.date();
        match groups.last_mut() {
            Some((known, list)) if *known == day => list.push(entry),
            _ => groups.push((day, vec![entry])),
        }
    }
    groups
}

fn day_label(day: chrono::NaiveDate) -> String {
    let today = chrono::Local::now().date_naive();
    if day == today {
        return t!("lib.today").to_string();
    }
    if Some(day) == today.pred_opt() {
        return t!("lib.yesterday").to_string();
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
    .to_string()
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

fn select_day(
    library: &mut LibraryPageState,
    entries: &[&LibraryEntry],
    anchor: &mut Option<PathBuf>,
) {
    let all_selected =
        !entries.is_empty() && entries.iter().all(|entry| library.is_selected(&entry.path));
    if all_selected {
        for entry in entries {
            library.selection.remove(&entry.path);
        }
        *anchor = None;
    } else {
        for entry in entries {
            library.selection.insert(entry.path.clone());
        }
        *anchor = entries.last().map(|entry| entry.path.clone());
    }
}

fn select_click(
    library: &mut LibraryPageState,
    entries: &[LibraryEntry],
    anchor: &mut Option<PathBuf>,
    path: &Path,
    ctrl: bool,
    shift: bool,
) {
    if shift {
        let base = anchor.clone().unwrap_or_else(|| path.to_path_buf());
        let from = entries.iter().position(|entry| entry.path == base);
        let to = entries.iter().position(|entry| entry.path == path);
        if let (Some(from), Some(to)) = (from, to) {
            let (from, to) = if from <= to { (from, to) } else { (to, from) };
            library.selection = entries[from..=to]
                .iter()
                .map(|entry| entry.path.clone())
                .collect();
        } else {
            library.select_only(path);
        }
        *anchor = Some(path.to_path_buf());
        return;
    }
    if ctrl {
        library.toggle(path);
        *anchor = Some(path.to_path_buf());
        return;
    }
    library.select_only(path);
    *anchor = Some(path.to_path_buf());
}

fn grid_columns(available: f32) -> (usize, f32) {
    let columns = (((available + TILE_GAP) / (MIN_TILE + TILE_GAP)).floor() as usize).max(1);
    let gaps = TILE_GAP * (columns.saturating_sub(1)) as f32;
    let width = ((available - gaps) / columns as f32).floor().max(1.0);
    (columns, width)
}

fn card(
    ctx: &mut CardCtx<'_>,
    ui: &mut egui::Ui,
    entry: &LibraryEntry,
    width: f32,
) -> Option<CardAction> {
    let width = width - 2.0 * TILE_BORDER;
    let p = theme::palette(ui);
    let media_height = (width * MEDIA_ASPECT).round().max(1.0);
    let tile_rect = Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(width, media_height + INFO_HEIGHT),
    );
    let hovered = ctx.library.hovered.as_deref() == Some(entry.path.as_path())
        || (ui.is_rect_visible(tile_rect)
            && ui
                .ctx()
                .pointer_hover_pos()
                .is_some_and(|pos| tile_rect.contains(pos)));
    let selected = ctx.library.is_selected(&entry.path);
    let tools = card_tools(entry);
    let mut action = None;
    let mut media_resp: Option<egui::Response> = None;
    let mut name_resp: Option<egui::Response> = None;
    let mut strip_clicked = false;

    egui::Frame::new()
        .fill(if selected { p.accent_soft } else { p.card })
        .stroke(Stroke::new(2.0, if selected { p.accent } else { p.border }))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.set_max_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let size = egui::vec2(width, media_height);
                let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
                media_resp = Some(response.clone());
                let painter = ui.painter_at(rect);
                let top = CornerRadius {
                    nw: 10,
                    ne: 10,
                    sw: 0,
                    se: 0,
                };
                painter.rect_filled(rect, top, p.card_hover);

                let preview = ctx.previews.get(&entry.path).cloned();
                let hover_texture = if hovered
                    && matches!(entry.kind, MediaKind::Video | MediaKind::Animation)
                {
                    if let Some(PreviewState::Ready(file)) = ctx.hover_previews.get(&entry.path) {
                        ctx.textures.hover_frame(
                            ui.ctx(),
                            &entry.path,
                            file,
                            entry.details.and_then(|details| details.duration_secs),
                        )
                    } else {
                        ctx.hover_requests.push(entry.path.clone());
                        None
                    }
                } else {
                    None
                };
                let texture = match hover_texture {
                    Some(texture) => Some((texture.id(), texture.size_vec2())),
                    None => match preview {
                        Some(PreviewState::Ready(file)) => ctx
                            .textures
                            .get_or_load(ui.ctx(), &file)
                            .map(|texture| (texture.id(), texture.size_vec2())),
                        _ => {
                            if entry.kind != MediaKind::Audio && ui.is_rect_visible(tile_rect) {
                                ctx.requests
                                    .push((entry.path.clone(), PreviewKind::Thumbnail));
                            }
                            None
                        }
                    },
                };
                match texture {
                    Some((texture_id, texture_size)) => {
                        egui::Image::new(egui::load::SizedTexture::new(texture_id, size))
                            .uv(cover_uv(texture_size, size))
                            .corner_radius(top)
                            .paint_at(ui, rect);
                    }
                    None => {
                        let kind_icon = kind_icon(entry.kind);
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            kind_icon,
                            FontId::proportional(32.0),
                            p.text_weak,
                        );
                    }
                }

                let pad = 7.0;
                badge(
                    &painter,
                    rect.left_top() + egui::vec2(pad, pad),
                    egui::Align2::LEFT_TOP,
                    &entry.extension(),
                );
                badge(
                    &painter,
                    rect.right_top() + egui::vec2(-pad, pad),
                    egui::Align2::RIGHT_TOP,
                    &human_size(entry.size),
                );
                let bottom = rect.bottom() - pad;
                if let Some(details) = entry.details.filter(|details| details.width > 0) {
                    badge(
                        &painter,
                        egui::pos2(rect.left() + pad, bottom),
                        egui::Align2::LEFT_BOTTOM,
                        &format!("{}×{}", details.width, details.height),
                    );
                }
                if entry.kind != MediaKind::Image {
                    let mut text = icon::PLAY.to_string();
                    if let Some(seconds) = entry.details.and_then(|details| details.duration_secs) {
                        text = format!("{text} {}", human_duration(seconds));
                    }
                    if entry.details.is_some_and(|details| details.audio) {
                        text = format!("{text}  {}", icon::SPEAKER_HIGH);
                    }
                    badge(
                        &painter,
                        egui::pos2(rect.right() - pad, bottom),
                        egui::Align2::RIGHT_BOTTOM,
                        &text,
                    );
                }

                if response.clicked() && !response.double_clicked() {
                    let (ctrl, shift) =
                        ui.input(|input| (input.modifiers.command, input.modifiers.shift));
                    select_click(
                        ctx.library,
                        ctx.entries,
                        ctx.anchor,
                        &entry.path,
                        ctrl,
                        shift,
                    );
                }
                if response.double_clicked() {
                    action = Some(CardAction::Viewer(entry.path.clone()));
                }

                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(STRIP_PAD, 6))
                    .show(ui, |ui| {
                        let strip = ui.interact(
                            ui.max_rect(),
                            ui.id().with(("library-strip", &entry.path)),
                            Sense::click(),
                        );
                        strip_clicked = strip.clicked();
                        let inner = width - 2.0 * f32::from(STRIP_PAD);
                        ui.set_width(inner);
                        ui.set_max_width(inner);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let label = if hovered {
                            egui::RichText::new(&entry.name)
                                .size(12.5)
                                .color(p.text_weak)
                        } else {
                            egui::RichText::new(&entry.title).size(12.5)
                        };
                        name_resp =
                            Some(ui.add(egui::Label::new(label).truncate().sense(Sense::click())));
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                            for tool in tools.iter().filter(|tool| tool.main) {
                                if theme::icon_button(ui, tool.icon, &tool.label).clicked() {
                                    action = Some(tool.action.clone());
                                    strip_clicked = false;
                                }
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    let more = theme::icon_button(
                                        ui,
                                        icon::DOTS_THREE_CIRCLE,
                                        &t!("lib.more"),
                                    );
                                    if let Some(chosen) =
                                        tools_menu(egui::Popup::menu(&more), &tools)
                                    {
                                        action = Some(chosen);
                                        strip_clicked = false;
                                    }
                                },
                            );
                        });
                    });
            });
        });

    if action.is_none() && (strip_clicked || name_resp.as_ref().is_some_and(|r| r.clicked())) {
        let (ctrl, shift) = ui.input(|input| (input.modifiers.command, input.modifiers.shift));
        select_click(
            ctx.library,
            ctx.entries,
            ctx.anchor,
            &entry.path,
            ctrl,
            shift,
        );
    }
    if let Some(target) = media_resp
        .into_iter()
        .chain(name_resp)
        .reduce(|a, b| a.union(b))
    {
        if let Some(chosen) = tools_menu(egui::Popup::context_menu(&target), &tools) {
            action = Some(chosen);
        }
    }
    action
}

fn kind_icon(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => icon::IMAGE,
        MediaKind::Video => icon::FILE_VIDEO,
        MediaKind::Animation => icon::GIF,
        MediaKind::Audio => icon::MUSIC_NOTES,
    }
}

fn card_tools(entry: &LibraryEntry) -> Vec<Tool> {
    let tool = |icon: &'static str, label: String, action: CardAction, main: bool| Tool {
        icon,
        label,
        action,
        main,
        danger: false,
    };
    let path = &entry.path;
    let mut list = vec![
        tool(
            icon::ARROW_SQUARE_OUT,
            t!("lib.open").to_string(),
            CardAction::Open(path.clone()),
            true,
        ),
        tool(
            icon::FOLDER_OPEN,
            t!("lib.reveal").to_string(),
            CardAction::Reveal(path.clone()),
            true,
        ),
        tool(
            icon::COPY,
            t!("lib.copy_file").to_string(),
            CardAction::CopyFile(path.clone()),
            true,
        ),
    ];
    if entry.kind == MediaKind::Image {
        list.push(tool(
            icon::IMAGE,
            t!("lib.copy_image").to_string(),
            CardAction::CopyImage(path.clone()),
            false,
        ));
    }
    list.extend([
        tool(
            icon::CLIPBOARD,
            t!("lib.copy_path").to_string(),
            CardAction::CopyPath(path.clone()),
            false,
        ),
        tool(
            icon::LINK,
            t!("lib.copy_link").to_string(),
            CardAction::CopyLink(path.clone()),
            false,
        ),
        tool(
            icon::SHUFFLE,
            t!("lib.convert").to_string(),
            CardAction::Convert(path.clone()),
            true,
        ),
        tool(
            icon::PENCIL_SIMPLE,
            t!("lib.rename").to_string(),
            CardAction::Rename(path.clone()),
            false,
        ),
        tool(
            icon::CHECK,
            t!("lib.select").to_string(),
            CardAction::ToggleSelect(path.clone()),
            false,
        ),
    ]);
    list.push(Tool {
        danger: true,
        ..tool(
            icon::TRASH,
            t!("lib.delete").to_string(),
            CardAction::Delete(path.clone()),
            false,
        )
    });
    list
}

fn tools_menu(popup: egui::Popup<'_>, tools: &[Tool]) -> Option<CardAction> {
    popup
        .show(|ui| {
            ui.set_min_width(190.0);
            ui.spacing_mut().item_spacing.y = 2.0;
            let mut chosen = None;
            for tool in tools {
                if tool.danger {
                    ui.separator();
                }
                if theme::menu_item(ui, tool.icon, &tool.label, tool.danger).clicked() {
                    chosen = Some(tool.action.clone());
                }
            }
            chosen
        })
        .and_then(|result| result.inner)
}

fn badge(painter: &egui::Painter, pos: egui::Pos2, anchor: egui::Align2, text: &str) {
    let galley =
        painter.layout_no_wrap(text.to_owned(), FontId::proportional(11.0), Color32::WHITE);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = anchor.anchor_size(pos, size);
    painter.rect_filled(rect, CornerRadius::same(5), Color32::from_black_alpha(165));
    painter.galley(rect.min + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
}

fn cover_uv(texture: egui::Vec2, target: egui::Vec2) -> Rect {
    let texture_aspect = texture.x / texture.y.max(1.0);
    let target_aspect = target.x / target.y.max(1.0);
    if texture_aspect > target_aspect {
        let width = target_aspect / texture_aspect;
        Rect::from_min_max(
            egui::pos2((1.0 - width) / 2.0, 0.0),
            egui::pos2((1.0 + width) / 2.0, 1.0),
        )
    } else {
        let height = texture_aspect / target_aspect;
        Rect::from_min_max(
            egui::pos2(0.0, (1.0 - height) / 2.0),
            egui::pos2(1.0, (1.0 + height) / 2.0),
        )
    }
}

fn apply_card_action(
    ctx: &mut Ctx<'_>,
    action: CardAction,
    pending_delete: &mut PendingDelete,
    pending_rename: &mut PendingRename,
) {
    match action {
        CardAction::Viewer(path) => ctx.ui.open_viewer(path),
        CardAction::Open(path) => {
            if let Err(error) = mediaa2_platform::files::open(&path) {
                report(ctx, error);
            }
        }
        CardAction::Reveal(path) => {
            if let Err(error) = mediaa2_platform::files::reveal(&path) {
                report(ctx, error);
            }
        }
        CardAction::CopyFile(path) => {
            match mediaa2_platform::clipboard::copy_files(&[path.as_path()]) {
                Ok(()) => ctx.actions.notice(
                    NoticeLevel::Success,
                    t!("lib.copied_file").to_string(),
                    String::new(),
                ),
                Err(error) => report(ctx, error),
            }
        }
        CardAction::CopyImage(path) => {
            let copied = image::open(&path)
                .map_err(anyhow::Error::from)
                .and_then(|image| {
                    mediaa2_platform::clipboard::copy_image(mediaa2_core::frame::Frame::from_image(
                        image.to_rgba8(),
                    ))
                });
            match copied {
                Ok(()) => ctx.actions.notice(
                    NoticeLevel::Success,
                    t!("lib.copied_image").to_string(),
                    String::new(),
                ),
                Err(error) => report(ctx, error),
            }
        }
        CardAction::CopyPath(path) => {
            let text = path.to_string_lossy().into_owned();
            match mediaa2_platform::clipboard::copy_text(&text) {
                Ok(()) => ctx.actions.notice(
                    NoticeLevel::Success,
                    t!("lib.copied_path").to_string(),
                    String::new(),
                ),
                Err(error) => report(ctx, error),
            }
        }
        CardAction::CopyLink(path) => {
            match mediaa2_platform::clipboard::copy_text(&file_url(&path)) {
                Ok(()) => ctx.actions.notice(
                    NoticeLevel::Success,
                    t!("lib.copied_link").to_string(),
                    String::new(),
                ),
                Err(error) => report(ctx, error),
            }
        }
        CardAction::Convert(path) => start_conversion(ctx, vec![path]),
        CardAction::Rename(path) => {
            pending_rename.title = entry_title(ctx, &path);
            pending_rename.paths = vec![path];
        }
        CardAction::ToggleSelect(path) => ctx.ui.library.toggle(&path),
        CardAction::Delete(path) => pending_delete.paths = vec![path],
    }
}

fn entry_title(ctx: &Ctx<'_>, path: &Path) -> String {
    ctx.view
        .library
        .entries
        .iter()
        .find(|entry| entry.path == path)
        .map(|entry| entry.title.clone())
        .or_else(|| naming::parse_path(path).map(|name| name.title))
        .unwrap_or_else(|| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
}

fn pagination(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let Some(next) = ctx.view.library.next_offset else {
        return;
    };
    let loaded = ctx.view.library.entries.len();
    let remaining = ctx.view.library.total.saturating_sub(loaded as u32);
    let last = ui
        .ctx()
        .data_mut(|data| data.get_temp::<LoadRequest>(load_id()));
    let mut request = false;

    ui.add_space(6.0);
    let button = ui.add(egui::Button::new(format!(
        "{}  {}",
        icon::CARET_DOWN,
        t!("lib.count", n = remaining)
    )));
    if button.clicked() {
        request = true;
    }
    if ui.is_rect_visible(button.rect) {
        let already = last.is_some_and(|last| last.offset == next && last.entries == loaded);
        if !already {
            request = true;
        }
    }
    if request {
        ui.ctx().data_mut(|data| {
            data.insert_temp(
                load_id(),
                LoadRequest {
                    offset: next,
                    entries: loaded,
                },
            )
        });
        request_page(ctx, next);
    }
}

fn cleanup_modal(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let Some(days) = ui.ctx().data(|data| data.get_temp::<u32>(cleanup_id())) else {
        return;
    };
    let title = if days >= 30 {
        t!("lib.cleanup_month").to_string()
    } else {
        t!("lib.cleanup_week").to_string()
    };
    let text = if days >= 30 {
        t!("lib.cleanup_month_confirm").to_string()
    } else {
        t!("lib.cleanup_week_confirm").to_string()
    };
    let mut confirm = false;
    let mut cancel = false;
    let modal = egui::Modal::new(cleanup_id())
        .frame(theme::card(ui))
        .show(ui.ctx(), |ui| {
            ui.set_width(380.0);
            theme::popup_title(ui, &title);
            ui.label(egui::RichText::new(text).size(12.0));
            ui.add_space(10.0);
            let clear = t!("lib.cleanup_confirm").to_string();
            let keep = t!("common.cancel").to_string();
            let responses = theme::centered_buttons(
                ui,
                &[
                    theme::ModalButton {
                        kind: theme::ModalButtonKind::Danger,
                        icon: icon::CALENDAR_X,
                        label: &clear,
                        enabled: true,
                    },
                    theme::ModalButton {
                        kind: theme::ModalButtonKind::Secondary,
                        icon: icon::X,
                        label: &keep,
                        enabled: true,
                    },
                ],
            );
            confirm = responses[0].clicked();
            cancel = responses[1].clicked();
        });
    if confirm {
        ui.ctx()
            .data_mut(|data| data.remove_temp::<u32>(cleanup_id()));
        ctx.actions
            .send(ClientCommand::Library(LibraryCommand::Cleanup {
                older_days: days,
            }));
    } else if modal.should_close() || cancel {
        ui.ctx()
            .data_mut(|data| data.remove_temp::<u32>(cleanup_id()));
    }
}

fn delete_modal(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    pending: &mut PendingDelete,
    anchor: &mut Option<PathBuf>,
) {
    if pending.paths.is_empty() {
        return;
    }
    let count = pending.paths.len();
    let title = if count == 1 {
        t!("lib.delete_title").to_string()
    } else {
        t!("lib.delete_many_title").to_string()
    };
    let text = if count == 1 {
        let name = pending.paths[0]
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        t!("lib.delete_text", name = name).to_string()
    } else {
        t!("lib.count", n = count).to_string()
    };
    let confirm_label = t!("lib.delete_confirm").to_string();
    let keep_label = t!("lib.delete_keep").to_string();
    let mut confirmed = false;
    let mut cancelled = false;

    let modal = egui::Modal::new(delete_id())
        .frame(theme::card(ui))
        .show(ui.ctx(), |ui| {
            ui.set_width(if count == 1 { 340.0 } else { 360.0 });
            theme::popup_title(ui, &title);
            ui.label(text);
            ui.add_space(6.0);
            let buttons = [
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Danger,
                    icon: icon::TRASH,
                    label: &confirm_label,
                    enabled: true,
                },
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Secondary,
                    icon: icon::X,
                    label: &keep_label,
                    enabled: true,
                },
            ];
            let responses = theme::centered_buttons(ui, &buttons);
            if responses[0].clicked() {
                confirmed = true;
            }
            if responses[1].clicked() {
                cancelled = true;
            }
        });

    if modal.should_close() {
        cancelled = true;
    }
    if confirmed {
        let paths = std::mem::take(&mut pending.paths);
        for path in &paths {
            ctx.ui.library.selection.remove(path);
        }
        if anchor
            .as_ref()
            .is_some_and(|current| paths.contains(current))
        {
            *anchor = None;
        }
        ctx.actions
            .send(ClientCommand::Library(LibraryCommand::Delete {
                paths,
                permanent: false,
            }));
    } else if cancelled {
        pending.paths.clear();
    }
}

fn rename_modal(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, pending: &mut PendingRename) {
    if pending.paths.is_empty() {
        return;
    }
    let single = pending.paths.len() == 1;
    let path = pending.paths[0].clone();
    let current = naming::parse_path(&path);
    let ext = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_string();
    let title = if single {
        t!("lib.rename_title").to_string()
    } else {
        t!("lib.rename_many_title").to_string()
    };
    let hint = if single {
        current
            .as_ref()
            .map(|current| {
                let preview = naming::MediaName {
                    title: naming::sanitize(&pending.title, naming::MAX_TITLE),
                    ..current.clone()
                };
                t!(
                    "lib.rename_file",
                    name = format!("{}.{ext}", preview.stem())
                )
                .to_string()
            })
            .unwrap_or_default()
    } else {
        t!("lib.count", n = pending.paths.len()).to_string()
    };
    let confirm_label = t!("lib.rename_confirm").to_string();
    let cancel_label = t!("common.cancel").to_string();
    let mut draft = pending.title.clone();
    let mut confirmed = false;
    let mut cancelled = false;

    let modal = egui::Modal::new(rename_id())
        .frame(theme::card(ui))
        .show(ui.ctx(), |ui| {
            ui.set_width(if single { 380.0 } else { 400.0 });
            theme::popup_title(ui, &title);
            let edit = ui.add(egui::TextEdit::singleline(&mut draft).desired_width(f32::INFINITY));
            if !edit.has_focus() && ui.memory(|memory| memory.focused().is_none()) {
                edit.request_focus();
            }
            if !hint.is_empty() {
                theme::hint(ui, hint.clone());
            }
            ui.add_space(6.0);
            let buttons = [
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Primary,
                    icon: icon::CHECK,
                    label: &confirm_label,
                    enabled: true,
                },
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Secondary,
                    icon: icon::X,
                    label: &cancel_label,
                    enabled: true,
                },
            ];
            let responses = theme::centered_buttons(ui, &buttons);
            if responses[0].clicked() || ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                confirmed = true;
            }
            if responses[1].clicked() {
                cancelled = true;
            }
        });

    if modal.should_close() {
        cancelled = true;
    }
    if confirmed {
        let paths = std::mem::take(&mut pending.paths);
        let base = naming::sanitize(&draft, naming::MAX_TITLE);
        for (index, path) in paths.iter().enumerate() {
            let value = if single {
                base.clone()
            } else {
                format!("{base} {}", index + 1)
            };
            let target = match naming::rename_target(path, &value) {
                Ok(target) => target,
                Err(error) => {
                    report(ctx, error);
                    continue;
                }
            };
            ctx.actions
                .send(ClientCommand::Library(LibraryCommand::Rename {
                    from: path.clone(),
                    to: target.clone(),
                    title: value,
                }));
            ctx.ui.library.selection.remove(path);
            ctx.ui.library.selection.insert(target);
        }
    } else if cancelled {
        pending.paths.clear();
    } else {
        pending.title = draft;
    }
}
