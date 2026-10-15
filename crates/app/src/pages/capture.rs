//! Screenshot page.
//!
//! | action tiles                         |
//! | After capture | Displays  (same height) |
//! | Windows                              |

use super::{header, tile};
use crate::app::App;
use crate::overlay::Purpose;
use crate::{i18n, theme};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::settings::AfterCapture;
use mediaa_core::ImageFormat;
use rust_i18n::t;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    header(ui, &t!("shot.title"), "");

    let hk = app.settings.hotkeys.clone();
    let key_hint = |key: &str| if hk.enabled && !key.is_empty() { key.to_string() } else { String::new() };
    let w = super::tile_width(ui, 2);
    ui.horizontal_wrapped(|ui| {
        if tile(ui, icon::SELECTION, &t!("shot.area"), &key_hint(&hk.region_screenshot), w, false).clicked() {
            app.start_region_screenshot(&ctx, Purpose::Screenshot);
        }
        if tile(ui, icon::MONITOR, &t!("shot.full"), &key_hint(&hk.fullscreen_screenshot), w, false).clicked() {
            app.start_fullscreen_screenshot(&ctx, None);
        }
    });
    ui.add_space(4.0);

    let mut changed = false;
    let mut shot_monitor = None;
    let mut refresh_monitors = false;
    theme::equal_cards(
        ui,
        "shot-row",
        |ui| {
            theme::section(ui, icon::GEAR, &t!("shot.after"));
            let s = &mut app.settings.screenshot;
            theme::row(ui, &t!("shot.action"), |ui| {
                egui::ComboBox::from_id_salt("after-capture")
                    .selected_text(i18n::after_capture(s.after_capture))
                    .show_ui(ui, |ui| {
                        for a in AfterCapture::ALL {
                            changed |= ui.selectable_value(&mut s.after_capture, a, i18n::after_capture(a)).changed();
                        }
                    });
            });
            theme::row(ui, &t!("shot.format"), |ui| {
                let opts: Vec<_> = ImageFormat::ALL.iter().map(|f| (*f, f.label().to_string())).collect();
                changed |= theme::segmented(ui, &mut s.format, &opts);
            });
            theme::row(ui, "", |ui| {
                changed |= ui.checkbox(&mut s.hide_window, t!("shot.hide_window")).changed();
            });
        },
        |ui| {
            ui.horizontal(|ui| {
                theme::section(ui, icon::MONITOR, &t!("shot.displays"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    refresh_monitors = theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked();
                });
            });
            for m in &app.monitors {
                let label = super::record::monitor_label(m);
                if ui.add(theme::secondary(icon::CAMERA, &label).min_size(egui::vec2(ui.available_width(), 28.0))).clicked() {
                    shot_monitor = Some(m.clone());
                }
            }
        },
    );
    if changed {
        app.settings_changed();
    }
    if refresh_monitors {
        app.refresh_monitors();
    }
    if let Some(m) = shot_monitor {
        app.start_fullscreen_screenshot(&ctx, Some(m));
    }
    ui.add_space(4.0);

    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            theme::section(ui, icon::APP_WINDOW, &t!("shot.windows"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked() {
                    app.refresh_windows();
                }
            });
        });
        if app.windows.is_empty() {
            theme::hint(ui, t!("shot.no_windows"));
        }
        let mut capture = None;
        // Full width, so the scrollbar sits at the right edge of the card.
        egui::ScrollArea::vertical().max_height(190.0).auto_shrink([false, true]).id_salt("shot-windows").show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for w in &app.windows {
                ui.horizontal(|ui| {
                    if theme::icon_button(ui, icon::CAMERA, &t!("shot.capture_window")).clicked() {
                        capture = Some(w.id);
                    }
                    ui.add(egui::Label::new(RichText::new(w.label())).truncate());
                });
            }
        });
        if let Some(id) = capture {
            app.start_window_screenshot(&ctx, id);
        }
    });
}
