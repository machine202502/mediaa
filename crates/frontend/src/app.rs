mod actions;
pub(crate) mod capture;
mod clipboard_events;
mod desktop;
mod dev_input;
mod notices;
mod recording;
mod recovery;
pub(crate) mod state;
mod window;

use crate::features;
pub(crate) use crate::features::command::{AppCommand, RecordingCommand};
use crate::features::hotkeys::HotkeyAction;
use crate::features::navigation::{Page, Pending};
use crate::features::notices::{NoticeLevel, Notices};
use crate::features::recording::{RecordTarget, StopSource};
use crate::features::region_editor::Outcome;
use crate::features::window_lifecycle::{WindowEffect, WindowEvent, WindowLifecycle};
use crate::infra::clipboard_worker::ClipboardService;
use crate::infra::ipc::Connection;
use crate::ui::pages::library::thumbs::Thumbnails;
use crate::ui::pages::PageUi;
use crate::ui::{pages, theme};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use mediaa_core::settings::{AfterCapture, RecordProfile, ScreenChoice, TrayClickAction};
use mediaa_core::{library, paths, Frame, ImageFormat, Settings};
use mediaa_media::recorder::{RecoveryReport, StopAction};
use mediaa_platform::files;
use mediaa_platform::screen::{self, MonitorInfo, WindowInfo};
use mediaa_platform::single_instance::SingleInstance;
use mediaa_platform::window::{NativeWindow, WindowOpen};
use mediaa_protocol::{TrayCommandWire as TrayCommand, TrayStateWire as TrayState};
use raw_window_handle::HasWindowHandle;
use rust_i18n::t;
use state::{CatalogEvent, CatalogJob, DesktopEvent};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const HIDE_DELAY: Duration = Duration::from_millis(300);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(600);
const TRAY_IDLE_EXIT: Duration = Duration::from_secs(3);

#[cfg(test)]
type TestMonitorCapture = Arc<dyn Fn(screen::MonitorId) -> Result<Frame, String> + Send + Sync>;
#[cfg(test)]
type TestWindowCapture =
    Arc<dyn Fn(screen::CaptureWindowId) -> Result<Frame, String> + Send + Sync>;
#[cfg(test)]
type TestShotMonitorState = Arc<dyn Fn(screen::MonitorId) -> screen::MonitorState + Send + Sync>;
#[cfg(test)]
type TestShotWindowState = Arc<dyn Fn(&WindowInfo) -> screen::WindowState + Send + Sync>;

pub(crate) enum DaemonEvent {
    Sync(mediaa_sync::Frame),
}

