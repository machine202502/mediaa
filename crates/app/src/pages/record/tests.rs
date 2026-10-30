use super::output_kinds;
use super::status::starting_card;
use crate::pages::record::profile_editor;
use eframe::egui;
use egui_kittest::kittest::{NodeT, Queryable};
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
fn parallel_webp_is_shown_only_for_segmented_webp() {
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
    let segmented = t!("record.segmented_webp").to_string();
    let parallel = t!("record.parallel_webp").to_string();
    assert!(!harness
        .get_by_label(&segmented)
        .accesskit_node()
        .is_disabled());
    assert!(!harness
        .get_by_label(&parallel)
        .accesskit_node()
        .is_disabled());
    assert!(harness.state().region_record.segmented_webp);
    assert!(harness.state().region_record.parallel_webp);

    harness.state_mut().region_record.segmented_webp = false;
    harness.run_steps(2);
    assert!(!harness
        .get_by_label(&segmented)
        .accesskit_node()
        .is_disabled());
    assert!(harness
        .get_by_label(&parallel)
        .accesskit_node()
        .is_disabled());

    harness.state_mut().region_record.output = OutputKind::Anim(AnimFormat::Gif);
    harness.run();
    assert!(harness.query_by_label(&segmented).is_none());
    assert!(harness.query_by_label(&parallel).is_none());
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
