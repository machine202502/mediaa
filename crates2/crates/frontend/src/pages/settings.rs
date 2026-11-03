use super::record::mic_combo;
use super::{page_title, Ctx};
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_core::formats::{AnimFormat, AudioFormat, OutputKind, VideoContainer};
use mediaa2_core::limits::{FPS, GIF_COLORS, VIDEO_QUALITY};
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::screen::MonitorInfo;
use mediaa2_core::settings::{
    audio_default_kbps, AnimationQuality, AudioRecordSettings, GifDither, RecordPreset,
    RecordProfile, RecordSettings, ScreenChoice, Settings, ThemePreference, TrayClickAction,
    AUDIO_BITRATES, OUTPUT_HEIGHTS,
};
use mediaa2_i18n::{t, LANGUAGES};
use mediaa2_platform::startup::{self, AutoStart};
use mediaa2_protocol::{ClientCommand, LibraryCommand};

const RESET_OPEN: &str = "settings-reset-open";
const ADVANCED_OFF: &str = "settings-advanced-off";
const HOTKEY_CAPTURE: &str = "settings-hotkey-capture";
const HOTKEY_CONFLICT: &str = "settings-hotkey-conflict";

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let mut draft = ctx.ui.settings.draft.clone();
    let mut advanced = ctx.ui.settings.advanced;
    let mut changed = false;
    let mut notice: Option<(NoticeLevel, String)> = None;
    let media_dir = draft.media_dir.clone();

    header(ui, &mut draft, &mut advanced, &mut changed, &mut notice);
    top(ui, &mut draft, advanced, &mut changed, &mut notice);
    hotkeys(ui, &mut draft, &mut changed);
    profiles(
        ui,
        &mut draft,
        advanced,
        ctx.monitors,
        ctx.microphones,
        &mut changed,
    );
    ui.add_space(8.0);
    profile_heading(ui, icon::MICROPHONE, &t!("settings.hotkey_audio"), false);
    ui.add_space(2.0);
    changed |= audio_card(ui, &mut draft.hotkey_audio, !advanced, ctx.microphones);

    if changed {
        ctx.ui.settings.draft = draft;
        ctx.ui.settings.advanced = advanced;
        ctx.ui.settings.dirty = true;
        if ctx.ui.settings.draft.media_dir != media_dir {
            ctx.actions
                .send(ClientCommand::Library(LibraryCommand::Refresh));
        }
        let update = ctx.ui.settings.draft.clone();
        ctx.actions.set_settings(update);
    }
    if let Some((level, detail)) = notice {
        ctx.actions.notice(level, t!("notice.error"), detail);
    }
}

fn header(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    advanced: &mut bool,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    page_title(ui, &t!("settings.title"), "");
    ui.horizontal(|ui| {
        if ui
            .add(theme::secondary(
                icon::ARROW_COUNTER_CLOCKWISE,
                &t!("settings.reset"),
            ))
            .clicked()
        {
            ui.data_mut(|data| data.insert_temp(egui::Id::new(RESET_OPEN), true));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mut value = *advanced;
            if ui
                .checkbox(&mut value, t!("settings.advanced"))
                .on_hover_text(t!("settings.advanced_hint"))
                .changed()
            {
                if value {
                    *advanced = true;
                    draft.set_advanced_settings(true);
                    *changed = true;
                } else {
                    ui.data_mut(|data| data.insert_temp(egui::Id::new(ADVANCED_OFF), true));
                }
            }
        });
    });
    reset_modal(ui, draft, changed, notice);
    advanced_off_modal(ui, draft, advanced, changed, notice);
}

fn advanced_off_modal(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    advanced: &mut bool,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    let id = egui::Id::new(ADVANCED_OFF);
    if !ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    let mut confirm = false;
    let mut cancel = false;
    let modal =
        egui::Modal::new(egui::Id::new("settings-advanced-off-confirm")).show(ui.ctx(), |ui| {
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
            confirm = responses.first().is_some_and(|response| response.clicked());
            cancel = responses.get(1).is_some_and(|response| response.clicked());
        });
    if modal.should_close() || cancel {
        ui.data_mut(|data| data.insert_temp(id, false));
    }
    if confirm {
        let autostart = draft.launch_at_login;
        reset_values(draft);
        draft.advanced_settings = false;
        *advanced = false;
        if autostart {
            if let Err(error) = startup::configure(AutoStart::Disabled) {
                *notice = Some((
                    NoticeLevel::Error,
                    t!("err.autostart", error = format!("{error:#}")).to_string(),
                ));
            }
        }
        theme::apply_preference(ui.ctx(), draft.theme);
        *changed = true;
        ui.data_mut(|data| data.insert_temp(id, false));
    }
}

