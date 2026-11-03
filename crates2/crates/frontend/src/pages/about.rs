use super::{page_title, Ctx};
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_i18n::t;

const REPOSITORY_URL: &str = "https://github.com/machine202502/mediaa";

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    page_title(ui, &t!("settings.about"), "");
    let logo = ctx.textures.app_icon(ui.ctx(), crate::dev_agent());
    let mut refresh = false;
    let mut logs = false;
    let mut repository = false;
    let mut error = None;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&logo).fit_to_exact_size(egui::vec2(48.0, 48.0)));
            ui.vertical(|ui| {
                ui.label(egui::RichText::new("MediaA").size(20.0).strong());
                theme::hint(
                    ui,
                    t!("settings.version", version = env!("CARGO_PKG_VERSION")).to_string(),
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
                refresh = true;
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
                logs = true;
            }
            if ui
                .add(theme::secondary(
                    icon::GITHUB_LOGO,
                    &t!("settings.repository"),
                ))
                .on_hover_text(REPOSITORY_URL)
                .clicked()
            {
                repository = true;
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
            theme::row(ui, &keys, |ui| {
                ui.label(what.to_string());
            });
        }
    });
    if refresh {
        ctx.actions.refresh_catalogs();
    }
    if logs {
        let dir = mediaa2_core::paths::logs_dir();
        let _ = std::fs::create_dir_all(&dir);
        if let Err(value) = mediaa2_platform::files::open(&dir) {
            error = Some(value.to_string());
        }
    }
    if repository {
        if let Err(value) = mediaa2_platform::files::open(std::path::Path::new(REPOSITORY_URL)) {
            error = Some(value.to_string());
        }
    }
    if let Some(value) = error {
        ctx.actions.notice(
            mediaa2_core::notices::NoticeLevel::Error,
            t!("notice.error").to_string(),
            value,
        );
    }
}

fn section_help(ui: &mut egui::Ui, icon: &str, title: &str, help: &str) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(icon).size(15.0).color(p.accent));
        ui.label(egui::RichText::new(title).size(14.0).strong());
        ui.label(
            egui::RichText::new(icon::INFO)
                .size(13.0)
                .color(p.text_weak),
        )
        .on_hover_text(help);
    });
    ui.add_space(2.0);
}
