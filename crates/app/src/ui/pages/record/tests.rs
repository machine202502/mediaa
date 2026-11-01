#![allow(clippy::field_reassign_with_default)]
use super::output_kinds;
use super::status::starting_card;
use crate::ui::pages::record::profile_editor;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use mediaa_core::settings::RecordProfile;
use mediaa_core::{AnimFormat, OutputKind, Settings, VideoContainer};
use rust_i18n::t;
use std::time::Duration;

#[test]
fn quick_video_offers_only_video_containers() {
    assert_eq!(
        output_kinds(RecordProfile::Hotkey),
        vec![
            OutputKind::Video(VideoContainer::Mkv),
            OutputKind::Video(VideoContainer::Mp4),
        ]
    );
    assert_eq!(
        output_kinds(RecordProfile::Region),
        OutputKind::ALL.to_vec()
    );
}

#[test]
fn animation_editor_shows_one_frame_rate_and_resolution() {
    let _ui = crate::test_support::ui_test_guard();
    let _locale = crate::test_support::english_locale();
    let mut settings = Settings::default();
    settings.advanced_settings = true;
    settings.record.output = OutputKind::Anim(AnimFormat::Webp);
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::App, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    let animation_fps = t!("anim.fps").to_string();
    let video_fps = t!("record.fps").to_string();
    let resolution = t!("record.resolution").to_string();
    assert!(harness.query_by_label(&animation_fps).is_none());
    assert!(harness.query_by_label(&video_fps).is_some());
    assert_eq!(harness.get_all_by_label(&resolution).count(), 1);
}

#[test]
fn manual_video_edits_switch_the_preset_to_custom() {
    let _ui = crate::test_support::ui_test_guard();
    let settings = Settings::default();
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::App, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    assert!(harness
        .query_by_label(t!("record.preset").as_ref())
        .is_some());
    assert_eq!(
        harness.state().record.simple_preset(&harness.state().anim),
        Some(mediaa_core::settings::RecordPreset::Medium)
    );

    harness.state_mut().record.video_quality = 50;
    harness.run_steps(2);
    assert_eq!(
        harness.state().record.simple_preset(&harness.state().anim),
        None
    );
}

#[test]
fn audio_preset_defaults_to_medium_and_becomes_custom() {
    let _ui = crate::test_support::ui_test_guard();
    let audio = mediaa_core::settings::AudioRecordSettings::default();
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, audio: &mut mediaa_core::settings::AudioRecordSettings| {
            ui.set_width(1100.0);
            crate::ui::pages::audio::audio_editor(ui, audio, &[], "test", false, true);
        },
        audio,
    );
    harness.run_steps(2);
    assert_eq!(
        harness.state().preset(),
        Some(mediaa_core::settings::RecordPreset::Medium)
    );
    harness.state_mut().kbps = 160;
    harness.run_steps(2);
    assert_eq!(harness.state().preset(), None);
}

#[test]
fn switching_format_restores_that_formats_settings() {
    let _ui = crate::test_support::ui_test_guard();
    let mut settings = Settings::default();
    settings.advanced_settings = true;
    settings.record.output = OutputKind::Video(VideoContainer::Mp4);
    settings.record.fps = 30;
    settings.record.output_height = 720;
    settings.record.video_quality = 70;
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::App, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    harness.get_by_label("MKV").click();
    harness.run_steps(2);
    let record = &harness.state().record;
    assert_eq!(record.output, OutputKind::Video(VideoContainer::Mkv));
    assert_eq!((record.fps, record.output_height), (60, 1080));

    harness.state_mut().record.fps = 30;
    harness.state_mut().record.output_height = 1080;
    harness.state_mut().record.video_quality = 80;
    harness.run_steps(2);
    harness.get_by_label("MP4").click();
    harness.run_steps(2);
    let record = &harness.state().record;
    assert_eq!(record.output, OutputKind::Video(VideoContainer::Mp4));
    assert_eq!(
        (record.fps, record.output_height, record.video_quality),
        (30, 720, 70)
    );
}

#[test]
fn animation_preset_applies_webp_mode_and_limits() {
    let _ui = crate::test_support::ui_test_guard();
    let mut settings = Settings::default();
    settings.advanced_settings = true;
    settings.record.output = OutputKind::Anim(AnimFormat::Webp);
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::App, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    assert!(harness
        .query_by_label(t!("record.preset").as_ref())
        .is_some());
    let state = harness.state_mut();
    state.record.apply_preset(
        mediaa_core::settings::RecordPreset::Quality,
        &mut state.anim,
    );
    harness.run_steps(2);
    let record = &harness.state().record;
    assert_eq!(record.webp_mode, mediaa_core::settings::WebpMode::Quality);
    assert_eq!((record.fps, record.output_height), (30, 1080));
    assert_eq!(
        record.preset(&harness.state().anim),
        Some(mediaa_core::settings::RecordPreset::Quality)
    );
}