fn reset_modal(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    let id = egui::Id::new(RESET_OPEN);
    if !ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    let mut reset = false;
    let mut cancel = false;
    let modal = egui::Modal::new(egui::Id::new("settings-reset-confirm")).show(ui.ctx(), |ui| {
        ui.set_width(380.0);
        theme::popup_title(ui, &t!("settings.reset_title"));
        ui.label(t!("settings.reset_hint"));
        ui.add_space(8.0);
        let confirm = t!("settings.reset_confirm").to_string();
        let cancel_label = t!("common.cancel").to_string();
        let responses = theme::centered_buttons(
            ui,
            &[
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Danger,
                    icon: icon::ARROW_COUNTER_CLOCKWISE,
                    label: &confirm,
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
        reset = responses.first().is_some_and(|response| response.clicked());
        cancel = responses.get(1).is_some_and(|response| response.clicked());
    });
    if modal.should_close() || cancel {
        ui.data_mut(|data| data.insert_temp(id, false));
    }
    if reset {
        let autostart = draft.launch_at_login;
        reset_values(draft);
        if autostart {
            if let Err(error) = startup::configure(AutoStart::Disabled) {
                *notice = Some((
                    NoticeLevel::Error,
                    t!("err.autostart", error = format!("{error:#}")).to_string(),
                ));
            }
        }
        theme::apply_preference(ui.ctx(), draft.theme);
        *changed = true;
        ui.data_mut(|data| data.insert_temp(id, false));
    }
}

fn reset_values(settings: &mut Settings) {
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

fn top(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    advanced: bool,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if theme::is_wide(ui) {
            ui.columns(2, |cols| {
                if advanced {
                    advanced_left(&mut cols[0], draft, changed, notice);
                    advanced_right(&mut cols[1], draft, changed, notice);
                } else {
                    appearance(&mut cols[0], draft, false, changed);
                    startup(&mut cols[1], draft, changed, notice);
                    window_close(&mut cols[1], draft, changed);
                }
            });
        } else if advanced {
            advanced_left(ui, draft, changed, notice);
            ui.separator();
            advanced_right(ui, draft, changed, notice);
        } else {
            appearance(ui, draft, false, changed);
            startup(ui, draft, changed, notice);
            window_close(ui, draft, changed);
        }
    });
}

fn advanced_left(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    appearance(ui, draft, true, changed);
    startup(ui, draft, changed, notice);
    previews(ui, draft, changed);
}

fn advanced_right(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    window(ui, draft, changed);
    folders(ui, draft, changed, notice);
}

fn appearance(ui: &mut egui::Ui, draft: &mut Settings, advanced: bool, changed: &mut bool) {
    theme::section(ui, icon::SUN, &t!("settings.appearance"));
    theme::row(ui, &t!("settings.theme"), |ui| {
        ui.vertical(|ui| {
            let basic: Vec<(ThemePreference, String)> = [
                ThemePreference::System,
                ThemePreference::Light,
                ThemePreference::Dark,
            ]
            .into_iter()
            .map(|option| (option, theme_label(option)))
            .collect();
            let mut theme_changed = theme::segmented(ui, &mut draft.theme, &basic);
            if advanced {
                let extra: Vec<(ThemePreference, String)> =
                    [ThemePreference::Lime, ThemePreference::Peach]
                        .into_iter()
                        .map(|option| (option, theme_label(option)))
                        .collect();
                theme_changed |= theme::segmented(ui, &mut draft.theme, &extra);
            }
            if theme_changed {
                *changed = true;
            }
        });
    });
    theme::row(ui, &t!("settings.language"), |ui| {
        let mut language = draft.language.clone();
        theme::combo_box("settings-language")
            .selected_text(language_label(draft.language.as_deref()))
            .show_ui(ui, |ui| {
                ui.selectable_value(
                    &mut language,
                    None,
                    t!("settings.language_system").to_string(),
                );
                for (code, name) in LANGUAGES {
                    ui.selectable_value(&mut language, Some(code.to_string()), (*name).to_string());
                }
            });
        if language != draft.language {
            draft.language = language;
            *changed = true;
        }
    });
    if advanced {
        theme::row(ui, "", |ui| {
            if ui
                .checkbox(
                    &mut draft.notifications_enabled,
                    t!("settings.in_app_notifications"),
                )
                .changed()
            {
                *changed = true;
            }
        });
    }
}

fn startup(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    theme::section(ui, icon::ROCKET_LAUNCH, &t!("settings.startup"));
    theme::row(ui, "", |ui| {
        let mut launch = draft.launch_at_login;
        if ui
            .checkbox(&mut launch, t!("settings.launch_at_login"))
            .changed()
        {
            let mode = if launch {
                if draft.start_in_tray {
                    AutoStart::Tray
                } else {
                    AutoStart::Window
                }
            } else {
                AutoStart::Disabled
            };
            match startup::configure(mode) {
                Ok(()) => {
                    draft.launch_at_login = launch;
                    if !launch {
                        draft.start_in_tray = false;
                    }
                    *changed = true;
                }
                Err(error) => {
                    *notice = Some((
                        NoticeLevel::Error,
                        t!("err.autostart", error = format!("{error:#}")).to_string(),
                    ));
                }
            }
        }
    });
    theme::row(ui, "", |ui| {
        let autostart = draft.launch_at_login;
        let mut tray = draft.start_in_tray;
        if ui
            .add_enabled(
                autostart,
                egui::Checkbox::new(&mut tray, t!("settings.start_in_tray")),
            )
            .changed()
        {
            let mode = if tray {
                AutoStart::Tray
            } else {
                AutoStart::Window
            };
            match startup::configure(mode) {
                Ok(()) => {
                    draft.start_in_tray = tray;
                    *changed = true;
                }
                Err(error) => {
                    *notice = Some((
                        NoticeLevel::Error,
                        t!("err.autostart", error = format!("{error:#}")).to_string(),
                    ));
                }
            }
        }
    });
}

fn previews(ui: &mut egui::Ui, draft: &mut Settings, changed: &mut bool) {
    theme::section(ui, icon::FILM_STRIP, &t!("settings.previews"));
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(
                &mut draft.animated_previews,
                t!("settings.animated_previews"),
            )
            .on_hover_text(t!("settings.animated_previews_hint"))
            .changed()
        {
            *changed = true;
        }
    });
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(
                &mut draft.full_preview_videos,
                t!("settings.full_preview_videos"),
            )
            .on_hover_text(t!("settings.full_preview_videos_hint"))
            .changed()
        {
            *changed = true;
        }
    });
}

