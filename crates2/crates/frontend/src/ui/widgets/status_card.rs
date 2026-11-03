use crate::ui::theme;
use eframe::egui::{self, RichText};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusActionKind {
    Primary,
    Danger,
    Secondary,
    Icon,
}

pub struct StatusAction {
    pub icon: &'static str,
    pub label: String,
    pub kind: StatusActionKind,
    pub tooltip: Option<String>,
}

impl StatusAction {
    pub fn primary(icon: &'static str, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
            kind: StatusActionKind::Primary,
            tooltip: None,
        }
    }

    pub fn danger(icon: &'static str, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
            kind: StatusActionKind::Danger,
            tooltip: None,
        }
    }

    pub fn secondary(icon: &'static str, label: impl Into<String>) -> Self {
        Self {
            icon,
            label: label.into(),
            kind: StatusActionKind::Secondary,
            tooltip: None,
        }
    }

    pub fn icon(icon: &'static str, tooltip: impl Into<String>) -> Self {
        Self {
            icon,
            label: String::new(),
            kind: StatusActionKind::Icon,
            tooltip: Some(tooltip.into()),
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum StatusProgress {
    None,
    Indeterminate,
    Percent(f32),
}

pub struct StatusCard {
    pub icon: &'static str,
    pub color: egui::Color32,
    pub time: String,
    pub label: String,
    pub detail: Option<String>,
    pub progress: StatusProgress,
    pub actions: Vec<StatusAction>,
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

fn monospace_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::monospace(size),
                egui::Color32::WHITE,
            )
            .size()
            .x
    })
}

fn progress_bar(ui: &mut egui::Ui, progress: StatusProgress, time: &str) {
    let p = theme::palette(ui);
    let width = 300.0;
    let height = 18.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    let radius = height / 2.0;
    let percent = match progress {
        StatusProgress::None | StatusProgress::Indeterminate => None,
        StatusProgress::Percent(percent) => Some(percent.clamp(0.0, 1.0)),
    };
    let fill = percent.unwrap_or(1.0);
    painter.rect_filled(
        rect,
        radius,
        if percent.is_some() {
            p.border
        } else {
            p.accent
        },
    );
    let filled = rect.width() * fill;
    if percent.is_some() {
        painter.rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + filled, rect.bottom())),
            radius,
            p.accent,
        );
    }
    let pad = 10.0;
    if let Some(percent) = percent {
        let color = if filled > 34.0 { p.on_accent } else { p.text };
        painter.text(
            egui::pos2(rect.left() + pad, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{:.0}%", percent * 100.0),
            egui::FontId::proportional(11.0),
            color,
        );
    }
    let time_width = monospace_width(ui, time, 11.0);
    let time_left = rect.right() - pad - time_width;
    let color = if percent.is_some() && filled >= time_left - rect.left() {
        p.on_accent
    } else if percent.is_some() {
        p.text
    } else {
        p.on_accent
    };
    painter.text(
        egui::pos2(rect.right() - pad, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        time,
        egui::FontId::monospace(11.0),
        color,
    );
}

fn action_width(ui: &egui::Ui, action: &StatusAction) -> f32 {
    match action.kind {
        StatusActionKind::Icon => 28.0,
        _ => theme::button_width(ui, action.icon, &action.label),
    }
}

fn action_button(ui: &mut egui::Ui, action: &StatusAction) -> egui::Response {
    match action.kind {
        StatusActionKind::Primary => ui.add(theme::primary(ui, action.icon, &action.label)),
        StatusActionKind::Danger => ui.add(theme::danger(ui, action.icon, &action.label)),
        StatusActionKind::Secondary => ui.add(theme::secondary(action.icon, &action.label)),
        StatusActionKind::Icon => theme::framed_icon_button(
            ui,
            action.icon,
            action.tooltip.as_deref().unwrap_or(&action.label),
            28.0,
        ),
    }
}

pub fn status_card(ui: &mut egui::Ui, card: StatusCard) -> Option<usize> {
    let mut clicked = None;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(100.0);
        ui.add_space(4.0);
        let width = text_width(ui, card.icon, 24.0)
            + text_width(ui, &card.time, 28.0)
            + text_width(ui, &card.label, 13.0)
            + 16.0;
        theme::centered_row_sized(ui, width, |ui| {
            ui.label(RichText::new(card.icon).size(24.0).color(card.color));
            ui.label(RichText::new(&card.time).size(28.0).strong());
            ui.label(RichText::new(&card.label).color(theme::palette(ui).text_weak));
        });
        if let Some(detail) = &card.detail {
            theme::centered_row_sized(ui, text_width(ui, detail, 11.5), |ui| {
                theme::hint(ui, detail);
            });
        }
        if card.progress != StatusProgress::None {
            theme::centered_widget(ui, 300.0, |ui| {
                progress_bar(ui, card.progress, &card.time);
            });
        }
        if !card.actions.is_empty() {
            let spacing = ui.spacing().item_spacing.x;
            let widths: Vec<f32> = card
                .actions
                .iter()
                .map(|action| action_width(ui, action))
                .collect();
            let total: f32 =
                widths.iter().sum::<f32>() + spacing * card.actions.len().saturating_sub(1) as f32;
            if total + 24.0 <= ui.available_width() {
                theme::centered_row_sized(ui, total, |ui| {
                    for (index, action) in card.actions.iter().enumerate() {
                        if action_button(ui, action).clicked() {
                            clicked = Some(index);
                        }
                    }
                });
            } else {
                for (index, action) in card.actions.iter().enumerate() {
                    let width = widths[index];
                    if action.kind == StatusActionKind::Primary {
                        let available = ui.available_width();
                        if ui
                            .add_sized(
                                egui::vec2(available, 28.0),
                                theme::primary(ui, action.icon, &action.label),
                            )
                            .clicked()
                        {
                            clicked = Some(index);
                        }
                    } else {
                        theme::centered_widget(ui, width, |ui| {
                            if action_button(ui, action).clicked() {
                                clicked = Some(index);
                            }
                        });
                    }
                }
            }
        }
    });
    clicked
}
