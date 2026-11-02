use super::test_support::TestCatalog;
use super::{
    completion_sound_after_action, daemon_must_quit, page_from_args, recording_completion_sound,
    recovery_links, tray_click_stops_recording, tray_state_for_phase, NoticeLevel, Page,
    StartupAction, StopSource,
};
use crate::features::notices::NoticeAction;
use crate::features::recording::RecordingPhase;
use eframe::egui;
use mediaa_core::settings::{AnimSettings, AnimationQuality};
use mediaa_core::{AnimFormat, OutputKind, Settings, VideoContainer};
use std::time::{Duration, Instant};

fn parse(args: &[&str]) -> StartupAction {
    StartupAction::from_args(args.iter().map(|s| s.to_string()))
}

#[test]
fn closing_the_window_keeps_the_daemon_unless_the_user_asked_to_quit() {
    assert!(!daemon_must_quit(true, true, false));
    assert!(daemon_must_quit(false, true, false));
    assert!(daemon_must_quit(true, false, false));
    assert!(daemon_must_quit(true, true, true));
}

#[test]
fn tray_click_stops_recording_only_when_enabled_and_recording() {
    assert!(tray_click_stops_recording(true, true));
    assert!(!tray_click_stops_recording(false, true));
    assert!(!tray_click_stops_recording(true, false));
}

#[test]
fn animation_recording_combines_profile_timing_and_persisted_quality() {
    let quality = AnimationQuality {
        gif_colors: 64,
        webp_quality: 91,
        ..AnimationQuality::default()
    };
    let animation = AnimSettings::for_recording(60, Some(1080), &quality);
    assert_eq!((animation.fps, animation.max_height), (60, 1080));
    assert_eq!((animation.gif_colors, animation.webp_quality), (64, 91));
}

#[test]
fn completion_sound_covers_every_external_stop_source() {
    for output in [
        OutputKind::Video(VideoContainer::Mkv),
        OutputKind::Audio(mediaa_core::formats::AudioFormat::Mp3),
        OutputKind::Anim(AnimFormat::Webp),
        OutputKind::Anim(AnimFormat::Gif),
    ] {
        assert!(recording_completion_sound(output, StopSource::Tray));
        assert!(recording_completion_sound(output, StopSource::Hotkey));
        assert!(!recording_completion_sound(output, StopSource::Ui));
    }
}

#[test]
fn completion_sound_requires_a_successful_completion_action() {
    assert!(completion_sound_after_action(true, true));
    assert!(!completion_sound_after_action(true, false));
    assert!(!completion_sound_after_action(false, true));
}

#[test]
fn tray_stays_in_processing_state_until_saving_finishes() {
    assert_eq!(
        tray_state_for_phase(RecordingPhase::Idle),
        mediaa_protocol::TrayStateWire::Idle
    );
    assert_eq!(
        tray_state_for_phase(RecordingPhase::Saving),
        mediaa_protocol::TrayStateWire::Saving
    );
}

#[test]
fn starting_recording_is_busy_and_has_its_own_tray_state() {
    assert_eq!(
        tray_state_for_phase(RecordingPhase::Starting),
        mediaa_protocol::TrayStateWire::Starting
    );
}

#[test]
fn recovery_notification_copies_file_links() {
    let links = recovery_links(&[
        std::path::PathBuf::from("first source.mkv"),
        std::path::PathBuf::from("second.mkv"),
    ]);
    let links: Vec<_> = links.lines().collect();
    assert_eq!(links.len(), 2);
    assert!(links.iter().all(|link| link.starts_with("file://")));
    assert!(links[0].contains("first%20source.mkv"));
}

fn test_app(ctx: &egui::Context) -> super::App {
    super::App::new_for_test(
        ctx,
        Page::Screenshot,
        Settings::default(),
        TestCatalog {
            monitors: Vec::new(),
            windows: Vec::new(),
            microphones: Vec::new(),
            default_microphone: None,
            monitor_capture: None,
            window_capture: None,
        },
    )
}

#[test]
fn notice_copy_action_does_not_create_another_notice() {
    let ctx = egui::Context::default();
    let _ui_guard = crate::test_support::ui_test_guard();
    let mut app = test_app(&ctx);
    app.notices.push(
        NoticeLevel::Success,
        "Copied".into(),
        "Detail".into(),
        None,
        Vec::new(),
        false,
    );
    let before = app.notices.items.len();
    app.handle_notice_action(&ctx, NoticeAction::CopyLog("notice-0001".into()));
    let deadline = Instant::now() + Duration::from_secs(1);
    while app.clipboard.is_busy() && Instant::now() < deadline {
        app.poll_clipboard(&ctx);
        std::thread::yield_now();
    }
    app.poll_clipboard(&ctx);
    assert_eq!(app.notices.items.len(), before);
}

#[test]
fn silent_desktop_error_does_not_create_a_notice() {
    let ctx = egui::Context::default();
    let _ui_guard = crate::test_support::ui_test_guard();
    let mut app = test_app(&ctx);
    app.notices.push(
        NoticeLevel::Error,
        "Source".into(),
        "Detail".into(),
        None,
        Vec::new(),
        false,
    );
    let before = app.notices.items.len();
    app.desktop_events_tx
        .send(super::state::DesktopEvent {
            action: super::state::DesktopAction::Reveal("missing.mkv".into()),
            result: Err("reveal failed".into()),
            notify_errors: false,
        })
        .unwrap();
    app.poll_desktop();
    assert_eq!(app.notices.items.len(), before);
}

#[test]
fn startup_action_from_args() {
    assert_eq!(parse(&[]), StartupAction::None);
    assert_eq!(parse(&["--area"]), StartupAction::Area);
    assert_eq!(parse(&["-f"]), StartupAction::Fullscreen);
    assert_eq!(
        parse(&["--unknown", "--fullscreen"]),
        StartupAction::Fullscreen
    );
    assert_eq!(parse(&["--tray"]), StartupAction::Tray);
    assert_eq!(parse(&["--show"]), StartupAction::Show);
}

#[test]
fn page_from_args_picks_known_pages() {
    let args = |a: &str| vec!["mediaa".to_string(), a.to_string()];
    assert_eq!(page_from_args(&args("--page=record")), Some(Page::Record));
    assert_eq!(page_from_args(&args("--page=nope")), None);
    assert_eq!(page_from_args(&args("--area")), None);
}
