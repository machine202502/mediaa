use super::output_kinds;
use super::status::starting_card;
use crate::pages::record::profile_editor;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use mediaa_core::settings::RecordProfile;
use mediaa_core::{AnimFormat, OutputKind, Settings, VideoContainer};
use rust_i18n::t;
use std::time::Duration;

fn profile_has_audio_every_file(profile: RecordProfile, settings: Settings) -> bool {
    let mut harness = egui_kittest::Harness::new_ui_state(
        move |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, profile, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    let label = t!("record.audio_every_file").to_string();
    harness.query_by_label(&label).is_some()
}

#[test]
fn quick_video_offers_only_video_containers() {
    assert_eq!(
        output_kinds(RecordProfile::Hotkey),
        vec![
            OutputKind::Video(VideoContainer::Mp4),
            OutputKind::Video(VideoContainer::Mkv),
        ]
    );
    assert_eq!(
        output_kinds(RecordProfile::Region),
        OutputKind::ALL.to_vec()
    );
}

#[test]
fn animation_editor_shows_one_frame_rate_and_resolution() {
    let mut settings = Settings::default();
    settings.region_record.output = OutputKind::Anim(AnimFormat::Webp);
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::Region, &[], None, &mut refresh);
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
    let _ui = crate::app::test_support::ui_test_guard();
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
        .query_by_label(&t!("record.preset").to_string())
        .is_some());
    assert_eq!(
        harness.state().record.preset(&harness.state().anim),
        Some(mediaa_core::settings::RecordPreset::Quality)
    );

    harness.state_mut().record.video_quality = 50;
    harness.run_steps(2);
    assert_eq!(harness.state().record.preset(&harness.state().anim), None);
}

#[test]
fn audio_preset_defaults_to_quality_and_becomes_custom() {
    let _ui = crate::app::test_support::ui_test_guard();
    let audio = mediaa_core::settings::AudioRecordSettings::default();
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, audio: &mut mediaa_core::settings::AudioRecordSettings| {
            ui.set_width(1100.0);
            crate::pages::audio::audio_editor(ui, audio, &[], "test");
        },
        audio,
    );
    harness.run_steps(2);
    assert_eq!(
        harness.state().preset(),
        Some(mediaa_core::settings::RecordPreset::Quality)
    );
    harness.state_mut().kbps = 160;
    harness.run_steps(2);
    assert_eq!(harness.state().preset(), None);
}

#[test]
fn switching_format_restores_that_formats_settings() {
    let _ui = crate::app::test_support::ui_test_guard();
    let mut settings = Settings::default();
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
    assert_eq!((record.fps, record.output_height), (60, 2160));

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
    let _ui = crate::app::test_support::ui_test_guard();
    let mut settings = Settings::default();
    settings.region_record.output = OutputKind::Anim(AnimFormat::Webp);
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, settings: &mut Settings| {
            ui.set_width(1100.0);
            let mut refresh = false;
            profile_editor(ui, settings, RecordProfile::Region, &[], None, &mut refresh);
        },
        settings,
    );
    harness.run_steps(2);
    assert!(harness
        .query_by_label(&t!("record.preset").to_string())
        .is_some());
    assert_eq!(
        harness
            .state()
            .region_record
            .preset(&harness.state().region_anim),
        Some(mediaa_core::settings::RecordPreset::Fast)
    );

    let state = harness.state_mut();
    state.region_record.apply_preset(
        mediaa_core::settings::RecordPreset::Quality,
        &mut state.region_anim,
    );
    harness.run_steps(2);
    let record = &harness.state().region_record;
    assert_eq!(record.webp_mode, mediaa_core::settings::WebpMode::Quality);
    assert_eq!((record.fps, record.output_height), (30, 1080));
    assert_eq!(
        record.preset(&harness.state().region_anim),
        Some(mediaa_core::settings::RecordPreset::Quality)
    );
}

#[test]
fn single_source_profiles_hide_audio_in_every_file() {
    assert!(profile_has_audio_every_file(
        RecordProfile::App,
        Settings::default()
    ));
    assert!(!profile_has_audio_every_file(
        RecordProfile::Hotkey,
        Settings::default()
    ));
    let mut settings = Settings::default();
    settings.region_record.output = OutputKind::Video(VideoContainer::Mkv);
    assert!(!profile_has_audio_every_file(
        RecordProfile::Region,
        settings
    ));
}

#[test]
fn starting_recording_has_a_working_cancel_action() {
    let mut harness = egui_kittest::Harness::new_ui_state(
        |ui: &mut egui::Ui, state: &mut (Duration, bool)| {
            state.1 |= starting_card(ui, state.0);
        },
        (Duration::from_secs(1), false),
    );
    harness.run_steps(2);
    assert!(harness.query_by_label(&t!("record.starting")).is_some());
    harness.get_by_label_contains(&t!("common.cancel")).click();
    harness.run_steps(2);
    assert!(harness.state().1);
}