fn window(ui: &mut egui::Ui, draft: &mut Settings, changed: &mut bool) {
    theme::section(ui, icon::TRAY, &t!("settings.window"));
    theme::row(ui, &t!("settings.tray_left_click"), |ui| {
        let options = vec![
            (
                TrayClickAction::OpenWindow,
                t!("settings.tray_click_open_window").to_string(),
            ),
            (
                TrayClickAction::ScreenArea,
                t!("settings.tray_click_screen_area").to_string(),
            ),
        ];
        if theme::segmented(ui, &mut draft.tray_left_click, &options) {
            *changed = true;
        }
    });
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(
                &mut draft.stop_recording_on_left_click,
                t!("settings.stop_recording_on_left_click"),
            )
            .changed()
        {
            *changed = true;
        }
    });
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(&mut draft.tray_progress, t!("settings.tray_progress"))
            .on_hover_text(t!("settings.tray_progress_hint"))
            .changed()
        {
            *changed = true;
        }
    });
    window_close_row(ui, draft, changed);
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(
                &mut draft.highlight_recording_region,
                t!("settings.highlight_recording_region"),
            )
            .changed()
        {
            *changed = true;
        }
    });
    ui.add_enabled_ui(draft.highlight_recording_region, |ui| {
        theme::row(ui, &t!("settings.recording_region_outline_width"), |ui| {
            if ui
                .add(
                    egui::Slider::new(&mut draft.recording_region_outline_width, 2..=32)
                        .suffix(" px"),
                )
                .changed()
            {
                *changed = true;
            }
        });
    });
}

fn window_close(ui: &mut egui::Ui, draft: &mut Settings, changed: &mut bool) {
    theme::section(ui, icon::TRAY, &t!("settings.window"));
    window_close_row(ui, draft, changed);
}

fn window_close_row(ui: &mut egui::Ui, draft: &mut Settings, changed: &mut bool) {
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(&mut draft.close_to_tray, t!("settings.close_to_tray"))
            .changed()
        {
            *changed = true;
        }
    });
}

fn folders(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    changed: &mut bool,
    notice: &mut Option<(NoticeLevel, String)>,
) {
    theme::section(ui, icon::FOLDER_OPEN, &t!("settings.folders"));
    theme::row(ui, &t!("settings.media_dir"), |ui| {
        if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder")).clicked() {
            if let Some(picked) = mediaa2_platform::dialogs::pick_folder(&draft.media_dir) {
                draft.media_dir = picked;
                *changed = true;
            }
        }
        if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, &t!("settings.open_folder")).clicked() {
            if let Err(error) = mediaa2_platform::files::reveal(&draft.media_dir) {
                *notice = Some((NoticeLevel::Error, format!("{error:#}")));
            }
        }
        ui.add(egui::Label::new(draft.media_dir.to_string_lossy()).truncate());
    });
    theme::row(ui, &t!("settings.convert_dir"), |ui| {
        let current = draft.convert_dir();
        if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder")).clicked() {
            if let Some(picked) = mediaa2_platform::dialogs::pick_folder(&current) {
                draft.convert_dir = (picked != draft.media_dir).then_some(picked);
                *changed = true;
            }
        }
        if draft.convert_dir.is_some()
            && theme::icon_button(
                ui,
                icon::ARROW_COUNTER_CLOCKWISE,
                &t!("settings.convert_dir_reset"),
            )
            .clicked()
        {
            draft.convert_dir = None;
            *changed = true;
        }
        let text = draft
            .convert_dir
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned())
            .unwrap_or_else(|| t!("settings.convert_dir_library").to_string());
        ui.add(egui::Label::new(text).truncate());
    });
}

