use eframe::egui::{self, Color32, CornerRadius, Margin, Stroke};

use crate::ui::theme::palette;

pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, String)],
) -> bool {
    segmented_marked(ui, value, options, None)
}

pub fn segmented_marked<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, String)],
    marked: Option<T>,
) -> bool {
    let marks = marked
        .map(|mark| (mark, palette(ui).danger))
        .into_iter()
        .collect::<Vec<_>>();
    segmented_marked_many(ui, value, options, &marks)
}

pub fn segmented_marked_many<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    value: &mut T,
    options: &[(T, String)],
    marks: &[(T, Color32)],
) -> bool {
    let p = palette(ui);
    let mut changed = false;
    egui::Frame::new()
        .fill(p.card_hover)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            ui.horizontal_wrapped(|ui| {
                for (opt, label) in options {
                    let selected = *value == *opt;
                    let mark = marks
                        .iter()
                        .find(|(marked, _)| *marked == *opt)
                        .map(|(_, color)| *color);
                    let response = crate::ui::widgets::marker::marker_button(
                        ui,
                        crate::ui::widgets::marker::MarkerButton {
                            label,
                            font: ui
                                .style()
                                .text_styles
                                .get(&egui::TextStyle::Button)
                                .cloned()
                                .unwrap_or_else(|| egui::FontId::proportional(14.0)),
                            color: if selected { p.on_accent } else { p.text },
                            fill: if selected {
                                p.accent
                            } else {
                                Color32::TRANSPARENT
                            },
                            marker: mark,
                            height: 22.0,
                            min_width: 48.0,
                        },
                    );
                    if response.clicked() && !selected {
                        *value = *opt;
                        changed = true;
                    }
                }
            });
        });
    changed
}
