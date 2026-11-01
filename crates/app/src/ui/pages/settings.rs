use super::header;
use crate::logic::hotkeys::{conflicting_slot, slot_for_index};
use crate::ui::pages::ctx::Ctx;
use crate::{i18n, ui::theme};
mod hotkeys;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;

use mediaa_core::settings::{
    HotkeySettings, RecordPreset, RecordProfile, ScreenChoice, Settings, ThemePreference,
    TrayClickAction,
};
use mediaa_core::{AnimFormat, OutputKind, VideoContainer};
use mediaa_platform::screen::MonitorInfo;
use rust_i18n::t;

pub fn ui(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let enabled = ctx.recording.is_idle();
    ui.add_enabled_ui(enabled, |ui| body(ctx, ui));
}

fn body(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let gui = ui.ctx().clone();
    header(ui, &t!("settings.title"), "");
    let changed = std::cell::Cell::new(false);
    let mut previews_changed = false;
    let advanced_open_id = egui::Id::new("settings-advanced-off");
    let mut advanced_open = ui
        .ctx()
        .memory(|memory| memory.data.get_temp::<bool>(advanced_open_id))
        .unwrap_or(false);
    let reset_open_id = egui::Id::new("settings-reset-open");
    let mut reset_open = ui
        .ctx()
        .memory(|memory| memory.data.get_temp::<bool>(reset_open_id))
        .unwrap_or(false);
    ui.horizontal(|ui| {
        if ui
            .add(theme::secondary(
                icon::ARROW_COUNTER_CLOCKWISE,
                &t!("settings.reset"),
            ))
            .clicked()
        {
            reset_open = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut advanced = ctx.settings.advanced_settings;
            if ui
                .checkbox(&mut advanced, t!("settings.advanced"))
                .on_hover_text(t!("settings.advanced_hint"))
                .changed()
            {
                if advanced {
                    ctx.settings.set_advanced_settings(true);
                    changed.set(true);
                } else {
                    advanced_open = true;
                }
            }
        });
    });
    if reset_open {
        let mut do_reset = false;
        let mut cancel = false;
        let modal = egui::Modal::new(egui::Id::new("settings-reset-confirm")).show(&gui, |ui| {
            ui.set_width(380.0);
            theme::popup_title(ui, &t!("settings.reset_title"));
            ui.label(t!("settings.reset_hint"));
            ui.add_space(8.0);
            let reset_label = t!("settings.reset_confirm").to_string();
            let cancel_label = t!("common.cancel").to_string();
            let responses = theme::centered_buttons(
                ui,
                &[
                    theme::ModalButton {
                        kind: theme::ModalButtonKind::Danger,
                        icon: icon::ARROW_COUNTER_CLOCKWISE,
                        label: &reset_label,
                        enabled: true,
                    },
                    theme::ModalButton {
                        kind: theme::ModalButtonKind::Secondary,
                        icon: icon::X,
                        label: &cancel_label,
                        enabled: true,
                    },
                ],
            );
            do_reset = responses[0].clicked();
            cancel = responses[1].clicked();
        });
        if modal.should_close() || cancel {
            reset_open = false;
        }
        if do_reset {
            reset_user_settings(ctx, false);
            theme::apply_preference(&gui, ctx.settings.theme);
            reset_open = false;
            changed.set(true);
        }
    }
    if advanced_open {
        let mut confirm = false;
        let mut cancel = false;
        let modal =
            egui::Modal::new(egui::Id::new("settings-advanced-off-confirm")).show(&gui, |ui| {
                ui.set_width(420.0);
                theme::popup_title(ui, &t!("settings.advanced_off_title"));
                ui.label(t!("settings.advanced_off_text"));
                ui.add_space(8.0);
                let confirm_label = t!("settings.advanced_off_confirm").to_string();
                let cancel_label = t!("common.cancel").to_string();
                let responses = theme::centered_buttons(
                    ui,
                    &[
                        theme::ModalButton {
                            kind: theme::ModalButtonKind::Danger,
                            icon: icon::WARNING,
                            label: &confirm_label,
                            enabled: true,
                        },
                        theme::ModalButton {
                            kind: theme::ModalButtonKind::Secondary,
                            icon: icon::X,
                            label: &cancel_label,
                            enabled: true,
                        },
                    ],
                );
                confirm = responses[0].clicked();
                cancel = responses[1].clicked();
            });
        if modal.should_close() || cancel {
            advanced_open = false;
        }
        if confirm {
            reset_user_settings(ctx, true);
            theme::apply_preference(&gui, ctx.settings.theme);
            advanced_open = false;
            changed.set(true);
        }
    }
    ui.ctx()
        .memory_mut(|memory| memory.data.insert_temp(advanced_open_id, advanced_open));
    ui.ctx()
        .memory_mut(|memory| memory.data.insert_temp(reset_open_id, reset_open));

    let mut open_dir = None;
    let mut autostart_error = None;
    if !ctx.settings.advanced_settings {
        simple_top(ctx, ui, &changed, &mut autostart_error);
    } else {
        theme::split_card(
            ui,
            |ui| {
                theme::section(ui, icon::SUN, &t!("settings.appearance"));
                theme::row(ui, &t!("settings.theme"), |ui| {
                    let mut theme_changed = false;
                    ui.vertical(|ui| {
                        let basic: Vec<_> = [
                            ThemePreference::System,
                            ThemePreference::Light,
                            ThemePreference::Dark,
                        ]
                        .into_iter()
                        .map(|p| (p, i18n::theme(p)))
                        .collect();
                        theme_changed |= theme::segmented(ui, &mut ctx.settings.theme, &basic);
                        let extra: Vec<_> = [ThemePreference::Lime, ThemePreference::Peach]
                            .into_iter()
                            .map(|p| (p, i18n::theme(p)))
                            .collect();
                        theme_changed |= theme::segmented(ui, &mut ctx.settings.theme, &extra);
                    });
                    if theme_changed {
                        theme::apply_preference(&gui, ctx.settings.theme);
                        changed.set(true);
                    }
                });
                theme::row(ui, &t!("settings.language"), |ui| {
                    let system = format!(
                        "{} ({})",
                        t!("settings.language_system"),
                        i18n::resolve(None, sys_locale::get_locale().as_deref())
                    );
                    let current = match ctx.settings.language.as_deref() {
                        None => system.clone(),
                        Some(code) => i18n::LANGUAGES
                            .iter()
                            .find(|(c, _)| *c == code)
                            .map(|(_, n)| n.to_string())
                            .unwrap_or_else(|| code.to_string()),
                    };
                    crate::ui::theme::combo_box("language")
                        .selected_text(current)
                        .show_ui(ui, |ui| {
                            let mut lang = ctx.settings.language.clone();
                            ui.selectable_value(&mut lang, None, system);
                            for (code, name) in i18n::LANGUAGES {
                                ui.selectable_value(&mut lang, Some(code.to_string()), *name);
                            }
                            if lang != ctx.settings.language {
                                ctx.settings.language = lang;
                                i18n::apply(ctx.settings.language.as_deref());
                                changed.set(true);
                            }
                        });
                });
                if ctx.settings.advanced_settings {
                    theme::row(ui, "", |ui| {
                        if ui
                            .checkbox(
                                &mut ctx.settings.notifications_enabled,
                                t!("settings.in_app_notifications"),
                            )
                            .changed()
                        {
                            changed.set(true);
                        }
                    });
                    theme::section(ui, icon::ROCKET_LAUNCH, &t!("settings.startup"));
                    theme::row(ui, "", |ui| {
                        let mut enabled = ctx.settings.launch_at_login;
                        if ui
                            .checkbox(&mut enabled, t!("settings.launch_at_login"))
                            .changed()
                        {
                            let mode = if enabled {
                                if ctx.settings.start_in_tray {
                                    mediaa_platform::startup::AutoStart::Tray
                                } else {
                                    mediaa_platform::startup::AutoStart::Window
                                }
                            } else {
                                mediaa_platform::startup::AutoStart::Disabled
                            };
                            match mediaa_platform::startup::configure(mode) {
                                Ok(()) => {
                                    ctx.settings.launch_at_login = enabled;
                                    if !enabled {
                                        ctx.settings.start_in_tray = false;
                                    }
                                    changed.set(true);
                                }
                                Err(error) => autostart_error = Some(format!("{error:#}")),
                            }
                        }
                    });
                    theme::row(ui, "", |ui| {
                        let launch = ctx.settings.launch_at_login;
                        let mut enabled = ctx.settings.start_in_tray;
                        if ui
                            .add_enabled(
                                launch,
                                egui::Checkbox::new(&mut enabled, t!("settings.start_in_tray")),
                            )
                            .changed()
                        {
                            let update = if ctx.settings.launch_at_login {
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
                                    ctx.settings.start_in_tray = enabled;
                                    changed.set(true);
                                }
                                Err(error) => autostart_error = Some(format!("{error:#}")),
                            }
                        }
                    });
                }
                if ctx.settings.advanced_settings {
                    theme::section(ui, icon::FILM_STRIP, &t!("settings.previews"));
                    theme::row(ui, "", |ui| {
                        if ui
                            .checkbox(
                                &mut ctx.settings.animated_previews,
                                t!("settings.animated_previews"),
                            )
                            .on_hover_text(t!("settings.animated_previews_hint"))
                            .changed()
                        {
                            previews_changed = true;
                            changed.set(true);
                        }
                    });
                    theme::row(ui, "", |ui| {
                        if ui
                            .checkbox(
                                &mut ctx.settings.full_preview_videos,
                                t!("settings.full_preview_videos"),
                            )
                            .on_hover_text(t!("settings.full_preview_videos_hint"))
                            .changed()
                        {
                            previews_changed = true;
                            changed.set(true);
                        }
                    });
                }
            },
            |ui| {
                theme::section(ui, icon::TRAY, &t!("settings.window"));
                if ctx.settings.advanced_settings {
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
                        if theme::segmented(ui, &mut ctx.settings.tray_left_click, &opts) {
                            changed.set(true);
                        }
                    });
                    theme::row(ui, "", |ui| {
                        if ui
                            .checkbox(
                                &mut ctx.settings.stop_recording_on_left_click,
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
                                &mut ctx.settings.tray_progress,
                                t!("settings.tray_progress"),
                            )
                            .on_hover_text(t!("settings.tray_progress_hint"))
                            .changed()
                        {
                            changed.set(true);
                        }
                    });
                }
                theme::row(ui, "", |ui| {
                    if ui
                        .checkbox(
                            &mut ctx.settings.close_to_tray,
                            t!("settings.close_to_tray"),
                        )
                        .changed()
                    {
                        changed.set(true);
                    }
                });
                if ctx.settings.advanced_settings {
                    theme::row(ui, "", |ui| {
                        if ui
                            .checkbox(
                                &mut ctx.settings.highlight_recording_region,
                                t!("settings.highlight_recording_region"),
                            )
                            .changed()
                        {
                            changed.set(true);
                        }
                    });
                    ui.add_enabled_ui(ctx.settings.highlight_recording_region, |ui| {
                        theme::row(ui, &t!("settings.recording_region_outline_width"), |ui| {
                            if ui
                                .add(
                                    egui::Slider::new(
                                        &mut ctx.settings.recording_region_outline_width,
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
                }
                if !ctx.settings.advanced_settings {
                    return;
                }
                theme::section(ui, icon::FOLDER_OPEN, &t!("settings.folders"));
                let dir = &mut ctx.settings.media_dir;
                theme::row(ui, &t!("settings.media_dir"), |ui| {
                    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                        .clicked()
                    {
                        if let Some(p) = crate::adapters::dialogs::pick_folder(&*dir) {
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
                let media = ctx.settings.media_dir.clone();
                let convert_dir = &mut ctx.settings.convert_dir;
                theme::row(ui, &t!("settings.convert_dir"), |ui| {
                    let current = convert_dir.clone().unwrap_or_else(|| media.clone());
                    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                        .clicked()
                    {
                        if let Some(p) = crate::adapters::dialogs::pick_folder(&current) {
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
    }
    if let Some(error) = autostart_error {
        ctx.error(t!("err.autostart", error = error));
    }
    if let Some(dir) = open_dir {
        let _ = crate::adapters::files::ensure_dir(&dir);
        ctx.open_path(&dir);
    }
    ui.add_space(8.0);

    theme::section(ui, icon::TIMER, &t!("settings.hotkeys"));
    let hk = &mut ctx.settings.hotkeys;
    let hotkey_snapshot = hk.clone();
    let hk_changed = std::cell::Cell::new(false);
    let cap_id = egui::Id::new("hotkey-capture");
    let capturing: Option<usize> = ui.data(|d| d.get_temp(cap_id));
    let start = std::cell::Cell::new(None);
    let finished = std::cell::Cell::new(false);
    let enabled = std::cell::Cell::new(hk.enabled);
    let (enabled_ref, area, screen, video, audio, stop) = (
        &mut hk.enabled,
        &mut hk.region_screenshot,
        &mut hk.fullscreen_screenshot,
        &mut hk.start_video,
        &mut hk.start_audio,
        &mut hk.stop_recording,
    );
    theme::split_card(
        ui,
        |ui| {
            if ui
                .checkbox(enabled_ref, t!("settings.hotkeys_enable"))
                .changed()
            {
                hk_changed.set(true);
            }
            enabled.set(*enabled_ref);
            ui.add_enabled_ui(*enabled_ref, |ui| {
                for (i, label, value) in [
                    (0, t!("settings.hk_area"), area),
                    (1, t!("settings.hk_screen"), screen),
                ] {
                    let (s, f) = hotkey_row(ui, &label, value, i, capturing, &hotkey_snapshot);
                    if s {
                        start.set(Some(i));
                    }
                    if f {
                        finished.set(true);
                    }
                }
            });
        },
        |ui| {
            ui.add_enabled_ui(enabled.get(), |ui| {
                for (i, label, value) in [
                    (2, t!("settings.hk_video"), video),
                    (3, t!("settings.hk_audio"), audio),
                    (4, t!("settings.hk_stop"), stop),
                ] {
                    let (s, f) = hotkey_row(ui, &label, value, i, capturing, &hotkey_snapshot);
                    if s {
                        start.set(Some(i));
                    }
                    if f {
                        finished.set(true);
                    }
                }
            });
        },
    );
    if let Some(i) = start.get() {
        ui.data_mut(|d| d.insert_temp(cap_id, i));
        ctx.suspend_hotkeys();
    } else if finished.get() {
        ui.data_mut(|d| d.remove::<usize>(cap_id));
        changed.set(true);
        ctx.reapply_hotkeys();
    } else if hk_changed.get() {
        changed.set(true);
        ctx.reapply_hotkeys();
    }
    if ui
        .data(|data| data.get_temp::<bool>(egui::Id::new("hotkey-conflict")))
        .unwrap_or(false)
    {
        ui.colored_label(
            theme::palette(ui).warning,
            format!("{} {}", icon::WARNING, t!("settings.hk_conflict")),
        );
    }
    for p in ctx.hotkey_problems {
        ui.colored_label(theme::palette(ui).warning, format!("{} {p}", icon::WARNING));
    }

    ui.add_space(8.0);
    ui.label(
        RichText::new(format!("{}  {}", icon::RECORD, t!("settings.hotkey_video")))
            .strong()
            .size(15.0),
    );
    if ctx.settings.advanced_settings {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::row(ui, &t!("settings.hotkey_screen"), |ui| {
                if screen_combo(ui, &mut ctx.settings.hotkey_screen, ctx.monitors) {
                    changed.set(true);
                }
            });
        });
    }
    ui.add_space(4.0);
    ui.add_space(2.0);
    let mut refresh_mics = false;
    if crate::ui::pages::record::profile_editor(
        ui,
        ctx.settings,
        RecordProfile::Hotkey,
        ctx.mic_devices,
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
    if crate::ui::pages::record::profile_editor(
        ui,
        ctx.settings,
        RecordProfile::Region,
        ctx.mic_devices,
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
    let mics = ctx.mic_devices;
    let simple = !ctx.settings.advanced_settings;
    let (c, opened) = crate::ui::pages::audio::audio_editor(
        ui,
        &mut ctx.settings.hotkey_audio,
        mics,
        "hotkey",
        simple,
        mediaa_platform::audio::system_audio_supported(),
    );
    if c {
        changed.set(true);
    }
    refresh_mics |= opened;
    if refresh_mics {
        ctx.refresh_catalogs();
    }

    if previews_changed {
        ctx.preview_options_changed();
    }
    if changed.get() {
        ctx.settings_changed();
    }
}

fn reset_settings_values(settings: &mut Settings) {
    let advanced = settings.advanced_settings;
    let language = settings.language.clone();
    let theme = match settings.theme {
        ThemePreference::Lime | ThemePreference::Peach => ThemePreference::System,
        theme => theme,
    };
    let viewer_volume = settings.viewer_volume;
    *settings = Settings::default();
    settings.advanced_settings = advanced;
    settings.record.output = OutputKind::Video(VideoContainer::Mkv);
    settings.region_record.output = OutputKind::Anim(AnimFormat::Gif);
    let mut anim = settings.anim.clone();
    settings
        .record
        .apply_preset(RecordPreset::Medium, &mut anim);
    settings.anim = anim;
    let mut region_anim = settings.region_anim.clone();
    settings
        .region_record
        .apply_preset(RecordPreset::Fast, &mut region_anim);
    settings.region_anim = region_anim;
    let mut hotkey_anim = settings.hotkey_anim.clone();
    settings
        .hotkey_record
        .apply_preset(RecordPreset::Medium, &mut hotkey_anim);
    settings.hotkey_anim = hotkey_anim;
    settings.language = language;
    settings.theme = theme;
    settings.viewer_volume = viewer_volume;
}

fn reset_user_settings(ctx: &mut Ctx<'_>, clear_advanced: bool) {
    let autostart = ctx.settings.launch_at_login;
    reset_settings_values(ctx.settings);
    if clear_advanced {
        ctx.settings.advanced_settings = false;
    }
    if autostart {
        if let Err(error) =
            mediaa_platform::startup::configure(mediaa_platform::startup::AutoStart::Disabled)
        {
            log::warn!("disable autostart after settings reset: {error:#}");
        }
    }
    ctx.reapply_hotkeys();
    ctx.preview_options_changed();
    let settings = ctx.settings.clone();
    ctx.lib.refresh(&settings);
    ctx.settings_changed();
}
fn hotkey_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    index: usize,
    capturing: Option<usize>,
    snapshot: &HotkeySettings,
) -> (bool, bool) {
    let conflict_id = egui::Id::new("hotkey-conflict");
    let mut start = false;
    let mut finished = false;
    theme::row(ui, label, |ui| {
        let active = capturing == Some(index);
        let text = if active {
            t!("settings.hk_press").to_string()
        } else if value.is_empty() {
            "—".to_string()
        } else {
            value.clone()
        };
        let p = theme::palette(ui);
        let field =
            egui::Button::new(RichText::new(text).color(if active { p.accent } else { p.text }))
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
            start = true;
            ui.data_mut(|data| data.remove::<bool>(conflict_id));
        }
        if !value.is_empty() && theme::icon_button(ui, icon::X, &t!("settings.hk_clear")).clicked()
        {
            value.clear();
            finished = true;
        }
        if active {
            if let Some(combo) = capture_combo(ui) {
                if let Some(c) = combo {
                    let conflict = slot_for_index(index)
                        .and_then(|slot| conflicting_slot(snapshot, slot, &c))
                        .is_some();
                    if conflict {
                        ui.data_mut(|data| data.insert_temp(conflict_id, true));
                    } else {
                        *value = c;
                        ui.data_mut(|data| data.remove::<bool>(conflict_id));
                    }
                }
                finished = true;
            }
            ui.ctx().request_repaint();
        }
    });
    (start, finished)
}
fn simple_top(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    changed: &std::cell::Cell<bool>,
    autostart_error: &mut Option<String>,
) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if theme::is_wide(ui) {
            ui.columns(2, |cols| {
                appearance_section(ctx, &mut cols[0], changed);
                let ui = &mut cols[1];
                startup_section(ctx, ui, changed, autostart_error);
                window_close_row(ctx, ui, changed);
            });
        } else {
            appearance_section(ctx, ui, changed);
            startup_section(ctx, ui, changed, autostart_error);
            window_close_row(ctx, ui, changed);
        }
    });
}

fn appearance_section(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, changed: &std::cell::Cell<bool>) {
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
        if theme::segmented(ui, &mut ctx.settings.theme, &opts) {
            theme::apply_preference(ui.ctx(), ctx.settings.theme);
            changed.set(true);
        }
    });
    theme::row(ui, &t!("settings.language"), |ui| {
        let system = format!(
            "{} ({})",
            t!("settings.language_system"),
            i18n::resolve(None, sys_locale::get_locale().as_deref())
        );
        let current = match ctx.settings.language.as_deref() {
            None => system.clone(),
            Some(code) => i18n::LANGUAGES
                .iter()
                .find(|(c, _)| *c == code)
                .map(|(_, n)| n.to_string())
                .unwrap_or_else(|| code.to_string()),
        };
        crate::ui::theme::combo_box("language")
            .selected_text(current)
            .show_ui(ui, |ui| {
                let mut lang = ctx.settings.language.clone();
                ui.selectable_value(&mut lang, None, system);
                for (code, name) in i18n::LANGUAGES {
                    ui.selectable_value(&mut lang, Some(code.to_string()), *name);
                }
                if lang != ctx.settings.language {
                    ctx.settings.language = lang;
                    i18n::apply(ctx.settings.language.as_deref());
                    changed.set(true);
                }
            });
    });
}

fn window_close_row(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, changed: &std::cell::Cell<bool>) {
    theme::section(ui, icon::TRAY, &t!("settings.window"));
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(
                &mut ctx.settings.close_to_tray,
                t!("settings.close_to_tray"),
            )
            .changed()
        {
            changed.set(true);
        }
    });
}

fn startup_section(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    changed: &std::cell::Cell<bool>,
    autostart_error: &mut Option<String>,
) {
    theme::section(ui, icon::ROCKET_LAUNCH, &t!("settings.startup"));
    theme::row(ui, "", |ui| {
        let mut enabled = ctx.settings.launch_at_login;
        if ui
            .checkbox(&mut enabled, t!("settings.launch_at_login"))
            .changed()
        {
            let mode = if enabled {
                if ctx.settings.start_in_tray {
                    mediaa_platform::startup::AutoStart::Tray
                } else {
                    mediaa_platform::startup::AutoStart::Window
                }
            } else {
                mediaa_platform::startup::AutoStart::Disabled
            };
            match mediaa_platform::startup::configure(mode) {
                Ok(()) => {
                    ctx.settings.launch_at_login = enabled;
                    if !enabled {
                        ctx.settings.start_in_tray = false;
                    }
                    changed.set(true);
                }
                Err(error) => *autostart_error = Some(format!("{error:#}")),
            }
        }
    });
    theme::row(ui, "", |ui| {
        let launch = ctx.settings.launch_at_login;
        let mut enabled = ctx.settings.start_in_tray;
        if ui
            .add_enabled(
                launch,
                egui::Checkbox::new(&mut enabled, t!("settings.start_in_tray")),
            )
            .changed()
        {
            let update = if ctx.settings.launch_at_login {
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
                    ctx.settings.start_in_tray = enabled;
                    changed.set(true);
                }
                Err(error) => *autostart_error = Some(format!("{error:#}")),
            }
        }
    });
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
            .map_or_else(|| dev.clone(), crate::ui::pages::record::monitor_label),
    };
    let mut changed = false;
    crate::ui::theme::combo_box("hotkey-screen")
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

#[cfg(test)]
mod tests {
    #![allow(clippy::field_reassign_with_default)]
    use super::reset_settings_values;
    use mediaa_core::settings::{RecordPreset, Settings, ThemePreference};
    use mediaa_core::{AnimFormat, OutputKind, VideoContainer};
    use std::path::PathBuf;

    #[test]
    fn reset_keeps_language_theme_and_library_volume() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut settings = Settings::default();
        settings.language = Some("ru".into());
        settings.theme = ThemePreference::Light;
        settings.viewer_volume = 0.35;
        settings.advanced_settings = true;
        settings.media_dir = PathBuf::from("D:/Media");
        settings.record.fps = 17;
        reset_settings_values(&mut settings);
        assert_eq!(settings.language.as_deref(), Some("ru"));
        assert_eq!(settings.theme, ThemePreference::Light);
        assert_eq!(settings.viewer_volume, 0.35);
        assert!(settings.advanced_settings);
        assert_eq!(settings.media_dir, Settings::default().media_dir);
        assert_eq!(settings.record.fps, Settings::default().record.fps);
    }

    #[test]
    fn reset_drops_bonus_themes_to_system_defaults() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        for theme in [ThemePreference::Lime, ThemePreference::Peach] {
            let mut settings = Settings::default();
            settings.theme = theme;
            settings.language = Some("ru".into());
            settings.viewer_volume = 0.4;
            reset_settings_values(&mut settings);
            let mut expected = Settings::default();
            expected.record.output = OutputKind::Video(VideoContainer::Mkv);
            expected.record.fps = 60;
            expected.record.output_height = 1080;
            expected.record.video_quality = 85;
            expected.region_record.output = OutputKind::Anim(AnimFormat::Gif);
            expected.region_record.fps = 30;
            expected.region_record.output_height = 720;
            expected.region_anim.gif_colors = 256;
            expected.region_anim.gif_dither = mediaa_core::settings::GifDither::Bayer;
            expected.hotkey_record.fps = 60;
            expected.hotkey_record.output_height = 1080;
            expected.hotkey_record.video_quality = 85;
            expected.language = Some("ru".into());
            expected.viewer_volume = 0.4;
            assert_eq!(settings, expected);
            assert_eq!(settings.theme, ThemePreference::System);
        }
    }

    #[test]
    fn reset_is_exactly_defaults_except_kept_preferences() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let mut settings = Settings::default();
        settings.advanced_settings = true;
        settings.language = Some("ru".into());
        settings.theme = ThemePreference::Light;
        settings.viewer_volume = 0.35;
        settings.media_dir = PathBuf::from("D:/CustomMedia");
        settings.convert_dir = Some(PathBuf::from("D:/CustomConvert"));
        settings.record.fps = 17;
        settings.record.video_quality = 21;
        settings.record.show_no_signal = false;
        settings.record.hide_to_tray = false;
        settings.record.allow_unavailable_sources = true;
        settings.region_record.fps = 9;
        settings.hotkey_record.fps = 11;
        settings.anim.webp_quality = 4;
        settings.hotkeys.enabled = true;
        settings.launch_at_login = true;
        settings.start_in_tray = true;
        settings.close_to_tray = true;
        settings.tray_progress = false;
        settings.animated_previews = false;
        settings.full_preview_videos = false;
        settings.stop_recording_on_left_click = false;
        settings.highlight_recording_region = false;
        settings.recording_region_outline_width = 99;

        reset_settings_values(&mut settings);

        let mut expected = Settings::default();
        expected.advanced_settings = true;
        expected.record.output = OutputKind::Video(VideoContainer::Mkv);
        expected.record.fps = 60;
        expected.record.output_height = 1080;
        expected.record.video_quality = 85;
        expected.region_record.output = OutputKind::Anim(AnimFormat::Gif);
        expected.region_record.fps = 30;
        expected.region_record.output_height = 720;
        expected.region_anim.gif_colors = 256;
        expected.region_anim.gif_dither = mediaa_core::settings::GifDither::Bayer;
        expected.hotkey_record.fps = 60;
        expected.hotkey_record.output_height = 1080;
        expected.hotkey_record.video_quality = 85;
        expected.language = Some("ru".into());
        expected.theme = ThemePreference::Light;
        expected.viewer_volume = 0.35;
        assert_eq!(settings, expected);
        assert!(settings.advanced_settings);
        assert!(settings.record.show_no_signal);
        assert!(settings.record.hide_to_tray);
        assert!(settings.record.record_cursor);
        assert!(!settings.record.allow_unavailable_sources);
        assert_eq!(settings.language.as_deref(), Some("ru"));
        assert_eq!(settings.theme, ThemePreference::Light);
        assert_eq!(settings.viewer_volume, 0.35);
        assert_eq!(
            settings.record.preset(&settings.anim),
            Some(RecordPreset::Medium)
        );
        assert_eq!(
            settings.region_record.preset(&settings.region_anim),
            Some(RecordPreset::Fast)
        );
        assert_eq!(
            settings.record.output,
            OutputKind::Video(VideoContainer::Mkv)
        );
        assert_eq!(
            settings.region_record.output,
            OutputKind::Anim(AnimFormat::Gif)
        );
    }
}
