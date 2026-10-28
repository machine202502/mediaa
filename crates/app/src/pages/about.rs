use super::header;
use crate::app::App;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::paths;
use rust_i18n::t;

const REPOSITORY_URL: &str = "https://github.com/machine202502/mediaa";

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("settings.about"), "");
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let logo = app.logo(ui.ctx());
            ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(48.0, 48.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new("MediaA").size(20.0).strong());
                theme::hint(
                    ui,
                    t!("settings.version", version = env!("CARGO_PKG_VERSION")),
                );
            });
        });
        ui.add_space(6.0);
        section_help(
            ui,
            icon::PLUGS_CONNECTED,
            &t!("settings.devices"),
            &t!("settings.devices_hint"),
        );
        ui.horizontal(|ui| {
            if ui
                .add(theme::secondary(
                    icon::ARROWS_CLOCKWISE,
                    &t!("settings.refresh_devices"),
                ))
                .on_hover_text(t!("settings.devices_hint"))
                .clicked()
            {
                app.refresh_catalogs(ui.ctx());
            }
        });
        ui.add_space(8.0);
        section_help(
            ui,
            icon::WRENCH,
            &t!("settings.diagnostics"),
            &t!("settings.diagnostics_hint"),
        );
        ui.horizontal(|ui| {
            if ui
                .add(theme::secondary(icon::FOLDER_OPEN, &t!("settings.logs")))
                .on_hover_text(t!("settings.logs_hint"))
                .clicked()
            {
                app.open_path(ui.ctx(), &paths::logs_dir());
            }
            if ui
                .add(theme::secondary(
                    icon::GITHUB_LOGO,
                    &t!("settings.repository"),
                ))
                .on_hover_text(REPOSITORY_URL)
                .clicked()
            {
                ui.ctx().open_url(egui::OpenUrl::new_tab(REPOSITORY_URL));
            }
        });
        ui.add_space(8.0);
        section_help(
            ui,
            icon::FILE_TEXT,
            &t!("settings.contract"),
            &t!("settings.contract_title"),
        );
        let open_id = egui::Id::new("about-speech-contract");
        let mut open = ui.ctx().data(|data| data.get_temp(open_id).unwrap_or(false));
        if ui
            .add(theme::secondary(icon::FILE_TEXT, &t!("settings.contract")))
            .clicked()
        {
            open = true;
        }
        if open {
            egui::Window::new(t!("settings.contract_title"))
                .id(egui::Id::new("about-speech-contract-window"))
                .open(&mut open)
                .collapsible(false)
                .resizable(true)
                .default_width(560.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ui.ctx(), |ui| {
                    ui.set_min_width(480.0);
                    egui::ScrollArea::vertical()
                        .max_height(440.0)
                        .show(ui, |ui| {
                            ui.add(egui::Label::new(t!("settings.contract_body")).wrap());
                            ui.add_space(8.0);
                            ui.add(
                                egui::Label::new(
                                    RichText::new(t!("settings.contract_example")).monospace(),
                                )
                                .wrap(),
                            );
                        });
                });
        }
        ui.ctx().data_mut(|data| data.insert_temp(open_id, open));
    });
}

fn section_help(ui: &mut egui::Ui, icon: &str, title: &str, help: &str) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(15.0).color(p.accent));
        ui.label(RichText::new(title).size(14.0).strong());
        ui.label(RichText::new(icon::INFO).size(13.0).color(p.text_weak))
            .on_hover_text(help);
    });
    ui.add_space(2.0);
}
