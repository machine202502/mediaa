use super::record::{header, mic_combo_opened, preset_combo};
use super::Ctx;
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_core::formats::AudioFormat;
use mediaa2_core::settings::{
    audio_default_kbps, AudioRecordSettings, MicChoice, RecordPreset, AUDIO_BITRATES,
};
use mediaa2_i18n::t;

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    header(ui, &t!("audio.title"), "");
    command_error(ctx, ui);

    let busy = ctx.view.recording_active();

    let mut audio = ctx.ui.settings.draft.audio_record.clone();
    let simple = !ctx.ui.settings.draft.advanced_settings;
    let supported = mediaa2_platform::audio::system_audio_supported();
    let mut refresh = false;
    ui.add_enabled_ui(!busy, |ui| {
        let (_, opened) = audio_editor(ui, &mut audio, ctx.microphones, "page", simple, supported);
        refresh |= opened;
    });

    ctx.ui.audio.format = audio.format;
    ctx.ui.audio.kbps = audio.kbps;

    if audio != ctx.ui.settings.draft.audio_record {
        ctx.ui.settings.draft.audio_record = audio.clone();
        ctx.ui.settings.dirty = true;
        let settings = ctx.ui.settings.draft.clone();
        ctx.actions.set_settings(settings);
    }
    if refresh {
        ctx.actions.refresh_catalogs();
    }
}

pub(super) fn audio_editor(
    ui: &mut egui::Ui,
    audio: &mut AudioRecordSettings,
    microphones: &[String],
    salt: &str,
    simple: bool,
    system_audio_supported: bool,
) -> (bool, bool) {
    let changed = std::cell::Cell::new(false);
    let mark = |value: bool| changed.set(changed.get() | value);
    let mut refresh = false;
    let current_preset = audio.preset();
    let mut pending_preset = None;
    let (format, kbps, microphone, system_audio) = (
        &mut audio.format,
        &mut audio.kbps,
        &mut audio.microphone,
        &mut audio.system_audio,
    );
    theme::split_card(
        ui,
        |ui| {
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
                if theme::segmented(ui, format, &options) {
                    mark(true);
                    if let Some(default) = audio_default_kbps(*format) {
                        *kbps = default;
                    }
                }
            });
            if !format.lossless() {
                theme::row(ui, &t!("record.preset"), |ui| {
                    if let Some(preset) = preset_combo(
                        ui,
                        egui::Id::new(("audio-preset", salt)),
                        current_preset,
                        &RecordPreset::AUDIO,
                    ) {
                        pending_preset = Some(preset);
                    }
                });
            }
            if !simple && !format.lossless() {
                theme::row(ui, &t!("audio.bitrate"), |ui| {
                    theme::combo_box(("audio-kbps", salt))
                        .selected_text(format!("{kbps} kbit/s"))
                        .show_ui(ui, |ui| {
                            for value in AUDIO_BITRATES {
                                mark(
                                    ui.selectable_value(kbps, value, format!("{value} kbit/s"))
                                        .changed(),
                                );
                            }
                        });
                });
            }
        },
        |ui| {
            theme::section(ui, icon::MICROPHONE, &t!("audio.sources"));
            theme::row(ui, &t!("record.microphone"), |ui| {
                let (mic_changed, opened) = mic_combo_opened(
                    ui,
                    microphone,
                    microphones,
                    egui::Id::new(("audio-mic", salt)),
                );
                mark(mic_changed);
                refresh = opened;
            });
            theme::row(ui, "", |ui| {
                ui.add_enabled_ui(system_audio_supported, |ui| {
                    mark(
                        ui.checkbox(system_audio, t!("record.system_audio"))
                            .changed(),
                    );
                });
                if !system_audio_supported {
                    theme::hint(ui, t!("record.system_audio_linux"));
                }
            });
        },
    );
    if let Some(preset) = pending_preset {
        audio.apply_preset(preset);
        mark(true);
    }
    (changed.get(), refresh)
}

pub(super) fn start_bar(ctx: &Ctx<'_>, ui: &mut egui::Ui) -> bool {
    let audio = &ctx.ui.settings.draft.audio_record;
    let system_audio_supported = mediaa2_platform::audio::system_audio_supported();
    let recovery_active = ctx.view.recovery().running;
    let hotkeys = &ctx.ui.settings.draft.hotkeys;
    let mut start = false;
    let has_source =
        audio.microphone != MicChoice::Off || (audio.system_audio && system_audio_supported);
    let available = has_source && !recovery_active;
    ui.vertical_centered(|ui| {
        let button = theme::primary(ui, icon::MICROPHONE, &t!("audio.start"))
            .min_size(egui::vec2(220.0, 36.0));
        let mut response = ui.add_enabled(available, button);
        if hotkeys.enabled && !hotkeys.start_audio.is_empty() {
            response = response.on_hover_text(t!("record.hotkey_hint", key = hotkeys.start_audio));
        }
        if recovery_active {
            response = response.on_disabled_hover_text(t!("record.recovery_wait"));
        } else if !has_source {
            response = response.on_disabled_hover_text(t!("audio.no_source"));
        }
        if response.clicked() {
            start = true;
        }
    });
    start
}

fn command_error(ctx: &Ctx<'_>, ui: &mut egui::Ui) {
    let Some(error) = ctx.command_error else {
        return;
    };
    let palette = theme::palette(ui);
    ui.label(egui::RichText::new(error).size(12.0).color(palette.danger));
}
