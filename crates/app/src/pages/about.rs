use super::header;
use crate::app::App;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::paths;
use rust_i18n::t;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("settings.about"), "");
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let logo = app.logo(ui.ctx());
            ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(48.0, 48.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new("MediaA").size(20.0).strong());
                theme::hint(ui, t!("settings.version", version = env!("CARGO_PKG_VERSION")));
            });
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            for (name, ok) in &app.capabilities {
                let p = theme::palette(ui);
                let (ic, c) = if *ok { (icon::CHECK, p.success) } else { (icon::X, p.danger) };
                ui.label(RichText::new(format!("{ic} {name}")).color(c).size(11.5));
            }
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.add(theme::secondary(icon::FOLDER_OPEN, &t!("settings.logs"))).clicked() {
                app.open_path(&paths::logs_dir());
            }
            if ui.add(theme::secondary(icon::ARROWS_CLOCKWISE, &t!("settings.rescan_devices"))).clicked() {
                app.mic_devices = mediaa_platform::audio::input_devices();
                app.refresh_monitors();
            }
        });
    });
}