fn hotkeys(ui: &mut egui::Ui, draft: &mut Settings, changed: &mut bool) {
    ui.add_space(8.0);
    theme::section(ui, icon::TIMER, &t!("settings.hotkeys"));
    let start = std::cell::Cell::new(None::<usize>);
    let finished = std::cell::Cell::new(false);
    let changed_cell = std::cell::Cell::new(false);
    let capturing = ui.data(|data| data.get_temp::<usize>(egui::Id::new(HOTKEY_CAPTURE)));
    let snapshot = [
        draft.hotkeys.region_screenshot.clone(),
        draft.hotkeys.fullscreen_screenshot.clone(),
        draft.hotkeys.start_video.clone(),
        draft.hotkeys.start_audio.clone(),
        draft.hotkeys.stop_recording.clone(),
    ];
    let conflict = std::cell::Cell::new(
        ui.data(|data| data.get_temp::<bool>(egui::Id::new(HOTKEY_CONFLICT)))
            .unwrap_or(false),
    );
    let edit = HotkeyEdit {
        capturing,
        snapshot: &snapshot,
        start: &start,
        finished: &finished,
        changed: &changed_cell,
        conflict: &conflict,
    };
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if theme::is_wide(ui) {
            ui.columns(2, |cols| {
                hotkey_left(&mut cols[0], &mut draft.hotkeys, &edit);
                hotkey_right(&mut cols[1], &mut draft.hotkeys, &edit);
            });
        } else {
            hotkey_left(ui, &mut draft.hotkeys, &edit);
            ui.separator();
            hotkey_right(ui, &mut draft.hotkeys, &edit);
        }
    });
    if let Some(index) = start.get() {
        ui.data_mut(|data| data.insert_temp(egui::Id::new(HOTKEY_CAPTURE), index));
    } else if finished.get() {
        ui.data_mut(|data| data.remove::<usize>(egui::Id::new(HOTKEY_CAPTURE)));
    }
    ui.data_mut(|data| data.insert_temp(egui::Id::new(HOTKEY_CONFLICT), conflict.get()));
    if conflict.get() {
        ui.colored_label(
            theme::palette(ui).warning,
            format!("{} {}", icon::WARNING, t!("settings.hk_conflict")),
        );
    }
    *changed |= changed_cell.get();
}

struct HotkeyEdit<'a> {
    capturing: Option<usize>,
    snapshot: &'a [String; 5],
    start: &'a std::cell::Cell<Option<usize>>,
    finished: &'a std::cell::Cell<bool>,
    changed: &'a std::cell::Cell<bool>,
    conflict: &'a std::cell::Cell<bool>,
}

fn hotkey_left(
    ui: &mut egui::Ui,
    hotkeys: &mut mediaa2_core::settings::HotkeySettings,
    edit: &HotkeyEdit<'_>,
) {
    if ui
        .checkbox(&mut hotkeys.enabled, t!("settings.hotkeys_enable"))
        .changed()
    {
        edit.changed.set(true);
    }
    let enabled = hotkeys.enabled;
    ui.add_enabled_ui(enabled, |ui| {
        for (index, label, value) in [
            (0, t!("settings.hk_area"), &mut hotkeys.region_screenshot),
            (
                1,
                t!("settings.hk_screen"),
                &mut hotkeys.fullscreen_screenshot,
            ),
        ] {
            hotkey_row(ui, &label, value, index, edit);
        }
    });
}

fn hotkey_right(
    ui: &mut egui::Ui,
    hotkeys: &mut mediaa2_core::settings::HotkeySettings,
    edit: &HotkeyEdit<'_>,
) {
    let enabled = hotkeys.enabled;
    ui.add_enabled_ui(enabled, |ui| {
        for (index, label, value) in [
            (2, t!("settings.hk_video"), &mut hotkeys.start_video),
            (3, t!("settings.hk_audio"), &mut hotkeys.start_audio),
            (4, t!("settings.hk_stop"), &mut hotkeys.stop_recording),
        ] {
            hotkey_row(ui, &label, value, index, edit);
        }
    });
}

fn hotkey_row(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    index: usize,
    edit: &HotkeyEdit<'_>,
) {
    theme::row(ui, label, |ui| {
        let active = edit.capturing == Some(index);
        let text = if active {
            t!("settings.hk_press").to_string()
        } else if value.is_empty() {
            "—".to_string()
        } else {
            value.clone()
        };
        let palette = theme::palette(ui);
        let field = egui::Button::new(egui::RichText::new(text).color(if active {
            palette.accent
        } else {
            palette.text
        }))
        .stroke(egui::Stroke::new(
            1.0,
            if active {
                palette.accent
            } else {
                palette.border
            },
        ))
        .min_size(egui::vec2(170.0, 26.0));
        if ui
            .add(field)
            .on_hover_text(t!("settings.hk_click"))
            .clicked()
            && !active
        {
            edit.start.set(Some(index));
            edit.conflict.set(false);
        }
        if !value.is_empty() && theme::icon_button(ui, icon::X, &t!("settings.hk_clear")).clicked()
        {
            value.clear();
            edit.finished.set(true);
        }
        if active {
            if let Some(combo) = capture_combo(ui) {
                if let Some(combo) = combo {
                    if conflicts(index, &combo, edit.snapshot) {
                        edit.conflict.set(true);
                    } else {
                        *value = combo;
                        edit.changed.set(true);
                        edit.conflict.set(false);
                    }
                }
                edit.finished.set(true);
            }
            ui.ctx().request_repaint();
        }
    });
}

