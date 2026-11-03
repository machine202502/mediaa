use super::record::{header, monitor_label, tile, tile_width};
use super::Ctx;
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::screen::MonitorInfo;
use mediaa2_core::settings::AfterCapture;
use mediaa2_i18n::t;
use mediaa2_protocol::{ClientCommand, ScreenshotCommand};

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    header(ui, &t!("shot.title"), "");
    command_error(ctx, ui);

    let mut picker = false;
    let mut fullscreen = false;
    let mut refresh = false;
    let mut capture_monitor = None;
    let mut capture_window = None;

    let hotkeys = ctx.ui.settings.draft.hotkeys.clone();
    let key_hint = |key: &str| {
        if hotkeys.enabled && !key.is_empty() {
            key.to_string()
        } else {
            String::new()
        }
    };
    let width = tile_width(ui, 2);
    ui.horizontal_wrapped(|ui| {
        if tile(
            ui,
            icon::SELECTION,
            &t!("shot.area"),
            &key_hint(&hotkeys.region_screenshot),
            width,
            false,
        )
        .clicked()
        {
            picker = true;
        }
        if tile(
            ui,
            icon::MONITOR,
            &t!("shot.full"),
            &key_hint(&hotkeys.fullscreen_screenshot),
            width,
            false,
        )
        .clicked()
        {
            fullscreen = true;
        }
    });
    ui.add_space(4.0);

    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            theme::section(ui, icon::MONITOR, &t!("shot.displays"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked() {
                    refresh = true;
                }
            });
        });
        for monitor in ctx.monitors {
            let label = monitor_label(monitor);
            if ui
                .add(
                    theme::secondary(icon::CAMERA, &label)
                        .min_size(egui::vec2(ui.available_width(), 28.0)),
                )
                .clicked()
            {
                capture_monitor = Some(monitor.clone());
            }
        }
    });
    ui.add_space(4.0);

    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            theme::section(ui, icon::APP_WINDOW, &t!("shot.windows"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked() {
                    refresh = true;
                }
            });
        });
        if ctx.windows.is_empty() {
            theme::hint(ui, t!("shot.no_windows"));
        }
        egui::ScrollArea::vertical()
            .id_salt("shot-windows")
            .max_height(190.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for window in ctx.windows {
                    let label = window.label();
                    if ui
                        .add(
                            theme::secondary(icon::CAMERA, &label)
                                .min_size(egui::vec2(ui.available_width(), 28.0)),
                        )
                        .clicked()
                    {
                        capture_window = Some(window.clone());
                    }
                }
            });
    });

    if refresh {
        ctx.actions.refresh_catalogs();
    }
    if picker {
        ctx.actions
            .send(ClientCommand::Screenshot(ScreenshotCommand::StartPicker));
    }
    if fullscreen {
        match default_monitor(ctx.monitors) {
            Some(monitor) => {
                let monitor = monitor.clone();
                ctx.actions
                    .send(ClientCommand::Screenshot(ScreenshotCommand::Monitor {
                        monitor,
                        after: AfterCapture::CopyAndSave,
                    }));
            }
            None => ctx.actions.notice(
                NoticeLevel::Error,
                t!("err.no_display").to_string(),
                String::new(),
            ),
        }
    }
    if let Some(monitor) = capture_monitor {
        ctx.actions
            .send(ClientCommand::Screenshot(ScreenshotCommand::Monitor {
                monitor,
                after: AfterCapture::CopyAndSave,
            }));
    }
    if let Some(window) = capture_window {
        ctx.actions
            .send(ClientCommand::Screenshot(ScreenshotCommand::Window {
                window,
                after: AfterCapture::CopyAndSave,
            }));
    }
}

fn default_monitor(monitors: &[MonitorInfo]) -> Option<&MonitorInfo> {
    monitors
        .iter()
        .find(|monitor| monitor.primary)
        .or_else(|| monitors.first())
}

fn command_error(ctx: &Ctx<'_>, ui: &mut egui::Ui) {
    let Some(error) = ctx.command_error else {
        return;
    };
    let palette = theme::palette(ui);
    ui.label(egui::RichText::new(error).size(12.0).color(palette.danger));
}
