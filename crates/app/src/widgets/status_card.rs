use crate::theme;
use eframe::egui::{self, RichText};

pub struct StatusCard<'a> {
    pub icon: &'a str,
    pub color: egui::Color32,
    pub time: &'a str,
    pub label: &'a str,
    pub detail: Option<&'a str>,
    pub progress: Option<f32>,
    pub action: Option<(&'a str, &'a str)>,
}

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            )
            .size()
            .x
    })
}

fn progress_bar(ui: &mut egui::Ui, progress: Option<f32>, time: &str) {
    let p = theme::palette(ui);
    let width = 300.0;
    let height = 18.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    let radius = height / 2.0;
    let fill = progress.unwrap_or(1.0).clamp(0.0, 1.0);
    painter.rect_filled(
        rect,
        radius,
        if progress.is_some() {
            p.border
        } else {
            p.accent
        },
    );
    let filled = rect.width() * fill;
    if progress.is_some() {
        painter.rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + filled, rect.bottom())),
            radius,
            p.accent,
        );
    }
    let pad = 10.0;
    if let Some(progress) = progress {
        let color = if filled > 34.0 { p.on_accent } else { p.text };
        painter.text(
            egui::pos2(rect.left() + pad, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{:.0}%", progress * 100.0),
            egui::FontId::proportional(11.0),
            color,
        );
    }
    painter.text(
        egui::pos2(rect.right() - pad, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        time,
        egui::FontId::monospace(11.0),
        if progress.is_some() {
            p.text
        } else {
            p.on_accent
        },
    );
}

pub fn status_card(ui: &mut egui::Ui, card: StatusCard<'_>) -> bool {
    let mut action = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(100.0);
        ui.add_space(4.0);
        let width = text_width(ui, card.icon, 24.0)
            + text_width(ui, card.time, 28.0)
            + text_width(ui, card.label, 13.0)
            + 16.0;
        theme::centered_row_sized(ui, width, |ui| {
            ui.label(RichText::new(card.icon).size(24.0).color(card.color));
            ui.label(RichText::new(card.time).size(28.0).strong());
            ui.label(RichText::new(card.label).color(theme::palette(ui).text_weak));
        });
        if let Some(detail) = card.detail {
            theme::centered_row_sized(ui, text_width(ui, detail, 11.5), |ui| {
                theme::hint(ui, detail);
            });
        }
        theme::centered_widget(ui, 300.0, |ui| {
            progress_bar(ui, card.progress, card.time);
        });
        if let Some((action_icon, label)) = card.action {
            let width = text_width(ui, &format!("{action_icon}  {label}"), 13.0) + 16.0;
            theme::centered_widget(ui, width, |ui| {
                if ui.add(theme::secondary(action_icon, label)).clicked() {
                    action = true;
                }
            });
        }
    });
    action
}
