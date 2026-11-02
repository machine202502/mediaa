use eframe::egui::{self, CornerRadius, Rect, Sense, Stroke};

use crate::ui::theme::palette;

pub fn track_slider(
    ui: &mut egui::Ui,
    width: f32,
    frac: f32,
    id: &str,
) -> (egui::Response, Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(40.0), 24.0), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 14.0, 4.0));
    let p = palette(ui);
    let accent = p.accent;
    let x = track.left() + track.width() * frac.clamp(0.0, 1.0);
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), p.border);
    painter.rect_filled(
        Rect::from_min_max(track.min, egui::pos2(x, track.max.y)),
        CornerRadius::same(2),
        accent,
    );
    let knob = if resp.hovered() || resp.dragged() {
        8.0
    } else {
        7.0
    };
    painter.circle(
        egui::pos2(x, track.center().y),
        knob,
        p.card,
        Stroke::new(1.5, accent),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let value = resp
        .interact_pointer_pos()
        .filter(|_| resp.clicked() || resp.dragged())
        .map(|p| ((p.x - track.left()) / track.width()).clamp(0.0, 1.0));
    (resp, value)
}
