use super::header;
use crate::app::App;
use crate::{hotkeys, i18n, theme};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

use mediaa_core::settings::{
    AfterCompletionAction, RecordProfile, ScreenChoice, ThemePreference, TrayClickAction,
};
use mediaa_platform::screen::MonitorInfo;
use rust_i18n::t;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    header(ui, &t!("settings.title"), "");
    let changed = std::cell::Cell::new(false);

    let mut open_dir = None;
    let mut autostart_error = None;
    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::SUN, &t!("settings.appearance"));
            theme::row(ui, &t!("settings.theme"), |ui| {
                let opts: Vec<_> = [
                    ThemePreference::System,
                    ThemePreference::Light,
                    ThemePreference::Dark,
                ]
                .into_iter()
                .map(|p| (p, i18n::theme(p)))
                .collect();
                if theme::segmented(ui, &mut app.settings.theme, &opts) {
                    theme::apply_preference(&ctx, app.settings.theme);
                    changed.set(true);
                }
            });
            theme::row(ui, &t!("settings.language"), |ui| {
                let system = format!(
                    "{} ({})",
                    t!("settings.language_system"),
                    i18n::resolve(None, sys_locale::get_locale().as_deref())
                );
                let current = match &app.settings.language {
                    None => system.clone(),
                    Some(code) => i18n::LANGUAGES
                        .iter()
                        .find(|(c, _)| c == code)
                        .map(|(_, n)| n.to_string())
                        .unwrap_or_else(|| code.clone()),
                };
                egui::ComboBox::from_id_salt("language")
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        let mut lang = app.settings.language.clone();
                        ui.selectable_value(&mut lang, None, system);
                        for (code, name) in i18n::LANGUAGES {
                            ui.selectable_value(&mut lang, Some(code.to_string()), *name);
                        }
                        if lang != app.settings.language {
                            app.settings.language = lang;
                            i18n::apply(app.settings.language.as_deref());
                            changed.set(true);
                        }
                    });
            });
            theme::section(ui, icon::ROCKET_LAUNCH, &t!("settings.startup"));
            theme::row(ui, "", |ui| {
                let mut enabled = app.settings.launch_at_login;
                if ui
                    .checkbox(&mut enabled, t!("settings.launch_at_login"))
                    .changed()
                {
                    let mode = if enabled {
                        if app.settings.start_in_tray {
                            mediaa_platform::startup::AutoStart::Tray
                        } else {
                            mediaa_platform::startup::AutoStart::Window
                        }
                    } else {
                        mediaa_platform::startup::AutoStart::Disabled
                    };
                    match mediaa_platform::startup::configure(mode) {
                        Ok(()) => {
                            app.settings.launch_at_login = enabled;
                            changed.set(true);
                        }
                        Err(error) => autostart_error = Some(format!("{error:#}")),
                    }
                }
            });
            theme::row(ui, "", |ui| {
                let mut enabled = app.settings.start_in_tray;
                if ui
                    .checkbox(&mut enabled, t!("settings.start_in_tray"))
                    .changed()
                {
                    let update = if app.settings.launch_at_login {
                        let mode = if enabled {
                            mediaa_platform::startup::AutoStart::Tray
                        } else {
                            mediaa_platform::startup::AutoStart::Window
                        };
                        mediaa_platform::startup::configure(mode)
                    } else {
                        Ok(())
                    };
                    match update {
                        Ok(()) => {
                            app.settings.start_in_tray = enabled;
                            changed.set(true);
                        }
                        Err(error) => autostart_error = Some(format!("{error:#}")),
                    }
                }
            });
            theme::section(ui, icon::RECORD, &t!("settings.after_completion"));
            theme::row(ui, &t!("settings.after_completion_action"), |ui| {
                let opts = [
                    (
                        AfterCompletionAction::OpenLibrary,
                        t!("settings.after_completion_library").to_string(),
                    ),
                    (
                        AfterCompletionAction::CopyFile,
                        t!("settings.after_completion_copy").to_string(),
                    ),
                ];
                if theme::segmented(ui, &mut app.settings.after_completion, &opts) {
                    changed.set(true);
                }
            });
        },
        |ui| {
            theme::section(ui, icon::TRAY, &t!("settings.window"));
            theme::row(ui, &t!("settings.tray_left_click"), |ui| {
                let opts = [
                    (
                        TrayClickAction::OpenWindow,
                        t!("settings.tray_click_open_window").to_string(),
                    ),
                    (
                        TrayClickAction::ScreenArea,
                        t!("settings.tray_click_screen_area").to_string(),
                    ),
                ];
                if theme::segmented(ui, &mut app.settings.tray_left_click, &opts) {
                    changed.set(true);
                }
            });
            theme::row(ui, "", |ui| {
                if ui
                    .checkbox(
                        &mut app.settings.stop_recording_on_left_click,
                        t!("settings.stop_recording_on_left_click"),
                    )
                    .changed()
                {
                    changed.set(true);
                }
            });
            theme::row(ui, "", |ui| {
                if ui
                    .checkbox(
                        &mut app.settings.close_to_tray,
                        t!("settings.close_to_tray"),
                    )
                    .changed()
                {
                    changed.set(true);
                }
            });
            theme::row(ui, "", |ui| {
                if ui
                    .checkbox(
                        &mut app.settings.highlight_recording_region,
                        t!("settings.highlight_recording_region"),
                    )
                    .changed()
                {
                    changed.set(true);
                }
            });
            ui.add_enabled_ui(app.settings.highlight_recording_region, |ui| {
                theme::row(ui, &t!("settings.recording_region_outline_width"), |ui| {
                    if ui
                        .add(
                            egui::Slider::new(
                                &mut app.settings.recording_region_outline_width,
                                2..=32,
                            )
                            .suffix(" px"),
                        )
                        .changed()
                    {
                        changed.set(true);
                    }
                });
            });
            theme::section(ui, icon::FOLDER_OPEN, &t!("settings.folders"));
            let dir = &mut app.settings.media_dir;
            theme::row(ui, &t!("settings.media_dir"), |ui| {
                if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new().set_directory(&*dir).pick_folder() {
                        *dir = p;
                        changed.set(true);
                    }
                }
                if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, &t!("settings.open_folder"))
                    .clicked()
                {
                    open_dir = Some(dir.clone());
                }
                ui.add(egui::Label::new(dir.to_string_lossy()).truncate());
            });
            let media = app.settings.media_dir.clone();
            let convert_dir = &mut app.settings.convert_dir;
            theme::row(ui, &t!("settings.convert_dir"), |ui| {
                let current = convert_dir.clone().unwrap_or_else(|| media.clone());
                if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                    .clicked()
                {
                    if let Some(p) = rfd::FileDialog::new().set_directory(&current).pick_folder() {
                        *convert_dir = (p != media).then_some(p);
                        changed.set(true);
                    }
                }
                if convert_dir.is_some()
                    && theme::icon_button(
                        ui,
                        icon::ARROW_COUNTER_CLOCKWISE,
                        &t!("settings.convert_dir_reset"),
                    )
                    .clicked()
                {
                    *convert_dir = None;
                    changed.set(true);
                }
                let text = if convert_dir.is_some() {
                    current.to_string_lossy().into_owned()
                } else {
                    t!("settings.convert_dir_library").to_string()
                };
                ui.add(egui::Label::new(text).truncate());
            });
        },
    );
    if let Some(error) = autostart_error {
        app.error(t!("err.autostart", error = error));
    }
    if let Some(dir) = open_dir {
        let _ = std::fs::create_dir_all(&dir);
        app.open_path(&dir);
    }
    ui.add_space(8.0);

    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::TIMER, &t!("settings.hotkeys"));
            let hk = &mut app.settings.hotkeys;
            let mut hk_changed = false;
            hk_changed |= ui
                .checkbox(&mut hk.enabled, t!("settings.hotkeys_enable"))
                .changed();
            let cap_id = egui::Id::new("hotkey-capture");
            let capturing: Option<usize> = ui.data(|d| d.get_temp(cap_id));
            let mut start = None;
            let mut finished = false;
            ui.add_enabled_ui(hk.enabled, |ui| {
                for (i, (label, value)) in [
                    (t!("settings.hk_area"), &mut hk.region_screenshot),
                    (t!("settings.hk_screen"), &mut hk.fullscreen_screenshot),
                    (t!("settings.hk_video"), &mut hk.start_video),
                    (t!("settings.hk_audio"), &mut hk.start_audio),
                    (t!("settings.hk_stop"), &mut hk.stop_recording),
                ]
                .into_iter()
                .enumerate()
                {
                    theme::row(ui, &label, |ui| {
                        let active = capturing == Some(i);
                        let text = if active {
                            t!("settings.hk_press").to_string()
                        } else if value.is_empty() {
                            "—".to_string()
                        } else {
                            value.clone()
                        };
                        let p = theme::palette(ui);
                        let field = egui::Button::new(RichText::new(text).color(if active {
                            p.accent
                        } else {
                            p.text
                        }))
                        .stroke(egui::Stroke::new(
                            1.0,
                            if active { p.accent } else { p.border },
                        ))
                        .min_size(egui::vec2(170.0, 26.0));
                        if ui
                            .add(field)
                            .on_hover_text(t!("settings.hk_click"))
                            .clicked()
                            && !active
                        {
                            start = Some(i);
                        }
                        if !value.is_empty()
                            && theme::icon_button(ui, icon::X, &t!("settings.hk_clear")).clicked()
                        {
                            value.clear();
                            finished = true;
                        }
                        if active {
                            if let Some(combo) = capture_combo(ui) {
                                if let Some(c) = combo {
                                    *value = c;
                                }
                                finished = true;
                            }
                            ui.ctx().request_repaint();
                        }
                    });
                }
            });
            if let Some(i) = start {
                ui.data_mut(|d| d.insert_temp(cap_id, i));
                app.suspend_hotkeys();
            } else if finished {
                ui.data_mut(|d| d.remove::<usize>(cap_id));
                changed.set(true);
                app.reapply_hotkeys();
            } else if hk_changed {
                changed.set(true);
                app.reapply_hotkeys();
            }
            for p in &app.hotkey_problems {
                ui.colored_label(theme::palette(ui).warning, format!("{} {p}", icon::WARNING));
            }
        },
        |ui| {
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
        },
    );

    ui.add_space(8.0);
    ui.label(
        RichText::new(format!("{}  {}", icon::RECORD, t!("settings.hotkey_video")))
            .strong()
            .size(15.0),
    );
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::row(ui, &t!("settings.hotkey_screen"), |ui| {
            if screen_combo(ui, &mut app.settings.hotkey_screen, &app.monitors) {
                changed.set(true);
            }
        });
    });
    ui.add_space(4.0);
    ui.add_space(2.0);
    let mut refresh_mics = false;
    if crate::pages::record::profile_editor(
        ui,
        &mut app.settings,
        RecordProfile::Hotkey,
        &app.mic_devices,
        None,
        &mut refresh_mics,
    ) {
        changed.set(true);
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "{}  {}",
            icon::SELECTION,
            t!("settings.region_record")
        ))
        .strong()
        .size(15.0),
    );
    if crate::pages::record::profile_editor(
        ui,
        &mut app.settings,
        RecordProfile::Region,
        &app.mic_devices,
        None,
        &mut refresh_mics,
    ) {
        changed.set(true);
    }
    ui.add_space(8.0);
    ui.label(
        RichText::new(format!(
            "{}  {}",
            icon::MICROPHONE,
            t!("settings.hotkey_audio")
        ))
        .strong(),
    );
    ui.add_space(2.0);
    let mics = app.mic_devices.clone();
    let (c, opened) =
        crate::pages::audio::audio_editor(ui, &mut app.settings.hotkey_audio, &mics, "hotkey");
    if c {
        changed.set(true);
    }
    refresh_mics |= opened;
    if refresh_mics {
        app.refresh_mics();
    }

    if changed.get() {
        app.settings_changed();
    }
}

