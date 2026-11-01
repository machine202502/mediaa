use eframe::egui::{self, Color32, CornerRadius};

use crate::theme::palette;

fn sized(btn: egui::Button<'static>) -> egui::Button<'static> {
    btn.corner_radius(CornerRadius::same(6))
        .min_size(egui::vec2(0.0, 28.0))
}

pub fn primary(ui: &egui::Ui, icon: &str, text: &str) -> egui::Button<'static> {
    let p = palette(ui);
    sized(
        egui::Button::new(
            egui::RichText::new(format!("{icon}  {text}"))
                .color(p.on_accent)
                .strong(),
        )
        .fill(p.accent),
    )
}

pub fn danger(ui: &egui::Ui, icon: &str, text: &str) -> egui::Button<'static> {
    let p = palette(ui);
    sized(
        egui::Button::new(egui::RichText::new(format!("{icon}  {text}")).color(Color32::WHITE))
            .fill(p.danger),
    )
}

pub fn secondary(icon: &str, text: &str) -> egui::Button<'static> {
    sized(egui::Button::new(format!("{icon}  {text}")))
}

pub fn button_width(ui: &egui::Ui, icon: &str, text: &str) -> f32 {
    let width = ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                format!("{icon}  {text}"),
                egui::FontId::proportional(13.0),
                egui::Color32::WHITE,
            )
            .size()
            .x
    });
    width + 16.0
}
