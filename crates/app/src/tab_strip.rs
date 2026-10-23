//! A strip of document tabs (chats today, studios later): fixed-width titles cut with «…»,
//! a «×» on the current tab and on the one under the pointer, and horizontal scrolling when
//! the tabs do not fit. The current tab is scrolled into view when it changes. No tooltips.

use crate::theme;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;
use std::hash::Hash;

/// Width of a tab: titles are cut to it.
pub const TAB_WIDTH: f32 = 140.0;
/// Height of a tab. The strip is always this plus [`theme::TAB_SCROLL_BAR`].
pub const TAB_HEIGHT: f32 = 24.0;
/// Room between a tab's edge and its title.
const PAD: f32 = 8.0;
/// Room the «×» takes on the right of a tab.
const CLOSE: f32 = 20.0;

/// One tab: a key, its title and whether it is the current one.
pub struct Tab<I> {
    pub id: I,
    pub title: String,
    pub active: bool,
    /// Tabs that cannot be closed (e.g. a chat that is not stored yet) get no «×».
    pub closable: bool,
}

impl<I> Tab<I> {
    pub fn new(id: I, title: impl Into<String>, active: bool) -> Tab<I> {
        Tab { id, title: title.into(), active, closable: true }
    }

    /// A tab without a «×».
    pub fn fixed(mut self) -> Tab<I> {
        self.closable = false;
        self
    }
}

/// What the user did with the strip.
pub enum TabAction<I> {
    /// A tab was clicked.
    Selected(I),
    /// A tab's «×» was clicked.
    Closed(I),
}

