use super::header;
use crate::features::intent::Intent;
use crate::ui::caches::Caches;
use crate::ui::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::settings::ThemePreference;
use rust_i18n::t;

const REPOSITORY_URL: &str = "https://github.com/machine202502/mediaa";

pub fn ui(
    theme_preference: ThemePreference,
    caches: &mut Caches,
    ui: &mut egui::Ui,
) -> Vec<Intent> {
    let mut intents = Vec::new();
    header(ui, &t!("settings.about"), "");
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let logo = caches.logo(ui.ctx(), theme_preference);
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
                intents.push(Intent::RefreshCatalogs);
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
                intents.push(Intent::OpenLogs);
            }
            if ui
                .add(theme::secondary(
                    icon::GITHUB_LOGO,
                    &t!("settings.repository"),
                ))
                .on_hover_text(REPOSITORY_URL)
                .clicked()
            {
                intents.push(Intent::OpenUrl(REPOSITORY_URL.to_string()));
            }
        });
        ui.add_space(8.0);
    });
    ui.add_space(8.0);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::SELECTION, &t!("settings.picker_keys"));
        for (keys, what) in [
            ("Enter".to_string(), t!("keys.default_action")),
            ("Ctrl+C".to_string(), t!("keys.copy")),
            ("Ctrl+S".to_string(), t!("keys.save")),
            ("Ctrl+Z".to_string(), t!("keys.undo")),
            ("Ctrl+A".to_string(), t!("keys.select_all")),
            (
                format!(
                    "{}{}{}{}",
                    icon::ARROW_LEFT,
                    icon::ARROW_UP,
                    icon::ARROW_RIGHT,
                    icon::ARROW_DOWN
                ),
                t!("keys.move"),
            ),
            (
                format!(
                    "Shift + {}{}{}{}",
                    icon::ARROW_LEFT,
                    icon::ARROW_UP,
                    icon::ARROW_RIGHT,
                    icon::ARROW_DOWN
                ),
                t!("keys.resize"),
            ),
            ("Tab".to_string(), t!("keys.next_display")),
            ("Esc".to_string(), t!("keys.cancel")),
        ] {
            theme::row(ui, &keys, |ui| ui.label(what));
        }
    });
    intents
}

pub(crate) fn contract_window(ctx: &egui::Context, open: &mut bool) {
    if !*open {
        return;
    }
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new("speech-contract-modal")).show(ctx, |ui| {
        ui.set_width(560.0);
        theme::popup_title(ui, &t!("settings.contract_title"));
        egui::ScrollArea::vertical()
            .max_height(440.0)
            .show(ui, |ui| {
                ui.add(egui::Label::new(t!("settings.contract_body")).wrap());
                ui.add_space(8.0);
                ui.add(
                    egui::Label::new(RichText::new(t!("settings.contract_example")).monospace())
                        .wrap(),
                );
            });
        ui.add_space(8.0);
        ui.set_width(560.0);
        ui.add_space(7.0);
        let close_label = t!("common.close").to_string();
        ui.vertical_centered(|ui| {
            if ui.add(theme::secondary(icon::X, &close_label)).clicked() {
                close = true;
            }
        });
    });
    if close || modal.should_close() {
        *open = false;
    }
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
