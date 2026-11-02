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
            recording_view: Default::default(),
            settings,
            settings_dirty: None,
            page,
            page_scroll: Vec::new(),
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
            tray_available: true,
            tray_applied: None,
            daemon_events: None,
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
            presence: Default::default(),
            window: WindowLifecycle::new(false, std::time::Instant::now()),
            #[cfg(test)]
            recording: Default::default(),
            capture: Default::default(),
            overlay: None,
            clipboard: ClipboardService::noop(Default::default()),
            wake: Default::default(),
            region_outline: Default::default(),
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx, None),
            previews: Default::default(),
            hover_preview_rx: crossbeam_channel::unbounded().1,
            agent_commands_rx: crossbeam_channel::unbounded().1,
            agent_requests_rx: crossbeam_channel::unbounded().1,
            clipboard_rx: None,
            pending_agent_sends: Default::default(),
            preview_rx: crossbeam_channel::unbounded().1,
            conversion: Default::default(),
            agent_ui: Default::default(),
            recovery_running: false,
            housekeeping_jobs: Vec::new(),
            _instance: None,
            daemon_lost: Default::default(),
            daemon_control: None,
            sync: Default::default(),
            next_command: 1,
            last_notice_id: 0,
            picker_shown_sent: false,
            native_window: None,
            startup: StartupAction::None,
            caches: Default::default(),
            dev_input: None,
            fixed_catalogs: true,
            monitor_capture: catalog.monitor_capture,
            window_capture: catalog.window_capture,
            shot_monitor_state: Some(Arc::new(|_| screen::MonitorState::Available)),
            shot_window_state: Some(Arc::new(|_| screen::WindowState::Available)),
            completion_sound_enabled: false,
        }
    }
}