fn conflicts(index: usize, candidate: &str, values: &[String; 5]) -> bool {
    values.iter().enumerate().any(|(other, value)| {
        other != index
            && !value.trim().is_empty()
            && !actions_may_share(index, other)
            && same_combo(candidate, value)
    })
}

fn actions_may_share(left: usize, right: usize) -> bool {
    matches!((left, right), (4, 2) | (2, 4) | (4, 3) | (3, 4))
}

fn same_combo(left: &str, right: &str) -> bool {
    if left.trim().is_empty() || right.trim().is_empty() {
        return false;
    }
    normalize_combo(left) == normalize_combo(right)
}

fn normalize_combo(combo: &str) -> String {
    let mut parts: Vec<String> = combo
        .split('+')
        .map(|part| part.trim().to_lowercase())
        .filter(|part| !part.is_empty())
        .collect();
    parts.sort();
    parts.join("+")
}

fn capture_combo(ui: &egui::Ui) -> Option<Option<String>> {
    let events = ui.input(|input| input.events.clone());
    for event in events {
        if let egui::Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } = event
        {
            if key == egui::Key::Escape {
                return Some(None);
            }
            if let Some(combo) = combo_from_key(modifiers, key) {
                return Some(Some(combo));
            }
        }
    }
    None
}

fn combo_from_key(modifiers: egui::Modifiers, key: egui::Key) -> Option<String> {
    if key == egui::Key::Escape {
        return None;
    }
    let token = key.name().to_uppercase().replace(' ', "");
    let mut parts = Vec::new();
    if modifiers.ctrl {
        parts.push("Ctrl".to_string());
    }
    if modifiers.alt {
        parts.push("Alt".to_string());
    }
    if modifiers.shift {
        parts.push("Shift".to_string());
    }
    if modifiers.mac_cmd {
        parts.push("Super".to_string());
    }
    let has_modifier = !parts.is_empty();
    if !has_modifier && !combo_token_is_standalone(&token) {
        return None;
    }
    parts.push(pretty_key(&token));
    Some(parts.join("+"))
}

fn combo_token_is_standalone(token: &str) -> bool {
    (token.starts_with('F') && token[1..].parse::<u8>().is_ok())
        || matches!(token, "PRINTSCREEN" | "PAUSE" | "SCROLLLOCK" | "INSERT")
}

fn pretty_key(token: &str) -> String {
    match token {
        "PRINTSCREEN" => "PrintScreen".into(),
        "PAGEUP" => "PageUp".into(),
        "PAGEDOWN" => "PageDown".into(),
        "SCROLLLOCK" => "ScrollLock".into(),
        token if token.len() > 1 && !token.starts_with('F') => {
            let mut chars = token.chars();
            chars
                .next()
                .map(|first| first.to_string() + &chars.as_str().to_lowercase())
                .unwrap_or_default()
        }
        token => token.to_string(),
    }
}

fn profiles(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    advanced: bool,
    monitors: &[MonitorInfo],
    microphones: &[String],
    changed: &mut bool,
) {
    ui.add_space(8.0);
    profile_heading(ui, icon::RECORD, &t!("settings.hotkey_video"), true);
    if advanced {
        let mut screen_changed = false;
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::row(ui, &t!("settings.hotkey_screen"), |ui| {
                if screen_combo(ui, &mut draft.hotkey_screen, monitors) {
                    screen_changed = true;
                }
            });
        });
        *changed |= screen_changed;
    }
    ui.add_space(4.0);
    ui.add_space(2.0);
    *changed |= profile_card(ui, draft, RecordProfile::Hotkey, advanced, microphones);
    ui.add_space(8.0);
    profile_heading(ui, icon::CROP, &t!("settings.region_record"), true);
    *changed |= profile_card(ui, draft, RecordProfile::Region, advanced, microphones);
}

fn profile_heading(ui: &mut egui::Ui, icon: &str, title: &str, _large: bool) {
    theme::section(ui, icon, title);
}

