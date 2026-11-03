use eframe::egui;

use super::button::{button_width, danger, primary, secondary};

pub const POPUP_TITLE: f32 = 14.0;

#[derive(Clone, Copy)]
pub enum ModalButtonKind {
    #[allow(dead_code, reason = "primary modal buttons are built by pages")]
    Primary,
    Secondary,
    Danger,
}

pub struct ModalButton<'a> {
    pub kind: ModalButtonKind,
    pub icon: &'a str,
    pub label: &'a str,
    pub enabled: bool,
}

pub fn centered_buttons(ui: &mut egui::Ui, buttons: &[ModalButton<'_>]) -> Vec<egui::Response> {
    let spacing = ui.spacing().item_spacing.x;
    let total: f32 = buttons
        .iter()
        .map(|button| button_width(ui, button.icon, button.label))
        .sum::<f32>()
        + spacing * buttons.len().saturating_sub(1) as f32;
    let offset = ((ui.available_width() - total) / 2.0).max(0.0);
    ui.horizontal(|ui| {
        ui.add_space(offset);
        buttons
            .iter()
            .map(|spec| {
                let button = match spec.kind {
                    ModalButtonKind::Primary => primary(ui, spec.icon, spec.label),
                    ModalButtonKind::Secondary => secondary(spec.icon, spec.label),
                    ModalButtonKind::Danger => danger(ui, spec.icon, spec.label),
                };
                ui.add_enabled(spec.enabled, button)
            })
            .collect()
    })
    .inner
}

pub fn popup_title(ui: &mut egui::Ui, title: &str) {
    let color = ui.visuals().strong_text_color();
    let galley = ui.painter().layout_no_wrap(
        title.to_owned(),
        egui::FontId::proportional(POPUP_TITLE),
        color,
    );
    let size = galley.size();
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), size.y),
        egui::Sense::hover(),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, title));
    let origin = egui::pos2(rect.center().x - size.x / 2.0, rect.top());
    let painter = ui.painter();
    painter.galley(origin, galley.clone(), color);
    painter.galley(origin + egui::vec2(0.6, 0.0), galley.clone(), color);
    painter.galley(origin + egui::vec2(0.0, 0.4), galley, color);
}
