use super::{header, tile};
use crate::{i18n, theme};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::settings::AfterCapture;
use mediaa_core::ImageFormat;
use mediaa_core::Settings;
use mediaa_platform::screen::{MonitorInfo, WindowId, WindowInfo};
use rust_i18n::t;

pub enum CaptureUiAction {
    SettingsChanged,
    RefreshCatalogs,
    StartRegion,
    StartFullscreen(Option<MonitorInfo>),
    StartWindow(WindowId),
}

pub fn ui(
    settings: &mut Settings,
    monitors: &[MonitorInfo],
    windows: &[WindowInfo],
    ui: &mut egui::Ui,
) -> Vec<CaptureUiAction> {
    let mut actions = Vec::new();
    header(ui, &t!("shot.title"), "");

    let hk = settings.hotkeys.clone();
    let key_hint = |key: &str| {
        if hk.enabled && !key.is_empty() {
            key.to_string()
        } else {
            String::new()
        }
    };
    let w = super::tile_width(ui, 2);
    ui.horizontal_wrapped(|ui| {
        if tile(
            ui,
            icon::SELECTION,
            &t!("shot.area"),
            &key_hint(&hk.region_screenshot),
            w,
            false,
        )
        .clicked()
        {
            actions.push(CaptureUiAction::StartRegion);
        }
        if tile(
            ui,
            icon::MONITOR,
            &t!("shot.full"),
            &key_hint(&hk.fullscreen_screenshot),
            w,
            false,
        )
        .clicked()
        {
            actions.push(CaptureUiAction::StartFullscreen(None));
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
            let s = &mut settings.screenshot;
            theme::row(ui, &t!("shot.action"), |ui| {
                egui::ComboBox::from_id_salt("after-capture")
                    .selected_text(i18n::after_capture(s.after_capture))
                    .show_ui(ui, |ui| {
                        for a in AfterCapture::ALL {
                            changed |= ui
                                .selectable_value(&mut s.after_capture, a, i18n::after_capture(a))
                                .changed();
                        }
                    });
            });
            theme::row(ui, &t!("shot.format"), |ui| {
                let opts: Vec<_> = ImageFormat::ALL
                    .iter()
                    .map(|f| (*f, f.label().to_string()))
                    .collect();
                changed |= theme::segmented(ui, &mut s.format, &opts);
            });
            theme::row(ui, "", |ui| {
                changed |= ui
                    .checkbox(&mut s.hide_window, t!("shot.hide_window"))
                    .changed();
            });
        },
        |ui| {
            ui.horizontal(|ui| {
                theme::section(ui, icon::MONITOR, &t!("shot.displays"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    refresh_monitors =
                        theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh"))
                            .clicked();
                });
            });
            for m in monitors {
                let label = super::record::monitor_label(m);
                if ui
                    .add(
                        theme::secondary(icon::CAMERA, &label)
                            .min_size(egui::vec2(ui.available_width(), 28.0)),
                    )
                    .clicked()
                {
                    shot_monitor = Some(m.clone());
                }
            }
        },
    );
    if changed {
        actions.push(CaptureUiAction::SettingsChanged);
    }
    if refresh_monitors {
        actions.push(CaptureUiAction::RefreshCatalogs);
    }
    if let Some(m) = shot_monitor {
        actions.push(CaptureUiAction::StartFullscreen(Some(m)));
    }
    ui.add_space(4.0);

    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            theme::section(ui, icon::APP_WINDOW, &t!("shot.windows"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked() {
                    actions.push(CaptureUiAction::RefreshCatalogs);
                }
            });
        });
        if windows.is_empty() {
            theme::hint(ui, t!("shot.no_windows"));
        }
        let mut capture = None;
        egui::ScrollArea::vertical()
            .max_height(190.0)
            .auto_shrink([false, true])
            .id_salt("shot-windows")
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for w in windows {
                    ui.horizontal(|ui| {
                        if theme::icon_button(ui, icon::CAMERA, &t!("shot.capture_window"))
                            .clicked()
                        {
                            capture = Some(w.id);
                        }
                        ui.add(egui::Label::new(RichText::new(w.label())).truncate());
                    });
                }
            });
        if let Some(id) = capture {
            actions.push(CaptureUiAction::StartWindow(id));
        }
    });
    actions
}