fn screen_combo(ui: &mut egui::Ui, value: &mut ScreenChoice, monitors: &[MonitorInfo]) -> bool {
    let label = |choice: &ScreenChoice| match choice {
        ScreenChoice::Primary => t!("settings.screen_primary").to_string(),
        ScreenChoice::UnderCursor => t!("settings.screen_cursor").to_string(),
        ScreenChoice::Display(device) => monitors
            .iter()
            .find(|monitor| &monitor.device == device)
            .map(super::record::monitor_label)
            .unwrap_or_else(|| device.clone()),
    };
    let mut changed = false;
    theme::combo_box("settings-hotkey-screen")
        .selected_text(label(value))
        .show_ui(ui, |ui| {
            let mut options = vec![ScreenChoice::Primary, ScreenChoice::UnderCursor];
            options.extend(
                monitors
                    .iter()
                    .map(|monitor| ScreenChoice::Display(monitor.device.clone())),
            );
            for option in options {
                let text = label(&option);
                changed |= ui.selectable_value(value, option, text).changed();
            }
        });
    changed
}

fn profile_card(
    ui: &mut egui::Ui,
    draft: &mut Settings,
    profile: RecordProfile,
    advanced: bool,
    microphones: &[String],
) -> bool {
    let mut changed = false;
    let (record, anim) = draft.profile_mut(profile);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if theme::is_wide(ui) {
            ui.columns(2, |cols| {
                changed |= output_fields(&mut cols[0], record, anim, profile, advanced);
                changed |= side_fields(&mut cols[1], record, anim, profile, advanced, microphones);
            });
        } else {
            changed |= output_fields(ui, record, anim, profile, advanced);
            ui.separator();
            changed |= side_fields(ui, record, anim, profile, advanced, microphones);
        }
    });
    changed
}

fn output_fields(
    ui: &mut egui::Ui,
    record: &mut RecordSettings,
    anim: &mut AnimationQuality,
    profile: RecordProfile,
    advanced: bool,
) -> bool {
    let mut changed = false;
    let salt = profile_salt(profile);
    theme::section(ui, icon::FILM_STRIP, &t!("record.output"));
    theme::row(ui, &t!("record.format"), |ui| {
        let kinds = if advanced {
            output_kinds(profile)
        } else {
            simple_kinds(profile)
        };
        let options: Vec<(OutputKind, String)> = kinds
            .into_iter()
            .map(|kind| (kind, kind.label().to_string()))
            .collect();
        let mut chosen = record.output;
        if theme::segmented(ui, &mut chosen, &options) && chosen != record.output {
            record.switch_output(chosen, anim);
            changed = true;
        }
    });
    let presets: Vec<RecordPreset> = record.preset_options().to_vec();
    if !presets.is_empty() {
        theme::row(ui, &t!("record.preset"), |ui| {
            let current = record.preset(anim);
            let selected = current
                .map(preset_label)
                .unwrap_or_else(|| t!("record.preset_custom").to_string());
            let mut picked = None;
            theme::combo_box(("settings-preset", salt))
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for preset in presets {
                        if ui
                            .selectable_label(current == Some(preset), preset_label(preset))
                            .clicked()
                        {
                            picked = Some(preset);
                        }
                    }
                });
            if let Some(preset) = picked {
                record.apply_preset(preset, anim);
                changed = true;
            }
        });
    }
    if advanced || profile != RecordProfile::Region {
        theme::row(ui, &t!("record.resolution"), |ui| {
            if resolution_picker(
                ui,
                &mut record.output_height,
                egui::Id::new(("settings-height", salt)),
            ) {
                changed = true;
            }
        });
    }
    if advanced {
        theme::row(ui, &t!("record.fps"), |ui| {
            if ui
                .add(egui::Slider::new(&mut record.fps, FPS.min..=FPS.max).suffix(" fps"))
                .changed()
            {
                changed = true;
            }
        });
    }
    if advanced && matches!(record.output, OutputKind::Video(_)) {
        theme::row(ui, &t!("record.quality"), |ui| {
            if ui
                .add(
                    egui::Slider::new(
                        &mut record.video_quality,
                        VIDEO_QUALITY.min as u8..=VIDEO_QUALITY.max as u8,
                    )
                    .suffix("%"),
                )
                .changed()
            {
                changed = true;
            }
        });
    }
    theme::row(ui, "", |ui| {
        if ui
            .checkbox(&mut record.record_cursor, t!("record.cursor"))
            .changed()
        {
            changed = true;
        }
    });
    if advanced {
        theme::row(ui, "", |ui| {
            if ui
                .checkbox(&mut record.show_no_signal, t!("record.show_no_signal"))
                .on_hover_text(t!("record.show_no_signal_hint"))
                .changed()
            {
                changed = true;
            }
        });
        if profile != RecordProfile::Region {
            theme::row(ui, "", |ui| {
                if ui
                    .checkbox(
                        &mut record.allow_unavailable_sources,
                        t!("record.allow_unavailable_sources"),
                    )
                    .on_hover_text(t!("record.allow_unavailable_sources_hint"))
                    .changed()
                {
                    changed = true;
                }
            });
        }
    }
    changed
}

