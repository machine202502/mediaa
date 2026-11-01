use eframe::egui::{self, Color32, FontId, Response, Stroke};

pub struct MarkerButton<'a> {
    pub label: &'a str,
    pub font: FontId,
    pub color: Color32,
    pub fill: Color32,
    pub marker: Option<Color32>,
    pub height: f32,
    pub min_width: f32,
}

pub fn marker_button(ui: &mut egui::Ui, button: MarkerButton<'_>) -> Response {
    let padding = 8.0;
    let marker_space = 12.0;
    let galley = ui
        .painter()
        .layout_no_wrap(button.label.to_owned(), button.font, button.color);
    let width = (galley.size().x
        + padding * 2.0
        + if button.marker.is_some() {
            marker_space
        } else {
            0.0
        })
    .max(button.min_width);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, button.height), egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        if button.fill != Color32::TRANSPARENT {
            painter.rect_filled(rect, egui::CornerRadius::same(6), button.fill);
        }
        let mut text_left = rect.left() + padding;
        if let Some(color) = button.marker {
            painter.circle_filled(egui::pos2(text_left + 4.0, rect.center().y), 3.0, color);
            text_left += marker_space;
        }
        painter.galley(
            egui::pos2(text_left, rect.center().y - galley.size().y / 2.0),
            galley,
            button.color,
        );
        if response.hovered() {
            painter.rect_stroke(
                rect,
                egui::CornerRadius::same(6),
                Stroke::new(1.0, button.color.gamma_multiply(0.25)),
                egui::StrokeKind::Inside,
            );
        }
    }
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), button.label)
    });
    response
}