fn capture_combo(ui: &egui::Ui) -> Option<Option<String>> {
    let (events, modifiers) = ui.input(|i| (i.events.clone(), i.modifiers));
    for e in events {
        if let egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = e
        {
            if key == egui::Key::Escape {
                return Some(None);
            }
            if let Some(combo) = hotkeys::combo_from_key(modifiers, key) {
                return Some(Some(combo));
            }
        }
    }
    if mediaa_platform::keys::print_screen_down() {
        return Some(hotkeys::combo_from_token(modifiers, "PRINTSCREEN"));
    }
    None
}

fn screen_combo(ui: &mut egui::Ui, value: &mut ScreenChoice, monitors: &[MonitorInfo]) -> bool {
    let label = |c: &ScreenChoice| match c {
        ScreenChoice::Primary => t!("settings.screen_primary").to_string(),
        ScreenChoice::UnderCursor => t!("settings.screen_cursor").to_string(),
        ScreenChoice::Display(dev) => monitors
            .iter()
            .find(|m| &m.device == dev)
            .map_or_else(|| dev.clone(), crate::pages::record::monitor_label),
    };
    let mut changed = false;
    egui::ComboBox::from_id_salt("hotkey-screen")
        .selected_text(label(value))
        .show_ui(ui, |ui| {
            let mut options = vec![ScreenChoice::Primary, ScreenChoice::UnderCursor];
            options.extend(
                monitors
                    .iter()
                    .map(|m| ScreenChoice::Display(m.device.clone())),
            );
            for o in options {
                let text = label(&o);
                changed |= ui.selectable_value(value, o, text).changed();
            }
        });
    changed
}
