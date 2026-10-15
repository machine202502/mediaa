//! Record audio page: microphone and / or system sound into MP3, M4A, WAV, FLAC or OGG.
//! Only one recording runs at a time (video or audio); the start button sits at the bottom
//! like on the video page.

use super::header;
use crate::app::App;
use crate::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_core::formats::AudioFormat;
use mediaa_core::settings::{AudioRecordSettings, MicChoice, RecordProfile, AUDIO_BITRATES};
use rust_i18n::t;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    header(ui, &t!("audio.title"), "");
    if super::record::live_status(app, ui, &ctx) {
        return;
    }

    let mics = app.mic_devices.clone();
    let (changed, refresh_mics) = audio_editor(ui, &mut app.settings.audio_record, &mics, "page");
    if changed {
        app.settings_changed();
    }
    if refresh_mics {
        app.refresh_mics();
    }
}

/// Output (format, bitrate) and sources (microphone, system sound) of an audio recording in one
/// card; also used on the Settings page for the hotkey / tray settings. Returns (changed,
/// microphone list just opened).
pub fn audio_editor(ui: &mut egui::Ui, a: &mut AudioRecordSettings, mics: &[String], salt: &str) -> (bool, bool) {
    let changed = std::cell::Cell::new(false);
    let mark = |c: bool| changed.set(changed.get() | c);
    let mut refresh_mics = false;
    let (format, kbps, microphone, system_audio) = (&mut a.format, &mut a.kbps, &mut a.microphone, &mut a.system_audio);
    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::FILE_AUDIO, &t!("audio.output"));
            theme::row(ui, &t!("record.format"), |ui| {
                let opts: Vec<(AudioFormat, String)> = AudioFormat::ALL.iter().map(|f| (*f, f.label().to_string())).collect();
                mark(theme::segmented(ui, format, &opts));
            });
            if !format.lossless() {
                theme::row(ui, &t!("audio.bitrate"), |ui| {
                    egui::ComboBox::from_id_salt(("audio-kbps", salt)).selected_text(format!("{} kbit/s", kbps)).show_ui(ui, |ui| {
                        for k in AUDIO_BITRATES {
                            mark(ui.selectable_value(kbps, k, format!("{k} kbit/s")).changed());
                        }
                    });
                });
            }
        },
        |ui| {
            theme::section(ui, icon::MICROPHONE, &t!("audio.sources"));
            theme::row(ui, &t!("record.microphone"), |ui| {
                let (c, opened) = super::record::mic_combo(ui, microphone, mics, egui::Id::new(("audio-mic", salt)));
                mark(c);
                refresh_mics = opened;
            });
            theme::row(ui, "", |ui| {
                let supported = mediaa_platform::audio::system_audio_supported();
                ui.add_enabled_ui(supported, |ui| {
                    mark(ui.checkbox(system_audio, t!("record.system_audio")).changed());
                });
                if !supported {
                    theme::hint(ui, t!("record.system_audio_linux"));
                }
            });
        },
    );
    (changed.get(), refresh_mics)
}

/// "Start audio recording" bar at the bottom of the window.
pub fn start_bar(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let a = &app.settings.audio_record;
    let has_source = a.microphone != MicChoice::Off || (a.system_audio && mediaa_platform::audio::system_audio_supported());
    ui.vertical_centered(|ui| {
        let btn = theme::primary(ui, icon::MICROPHONE, &t!("audio.start")).min_size(egui::vec2(220.0, 36.0));
        let mut resp = ui.add_enabled(has_source, btn);
        let hk = &app.settings.hotkeys;
        if hk.enabled && !hk.start_audio.is_empty() {
            resp = resp.on_hover_text(t!("record.hotkey_hint", key = hk.start_audio));
        }
        if !has_source {
            resp = resp.on_disabled_hover_text(t!("audio.no_source"));
        }
        if resp.clicked() {
            app.start_audio_recording(&ctx, RecordProfile::App);
        }
    });
}