pub struct App {
    pub settings: Settings,
    settings_dirty: Option<Instant>,
    pub page: Page,
    pub(crate) page_scroll: Vec<(Page, f32)>,
    pub last_capture_page: Page,
    pub last_main_page: Page,
    pub last_agent_page: Page,
    notices: Notices,
    tray_available: bool,
    tray_applied: Option<(TrayState, bool)>,
    daemon_events: Option<Receiver<DaemonEvent>>,
    pub hotkey_problems: Vec<String>,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub mic_devices: Vec<String>,
    default_mic_device: Option<String>,
    catalog_job: Option<CatalogJob>,
    desktop_events: Receiver<DesktopEvent>,
    desktop_events_tx: Sender<DesktopEvent>,
    desktop_jobs: Vec<crate::features::jobs::Job<()>>,
    pending: Option<(Instant, Pending)>,
    presence: crate::features::window_presence::WindowPresence,
    window: WindowLifecycle,
    #[cfg(test)]
    #[cfg(test)]
    pub(crate) recording: features::recording::RecordingController,
    pub(crate) capture: features::capture::AppCapture,
    pub(crate) overlay: Option<crate::ui::region_editor::Overlay>,
    pub(crate) clipboard: ClipboardService,
    pub(crate) wake: mediaa_core::wake::Wake,
    region_outline: mediaa_platform::outline::RegionOutline,
    pub lib: features::library::LibraryController,
    pub thumbs: Thumbnails,
    preview_rx: Receiver<mediaa_protocol::backend::PreviewRequestWire>,
    hover_preview_rx: Receiver<mediaa_protocol::backend::PreviewRequestWire>,
    agent_commands_rx: Receiver<crate::ui::pages::agent::chat::AgentCommand>,
    agent_requests_rx: Receiver<mediaa_protocol::backend::AgentRequestWire>,
    clipboard_rx: Option<Receiver<mediaa_protocol::backend::ClipboardWire>>,
    pending_agent_sends: std::collections::HashSet<u64>,
    pub previews: crate::ui::pages::library::hover_preview::Previews,
    pub(crate) conversion: features::conversion::ConversionController,
    pub(crate) agent_ui: pages::agent::AgentUi,
    recovery_running: bool,
    housekeeping_jobs: Vec<crate::features::jobs::Job<()>>,
    _instance: Option<SingleInstance>,
    daemon_lost: Arc<AtomicBool>,
    daemon_control: Option<Connection>,
    sync: mediaa_sync::Client,
    next_command: u64,
    last_notice_id: u64,
    picker_shown_sent: bool,
    native_window: Option<NativeWindow>,
    startup: StartupAction,
    pub(crate) caches: crate::ui::caches::Caches,
    dev_input: Option<dev_input::DevInput>,
    recording_view: crate::features::recording::ViewState,
    #[cfg(test)]
    fixed_catalogs: bool,
    #[cfg(test)]
    monitor_capture: Option<TestMonitorCapture>,
    #[cfg(test)]
    window_capture: Option<TestWindowCapture>,
    #[cfg(test)]
    shot_monitor_state: Option<TestShotMonitorState>,
    #[cfg(test)]
    shot_window_state: Option<TestShotWindowState>,
    #[cfg(test)]
    completion_sound_enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupAction {
    #[default]
    None,
    Area,
    Fullscreen,
    Tray,
    Show,
}

impl StartupAction {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        args.into_iter()
            .find_map(|a| match a.as_str() {
                "--area" | "-a" => Some(StartupAction::Area),
                "--fullscreen" | "-f" => Some(StartupAction::Fullscreen),
                "--tray" => Some(StartupAction::Tray),
                "--show" => Some(StartupAction::Show),
                _ => None,
            })
            .unwrap_or_default()
    }
}