fn side_fields(
    ui: &mut egui::Ui,
    record: &mut RecordSettings,
    anim: &mut AnimationQuality,
    profile: RecordProfile,
    advanced: bool,
    microphones: &[String],
) -> bool {
    let mut changed = false;
    let salt = profile_salt(profile);
    match record.output {
        OutputKind::Audio(_) => {}
        OutputKind::Video(_) => {
            theme::section(ui, icon::MICROPHONE, &t!("record.audio"));
            theme::row(ui, "", |ui| {
                if ui
                    .checkbox(&mut record.record_audio, t!("record.record_audio"))
                    .changed()
                {
                    changed = true;
                }
            });
            ui.add_enabled_ui(record.record_audio, |ui| {
                theme::row(ui, &t!("record.microphone"), |ui| {
                    if mic_combo(
                        ui,
                        &mut record.microphone,
                        microphones,
                        egui::Id::new(("settings-mic", salt)),
                    ) {
                        changed = true;
                    }
                });
                theme::row(ui, "", |ui| {
                    let supported = mediaa2_platform::audio::system_audio_supported();
                    ui.add_enabled_ui(supported, |ui| {
                        if ui
                            .checkbox(&mut record.system_audio, t!("record.system_audio"))
                            .changed()
                        {
                            changed = true;
                        }
                    });
                    if !supported {
                        theme::hint(ui, t!("record.system_audio_linux"));
                    }
                });
            });
        }
        OutputKind::Anim(format) => {
            if advanced {
                theme::section(ui, icon::GIF, &t!("record.animation"));
                if format != AnimFormat::Webp {
                    theme::row(ui, &t!("anim.colors"), |ui| {
                        if ui
                            .add(egui::Slider::new(
                                &mut anim.gif_colors,
                                GIF_COLORS.min..=GIF_COLORS.max,
                            ))
                            .changed()
                        {
                            changed = true;
                        }
                    });
                    theme::row(ui, &t!("anim.dither"), |ui| {
                        let mut dither = anim.gif_dither;
                        theme::combo_box(("settings-dither", salt))
                            .selected_text(dither.label())
                            .show_ui(ui, |ui| {
                                for option in GifDither::ALL {
                                    ui.selectable_value(&mut dither, option, option.label());
                                }
                            });
                        if dither != anim.gif_dither {
                            anim.gif_dither = dither;
                            changed = true;
                        }
                    });
                }
                if format != AnimFormat::Gif {
                    theme::row(ui, &t!("anim.webp_quality"), |ui| {
                        if ui
                            .add(
                                egui::Slider::new(
                                    &mut anim.webp_quality,
                                    VIDEO_QUALITY.min as u8..=VIDEO_QUALITY.max as u8,
                                )
                                .suffix("%"),
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    });
                }
            }
        }
    }
    changed
}

fn audio_card(
    ui: &mut egui::Ui,
    audio: &mut AudioRecordSettings,
    simple: bool,
    microphones: &[String],
) -> bool {
    let mut changed = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        if theme::is_wide(ui) {
            ui.columns(2, |cols| {
                changed |= audio_output(&mut cols[0], audio, simple);
                changed |= audio_sources(&mut cols[1], audio, microphones);
            });
        } else {
            changed |= audio_output(ui, audio, simple);
            ui.separator();
            changed |= audio_sources(ui, audio, microphones);
        }
    });
    changed
}

fn audio_output(ui: &mut egui::Ui, audio: &mut AudioRecordSettings, simple: bool) -> bool {
    let mut changed = false;
    theme::section(ui, icon::FILE_AUDIO, &t!("audio.output"));
    theme::row(ui, &t!("record.format"), |ui| {
        let formats: &[AudioFormat] = if simple {
            &AudioFormat::SIMPLE
        } else {
            &AudioFormat::ALL
        };
        let options: Vec<(AudioFormat, String)> = formats
            .iter()
            .map(|format| (*format, format.label().to_string()))
            .collect();
        let mut chosen = audio.format;
        if theme::segmented(ui, &mut chosen, &options) && chosen != audio.format {
            audio.format = chosen;
            if let Some(kbps) = audio_default_kbps(chosen) {
                audio.kbps = kbps;
            }
            changed = true;
        }
    });
    if !audio.format.lossless() {
        theme::row(ui, &t!("record.preset"), |ui| {
            let current = audio.preset();
            let selected = current
                .map(preset_label)
                .unwrap_or_else(|| t!("record.preset_custom").to_string());
            let mut picked = None;
            theme::combo_box("settings-audio-preset")
                .selected_text(selected)
                .show_ui(ui, |ui| {
                    for preset in RecordPreset::AUDIO {
                        if ui
                            .selectable_label(current == Some(preset), preset_label(preset))
                            .clicked()
                        {
                            picked = Some(preset);
                        }
                    }
                });
            if let Some(preset) = picked {
                audio.apply_preset(preset);
                changed = true;
            }
        });
    }
    if !simple && !audio.format.lossless() {
        theme::row(ui, &t!("audio.bitrate"), |ui| {
            let mut kbps = audio.kbps;
            theme::combo_box("settings-audio-kbps")
                .selected_text(format!("{kbps} kbit/s"))
                .show_ui(ui, |ui| {
                    for value in AUDIO_BITRATES {
                        ui.selectable_value(&mut kbps, value, format!("{value} kbit/s"));
                    }
                });
            if kbps != audio.kbps {
                audio.kbps = kbps;
                changed = true;
            }
        });
    }
    changed
}

