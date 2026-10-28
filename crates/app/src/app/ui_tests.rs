use super::test_support::TestCatalog;
use super::{App, Page, Pending};
use crate::tray::TrayCommand;
use eframe::egui;
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::Harness;
use mediaa_core::formats::AudioFormat;
use mediaa_core::settings::{AfterCapture, MicChoice, ScreenChoice, ThemePreference};
use mediaa_core::source::{FrameSource, SourceFactory};
use mediaa_core::{AnimFormat, Frame, ImageFormat, OutputKind, Settings, VideoContainer};
use mediaa_media::convert::{StillFormat, Target};
use mediaa_media::recorder::{RecordingRequest, RecordingSession, SessionEvent, SessionNotice};
use mediaa_platform::screen::{MonitorId, MonitorInfo, WindowId, WindowInfo};
use rust_i18n::t;
use std::path::PathBuf;

static RECORDING_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn recording_guard() -> std::sync::MutexGuard<'static, ()> {
    RECORDING_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct UiState {
    page: Page,
    settings: Settings,
    catalog: TestCatalog,
    app: Option<App>,
    _ui_guard: std::sync::MutexGuard<'static, ()>,
}

impl UiState {
    fn new(
        page: Page,
        settings: Settings,
        catalog: TestCatalog,
        ui_guard: std::sync::MutexGuard<'static, ()>,
    ) -> Self {
        Self {
            page,
            settings,
            catalog,
            app: None,
            _ui_guard: ui_guard,
        }
    }

    fn app(&mut self, ctx: &egui::Context) -> &mut App {
        self.app.get_or_insert_with(|| {
            App::new_for_test(
                ctx,
                self.page,
                self.settings.clone(),
                TestCatalog {
                    monitors: self.catalog.monitors.clone(),
                    windows: self.catalog.windows.clone(),
                    microphones: self.catalog.microphones.clone(),
                    default_microphone: self.catalog.default_microphone.clone(),
                    monitor_capture: self.catalog.monitor_capture.clone(),
                    window_capture: self.catalog.window_capture.clone(),
                },
            )
        })
    }
}

fn primary_monitor() -> MonitorInfo {
    MonitorInfo {
        id: MonitorId::new(1),
        device: r"\\.\DISPLAY1".into(),
        index: 0,
        name: "Primary".into(),
        x: 0,
        y: 0,
        width: 1920,
        height: 1080,
        scale: 1.0,
        primary: true,
    }
}

fn secondary_monitor() -> MonitorInfo {
    MonitorInfo {
        id: MonitorId::new(2),
        device: r"\\.\DISPLAY2".into(),
        index: 1,
        name: "Secondary".into(),
        x: 1920,
        y: 0,
        width: 2560,
        height: 1440,
        scale: 1.0,
        primary: false,
    }
}

fn window() -> WindowInfo {
    WindowInfo {
        id: WindowId::new(7),
        title: "Quarterly planning — a deliberately long document title".into(),
        app: "Browser".into(),
        pid: 42,
        width: 1440,
        height: 900,
    }
}

fn second_window() -> WindowInfo {
    WindowInfo {
        id: WindowId::new(8),
        title: "Chat".into(),
        app: "Messenger".into(),
        pid: 43,
        width: 900,
        height: 700,
    }
}

fn catalog() -> TestCatalog {
    TestCatalog {
        monitors: vec![primary_monitor(), secondary_monitor()],
        windows: vec![window(), second_window()],
        microphones: vec!["Headset".into(), "Studio USB Microphone".into()],
        default_microphone: Some("Headset".into()),
        monitor_capture: None,
        window_capture: None,
    }
}

struct TestFrameSource {
    frame: Frame,
}

impl FrameSource for TestFrameSource {
    fn name(&self) -> String {
        "UI test source".into()
    }

    fn size(&self) -> (u32, u32) {
        (self.frame.width, self.frame.height)
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        Ok(self.frame.clone())
    }
}

fn test_recording(label: &str, dir: &std::path::Path) -> RecordingSession {
    let frame = Frame::filled(160, 120, [40, 90, 180, 255]);
    let source: SourceFactory = Box::new(move || {
        Ok(Box::new(TestFrameSource {
            frame: frame.clone(),
        }))
    });
    let request = RecordingRequest::builder(
        OutputKind::Video(VideoContainer::Mp4),
        vec![source],
        Vec::new(),
        mediaa_core::paths::MediaDir::new(dir.join("out")),
        mediaa_core::paths::WorkDir::new(dir.join("work")),
        label.into(),
    )
    .video(10, 80, None)
    .build()
    .unwrap();
    RecordingSession::start(request).unwrap()
}

