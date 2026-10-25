use super::*;

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
            overlay: None,
            pending: None,
            hidden_by_us: false,
            ghosted: false,
            rec: Default::default(),
            recorder: RecState::Idle,
            screenshot_job: None,
            capture_job: None,
            copy_image_job: None,
            clipboard: ClipboardService::new(ctx.clone()),
            recording_region: None,
            region_outline: Default::default(),
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx),
            conv: Default::default(),
            quit_when_idle: false,
            tasks: TaskSupervisor::default(),
            recovery_rx: None,
            recovery_task: None,
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