/// Tabs in a horizontally scrolling strip; returns the click (if any). Tabs show no tooltip;
/// `close_label` is the accessible name of the «×».
pub fn tab_strip<I: Copy + Eq + Hash>(
    ui: &mut egui::Ui,
    id_salt: impl Hash + std::fmt::Debug,
    tabs: &[Tab<I>],
    close_label: &str,
    roster: &crate::pages::agent::mentions::Roster,
) -> Option<TabAction<I>> {
    let strip_id = ui.id().with(id_salt);
    // The current tab changes: bring it into view (once, not every frame).
    let active = tabs.iter().position(|t| t.active);
    let scroll_id = strip_id.with("scroll");
    let scroll_to = ui.data(|d| d.get_temp::<Option<usize>>(scroll_id).flatten()) != active;
    if scroll_to {
        ui.data_mut(|d| d.insert_temp(scroll_id, active));
    }

    let mut action = None;
    // The bar's strip is reserved whether or not the bar is drawn, so titles do not move.
    let strip_h = TAB_HEIGHT + theme::TAB_SCROLL_BAR;
    let (strip_rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), strip_h), Sense::hover());
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(strip_rect));
    // A wheel over the strip scrolls it, as it does in VS Code.
    row.style_mut().always_scroll_the_only_direction = true;
    theme::tab_row_scroll(&mut row);
    egui::ScrollArea::horizontal()
        .id_salt(strip_id.with("area"))
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(&mut row, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    for (i, tab) in tabs.iter().enumerate() {
                        let (rect, resp) = ui.allocate_exact_size(Vec2::new(TAB_WIDTH, TAB_HEIGHT), Sense::click());
                        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, tab.active, &tab.title));
                        if scroll_to && Some(i) == active {
                            ui.scroll_to_rect(rect, None);
                        }
                        let hovered = resp.hovered();
                        let p = theme::palette(ui);
                        let color = if tab.active || hovered { p.text } else { p.text_weak };
                        let close_rect = egui::Rect::from_center_size(
                            egui::pos2(rect.max.x - CLOSE / 2.0 - 2.0, rect.center().y),
                            Vec2::splat(18.0),
                        );
                        let close = (tab.closable && (tab.active || hovered)).then(|| {
                            let r = ui.interact(close_rect, strip_id.with(("close", i)), Sense::click());
                            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, close_label));
                            r
                        });

                        if ui.is_rect_visible(rect) {
                            let (fill, stroke) = if tab.active {
                                (p.accent_soft, Stroke::new(1.0, p.accent))
                            } else if hovered {
                                (p.card_hover, Stroke::new(1.0, p.border))
                            } else {
                                (Color32::TRANSPARENT, Stroke::NONE)
                            };
                            let painter = ui.painter();
                            painter.rect(rect, CornerRadius::same(6), fill, stroke, egui::StrokeKind::Inside);
                            // Title, one row, cut with «…».
                            let title_w = (TAB_WIDTH - PAD - if close.is_some() { CLOSE } else { PAD }).max(10.0);
                            let mut job = egui::text::LayoutJob {
                                wrap: egui::text::TextWrapping {
                                    max_width: title_w,
                                    max_rows: 1,
                                    break_anywhere: true,
                                    overflow_character: Some('…'),
                                },
                                ..Default::default()
                            };
                            crate::pages::agent::mentions::append(
                                &mut job,
                                &tab.title,
                                roster,
                                egui::TextFormat { font_id: FontId::proportional(12.5), color, ..Default::default() },
                                &p,
                            );
                            let galley = painter.layout_job(job);
                            painter.galley(egui::pos2(rect.min.x + PAD, rect.center().y - galley.size().y / 2.0), galley, color);
                            if let Some(r) = &close {
                                if r.hovered() {
                                    painter.rect_filled(r.rect.shrink(1.0), CornerRadius::same(4), p.card_hover);
                                }
                                painter.text(r.rect.center(), Align2::CENTER_CENTER, icon::X, FontId::proportional(11.0), if r.hovered() { p.danger } else { p.text_weak });
                            }
                        }

                        if hovered {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if let Some(r) = close {
                            // No tooltips on tabs: neither on the title nor on the «×».
                            if r.clicked() {
                                action = Some(TabAction::Closed(tab.id));
                                continue;
                            }
                        }
                        if resp.clicked() {
                            action = Some(TabAction::Selected(tab.id));
                        }
                    }
                });
        });
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    fn harness<'a>(tabs: &'a [Tab<i64>]) -> egui_kittest::Harness<'a, Option<TabAction<i64>>> {
        egui_kittest::Harness::new_ui_state(
            move |ui, action: &mut Option<TabAction<i64>>| {
                ui.set_width(400.0);
                if let Some(a) = tab_strip(ui, "test-tabs", tabs, "Закрыть", &crate::pages::agent::mentions::Roster::default()) {
                    *action = Some(a);
                }
            },
            None,
        )
    }

    #[test]
    fn a_click_picks_a_tab_and_its_cross_closes_the_current_one() {
        use egui_kittest::kittest::Queryable;
        let tabs = [Tab::new(1i64, "Первый".to_string(), true), Tab::new(2, "Второй".to_string(), false)];
        // The current tab has a «×»; the other one gets it only under the pointer.
        let mut picked = harness(&tabs);
        picked.run();
        assert!(picked.query_by_label("Первый").is_some() && picked.query_by_label("Второй").is_some());
        picked.get_by_label("Второй").click();
        picked.run();
        assert!(matches!(picked.state(), Some(TabAction::Selected(2))), "a tab click picks it");

        let mut closed = harness(&tabs);
        closed.run();
        closed.get_by_label("Закрыть").click();
        closed.run();
        assert!(matches!(closed.state(), Some(TabAction::Closed(1))), "the «×» closes the current tab");
    }

    fn strip_height(n: usize, width: f32) -> f32 {
        let tabs: Vec<Tab<i64>> = (0..n).map(|i| Tab::new(i as i64, format!("Вкладка {i}"), i == 0)).collect();
        let mut harness = egui_kittest::Harness::new_ui_state(
            move |ui, height: &mut f32| {
                ui.set_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let y = ui.cursor().min.y;
                tab_strip(ui, "height", &tabs, "Закрыть", &crate::pages::agent::mentions::Roster::default());
                *height = ui.cursor().min.y - y;
            },
            0.0,
        );
        harness.run();
        *harness.state()
    }

    /// One tab and a row that does not fit reserve the same bar strip.
    #[test]
    fn the_row_height_does_not_depend_on_the_bar() {
        let one = strip_height(1, 400.0);
        let many = strip_height(8, 400.0);
        let expected = TAB_HEIGHT + theme::TAB_SCROLL_BAR;
        assert!((one - expected).abs() < 0.5, "one tab: {one} != {expected}");
        assert!((many - one).abs() < 0.5, "overflow changed the row: {one} -> {many}");
    }
}