fn finish_recording(app: &mut App, ctx: &egui::Context) {
    for _ in 0..500 {
        app.poll_recorder(ctx);
        if app.recording.is_idle() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("recording did not reach a terminal state");
}

fn capture_catalog() -> TestCatalog {
    let mut catalog = catalog();
    catalog.monitor_capture = Some(std::sync::Arc::new(|_| {
        Ok(Frame::filled(96, 64, [24, 80, 160, 255]))
    }));
    catalog.window_capture = Some(std::sync::Arc::new(|_| {
        Ok(Frame::filled(80, 50, [160, 80, 24, 255]))
    }));
    catalog
}

fn settings() -> Settings {
    Settings {
        language: Some("en".into()),
        theme: ThemePreference::Dark,
        media_dir: PathBuf::from(r"C:\Users\Test\Documents\MediaA"),
        ..Settings::default()
    }
}

fn harness(page: Page) -> Harness<'static, UiState> {
    harness_with(page, settings(), catalog(), egui::vec2(1180.0, 900.0))
}

fn agent_settings() -> Settings {
    let mut settings = settings();
    settings.media_dir = std::env::temp_dir().join("mediaa-agent-ui-tests");
    settings
}

fn harness_with(
    page: Page,
    settings: Settings,
    catalog: TestCatalog,
    size: egui::Vec2,
) -> Harness<'static, UiState> {
    harness_view(page, settings, catalog, size, 1.0, egui::Theme::Dark)
}

fn harness_view(
    page: Page,
    settings: Settings,
    catalog: TestCatalog,
    size: egui::Vec2,
    pixels_per_point: f32,
    theme: egui::Theme,
) -> Harness<'static, UiState> {
    let locale = settings.language.as_deref().unwrap_or("en");
    let ui_guard = super::test_support::ui_test_guard();
    rust_i18n::set_locale(locale);
    Harness::builder()
        .with_size(size)
        .with_pixels_per_point(pixels_per_point)
        .with_os(egui::os::OperatingSystem::Windows)
        .with_theme(theme)
        .build_ui_state(
            |ui, state: &mut UiState| {
                let app = state.app(ui.ctx());
                crate::pages::shell(app, ui);
            },
            UiState::new(page, settings, catalog, ui_guard),
        )
}

fn run_pending(app: &mut App, ctx: &egui::Context) {
    let (_, pending) = app.pending.take().expect("a pending capture");
    app.run_pending(ctx, pending);
}

