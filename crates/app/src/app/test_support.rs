use super::*;

static UI_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub(crate) fn ui_test_guard() -> std::sync::MutexGuard<'static, ()> {
    UI_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[derive(Clone)]
pub(crate) struct TestCatalog {
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub microphones: Vec<String>,
    pub default_microphone: Option<String>,
    pub monitor_capture: Option<super::TestMonitorCapture>,
    pub window_capture: Option<super::TestWindowCapture>,
}

impl App {
    pub(crate) fn new_for_test(
        ctx: &egui::Context,
        page: Page,
        settings: Settings,
        catalog: TestCatalog,
    ) -> Self {
        theme::install(ctx);
        theme::apply_preference(ctx, settings.theme);
        crate::i18n::apply(settings.language.as_deref());
        let (desktop_events_tx, desktop_events) = crossbeam_channel::bounded(64);
        Self {
            settings,
            settings_dirty: None,
            settings_writer: SettingsWriter::discard(),
            page,
            last_capture_page: if page.section() == Some(pages::Section::Capture) {
                page
            } else {
                Page::Screenshot
            },
            last_main_page: if matches!(
                page.section(),
                Some(pages::Section::Library | pages::Section::Convert | pages::Section::Capture)
            ) {
                page
            } else {
                Page::Screenshot
            },
            last_agent_page: if page.section() == Some(pages::Section::Agent) {
                page
            } else {
                Page::Catalogs
            },
            notices: Notices::default(),
            tray: None,
            tray_rx: None,
            hotkeys: None,
            hotkey_rx: None,
            hotkey_problems: Vec::new(),
            monitors: catalog.monitors,
            windows: catalog.windows,
            mic_devices: catalog.microphones,
            default_mic_device: catalog.default_microphone,
            catalog_job: None,
            desktop_events,
            desktop_events_tx,
            desktop_jobs: Vec::new(),
            pending: None,
            hidden_by_us: false,
            recording_quiet: false,
            ghosted: false,
            recording: Default::default(),
            capture: Default::default(),
            clipboard: ClipboardService::noop(ctx.clone()),
            region_outline: Default::default(),
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx),
            conversion: Default::default(),
            agent: features::agent::AgentController::new(
                std::sync::Arc::new(mediaa_engine::testing::FakeEngine::default()),
                mediaa_core::slots::Slots::new(),
            ),
            quit_when_idle: false,
            recovery_job: None,
            housekeeping_jobs: Vec::new(),
            last_cache_sweep: std::time::Instant::now(),
            _instance: None,
            native_window: None,
            startup: StartupAction::None,
            logo: None,
            fixed_catalogs: true,
            monitor_capture: catalog.monitor_capture,
            window_capture: catalog.window_capture,
            completion_sound_enabled: false,
        }
    }
}