fn audio_sources(
    ui: &mut egui::Ui,
    audio: &mut AudioRecordSettings,
    microphones: &[String],
) -> bool {
    let mut changed = false;
    theme::section(ui, icon::MICROPHONE, &t!("audio.sources"));
    theme::row(ui, &t!("record.microphone"), |ui| {
        if mic_combo(
            ui,
            &mut audio.microphone,
            microphones,
            egui::Id::new("settings-audio-mic"),
        ) {
            changed = true;
        }
    });
    theme::row(ui, "", |ui| {
        let supported = mediaa2_platform::audio::system_audio_supported();
        ui.add_enabled_ui(supported, |ui| {
            if ui
                .checkbox(&mut audio.system_audio, t!("record.system_audio"))
                .changed()
            {
                changed = true;
            }
        });
        if !supported {
            theme::hint(ui, t!("record.system_audio_linux"));
        }
    });
    changed
}

fn output_kinds(profile: RecordProfile) -> Vec<OutputKind> {
    OutputKind::ALL
        .into_iter()
        .filter(|kind| profile != RecordProfile::Hotkey || matches!(kind, OutputKind::Video(_)))
        .collect()
}

fn simple_kinds(profile: RecordProfile) -> Vec<OutputKind> {
    match profile {
        RecordProfile::Region => vec![
            OutputKind::Anim(AnimFormat::Gif),
            OutputKind::Anim(AnimFormat::Webp),
        ],
        RecordProfile::App | RecordProfile::Hotkey => vec![
            OutputKind::Video(VideoContainer::Mkv),
            OutputKind::Video(VideoContainer::Mp4),
        ],
    }
}

fn profile_salt(profile: RecordProfile) -> u8 {
    match profile {
        RecordProfile::App => 0,
        RecordProfile::Hotkey => 1,
        RecordProfile::Region => 2,
    }
}

fn resolution_picker(ui: &mut egui::Ui, value: &mut u32, id: egui::Id) -> bool {
    let label = |height: u32| match height {
        0 => t!("record.res_original").to_string(),
        2160 => "2160p (4K)".to_string(),
        1440 => "1440p (QHD)".to_string(),
        1080 => "1080p (FHD)".to_string(),
        720 => "720p (HD)".to_string(),
        height => format!("{height}p"),
    };
    let custom_id = id.with("custom");
    let mut custom = ui
        .data(|data| data.get_temp::<bool>(custom_id))
        .unwrap_or(*value != 0 && !OUTPUT_HEIGHTS.contains(value));
    let mut changed = false;
    let selected = if custom {
        t!("record.res_custom").to_string()
    } else {
        label(*value)
    };
    theme::combo_box(id)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(!custom && *value == 0, label(0))
                .clicked()
            {
                *value = 0;
                custom = false;
                changed = true;
            }
            for &height in &OUTPUT_HEIGHTS {
                if ui
                    .selectable_label(!custom && *value == height, label(height))
                    .clicked()
                {
                    *value = height;
                    custom = false;
                    changed = true;
                }
            }
            if ui
                .selectable_label(custom, t!("record.res_custom"))
                .clicked()
                && !custom
            {
                custom = true;
                if *value == 0 {
                    *value = 480;
                }
                changed = true;
            }
        })
        .response
        .on_hover_text(t!("record.resolution_hint"));
    if custom {
        changed |= ui
            .add(egui::DragValue::new(value).range(16..=4320).suffix(" px"))
            .changed();
    }
    ui.data_mut(|data| data.insert_temp(custom_id, custom));
    changed
}

fn theme_label(theme: ThemePreference) -> String {
    match theme {
        ThemePreference::System => t!("theme.system").to_string(),
        ThemePreference::Light => t!("theme.light").to_string(),
        ThemePreference::Dark => t!("theme.dark").to_string(),
        ThemePreference::Lime => t!("theme.lime").to_string(),
        ThemePreference::Peach => t!("theme.peach").to_string(),
    }
}

fn language_label(language: Option<&str>) -> String {
    match language {
        None => t!("settings.language_system").to_string(),
        Some(code) => LANGUAGES
            .iter()
            .find(|(value, _)| *value == code)
            .map(|(_, name)| (*name).to_string())
            .unwrap_or_else(|| code.to_string()),
    }
}

fn preset_label(preset: RecordPreset) -> String {
    match preset {
        RecordPreset::Quality => t!("record.preset_quality").to_string(),
        RecordPreset::Medium => t!("record.preset_medium").to_string(),
        RecordPreset::Fast => t!("record.preset_fast").to_string(),
    }
}
