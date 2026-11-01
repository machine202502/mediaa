use eframe::egui::{self, Color32, CornerRadius, FontId, Stroke};

use crate::theme::palette;

pub const ICON_BUTTON: f32 = 26.0;

pub fn framed_icon_button(
    ui: &mut egui::Ui,
    icon: &str,
    tooltip: &str,
    side: f32,
) -> egui::Response {
    let p = palette(ui);
    let button = egui::Button::new(egui::RichText::new(icon).size(15.0).color(p.text))
        .fill(p.card_hover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(6))
        .min_size(egui::vec2(side, side));
    let response = ui.add(button).on_hover_text(tooltip);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tooltip));
    response
}

pub fn icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    icon_button_sized(ui, icon, tooltip, ICON_BUTTON)
}

pub fn icon_button_sized(
    ui: &mut egui::Ui,
    icon: &str,
    tooltip: &str,
    side: f32,
) -> egui::Response {
    icon_face(ui, icon, tooltip, side).on_hover_text(tooltip)
}

pub fn icon_face(ui: &mut egui::Ui, icon: &str, label: &str, side: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if ui.is_rect_visible(rect) {
        let p = palette(ui);
        let (fill, stroke) = if resp.is_pointer_button_down_on() {
            (p.accent, p.accent)
        } else if resp.hovered() {
            (p.accent_soft, p.accent)
        } else {
            (Color32::TRANSPARENT, Color32::TRANSPARENT)
        };
        let color = if !ui.is_enabled() {
            p.text_weak
        } else if resp.is_pointer_button_down_on() {
            p.on_accent
        } else {
            p.text
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(6),
            fill,
            Stroke::new(1.0, stroke),
            egui::StrokeKind::Inside,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            FontId::proportional(14.0),
            color,
        );
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    resp
}
