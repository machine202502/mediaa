use super::notices::NoticeAction;
use super::{
    close_request, completion_sound_after_action, notice_summary, page_from_args,
    recording_animation_settings, recording_completion_sound, recording_tray_state, recovery_links,
    tray_click_stops_recording, CloseRequest, NoticeLevel, Notices, Page, RecState, StartupAction,
    StopSource,
};
use crate::app::tasks::{TaskKind, TaskSupervisor};
use eframe::egui;
use egui_kittest::kittest::Queryable;
use mediaa_core::{AnimFormat, OutputKind, Settings, VideoContainer};
use mediaa_media::recorder::RecordingProgress;

fn parse(args: &[&str]) -> StartupAction {
    StartupAction::from_args(args.iter().map(|s| s.to_string()))
}

#[test]
fn closing_the_window_hides_to_the_tray_unless_turned_off() {
    assert_eq!(close_request(true, true, false), CloseRequest::Hide);
    assert_eq!(close_request(false, true, false), CloseRequest::Allow);
    assert_eq!(close_request(true, false, false), CloseRequest::Allow);
    assert_eq!(close_request(false, true, true), CloseRequest::KeepVisible);
    assert_eq!(close_request(false, false, true), CloseRequest::KeepVisible);
    assert_eq!(close_request(true, false, true), CloseRequest::KeepVisible);
}

#[test]
fn tray_click_stops_recording_only_when_enabled_and_recording() {
    assert!(tray_click_stops_recording(true, true));
    assert!(!tray_click_stops_recording(false, true));
    assert!(!tray_click_stops_recording(true, false));
}

#[test]
fn animation_recording_uses_the_visible_shared_settings() {
    let animation = Settings::default().region_anim;
    let webp = OutputKind::Anim(AnimFormat::Webp);
    let animation = recording_animation_settings(webp, 60, Some(1080), &animation);
    assert_eq!((animation.fps, animation.max_height), (60, 1080));
    let mkv = OutputKind::Video(VideoContainer::Mkv);
    let animation = Settings::default().region_anim;
    assert_eq!(
        recording_animation_settings(mkv, 60, Some(1080), &animation),
        animation
    );
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
        recording_tray_state(&RecState::Idle),
        crate::tray::TrayState::Idle
    );
    let (_tx, events) = crossbeam_channel::unbounded();
    let saving = RecState::Saving {
        events,
        progress_state: RecordingProgress::default(),
        progress: None,
        started: std::time::Instant::now(),
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        sound_on_completion: false,
        task: None,
    };
    assert_eq!(
        recording_tray_state(&saving),
        crate::tray::TrayState::Saving
    );
}