#[test]
fn starting_recording_has_a_working_cancel_action() {
    let _ui = crate::test_support::ui_test_guard();
    let _locale = crate::test_support::english_locale();
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, state: &mut (Duration, bool)| {
            let ctx = ui.ctx().clone();
            state.1 |= starting_card(ui, state.0, &ctx);
        },
        (Duration::from_secs(1), false),
    );
    harness.run_steps(2);
    assert!(harness.query_by_label(&t!("record.starting")).is_some());
    harness.get_by_label_contains(&t!("common.cancel")).click();
    harness.run_steps(2);
    harness
        .get_by_label_contains(&t!("record.cancel_confirm"))
        .click();
    harness.run_steps(2);
    assert!(harness.state().1);
}

#[cfg(windows)]
#[test]
fn starting_card_uses_the_new_status_format() {
    let _ui = crate::test_support::ui_test_guard();
    crate::i18n::apply(Some("en"));
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, cancel: &mut bool| {
            ui.set_width(520.0);
            let ctx = ui.ctx().clone();
            *cancel |= starting_card(ui, Duration::from_secs(3), &ctx);
        },
        false,
    );
    crate::ui::theme::install(&harness.ctx);
    harness.run_steps(2);
    harness.snapshot("record-status-starting");
}

#[test]
fn only_advanced_settings_show_signal_and_tray_controls() {
    let _ui = crate::test_support::ui_test_guard();
    let _locale = crate::test_support::english_locale();
    let labels = [
        t!("record.show_no_signal").to_string(),
        t!("record.allow_unavailable_sources").to_string(),
        t!("record.hide_to_tray").to_string(),
    ];
    for advanced in [false, true] {
        let mut settings = Settings::default();
        settings.advanced_settings = advanced;
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui: &mut egui::Ui, settings: &mut Settings| {
                ui.set_width(1100.0);
                let mut refresh = false;
                profile_editor(ui, settings, RecordProfile::App, &[], None, &mut refresh);
            },
            settings,
        );
        harness.run_steps(2);
        for label in &labels {
            assert_eq!(
                harness.query_by_label(label).is_some(),
                advanced,
                "{label} visibility in advanced={advanced}"
            );
        }
    }
}

#[test]
fn hotkey_video_only_exposes_preset_and_audio() {
    let _ui = crate::test_support::ui_test_guard();
    let mut settings = Settings::default();
    settings.advanced_settings = false;
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::Hotkey, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    for label in [
        t!("record.resolution"),
        t!("record.fps"),
        t!("record.quality"),
    ] {
        assert!(harness.query_by_label(&label).is_none(), "{label}");
    }
    assert!(harness.query_by_label(&t!("record.format")).is_some());
    assert!(harness.query_by_label(&t!("record.preset")).is_some());
    assert!(harness.query_by_label("MP4").is_some());
    assert!(harness.query_by_label("MKV").is_some());
    assert_eq!(
        harness.state().hotkey_record.output,
        OutputKind::Video(VideoContainer::Mkv)
    );
    harness.get_by_label("MP4").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().hotkey_record.output,
        OutputKind::Video(VideoContainer::Mp4)
    );
    harness.state_mut().advanced_settings = true;
    harness.run_steps(2);
    assert!(harness.query_by_label(&t!("record.format")).is_some());
    harness.get_by_label("MP4").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().hotkey_record.output,
        OutputKind::Video(VideoContainer::Mp4)
    );
}

#[test]
fn region_recording_only_exposes_preset_and_gif_options() {
    let _ui = crate::test_support::ui_test_guard();
    use egui_kittest::kittest::NodeT;
    let mut settings = Settings::default();
    settings.advanced_settings = false;
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::Region, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    for label in [
        t!("record.resolution"),
        t!("record.fps"),
        t!("anim.colors"),
        t!("anim.dither"),
    ] {
        assert!(harness.query_by_label(&label).is_none(), "{label}");
    }
    assert!(harness.query_by_label(&t!("record.format")).is_some());
    assert!(harness.query_by_label(&t!("record.preset")).is_some());
    assert!(harness.query_by_label("GIF").is_some());
    assert!(harness.query_by_label("WebP").is_some());
    assert!(!harness
        .get_by_label(&t!("record.cursor"))
        .accesskit_node()
        .is_disabled());
    harness.get_by_label(&t!("record.cursor")).click();
    harness.run_steps(2);
    assert!(!harness.state().region_record.record_cursor);
    assert_eq!(
        harness.state().region_record.output,
        OutputKind::Anim(AnimFormat::Gif)
    );
    harness.get_by_label("WebP").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().region_record.output,
        OutputKind::Anim(AnimFormat::Webp)
    );
    harness.state_mut().advanced_settings = true;
    harness.run_steps(2);
    assert!(harness.query_by_label(&t!("record.format")).is_some());
    harness.get_by_label("WebP").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().region_record.output,
        OutputKind::Anim(AnimFormat::Webp)
    );
}
