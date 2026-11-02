use super::header;
use crate::features::intent::Intent;
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_core::formats::AudioFormat;
use mediaa_core::settings::{AudioRecordSettings, MicChoice, RecordPreset, AUDIO_BITRATES};
use mediaa_core::Settings;
use rust_i18n::t;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AudioView {
    pub live: bool,
    pub system_audio: bool,
}

pub fn ui(
    view: AudioView,
    settings: &mut Settings,
    microphones: &[String],
    ui: &mut egui::Ui,
) -> Vec<Intent> {
    let mut intents = Vec::new();
    header(ui, &t!("audio.title"), "");
    if view.live {
        ui.disable();
    }

    let simple = !settings.advanced_settings;
    let (changed, refresh_mics) = audio_editor(
        ui,
        &mut settings.audio_record,
        microphones,
        "page",
        simple,
        view.system_audio,
    );
    if changed {
        intents.push(Intent::SettingsEdited);
    }
    if refresh_mics {
        intents.push(Intent::RefreshCatalogs);
    }
    intents
}

pub fn audio_editor(
    ui: &mut egui::Ui,
    a: &mut AudioRecordSettings,
    mics: &[String],
    salt: &str,
    simple: bool,
    system_audio_supported: bool,
) -> (bool, bool) {
    let changed = std::cell::Cell::new(false);
    let mark = |c: bool| changed.set(changed.get() | c);
    let mut refresh_mics = false;
    let current_preset = a.preset();
    let pending_preset = std::cell::Cell::new(None);
    let (format, kbps, microphone, system_audio) = (
        &mut a.format,
        &mut a.kbps,
        &mut a.microphone,
        &mut a.system_audio,
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
                let opts: Vec<(AudioFormat, String)> = formats
                    .iter()
                    .map(|f| (*f, f.label().to_string()))
                    .collect();
                if theme::segmented(ui, format, &opts) {
                    mark(true);
                    if let Some(default) = mediaa_core::settings::audio_default_kbps(*format) {
                        *kbps = default;
                    }
                }
            });
            if !format.lossless() {
                theme::row(ui, &t!("record.preset"), |ui| {
                    if let Some(preset) = super::preset_combo(
                        ui,
                        egui::Id::new(("audio-preset", salt)),
                        current_preset,
                        &RecordPreset::AUDIO,
                    ) {
                        pending_preset.set(Some(preset));
                    }
                });
            }
            if !simple && !format.lossless() {
                theme::row(ui, &t!("audio.bitrate"), |ui| {
                    crate::ui::theme::combo_box(("audio-kbps", salt))
                        .selected_text(format!("{} kbit/s", kbps))
                        .show_ui(ui, |ui| {
                            for k in AUDIO_BITRATES {
                                mark(
                                    ui.selectable_value(kbps, k, format!("{k} kbit/s"))
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
                let (c, opened) = super::record::mic_combo(
                    ui,
                    microphone,
                    mics,
                    egui::Id::new(("audio-mic", salt)),
                );
                mark(c);
                refresh_mics = opened;
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
    if let Some(preset) = pending_preset.get() {
        a.apply_preset(preset);
        changed.set(true);
    }
    (changed.get(), refresh_mics)
}

pub fn start_bar(
    settings: &Settings,
    recovery_active: bool,
    system_audio_supported: bool,
    ui: &mut egui::Ui,
) -> bool {
    let mut start = false;
    let a = &settings.audio_record;
    let has_source = a.microphone != MicChoice::Off || (a.system_audio && system_audio_supported);
    let available = has_source && !recovery_active;
    ui.vertical_centered(|ui| {
        let btn = theme::primary(ui, icon::MICROPHONE, &t!("audio.start"))
            .min_size(egui::vec2(220.0, 36.0));
        let mut resp = ui.add_enabled(available, btn);
        let hk = &settings.hotkeys;
        if hk.enabled && !hk.start_audio.is_empty() {
            resp = resp.on_hover_text(t!("record.hotkey_hint", key = hk.start_audio));
        }
        if recovery_active {
            resp = resp.on_disabled_hover_text(t!("record.recovery_wait"));
        } else if !has_source {
            resp = resp.on_disabled_hover_text(t!("audio.no_source"));
        }
        if resp.clicked() {
            start = true;
        }
    });
    start
}
