//! A strip of document tabs (chats today, studios later): fixed-width titles cut with «…»,
//! a «×» on the current tab and on the one under the pointer, and horizontal scrolling when
//! the tabs do not fit. The current tab is scrolled into view when it changes.

use crate::theme;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;
use std::hash::Hash;

/// Width of a tab: titles are cut to it.
pub const TAB_WIDTH: f32 = 140.0;
/// Height of a tab (the strip is a little taller with its scroll bar).
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

/// Tabs in a horizontally scrolling strip; returns the click (if any). `close_tooltip` is the
/// hint of the «×».
pub fn tab_strip<I: Copy + Eq + Hash>(
    ui: &mut egui::Ui,
    id_salt: impl Hash + std::fmt::Debug,
    tabs: &[Tab<I>],
    close_tooltip: &str,
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
    ui.scope(|ui| {
        // A wheel over the strip scrolls it, as it does in VS Code.
        ui.style_mut().always_scroll_the_only_direction = true;
        egui::ScrollArea::horizontal()
            .id_salt(strip_id.with("area"))
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
            .show(ui, |ui| {
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
                            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, close_tooltip));
                            r
                        });
                        let close_hovered = close.as_ref().is_some_and(|r| r.hovered());

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
                            let mut job = egui::text::LayoutJob::single_section(
                                tab.title.clone(),
                                egui::TextFormat { font_id: FontId::proportional(12.5), color, ..Default::default() },
                            );
                            job.wrap = egui::text::TextWrapping {
                                max_width: title_w,
                                max_rows: 1,
                                break_anywhere: true,
                                overflow_character: Some('…'),
                            };
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
                            let r = r.on_hover_text(close_tooltip);
                            if r.clicked() {
                                action = Some(TabAction::Closed(tab.id));
                                continue;
                            }
                        }
                        let resp = if close_hovered { resp } else { resp.on_hover_text(&tab.title) };
                        if resp.clicked() {
                            action = Some(TabAction::Selected(tab.id));
                        }
                    }
                });
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
                if let Some(a) = tab_strip(ui, "test-tabs", tabs, "Закрыть") {
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
}
