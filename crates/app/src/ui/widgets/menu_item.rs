use eframe::egui::{self, CornerRadius, FontId};

use crate::ui::theme::palette;

pub fn menu_item(ui: &mut egui::Ui, icon: &str, label: &str, danger: bool) -> egui::Response {
    let p = palette(ui);
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.0), egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        let color = if danger { p.danger } else { p.text };
        if resp.hovered() || resp.has_focus() {
            let tint = if danger {
                p.danger.gamma_multiply(0.15)
            } else {
                p.accent_soft
            };
            ui.painter().rect_filled(rect, CornerRadius::same(6), tint);
        }
        let font = FontId::proportional(13.5);
        ui.painter().text(
            rect.left_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            icon,
            FontId::proportional(15.0),
            color,
        );
        ui.painter().text(
            rect.left_center() + egui::vec2(36.0, 0.0),
            egui::Align2::LEFT_CENTER,
            label,
            font,
            color,
        );
    }
    resp
}
