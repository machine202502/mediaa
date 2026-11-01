use crate::logic::library::viewer::{Viewer, ViewerAction};
use crate::logic::library::{
    LibraryAction, LibraryController, Tool, CHUNK, GAP, MIN_TILE, STRIP_PAD, TILE_BORDER,
};
use crate::ui::pages::ctx::Ctx;
use crate::ui::pages::header;
use crate::ui::pages::library::hover_preview::Previews;
use crate::ui::pages::library::thumbs::Thumbnails;
use crate::ui::pages::library::viewer::ViewerTexture;
use crate::ui::theme;
use chrono::Datelike;
use eframe::egui::{self, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;
use mediaa_core::library::{self, Details, LibraryEntry};
use mediaa_core::{MediaKind, Settings};
use mediaa_storage::library_index::{LibraryIndex, Sort};
use rust_i18n::t;

use super::actions::{
    apply_cleanup_report, cleanup_dialog, delete_dialog, delete_many_dialog, finish_cleanup,
    rename_dialog, rename_many_dialog, run_action,
};
use super::grid::{grid_columns, kind_filter, library_tile, toolbar_tools, TOOLBAR_ONE_LINE};
pub(crate) const MAIN_TOOLS: usize = 4;
const _: () = assert!(
    MIN_TILE
        >= (MAIN_TOOLS + 1) as f32 * theme::ICON_BUTTON
            + 2.0 * STRIP_PAD as f32
            + 2.0 * TILE_BORDER,
    "the main tools and the menu button fit a minimal tile"
);

pub(crate) fn tools(e: &LibraryEntry) -> Vec<Tool> {
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

pub(crate) fn tools_strip(ui: &mut egui::Ui, all: &[Tool]) -> (Option<LibraryAction>, bool) {
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

pub(crate) fn tools_menu(popup: egui::Popup<'_>, tools: &[Tool]) -> Option<LibraryAction> {
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

pub(crate) fn sort_label(s: Sort) -> String {
    match s {
        Sort::Newest => t!("lib.sort_newest"),
        Sort::Oldest => t!("lib.sort_oldest"),
        Sort::Title => t!("lib.sort_title"),
        Sort::Largest => t!("lib.sort_largest"),
    }
    .into()
}

impl LibraryController {
    pub(crate) fn ui(
        &mut self,
        settings: &Settings,
        wake: &crate::wake::Wake,
        thumbs: &mut Thumbnails,
        previews: &mut Previews,
        viewer_texture: &mut ViewerTexture,
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
        if let Some((dir, names)) = self.poll(settings, wake) {
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
            let gui = ui.ctx().clone();
            egui::Area::new(egui::Id::new("library-bulk-floating"))
                .order(egui::Order::Foreground)
                .anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 54.0))
                .show(&gui, |ui| {
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
            previews.stop();
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
                        if let Some(a) =
                            library_tile(self, thumbs, previews, ui, e, tile_w, &mut previewing)
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
            previews.stop();
        }
        if let Some(a) = action {
            actions.push(a);
        }
        self.hovered = self.hover_next.take();
        let viewer_action = self
            .viewer
            .as_mut()
            .and_then(|viewer| viewer.show(ui.ctx(), viewer_texture));
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

pub fn poll_cleanup(ctx: &mut Ctx<'_>) {
    if let Some((report, success)) = ctx.lib.poll_cleanup(ctx.wake) {
        apply_cleanup_report(ctx, &report);
        finish_cleanup(ctx, report, success);
    }
}

pub(crate) fn dialogs(ctx: &mut Ctx<'_>, gui: &egui::Context) {
    delete_dialog(ctx, gui);
    delete_many_dialog(ctx, gui);
    cleanup_dialog(ctx, gui);
    rename_dialog(ctx, gui);
    rename_many_dialog(ctx, gui);
}

pub(crate) fn apply_action(ctx: &mut Ctx<'_>, gui: &egui::Context, action: LibraryAction) {
    run_action(ctx, gui, action);
}

pub(crate) fn details_of(i: &mediaa_media::info::MediaInfo) -> Details {
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

pub(crate) fn overlay_badge(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: egui::Align2,
    text: String,
) {
    let galley = painter.layout_no_wrap(text, FontId::proportional(11.0), Color32::WHITE);
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = anchor.anchor_size(pos, size);
    painter.rect_filled(rect, CornerRadius::same(5), Color32::from_black_alpha(165));
    painter.galley(rect.min + egui::vec2(5.0, 2.0), galley, Color32::WHITE);
}

#[cfg(test)]
mod tests {
    use super::{tools, tools_strip, MAIN_TOOLS};
    use crate::logic::library::LibraryAction;

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
                if kind == MediaKind::Image { 10 } else { 9 },
                "{kind:?}"
            );
            assert!(
                list.iter()
                    .any(|tool| matches!(&tool.action, LibraryAction::ToggleSelect(_))),
                "{kind:?}: the menu can select the card"
            );
        }
    }

    #[test]
    fn more_button_opens_the_menu_and_its_items_act() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        use crate::logic::library::LibraryAction;
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
            |ui, chosen: &mut Option<LibraryAction>| {
                ui.set_width(300.0);
                if let Some(a) = tools_strip(ui, &list).0 {
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
            matches!(harness.state(), Some(LibraryAction::AskRename(_, t)) if t == "A"),
            "the item's action is returned"
        );
        assert!(
            harness.query_by_label(&rename).is_none(),
            "menu closes after a choice"
        );
        harness.get_by_label(&rust_i18n::t!("lib.reveal")).click();
        harness.run();
        assert!(matches!(harness.state(), Some(LibraryAction::Reveal(_))));
    }
}