fn finish_capture_jobs(app: &mut App, ctx: &egui::Context) {
    for _ in 0..200 {
        if app.capture.capture_busy() {
            app.poll_capture(ctx);
        }
        if app.capture.screenshot_busy() {
            app.poll_screenshot(ctx);
        }
        if !app.capture.capture_busy() && !app.capture.screenshot_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("mocked capture did not finish");
}

#[test]
fn record_page_shows_and_selects_mocked_displays() {
    let mut harness = harness(Page::Record);
    let primary = crate::pages::record::monitor_label(&primary_monitor());
    let secondary = crate::pages::record::monitor_label(&secondary_monitor());

    assert!(harness.query_by_label(&primary).is_some());
    assert!(harness.query_by_label(&secondary).is_some());
    assert!(!harness
        .get_by_label_contains(&t!("record.start"))
        .accesskit_node()
        .is_disabled());

    harness.get_by_label(&secondary).click();
    harness.run();

    let selected = &harness.state().app.as_ref().unwrap().recording.ui.monitors;
    assert!(selected.contains(&MonitorId::new(1)));
    assert!(selected.contains(&MonitorId::new(2)));
    assert!(matches!(
        harness
            .state()
            .app
            .as_ref()
            .unwrap()
            .record_target_from_ui()
            .unwrap(),
        super::RecordTarget::Screens(ref monitors) if monitors.len() == 2
    ));
}

#[test]
fn record_page_switches_to_mocked_windows_and_selects_one() {
    let mut harness = harness(Page::Record);
    harness.get_by_label(&t!("shot.window")).click();
    harness.run();

    let window_label = format!(
        "{}  ({}×{})",
        window().label(),
        window().width,
        window().height
    );
    assert!(harness.query_by_label(&window_label).is_some());
    harness.get_by_label(&window_label).click();
    harness.run();

    assert!(harness
        .state()
        .app
        .as_ref()
        .unwrap()
        .recording
        .ui
        .windows
        .contains(&WindowId::new(7)));
    assert!(!harness
        .get_by_label_contains(&t!("record.start"))
        .accesskit_node()
        .is_disabled());
}

#[test]
fn record_page_lists_and_selects_mocked_microphones() {
    let mut harness = harness(Page::Record);
    harness.get_by_value(&t!("record.mic_auto")).click();
    harness.run();

    assert!(harness.query_by_label("Headset").is_some());
    assert!(harness.query_by_label("Studio USB Microphone").is_some());
    harness.get_by_label("Studio USB Microphone").click();
    harness.run();

    assert_eq!(
        harness
            .state()
            .app
            .as_ref()
            .unwrap()
            .settings
            .record
            .microphone,
        MicChoice::Device("Studio USB Microphone".into())
    );
}

#[test]
fn audio_page_uses_mocked_microphones_and_disables_start_without_sources() {
    let mut harness = harness(Page::RecordAudio);
    harness.get_by_value(&t!("record.mic_auto")).click();
    harness.run();
    harness.get_by_label("Studio USB Microphone").click();
    harness.run();
    assert_eq!(
        harness
            .state()
            .app
            .as_ref()
            .unwrap()
            .settings
            .audio_record
            .microphone,
        MicChoice::Device("Studio USB Microphone".into())
    );
    drop(harness);

    let mut disabled_settings = settings();
    disabled_settings.audio_record.microphone = MicChoice::Off;
    disabled_settings.audio_record.system_audio = false;
    let mut harness = harness_with(
        Page::RecordAudio,
        disabled_settings,
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    assert!(harness
        .get_by_label_contains(&t!("audio.start"))
        .accesskit_node()
        .is_disabled());
    harness.get_by_label_contains(&t!("audio.start")).hover();
    harness.run();
    assert!(harness.query_by_label(&t!("audio.no_source")).is_some());
}

#[test]
fn audio_format_buttons_update_the_audio_output() {
    let mut harness = harness(Page::RecordAudio);
    for output in [
        AudioFormat::Mp3,
        AudioFormat::M4a,
        AudioFormat::Wav,
        AudioFormat::Flac,
        AudioFormat::Ogg,
    ] {
        harness.get_by_label(output.label()).click();
        harness.run();
        assert_eq!(
            harness
                .state()
                .app
                .as_ref()
                .unwrap()
                .settings
                .audio_record
                .format,
            output
        );
    }
}

#[test]
fn saved_but_missing_microphone_is_visible_to_the_user() {
    let mut selected = settings();
    selected.record.microphone = MicChoice::Device("Removed microphone".into());
    let harness = harness_with(Page::Record, selected, catalog(), egui::vec2(1180.0, 900.0));
    let label = t!("record.mic_missing", device = "Removed microphone").to_string();
    assert!(harness.query_by_value(&label).is_some());
}

#[test]
fn settings_page_uses_mocked_displays_and_isolates_microphone_profiles() {
    let mut harness = harness(Page::Settings);
    harness.get_by_value(&t!("settings.screen_primary")).click();
    harness.run();
    harness
        .get_by_label(&crate::pages::record::monitor_label(&secondary_monitor()))
        .click();
    harness.run();
    assert_eq!(
        harness.state().app.as_ref().unwrap().settings.hotkey_screen,
        ScreenChoice::Display(r"\\.\DISPLAY2".into())
    );

    harness
        .get_all_by_value(&t!("record.mic_auto"))
        .next()
        .expect("at least one microphone profile")
        .click_accesskit();
    harness.run();
    harness
        .get_by_label("Studio USB Microphone")
        .click_accesskit();
    harness.run();
    let settings = &harness.state().app.as_ref().unwrap().settings;
    assert_eq!(
        settings.hotkey_record.microphone,
        MicChoice::Device("Studio USB Microphone".into())
    );
    assert_eq!(settings.hotkey_audio.microphone, MicChoice::Auto);
}

#[test]
fn record_layout_stacks_profile_sections_only_when_narrow() {
    let wide = harness_with(
        Page::Record,
        settings(),
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    let format = wide.get_by_label(&t!("record.format")).rect();
    let audio = wide.get_by_label(&t!("record.audio")).rect();
    assert!(audio.left() > format.left());
    drop(wide);

    let narrow = harness_with(
        Page::Record,
        settings(),
        catalog(),
        egui::vec2(720.0, 1000.0),
    );
    let format = narrow.get_by_label(&t!("record.format")).rect();
    let audio = narrow.get_by_label(&t!("record.audio")).rect();
    assert!(audio.top() > format.bottom() + 20.0);
    let start = narrow.get_by_label_contains(&t!("record.start")).rect();
    let screen = narrow.ctx.content_rect();
    assert!(start.bottom() <= screen.bottom());
    assert!(start.left() >= 0.0 && start.right() <= screen.right());
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_record_screen() {
    let mut harness = harness(Page::Record);
    harness.snapshot("record-screen-wide");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_record_narrow() {
    let mut harness = harness_with(
        Page::Record,
        settings(),
        catalog(),
        egui::vec2(720.0, 1000.0),
    );
    harness.snapshot("record-screen-narrow");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_audio_sources() {
    let mut harness = harness(Page::RecordAudio);
    harness.snapshot("audio-sources");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_record_minimum_width() {
    let mut record = harness_with(
        Page::Record,
        settings(),
        catalog(),
        egui::vec2(520.0, 900.0),
    );
    record.snapshot("record-screen-minimum");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_audio_minimum_width() {
    let mut audio = harness_with(
        Page::RecordAudio,
        settings(),
        catalog(),
        egui::vec2(520.0, 900.0),
    );
    audio.snapshot("audio-sources-minimum");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_capture_page() {
    let mut screenshot = harness(Page::Screenshot);
    screenshot.snapshot("capture-page-wide");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_library_page() {
    let mut library = harness(Page::Library);
    library.snapshot("library-empty-wide");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_library_populated() {
    let dir = tempfile::tempdir().unwrap();
    for (name, color) in [
        (
            "Screenshot_with_a_deliberately_long_title_2026-01-02_10-00-00.png",
            [40, 90, 180, 255],
        ),
        ("Design_review_2026-01-01_10-00-00.png", [180, 70, 50, 255]),
        (
            "Recording_thumbnail_2026-01-01_09-00-00.png",
            [40, 150, 90, 255],
        ),
    ] {
        image::RgbaImage::from_pixel(320, 200, image::Rgba(color))
            .save(dir.path().join(name))
            .unwrap();
    }
    let mut selected = settings();
    selected.media_dir = dir.path().to_path_buf();
    let mut library = harness_with(
        Page::Library,
        selected,
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    library
        .state_mut()
        .app
        .as_mut()
        .unwrap()
        .lib
        .set_test_entries(dir.path());
    library.run_steps(4);
    library.snapshot("library-populated-wide");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_convert_page() {
    let mut convert = harness(Page::Convert);
    convert.snapshot("convert-empty-wide");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_convert_selected() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("Picture_2026-01-01_10-00-00.png");
    image::RgbaImage::from_pixel(320, 200, image::Rgba([20, 40, 80, 255]))
        .save(&input)
        .unwrap();
    let mut convert = harness(Page::Convert);
    convert
        .state_mut()
        .app
        .as_mut()
        .unwrap()
        .conversion
        .set_input(input);
    for _ in 0..100 {
        convert.run_steps(1);
        if convert
            .query_by_label(Target::Still(StillFormat::Jpeg).label())
            .is_some()
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    convert.snapshot("convert-selected-wide");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_settings_page() {
    let mut settings = harness(Page::Settings);
    settings.snapshot("settings-wide");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_capture_minimum() {
    let mut harness = harness_with(
        Page::Screenshot,
        settings(),
        catalog(),
        egui::vec2(520.0, 480.0),
    );
    harness.run_steps(2);
    harness.snapshot("capture-page-minimum");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_library_minimum() {
    let mut harness = harness_with(
        Page::Library,
        settings(),
        catalog(),
        egui::vec2(520.0, 480.0),
    );
    harness.run_steps(2);
    harness.snapshot("library-page-minimum");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_convert_minimum() {
    let mut harness = harness_with(
        Page::Convert,
        settings(),
        catalog(),
        egui::vec2(520.0, 480.0),
    );
    harness.run_steps(2);
    harness.snapshot("convert-page-minimum");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_settings_minimum() {
    let mut harness = harness_with(
        Page::Settings,
        settings(),
        catalog(),
        egui::vec2(520.0, 480.0),
    );
    harness.run_steps(2);
    harness.snapshot("settings-page-minimum");
}

#[test]
fn fullscreen_screenshot_uses_capture_button_format_and_saves() {
    let dir = tempfile::tempdir().unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().to_path_buf();
    selected.screenshot.after_capture = AfterCapture::Save;
    selected.screenshot.hide_window = false;
    let mut harness = harness_with(
        Page::Screenshot,
        selected,
        capture_catalog(),
        egui::vec2(1180.0, 900.0),
    );
    harness.get_by_label(&t!("shot.full")).click();
    harness.run();
    assert!(matches!(
        harness.state().app.as_ref().unwrap().pending,
        Some((_, Pending::FullscreenScreenshot { .. }))
    ));
    harness.get_by_label(ImageFormat::Webp.label()).click();
    harness.run();
    assert_eq!(
        harness
            .state()
            .app
            .as_ref()
            .unwrap()
            .settings
            .screenshot
            .format,
        ImageFormat::Webp
    );

    let ctx = harness.ctx.clone();
    let app = harness.state_mut().app.as_mut().unwrap();
    run_pending(app, &ctx);
    finish_capture_jobs(app, &ctx);

    let outputs = std::fs::read_dir(dir.path())
        .unwrap()
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "webp"))
        .count();
    assert_eq!(outputs, 1);
    assert!(app.notices.items.last().is_some());
}

#[test]
fn window_screenshot_uses_selected_window() {
    let dir = tempfile::tempdir().unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().to_path_buf();
    selected.screenshot.after_capture = AfterCapture::Save;
    selected.screenshot.hide_window = false;
    let mut harness = harness_with(
        Page::Screenshot,
        selected,
        capture_catalog(),
        egui::vec2(1180.0, 900.0),
    );
    harness
        .get_all_by_label(&t!("shot.capture_window"))
        .next()
        .expect("a mocked window")
        .click();
    harness.run();
    assert!(matches!(
        harness.state().app.as_ref().unwrap().pending,
        Some((_, Pending::WindowScreenshot(id))) if id.get() == 7
    ));

    let ctx = harness.ctx.clone();
    let app = harness.state_mut().app.as_mut().unwrap();
    run_pending(app, &ctx);
    finish_capture_jobs(app, &ctx);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().flatten().count(), 1);
}

#[test]
fn region_screenshot_opens_the_editor_with_mocked_frame() {
    let mut selected = settings();
    selected.screenshot.hide_window = false;
    let mut harness = harness_with(
        Page::Screenshot,
        selected,
        capture_catalog(),
        egui::vec2(1180.0, 900.0),
    );
    harness.get_by_label(&t!("shot.area")).click();
    harness.run();
    assert!(matches!(
        harness.state().app.as_ref().unwrap().pending,
        Some((_, Pending::RegionScreenshot { .. }))
    ));

    let ctx = harness.ctx.clone();
    let app = harness.state_mut().app.as_mut().unwrap();
    run_pending(app, &ctx);
    for _ in 0..200 {
        app.poll_capture(&ctx);
        if app.capture.overlay.is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(app.capture.overlay.is_some());
    assert!(!app.capture.screenshot_busy());
}

#[test]
fn several_mocked_windows_can_be_selected_for_recording() {
    let mut harness = harness(Page::Record);
    harness.get_by_label(&t!("shot.window")).click();
    harness.run();
    let first = format!(
        "{}  ({}×{})",
        window().label(),
        window().width,
        window().height
    );
    let second = format!(
        "{}  ({}×{})",
        second_window().label(),
        second_window().width,
        second_window().height
    );
    harness.get_by_label(&first).click();
    harness.run();
    harness.get_by_label(&second).click();
    harness.run();

    let app = harness.state().app.as_ref().unwrap();
    assert_eq!(app.recording.ui.windows.len(), 2);
    let target = app.record_target_from_ui().unwrap();
    assert!(matches!(target, super::RecordTarget::Windows(ref windows) if windows.len() == 2));
}

#[test]
fn recording_format_buttons_update_the_output_kind() {
    let mut harness = harness(Page::Record);
    for output in [
        OutputKind::Video(VideoContainer::Mp4),
        OutputKind::Video(VideoContainer::Mkv),
        OutputKind::Anim(AnimFormat::Gif),
        OutputKind::Anim(AnimFormat::Webp),
    ] {
        harness.get_by_label(output.label()).click();
        harness.run();
        assert_eq!(
            harness.state().app.as_ref().unwrap().settings.record.output,
            output
        );
    }
    assert!(harness
        .query_by_label(&t!("record.segmented_webp"))
        .is_some());
    assert!(harness
        .query_by_label(&t!("record.parallel_webp"))
        .is_some());
}

#[test]
fn stop_from_window_saves_a_synthetic_recording() {
    let _recording = recording_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().join("out");
    let mut harness = harness_with(Page::Record, selected, catalog(), egui::vec2(1180.0, 900.0));
    let session = test_recording("Window stop", dir.path());
    let events = session.events();
    let app = harness.state_mut().app.as_mut().unwrap();
    app.recording.test_recording(session, events);
    std::thread::sleep(std::time::Duration::from_millis(120));
    harness.run_steps(2);
    harness.get_by_label(&t!("record.stop_save")).click();
    harness.run_steps(2);
    assert!(matches!(
        harness.state().app.as_ref().unwrap().recording.view(),
        crate::features::recording::RecordingView::Saving { .. }
    ));
    let ctx = harness.ctx.clone();
    finish_recording(harness.state_mut().app.as_mut().unwrap(), &ctx);
    assert_eq!(
        std::fs::read_dir(dir.path().join("out")).unwrap().count(),
        1
    );
}

#[test]
fn stop_from_tray_uses_the_same_save_flow() {
    let _recording = recording_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().join("out");
    let mut harness = harness_with(Page::Record, selected, catalog(), egui::vec2(1180.0, 900.0));
    let session = test_recording("Tray stop", dir.path());
    let events = session.events();
    let (tray_tx, tray_rx) = crossbeam_channel::bounded(1);
    let ctx = harness.ctx.clone();
    let app = harness.state_mut().app.as_mut().unwrap();
    app.tray_rx = Some(tray_rx);
    app.recording.test_recording(session, events);
    std::thread::sleep(std::time::Duration::from_millis(120));
    tray_tx.send(TrayCommand::StopAndSave).unwrap();
    app.handle_tray(&ctx);
    assert!(matches!(app.recording.saving_sound(), Some(true)));
    finish_recording(app, &ctx);
    assert_eq!(
        std::fs::read_dir(dir.path().join("out")).unwrap().count(),
        1
    );
}

#[test]
fn active_recording_supports_pause_resume_and_save() {
    let _recording = recording_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().join("out");
    let mut harness = harness_with(Page::Record, selected, catalog(), egui::vec2(1180.0, 900.0));
    let session = test_recording("Pause resume", dir.path());
    let events = session.events();
    let app = harness.state_mut().app.as_mut().unwrap();
    app.recording.test_recording(session, events);
    std::thread::sleep(std::time::Duration::from_millis(120));
    harness.run_steps(1);
    harness
        .get_all_by_label_contains(&t!("record.pause"))
        .last()
        .expect("pause button")
        .click();
    harness.run_steps(3);
    assert!(harness.state().app.as_ref().unwrap().recording.is_paused());
    harness
        .get_all_by_label_contains(&t!("record.resume"))
        .last()
        .expect("resume button")
        .click();
    harness.run_steps(3);
    assert!(!harness.state().app.as_ref().unwrap().recording.is_paused());
    harness.get_by_label(&t!("record.stop_save")).click();
    harness.run_steps(1);
    let ctx = harness.ctx.clone();
    finish_recording(harness.state_mut().app.as_mut().unwrap(), &ctx);
    assert_eq!(
        std::fs::read_dir(dir.path().join("out")).unwrap().count(),
        1
    );
}

#[test]
fn active_recording_can_be_cancelled_without_publishing_output() {
    let _recording = recording_guard();
    let dir = tempfile::tempdir().unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().join("out");
    let mut harness = harness_with(Page::Record, selected, catalog(), egui::vec2(1180.0, 900.0));
    let session = test_recording("Cancel recording", dir.path());
    let events = session.events();
    let app = harness.state_mut().app.as_mut().unwrap();
    app.recording.test_recording(session, events);
    std::thread::sleep(std::time::Duration::from_millis(120));
    harness.run_steps(1);
    harness.get_by_label(&t!("record.cancel")).click();
    harness.run_steps(1);
    let ctx = harness.ctx.clone();
    finish_recording(harness.state_mut().app.as_mut().unwrap(), &ctx);
    assert_eq!(
        std::fs::read_dir(dir.path().join("out")).unwrap().count(),
        0
    );
}

#[test]
fn webp_fallback_notice_exposes_trace_and_reason() {
    let mut harness = harness(Page::Record);
    let (events_tx, events) = crossbeam_channel::bounded(2);
    let ctx = harness.ctx.clone();
    let app = harness.state_mut().app.as_mut().unwrap();
    app.recording.test_saving(events);
    events_tx
        .send(SessionEvent::Notice(SessionNotice::WebpFallback {
            trace_id: "webp-ui-failure".into(),
            detail: "segmented encoder failed: memory allocation error".into(),
        }))
        .unwrap();
    app.poll_recorder(&ctx);
    let notice = app.notices.items.last().expect("fallback notice");
    assert_eq!(notice.summary, t!("toast.webp_fallback"));
    assert_eq!(notice.log_ref, "webp-ui-failure");
    assert!(notice.detail.contains("memory allocation error"));
    assert!(notice.detail.contains("trace=webp-ui-failure"));
}

#[test]
fn settings_controls_change_application_state() {
    let mut harness = harness(Page::Settings);
    harness
        .get_by_label(&crate::i18n::theme(ThemePreference::Light))
        .click();
    harness.run();
    harness.get_by_label(&t!("settings.close_to_tray")).click();
    harness.run();
    harness
        .get_by_label(&t!("settings.tray_click_open_window"))
        .click();
    harness.run();
    harness
        .get_by_label(&t!("settings.after_completion_copy"))
        .click();
    harness.run();

    let settings = &harness.state().app.as_ref().unwrap().settings;
    assert_eq!(settings.theme, ThemePreference::Light);
    assert!(!settings.close_to_tray);
    assert_eq!(
        settings.tray_left_click,
        mediaa_core::settings::TrayClickAction::OpenWindow
    );
    assert_eq!(
        settings.after_completion,
        mediaa_core::settings::AfterCompletionAction::CopyFile
    );
}

#[test]
fn advanced_settings_toggles_update_application_state() {
    let mut harness = harness(Page::Settings);
    harness.run_steps(2);
    for label in [
        t!("settings.start_in_tray"),
        t!("settings.stop_recording_on_left_click"),
        t!("settings.highlight_recording_region"),
    ] {
        harness.get_by_label(&label).click();
        harness.run_steps(1);
    }

    let settings = &harness.state().app.as_ref().unwrap().settings;
    assert!(settings.start_in_tray);
    assert!(!settings.stop_recording_on_left_click);
    assert!(!settings.highlight_recording_region);
}

#[test]
fn library_filter_and_delete_work_through_the_ui() {
    let dir = tempfile::tempdir().unwrap();
    let image = dir.path().join("Image_2026-01-02_10-00-00.png");
    let video = dir.path().join("Video_2026-01-01_10-00-00.mp4");
    std::fs::write(&image, b"image").unwrap();
    std::fs::write(&video, b"video").unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().to_path_buf();
    let mut harness = harness_with(
        Page::Library,
        selected,
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    let app = harness.state_mut().app.as_mut().unwrap();
    app.lib.set_test_entries(dir.path());
    app.lib.set_test_trash();
    harness.run_steps(2);
    assert!(harness.query_by_label("Image").is_some());
    assert!(harness.query_by_label("Video").is_some());

    harness.get_by_label(&t!("lib.images")).click();
    harness.run_steps(2);
    assert!(harness.query_by_label("Image").is_some());
    assert!(harness.query_by_label("Video").is_none());

    harness.get_by_label(&t!("lib.more")).click();
    harness.run_steps(1);
    harness.get_by_label(&t!("lib.delete")).click_accesskit();
    harness.run_steps(2);
    let confirm = format!(
        "{}  {}",
        egui_phosphor::regular::TRASH,
        t!("lib.delete_confirm")
    );
    harness.get_by_label(&confirm).click_accesskit();
    harness.run_steps(2);

    let ctx = harness.ctx.clone();
    let app = harness.state_mut().app.as_mut().unwrap();
    for _ in 0..200 {
        crate::features::library::poll_cleanup(app, &ctx);
        if !app.lib.cleanup_active() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(!app.lib.cleanup_active());
    assert!(!image.exists());
    assert!(video.exists());
}

#[test]
fn convert_page_starts_and_finishes_a_selected_target() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("Picture_2026-01-01_10-00-00.png");
    image::RgbaImage::from_pixel(32, 24, image::Rgba([20, 40, 80, 255]))
        .save(&input)
        .unwrap();
    let mut selected = settings();
    selected.media_dir = dir.path().to_path_buf();
    let mut harness = harness_with(
        Page::Convert,
        selected,
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    harness
        .state_mut()
        .app
        .as_mut()
        .unwrap()
        .conversion
        .set_input(input);

    let jpeg = Target::Still(StillFormat::Jpeg).label();
    for _ in 0..200 {
        harness.run_steps(1);
        if harness.query_by_label(jpeg).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    harness.get_by_label(jpeg).click();
    harness.run_steps(1);
    harness
        .get_all_by_label_contains(&t!("convert.start"))
        .last()
        .expect("convert start button")
        .click();
    harness.run_steps(1);
    assert!(harness.state().app.as_ref().unwrap().conversion.is_busy());

    let ctx = harness.ctx.clone();
    let app = harness.state_mut().app.as_mut().unwrap();
    for _ in 0..500 {
        app.poll_conversion(&ctx);
        if !app.conversion.is_busy() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(!app.conversion.is_busy());
    assert_eq!(
        std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jpg"))
            .count(),
        1
    );
}

#[test]
fn key_pages_stay_inside_supported_window_widths() {
    let pages = [
        (Page::Screenshot, t!("shot.title").into_owned()),
        (Page::Record, t!("record.title").into_owned()),
        (Page::RecordAudio, t!("audio.title").into_owned()),
        (Page::Library, t!("lib.title").into_owned()),
        (Page::Convert, t!("convert.title").into_owned()),
        (Page::Settings, t!("settings.title").into_owned()),
    ];
    for width in [520.0, 720.0, 1180.0, 1600.0] {
        for (page, title) in &pages {
            let mut harness = harness_with(*page, settings(), catalog(), egui::vec2(width, 900.0));
            harness.run_steps(2);
            assert!(
                harness.query_by_label(title).is_some(),
                "{page:?} at width {width}: missing title"
            );
            let interactive: Vec<_> = harness
                .query_all_by(|node| {
                    !node.is_hidden()
                        && matches!(
                            node.role(),
                            egui::accesskit::Role::Button
                                | egui::accesskit::Role::CheckBox
                                | egui::accesskit::Role::ComboBox
                                | egui::accesskit::Role::Slider
                                | egui::accesskit::Role::TextInput
                        )
                })
                .collect();
            for node in interactive {
                let rect = node.rect();
                assert!(
                    rect.left() >= -1.0 && rect.right() <= width + 1.0,
                    "{page:?} at width {width}: {:?} {:?} outside viewport: {rect:?}",
                    node.accesskit_node().role(),
                    node.accesskit_node().label().or_else(|| node.value())
                );
            }
            if matches!(page, Page::Record | Page::RecordAudio) {
                let label = if *page == Page::Record {
                    t!("record.start")
                } else {
                    t!("audio.start")
                };
                let start = harness.get_by_label_contains(&label).rect();
                assert!(start.bottom() <= harness.ctx.content_rect().bottom());
            }
        }
    }
}

#[test]
fn the_agent_section_opens_catalogs() {
    let mut harness = harness_with(
        Page::Screenshot,
        agent_settings(),
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    harness.run_steps(2);
    harness.get_by_label_contains(&t!("section.agent")).click();
    harness.run_steps(2);
    assert!(harness
        .query_all_by(|node| node
            .label()
            .is_some_and(|label| label.contains(&*t!("nav.catalogs"))))
        .next()
        .is_some());
    assert!(harness
        .query_by_label_contains(&t!("agent.catalogs_empty"))
        .is_some());
}

#[test]
fn agent_pages_are_listed_in_the_sub_navigation() {
    let mut harness = harness_with(
        Page::Catalogs,
        agent_settings(),
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    harness.run_steps(2);
    for label in [
        t!("nav.catalogs"),
        t!("nav.queue"),
        t!("nav.budget"),
        t!("nav.providers"),
        t!("nav.models"),
    ] {
        assert!(
            harness
                .query_all_by(|node| node.label().is_some_and(|text| text.contains(&*label)))
                .next()
                .is_some(),
            "missing sub-navigation entry {label}"
        );
    }
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_agent_catalogs() {
    let mut harness = harness_with(
        Page::Catalogs,
        agent_settings(),
        catalog(),
        egui::vec2(1180.0, 900.0),
    );
    harness.snapshot("agent-catalogs-wide");
}

fn page_title(page: Page) -> String {
    match page {
        Page::Screenshot => t!("shot.title").into_owned(),
        Page::Record => t!("record.title").into_owned(),
        Page::RecordAudio => t!("audio.title").into_owned(),
        Page::Library => t!("lib.title").into_owned(),
        Page::Convert => t!("convert.title").into_owned(),
        Page::Catalogs => t!("nav.catalogs").into_owned(),
        Page::Providers => t!("agent.providers_title").into_owned(),
        Page::Models => t!("agent.models_title").into_owned(),
        Page::Queue => t!("queue.title").into_owned(),
        Page::Budget => t!("budget.title").into_owned(),
        Page::Settings => t!("settings.title").into_owned(),
        Page::About => t!("settings.about").into_owned(),
    }
}

fn assert_pages_fit(
    settings: Settings,
    width: f32,
    height: f32,
    pixels_per_point: f32,
    theme: egui::Theme,
) {
    for page in [
        Page::Screenshot,
        Page::Record,
        Page::RecordAudio,
        Page::Library,
        Page::Convert,
        Page::Settings,
    ] {
        let mut harness = harness_view(
            page,
            settings.clone(),
            catalog(),
            egui::vec2(width, height),
            pixels_per_point,
            theme,
        );
        harness.run_steps(2);
        assert!(
            harness.query_by_label(&page_title(page)).is_some(),
            "{page:?} at {width}x{height}, {pixels_per_point}x: missing title"
        );
        let interactive: Vec<_> = harness
            .query_all_by(|node| {
                !node.is_hidden()
                    && matches!(
                        node.role(),
                        egui::accesskit::Role::Button
                            | egui::accesskit::Role::CheckBox
                            | egui::accesskit::Role::ComboBox
                            | egui::accesskit::Role::Slider
                            | egui::accesskit::Role::TextInput
                    )
            })
            .collect();
        for node in interactive {
            let rect = node.rect();
            assert!(
                rect.left() >= -1.0 && rect.right() <= width + 1.0,
                "{page:?} at {width}x{height}, {pixels_per_point}x: {:?} {:?} outside viewport: {rect:?}",
                node.accesskit_node().role(),
                node.accesskit_node().label().or_else(|| node.value())
            );
        }
        if matches!(page, Page::Record | Page::RecordAudio) {
            let label = if page == Page::Record {
                t!("record.start")
            } else {
                t!("audio.start")
            };
            let start = harness.get_by_label_contains(&label).rect();
            assert!(start.bottom() <= harness.ctx.content_rect().bottom());
        }
    }
}

#[test]
fn pages_fit_supported_windows_dpi_scales() {
    for pixels_per_point in [1.0, 1.25, 1.5, 2.0] {
        assert_pages_fit(
            settings(),
            520.0,
            480.0,
            pixels_per_point,
            egui::Theme::Dark,
        );
    }
}

#[test]
fn all_pages_fit_russian_light_theme() {
    let mut russian = settings();
    russian.language = Some("ru".into());
    russian.theme = ThemePreference::Light;
    for width in [520.0, 720.0, 1180.0] {
        assert_pages_fit(russian.clone(), width, 760.0, 1.25, egui::Theme::Light);
    }
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_record_russian_light() {
    let mut russian = settings();
    russian.language = Some("ru".into());
    russian.theme = ThemePreference::Light;
    let mut harness = harness_view(
        Page::Record,
        russian,
        catalog(),
        egui::vec2(720.0, 900.0),
        1.25,
        egui::Theme::Light,
    );
    harness.snapshot("record-russian-light");
}

#[cfg(windows)]
#[test]
fn ui_regression_snapshot_settings_russian_light() {
    let mut russian = settings();
    russian.language = Some("ru".into());
    russian.theme = ThemePreference::Light;
    let mut harness = harness_view(
        Page::Settings,
        russian,
        catalog(),
        egui::vec2(1180.0, 900.0),
        1.25,
        egui::Theme::Light,
    );
    harness.snapshot("settings-russian-light");
}