impl App {
    pub(crate) fn new(
        cc: &eframe::CreationContext<'_>,
        startup: StartupAction,
        page: Option<Page>,
        instance: Option<SingleInstance>,
        settings: Option<Settings>,
        daemon: Option<crate::session::UiSession>,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        let daemon_lost = Arc::new(AtomicBool::new(false));
        let (daemon_control, daemon_events, tray_available, sync) = match daemon {
            Some(session) => {
                let lost = daemon_lost.clone();
                let ctx = ctx.clone();
                let (tx, rx) = crossbeam_channel::unbounded();
                let mut reader = session.reader;
                let buffered = session.buffered;
                let sync = session.sync;
                std::thread::spawn(move || {
                    for frame in buffered {
                        if tx.send(DaemonEvent::Sync(frame)).is_err() {
                            return;
                        }
                    }
                    while let Ok(frame) = reader.recv_frame() {
                        if tx.send(DaemonEvent::Sync(frame)).is_err() {
                            return;
                        }
                        ctx.request_repaint();
                    }
                    lost.store(true, Ordering::Release);
                    ctx.request_repaint();
                });
                (Some(session.control), Some(rx), session.tray, sync)
            }
            None => (None, None, false, mediaa_sync::Client::new()),
        };

        theme::install(ctx);
        let settings = settings.unwrap_or_else(Settings::load);
        mediaa_media::preview_cache::set_animated_previews_enabled(settings.animated_previews);
        mediaa_media::preview_cache::set_full_previews_enabled(settings.full_preview_videos);
        let wake = mediaa_core::wake::Wake::new(std::sync::Arc::new({
            let ctx = ctx.clone();
            move |delay: Option<Duration>| match delay {
                Some(delay) => ctx.request_repaint_after(delay),
                None => ctx.request_repaint(),
            }
        }));
        let mut clipboard = ClipboardService::new(wake.clone());
        let clipboard_rx = clipboard.take_commands();
        let mut output_dirs = vec![settings.media_dir.clone(), settings.convert_dir()];
        output_dirs.sort();
        output_dirs.dedup();
        theme::apply_preference(ctx, settings.theme);
        crate::i18n::apply(settings.language.as_deref());
        let (desktop_events_tx, desktop_events) = crossbeam_channel::bounded(64);

        let hotkey_problems = Vec::new();

        let notifications_enabled = settings.notifications_enabled;
        let (preview_tx, preview_rx) =
            crossbeam_channel::unbounded::<mediaa_protocol::backend::PreviewRequestWire>();
        let (hover_preview_tx, hover_preview_rx) =
            crossbeam_channel::unbounded::<mediaa_protocol::backend::PreviewRequestWire>();
        crate::features::preview::install(preview_tx.clone());
        let (agent_commands_tx, agent_commands_rx) =
            crossbeam_channel::unbounded::<crate::ui::pages::agent::chat::AgentCommand>();
        crate::ui::pages::agent::chat::install_agent_commands(agent_commands_tx);
        let (agent_requests_tx, agent_requests_rx) =
            crossbeam_channel::unbounded::<mediaa_protocol::backend::AgentRequestWire>();
        crate::features::agent::requests::install(agent_requests_tx);
        let start_hidden = startup == StartupAction::Tray
            || (startup != StartupAction::Show && settings.start_in_tray);
        let mut app = Self {
            settings,
            settings_dirty: None,
            page: Page::Screenshot,
            page_scroll: Vec::new(),
            last_capture_page: Page::Screenshot,
            last_main_page: Page::Screenshot,
            last_agent_page: Page::Catalogs,
            notices: Notices::with_enabled(notifications_enabled),
            tray_available,
            tray_applied: None,
            daemon_events,
            hotkey_problems,
            monitors: Vec::new(),
            windows: Vec::new(),
            mic_devices: Vec::new(),
            default_mic_device: None,
            catalog_job: None,
            desktop_events,
            desktop_events_tx,
            desktop_jobs: Vec::new(),
            pending: None,
            presence: Default::default(),
            window: WindowLifecycle::new(start_hidden, Instant::now()),
            #[cfg(test)]
            #[cfg(test)]
            recording: Default::default(),
            capture: Default::default(),
            overlay: None,
            clipboard,
            wake,
            region_outline: Default::default(),
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx, Some(preview_tx)),
            preview_rx,
            previews: crate::ui::pages::library::hover_preview::Previews::new(Some(
                hover_preview_tx,
            )),
            hover_preview_rx,
            agent_commands_rx,
            agent_requests_rx,
            clipboard_rx,
            pending_agent_sends: Default::default(),
            conversion: Default::default(),
            agent_ui: pages::agent::AgentUi::default(),
            picker_shown_sent: false,
            recovery_running: false,
            housekeeping_jobs: Vec::new(),
            _instance: instance,
            daemon_lost,
            daemon_control,
            sync,
            next_command: 1,
            last_notice_id: 0,
            native_window: cc.window_handle().ok().and_then(|h| {
                match NativeWindow::from_raw(h.as_raw()) {
                    WindowOpen::Supported(window) => Some(window),
                    WindowOpen::Unsupported(reason) => {
                        log::info!("native window: {reason}");
                        None
                    }
                }
            }),
            startup: StartupAction::None,
            caches: Default::default(),
            dev_input: dev_input::configured().map(dev_input::DevInput::new),
            recording_view: Default::default(),
            #[cfg(test)]
            fixed_catalogs: false,
            #[cfg(test)]
            monitor_capture: None,
            #[cfg(test)]
            window_capture: None,
            #[cfg(test)]
            shot_monitor_state: None,
            #[cfg(test)]
            shot_window_state: None,
            #[cfg(test)]
            completion_sound_enabled: true,
        };
        let picker_mode = matches!(startup, StartupAction::Area | StartupAction::Fullscreen);
        if picker_mode {
            app.monitors = screen::monitors().unwrap_or_default();
            app.window = WindowLifecycle::new(true, Instant::now());
        } else {
            app.refresh_catalogs(ctx);
            app.lib.refresh(&app.settings);
        }
        for p in app.hotkey_problems.clone() {
            app.warning(p);
        }
        let wants_tray = startup != StartupAction::Show
            && (startup == StartupAction::Tray || app.settings.start_in_tray);
        app.startup = if startup == StartupAction::None && wants_tray {
            StartupAction::Tray
        } else {
            startup
        };
        if wants_tray && !app.tray_available {
            app.show_window(ctx);
        }
        if crate::dev_mode::mode().background {
            app.window = WindowLifecycle::new(true, Instant::now());
            if let Ok(handle) = cc.window_handle() {
                if let raw_window_handle::RawWindowHandle::Win32(win) = handle.as_raw() {
                    let _ = crate::infra::native_window::prepare_handle(win.hwnd.get());
                }
            }
            if crate::dev_mode::mode().input.is_some() {
                let _ = crate::infra::native_window::background_show(
                    mediaa_core::paths::APP_IDENTITY.window_title,
                );
            }
        }
        if let Some(p) = page {
            app.page = p;
            if let Some(pages::Section::Capture) = p.section() {
                app.last_capture_page = p;
            }
            if let Some(pages::Section::Agent) = p.section() {
                app.last_agent_page = p;
            }
        }
        if !picker_mode {
            match crate::features::jobs::Job::spawn("startup-cleanup", Arc::default(), move || {
                for dir in output_dirs {
                    paths::remove_stale_temporary_files(&dir, Duration::from_secs(3600));
                }
            }) {
                Ok(job) => app.housekeeping_jobs.push(job),
                Err(error) => log::warn!("startup cleanup: {error}"),
            }
        }
        if !picker_mode {
            match crate::features::jobs::Job::spawn("font-warmup", Arc::default(), || {
                let _ = mediaa_render::annotate::fonts().len();
            }) {
                Ok(job) => app.housekeeping_jobs.push(job),
                Err(error) => log::warn!("font warmup: {error}"),
            }
        }
        if startup == StartupAction::Show {
            app.show_window(ctx);
        }
        app
    }

    pub fn settings_changed(&mut self) {
        self.settings_dirty = Some(Instant::now());
    }

    #[cfg(test)]
    pub(crate) fn set_hidden_for_test(&mut self, hidden: bool) {
        self.window = WindowLifecycle::new(hidden, Instant::now());
    }

    pub fn preview_options_changed(&mut self) {
        let animated = self.settings.animated_previews;
        let full = self.settings.full_preview_videos;
        mediaa_media::preview_cache::set_animated_previews_enabled(animated);
        mediaa_media::preview_cache::set_full_previews_enabled(full);
        self.previews.stop();
        if !animated || !full {
            let removed = mediaa_media::preview_cache::clear_previews(
                &self.settings.media_dir,
                !animated,
                !full,
            );
            log::info!(
                "preview cache cleared animated={} full={} files={removed}",
                !animated,
                !full
            );
        }
        self.settings_changed();
    }

    pub fn refresh_monitors(&mut self) {
        match screen::monitors() {
            Ok(m) => self.monitors = m,
            Err(e) => self.error(t!("err.list_displays", error = format!("{e:#}"))),
        }
    }

    pub fn refresh_mics(&mut self) {
        self.mic_devices = mediaa_platform::audio::input_devices();
        self.default_mic_device = mediaa_platform::audio::default_input_device();
    }

    pub(crate) fn refresh_catalogs(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        if self.fixed_catalogs {
            return;
        }
        if self.catalog_job.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(3);
        let ctx = ctx.clone();
        match crate::features::jobs::Job::spawn("catalog-load", Arc::default(), move || {
            let monitors = screen::monitors().map_err(|error| format!("{error:#}"));
            let _ = tx.send(CatalogEvent::Monitors(monitors));
            ctx.request_repaint();
            let windows = screen::windows().map_err(|error| format!("{error:#}"));
            let _ = tx.send(CatalogEvent::Windows(windows));
            ctx.request_repaint();
            let _ = tx.send(CatalogEvent::Microphones {
                devices: mediaa_platform::audio::input_devices(),
                default: mediaa_platform::audio::default_input_device(),
            });
            ctx.request_repaint();
        }) {
            Ok(job) => {
                self.catalog_job = Some(CatalogJob {
                    rx,
                    job,
                    started: Instant::now(),
                })
            }
            Err(error) => log::warn!("catalog load thread: {error}"),
        }
    }

    fn poll_catalogs(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.catalog_job.as_ref().map(|catalog| catalog.rx.clone()) else {
            return;
        };
        let mut disconnected = false;
        loop {
            match rx.try_recv() {
                Ok(CatalogEvent::Monitors(Ok(monitors))) => self.monitors = monitors,
                Ok(CatalogEvent::Monitors(Err(error))) => {
                    self.error(t!("err.list_displays", error = error))
                }
                Ok(CatalogEvent::Windows(Ok(windows))) => self.windows = windows,
                Ok(CatalogEvent::Windows(Err(error))) => {
                    self.error(t!("err.list_windows", error = error))
                }
                Ok(CatalogEvent::Microphones { devices, default }) => {
                    self.mic_devices = devices;
                    self.default_mic_device = default;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => {
                    if self
                        .catalog_job
                        .as_ref()
                        .is_some_and(|job| job.started.elapsed() >= CATALOG_TIMEOUT)
                    {
                        if let Some(job) = self.catalog_job.take() {
                            job.job.cancel();
                        }
                        self.error(t!("err.catalog_timeout"));
                        break;
                    }
                    ctx.request_repaint_after(Duration::from_millis(100));
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        if disconnected {
            if let Some(mut job) = self.catalog_job.take() {
                if job.job.poll().is_some_and(|result| result.is_err()) {
                    log::warn!("catalog worker stopped");
                }
            }
        }
    }

    pub fn recovery_active(&self) -> bool {
        self.recovery_running
    }

    pub fn reapply_hotkeys(&mut self) {
        self.send_settings();
    }

    pub fn suspend_hotkeys(&mut self) {
        self.send_sync_command(
            mediaa_protocol::HOTKEYS_PAUSED,
            serde_json::to_vec(&true).expect("bool"),
        );
        let _ = mediaa_platform::keys::print_screen_down();
    }

    fn send_settings(&mut self) {
        let id = self.next_command;
        self.next_command += 1;
        let frame = self.sync.command_frame(
            id,
            mediaa_protocol::SET_SETTINGS,
            mediaa_protocol::encode_settings(&self.settings),
        );
        self.send_sync_frame(frame);
    }
}

impl App {
    fn page_ctx(&mut self) -> crate::ui::pages::ctx::Ctx<'_> {
        let viewer_open = self.lib.viewer_open();
        let recovery_active = self.recovery_running;
        #[cfg(test)]
        self.recording_view.refresh(&self.recording);
        crate::ui::pages::ctx::Ctx {
            page: self.page,
            page_scroll: std::mem::take(&mut self.page_scroll),
            last_capture_page: self.last_capture_page,
            last_main_page: self.last_main_page,
            last_agent_page: self.last_agent_page,
            settings: &mut self.settings,
            lib: &mut self.lib,
            recording: &mut self.recording_view,
            capture: &mut self.capture,
            conversion: &mut self.conversion,
            agent_ui: &mut self.agent_ui,
            thumbs: &mut self.thumbs,
            previews: &mut self.previews,
            caches: &mut self.caches,
            notices: &mut self.notices,
            clipboard: &mut self.clipboard,
            wake: &self.wake,
            monitors: &self.monitors,
            windows: &self.windows,
            mic_devices: &self.mic_devices,
            hotkey_problems: &self.hotkey_problems,
            recovery_active,
            viewer_open,
            intents: Vec::new(),
        }
    }

    pub(crate) fn frame_ui(&mut self, ui: &mut egui::Ui) {
        let egui_ctx = ui.ctx().clone();
        let (action, intents) = {
            let mut frame = self.page_ctx();
            let action = crate::ui::pages::frame(&mut frame, ui);
            let intents = std::mem::take(&mut frame.intents);
            let page = frame.page;
            let scroll = std::mem::take(&mut frame.page_scroll);
            let capture_page = frame.last_capture_page;
            let main_page = frame.last_main_page;
            let agent_page = frame.last_agent_page;
            drop(frame);
            self.page = page;
            self.page_scroll = scroll;
            self.last_capture_page = capture_page;
            self.last_main_page = main_page;
            self.last_agent_page = agent_page;
            (action, intents)
        };
        if self.recording_view.take_cancel_requested() {
            #[cfg(test)]
            self.recording.cancel_finalize();
            #[cfg(not(test))]
            self.send_recording_command(crate::features::command::RecordingCommand::CancelFinalize);
        }
        if let Some(action) = action {
            self.handle_notice_action(&egui_ctx, action);
        }
        self.handle_intents(&egui_ctx, intents);
        #[cfg(not(test))]
        if let Some(request) = self.lib.take_removal() {
            self.send_library_removal(request);
        }
    }

    #[cfg(test)]
    pub(crate) fn apply_library_action(
        &mut self,
        ctx: &egui::Context,
        action: crate::features::library::LibraryAction,
    ) {
        let intents = {
            let mut frame = self.page_ctx();
            crate::ui::pages::library::apply_action(&mut frame, ctx, action);
            let intents = std::mem::take(&mut frame.intents);
            let page = frame.page;
            let scroll = std::mem::take(&mut frame.page_scroll);
            drop(frame);
            self.page = page;
            self.page_scroll = scroll;
            intents
        };
        self.handle_intents(ctx, intents);
    }

    pub(crate) fn poll_library_cleanup(&mut self, ctx: &egui::Context) {
        let intents = {
            let mut frame = self.page_ctx();
            crate::ui::pages::library::poll_cleanup(&mut frame);
            let intents = std::mem::take(&mut frame.intents);
            let page = frame.page;
            let scroll = std::mem::take(&mut frame.page_scroll);
            drop(frame);
            self.page = page;
            self.page_scroll = scroll;
            intents
        };
        self.handle_intents(ctx, intents);
        #[cfg(not(test))]
        if let Some(request) = self.lib.take_removal() {
            self.send_library_removal(request);
        }
    }
}
impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if let Some(input) = self.dev_input.as_mut() {
            input.append(raw_input);
            ctx.request_repaint_after(Duration::from_millis(16));
        }
    }

    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::dev_mode::keep_awake(ctx);
        if crate::dev_mode::mode().background
            && self.dev_input.is_none()
            && ctx.cumulative_frame_nr() < 3
        {
            let _ = crate::infra::native_window::hide_window(
                mediaa_core::paths::APP_IDENTITY.window_title,
            );
            let previous = crate::dev_mode::remembered_foreground();
            if previous != 0 {
                let _ = crate::infra::native_window::restore_foreground(previous);
            }
        }
        if self.daemon_lost.load(Ordering::Acquire) {
            log::info!("daemon is gone; closing the ui");
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.run_startup_action(ctx);
        self.handle_daemon_events(ctx);
        self.poll_backend_previews();
        self.poll_agent_commands();
        self.poll_agent_requests();
        self.poll_clipboard_commands();
        if let Some((at, _)) = &self.pending {
            if Instant::now() >= *at {
                let (_, p) = self.pending.take().expect("checked above");
                self.run_pending(ctx, p);
            } else {
                ctx.request_repaint_after(Duration::from_millis(30));
            }
        }
        self.poll_recording_start(ctx);
        self.poll_recorder(ctx);
        self.poll_screenshot(ctx);
        self.poll_capture(ctx);
        self.poll_copy_image(ctx);
        self.poll_clipboard(ctx);
        self.poll_catalogs(ctx);
        self.poll_desktop();
        #[cfg(test)]
        self.poll_conversion();

        if self
            ._instance
            .as_ref()
            .is_some_and(SingleInstance::take_activation)
        {
            self.show_window(ctx);
        }
        self.housekeeping_jobs.retain_mut(|job| {
            if let Some(result) = job.poll() {
                if let Err(error) = result {
                    log::warn!("background housekeeping stopped: {error:#}");
                }
                false
            } else {
                true
            }
        });
        self.poll_library_cleanup(ctx);
        if let Some((dir, names)) = self.lib.poll_background(&self.settings, &self.wake) {
            self.thumbs.retain(&dir, names);
        }
        if let Some(entries) = self.lib.prewarm() {
            self.thumbs.prewarm(&entries);
        }
        self.update_tray();
        self.show_recording_region(ctx);

        let busy = self.has_active_work();
        if ctx.input(|i| i.viewport().close_requested()) {
            log::info!(
                "window close requested: close_to_tray={} busy={} hidden={}",
                self.settings.close_to_tray,
                busy,
                self.window.is_hidden()
            );
            self.window_event(
                ctx,
                WindowEvent::CloseRequested {
                    busy,
                    close_to_tray: self.settings.close_to_tray,
                },
            );
        }
        self.window_event(
            ctx,
            WindowEvent::Tick {
                busy,
                idle_after: TRAY_IDLE_EXIT,
            },
        );
        if self.window.is_hidden() && !busy {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        if let Some(t) = self.settings_dirty {
            if t.elapsed() > SETTINGS_SAVE_DELAY {
                self.settings_dirty = None;
                self.send_settings();
            } else {
                ctx.request_repaint_after(SETTINGS_SAVE_DELAY + Duration::from_millis(50));
            }
        }
        // Agent execution lives in the backend. The UI only reads the shared
        // queue view; it must never start a second host.
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.thumbs.poll(&ctx);
        self.prewarm_region_overlay(&ctx);
        self.handle_overlay(&ctx);
        let picker_visible = self
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.visible());
        if picker_visible && !self.picker_shown_sent {
            self.picker_shown_sent = true;
            self.send_sync_command(mediaa_protocol::PICKER_SHOWN, Vec::new());
        } else if !picker_visible {
            self.picker_shown_sent = false;
        }
        self.frame_ui(ui);
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.settings.highlight_recording_region
            && self.recording_view.is_recording()
            && self.recording_view.region().is_some()
        {
            [0.0; 4]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    fn on_exit(&mut self) {
        log::info!(
            "ui exiting: close_to_tray={} daemon control={} quit_daemon={} hidden={} startup={:?}",
            self.settings.close_to_tray,
            self.daemon_control.is_some(),
            self.window.quit_daemon(),
            self.window.is_hidden(),
            self.startup
        );
        self.send_settings();
        if daemon_must_quit(
            self.settings.close_to_tray,
            self.tray_available,
            self.window.quit_daemon(),
        ) {
            self.send_sync_command(mediaa_protocol::QUIT, b"ui closed".to_vec());
        }
        self.thumbs.flush();
        self.lib.flush();
    }
}

fn daemon_must_quit(close_to_tray: bool, tray_available: bool, quit_requested: bool) -> bool {
    quit_requested || !close_to_tray || !tray_available
}

fn recovery_links(files: &[PathBuf]) -> String {
    files
        .iter()
        .map(|path| library::file_url(path))
        .collect::<Vec<_>>()
        .join("\n")
}

fn tray_click_stops_recording(enabled: bool, recording: bool) -> bool {
    enabled && recording
}

#[cfg(test)]
fn recording_completion_sound(_output: mediaa_core::OutputKind, source: StopSource) -> bool {
    matches!(source, StopSource::Tray | StopSource::Hotkey)
}

fn completion_sound_after_action(requested: bool, action_succeeded: bool) -> bool {
    requested && action_succeeded
}

fn recording_tray_state(recording: &features::recording::ViewState) -> TrayState {
    tray_state_for_phase(recording.phase())
}

fn tray_state_for_phase(phase: features::recording::RecordingPhase) -> TrayState {
    match phase {
        features::recording::RecordingPhase::Idle => TrayState::Idle,
        features::recording::RecordingPhase::Starting => TrayState::Starting,
        features::recording::RecordingPhase::Recording { paused: true } => TrayState::Paused,
        features::recording::RecordingPhase::Recording { paused: false } => TrayState::Recording,
        features::recording::RecordingPhase::Saving => TrayState::Saving,
    }
}

pub fn page_from_args(args: &[String]) -> Option<Page> {
    args.iter().find_map(|a| match a.strip_prefix("--page=")? {
        "screenshot" => Some(Page::Screenshot),
        "record" => Some(Page::Record),
        "audio" => Some(Page::RecordAudio),
        "library" => Some(Page::Library),
        "convert" => Some(Page::Convert),
        "catalogs" => Some(Page::Catalogs),
        "providers" => Some(Page::Providers),
        "models" => Some(Page::Models),
        "queue" => Some(Page::Queue),
        "budget" => Some(Page::Budget),
        "settings" => Some(Page::Settings),
        "about" => Some(Page::About),
        _ => None,
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod ui_tests;