#[test]
fn starting_recording_is_busy_and_has_its_own_tray_state() {
    let (_tx, rx) = crossbeam_channel::bounded(1);
    let mut tasks = TaskSupervisor::default();
    let task = tasks
        .spawn(TaskKind::RecordingStart, "starting-test", None, || {})
        .unwrap();
    let starting = RecState::Starting {
        rx,
        task,
        started: std::time::Instant::now(),
        page: Page::Record,
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    assert!(!starting.is_idle());
    assert_eq!(
        recording_tray_state(&starting),
        crate::tray::TrayState::Starting
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

#[test]
fn toast_text_stays_on_one_short_line() {
    let detail = format!("Ошибка:\n{}", "C:\\very\\long\\path\\".repeat(20));
    let summary = notice_summary(&detail, "Ошибка");
    assert_eq!(summary, "Ошибка");
    assert!(!summary.ends_with('…'));
    assert_eq!(
            notice_summary(
                "Не удалось сохранить запись: очень длинное техническое описание ошибки с путём и цепочкой повторов",
                "Ошибка"
            ),
            "Не удалось сохранить запись:"
        );
    assert_eq!(notice_summary("Коротко", "Ошибка"), "Коротко");

    let mut notices = Notices::default();
    notices.push(
        NoticeLevel::Error,
        summary,
        detail.clone(),
        None,
        Vec::new(),
        true,
    );
    notices.push(
        NoticeLevel::Warning,
        "Предупреждение".into(),
        "Полный текст".into(),
        Some("webp-test".into()),
        Vec::new(),
        false,
    );
    assert_eq!(notices.items[0].id, 1);
    assert_eq!(notices.items[1].id, 2);
    assert_eq!(notices.items[0].detail, detail);
    assert_eq!(notices.items[1].log_ref, "webp-test");
}

#[test]
fn persistent_notice_is_not_evicted_by_new_transient_toasts() {
    let mut notices = Notices::default();
    notices.push(
        NoticeLevel::Warning,
        "Recovery".into(),
        "Files".into(),
        None,
        Vec::new(),
        true,
    );
    for index in 0..8 {
        notices.push(
            NoticeLevel::Info,
            format!("Message {index}"),
            String::new(),
            None,
            Vec::new(),
            false,
        );
    }
    assert_eq!(notices.items.len(), 5);
    assert!(notices
        .items
        .iter()
        .any(|notice| notice.summary == "Recovery"));
}

#[test]
fn notice_expands_to_the_full_context_and_keeps_its_id() {
    let mut notices = Notices::default();
    let summary = "Скриншот 3200×2000 скопирован и сохранён: Recording screenshot.png";
    notices.push(
        NoticeLevel::Success,
        summary.into(),
        "Полный технический контекст без сокращений".into(),
        Some("webp-1234-0001".into()),
        vec![
            "C:\\Videos\\Recording source part 1.mkv".into(),
            "C:\\Videos\\Recording source part 2.mkv".into(),
        ],
        true,
    );
    let observed = std::sync::Arc::new(std::sync::Mutex::new(None));
    let sink = observed.clone();
    let mut harness = egui_kittest::Harness::new_ui_state(
        move |ui: &mut egui::Ui, notices: &mut Notices| {
            if let Some(action) = notices.show(ui.ctx()) {
                *sink.lock().unwrap() = Some(action);
            }
        },
        notices,
    );
    crate::theme::install(&harness.ctx);
    harness.run();
    let log_label = format!("{} webp-1234-0001", rust_i18n::t!("notice.log_id"));
    let copy_label = rust_i18n::t!("notice.copy").to_string();
    assert!(harness.query_by_label(summary).is_some());
    assert!(harness.query_by_label(&log_label).is_none());
    assert!(harness
        .query_by_label("Полный технический контекст без сокращений")
        .is_none());
    if let Ok(path) = std::env::var("MEDIAA_NOTICE_SNAPSHOT") {
        harness
            .render()
            .unwrap()
            .save(format!("{path}-collapsed.png"))
            .unwrap();
        let mut expanded = harness.state().clone();
        expanded.items[0].expanded = true;
        let mut preview = egui_kittest::Harness::new_ui_state(
            |ui: &mut egui::Ui, notices: &mut Notices| {
                notices.show(ui.ctx());
            },
            expanded,
        );
        crate::theme::install(&preview.ctx);
        preview.run();
        preview
            .render()
            .unwrap()
            .save(format!("{path}-expanded.png"))
            .unwrap();
    }
    harness.get_by_label("▾").click();
    harness.run();
    assert!(harness
        .query_by_label("Полный технический контекст без сокращений")
        .is_some());
    assert!(harness.query_by_label(&log_label).is_some());
    harness.get_by_label(&copy_label).click();
    harness.run();
    assert_eq!(
        observed.lock().unwrap().clone(),
        Some(NoticeAction::CopyLog("webp-1234-0001".into()))
    );
    harness.get_by_label("▴").click();
    harness.run();
    assert!(harness
        .query_by_label("Полный технический контекст без сокращений")
        .is_none());
    assert_eq!(harness.state().items[0].id, 1);
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
}

#[test]
fn page_from_args_picks_known_pages() {
    let args = |a: &str| vec!["mediaa".to_string(), a.to_string()];
    assert_eq!(page_from_args(&args("--page=record")), Some(Page::Record));
    assert_eq!(page_from_args(&args("--page=nope")), None);
    assert_eq!(page_from_args(&args("--area")), None);
}
