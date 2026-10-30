mod actions;
pub(crate) mod capture;
mod clipboard_events;
pub(crate) mod command;
mod desktop;
mod notices;
mod recording;
mod recovery;
pub(crate) mod state;
mod window;

use crate::features;
use crate::features::library::thumbs::Thumbnails;
use crate::features::recording::{RecordTarget, StopSource};
use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::overlay::{self, Outcome};
use crate::services::clipboard::ClipboardService;
use crate::services::settings_writer::SettingsWriter;
use crate::tray::{Tray, TrayCommand, TrayState};
use crate::{pages, theme};
use command::{AppCommand, RecordingCommand};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use mediaa_core::settings::{AfterCapture, RecordProfile, ScreenChoice, TrayClickAction};
use mediaa_core::source::{AudioSource, SourceFactory};
use mediaa_core::{library, naming, paths, Frame, ImageFormat, OutputKind, Settings};
use mediaa_media::recorder::{
    recover_incomplete_sessions, AudioLayout, RecordingRequest, RecoveryReport, SessionEvent,
    SessionFailure, SessionNotice, StopAction,
};
use mediaa_platform::audio::{CpalSource, CpalTarget};
use mediaa_platform::files;
use mediaa_platform::screen::{self, MonitorInfo, WindowInfo};
use mediaa_platform::single_instance::SingleInstance;
use mediaa_platform::window::NativeWindow;
use mediaa_storage::screenshot;
use notices::{notice_summary, NoticeLevel, Notices};
use raw_window_handle::HasWindowHandle;
use rust_i18n::t;
use state::{CatalogEvent, CatalogJob, DesktopEvent, Page, Pending};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HIDE_DELAY: Duration = Duration::from_millis(300);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(600);
const SETTINGS_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
const CACHE_SWEEP_EVERY: Duration = Duration::from_secs(30 * 60);

#[cfg(test)]
type TestMonitorCapture = Arc<dyn Fn(screen::MonitorId) -> Result<Frame, String> + Send + Sync>;
#[cfg(test)]
type TestWindowCapture = Arc<dyn Fn(screen::WindowId) -> Result<Frame, String> + Send + Sync>;

pub struct App {
    pub settings: Settings,
    settings_dirty: Option<Instant>,
    settings_writer: SettingsWriter,
    pub page: Page,
    pub last_capture_page: Page,
    pub last_main_page: Page,
    pub last_agent_page: Page,
    notices: Notices,
    tray: Option<Tray>,
    tray_rx: Option<Receiver<TrayCommand>>,
    hotkeys: Option<Hotkeys>,
    hotkey_rx: Option<Receiver<u32>>,
    pub hotkey_problems: Vec<String>,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub mic_devices: Vec<String>,
    default_mic_device: Option<String>,
    catalog_job: Option<CatalogJob>,
    desktop_events: Receiver<DesktopEvent>,
    desktop_events_tx: Sender<DesktopEvent>,
    desktop_jobs: Vec<crate::jobs::Job<()>>,
    pending: Option<(Instant, Pending)>,
    hidden_by_us: bool,
    recording_quiet: bool,
    ghosted: bool,
    pub(crate) recording: features::recording::RecordingController,
    pub(crate) capture: features::capture::CaptureController,
    pub(crate) clipboard: ClipboardService,
    region_outline: mediaa_platform::outline::RegionOutline,
    pub lib: features::library::LibraryController,
    pub thumbs: Thumbnails,
    pub(crate) conversion: features::conversion::ConversionController,
    pub agent: features::agent::AgentController,
    quit_when_idle: bool,
    recovery_job: Option<crate::jobs::Job<RecoveryReport>>,
    housekeeping_jobs: Vec<crate::jobs::Job<()>>,
    last_cache_sweep: Instant,
    _instance: Option<SingleInstance>,
    native_window: Option<NativeWindow>,
    startup: StartupAction,
    logo: Option<egui::TextureHandle>,
    #[cfg(test)]
    fixed_catalogs: bool,
    #[cfg(test)]
    monitor_capture: Option<TestMonitorCapture>,
    #[cfg(test)]
    window_capture: Option<TestWindowCapture>,
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
}

impl StartupAction {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        args.into_iter()
            .find_map(|a| match a.as_str() {
                "--area" | "-a" => Some(StartupAction::Area),
                "--fullscreen" | "-f" => Some(StartupAction::Fullscreen),
                "--tray" => Some(StartupAction::Tray),
                _ => None,
            })
            .unwrap_or_default()
    }
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        startup: StartupAction,
        page: Option<Page>,
        instance: Option<SingleInstance>,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        let mut settings = Settings::load();
        mediaa_agent::secrets::load(&mut settings.agent);
        mediaa_media::preview_cache::set_animated_previews_enabled(settings.animated_previews);
        mediaa_media::preview_cache::set_full_previews_enabled(settings.full_preview_videos);
        let clipboard = ClipboardService::new(ctx.clone());
        let previews = mediaa_media::preview_cache::previews_dir(&settings.media_dir);
        let mut output_dirs = vec![settings.media_dir.clone(), settings.convert_dir()];
        output_dirs.push(previews.clone());
        output_dirs.sort();
        output_dirs.dedup();
        theme::apply_preference(ctx, settings.theme);
        crate::i18n::apply(settings.language.as_deref());
        let (desktop_events_tx, desktop_events) = crossbeam_channel::bounded(64);

        let (tray, tray_rx, tray_problem) = match Tray::new(ctx) {
            Ok((t, rx)) => (Some(t), Some(rx), None),
            Err(e) => {
                log::warn!("tray unavailable: {e:#}");
                (None, None, Some(format!("{e:#}")))
            }
        };
        let (mut hotkeys, hotkey_rx, hotkey_manager_problem) = match Hotkeys::new(ctx) {
            Ok((h, rx)) => (Some(h), Some(rx), None),
            Err(e) => {
                log::warn!("global hotkeys unavailable: {e:#}");
                (None, None, Some(format!("{e:#}")))
            }
        };
        let hotkey_problems = hotkeys
            .as_mut()
            .map(|h| h.apply(&settings.hotkeys))
            .unwrap_or_default();
        let slots = mediaa_core::slots::Slots::new();
        let connections = std::sync::Arc::new(mediaa_engine::Connections::new(
            settings.agent.connections.clone(),
        ));

        let mut app = Self {
            settings,
            settings_dirty: None,
            settings_writer: SettingsWriter::create(),
            page: Page::Screenshot,
            last_capture_page: Page::Screenshot,
            last_main_page: Page::Screenshot,
            last_agent_page: Page::Catalogs,
            notices: Notices::default(),
            tray,
            tray_rx,
            hotkeys,
            hotkey_rx,
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
            hidden_by_us: false,
            recording_quiet: false,
            ghosted: false,
            recording: Default::default(),
            capture: Default::default(),
            clipboard,
            region_outline: Default::default(),
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx),
            conversion: Default::default(),
            agent: features::agent::AgentController::with_connections(connections, slots),
            quit_when_idle: false,
            recovery_job: None,
            housekeeping_jobs: Vec::new(),
            last_cache_sweep: Instant::now(),
            _instance: instance,
            native_window: cc
                .window_handle()
                .ok()
                .and_then(|h| NativeWindow::from_raw(h.as_raw())),
            startup: StartupAction::None,
            logo: None,
            #[cfg(test)]
            fixed_catalogs: false,
            #[cfg(test)]
            monitor_capture: None,
            #[cfg(test)]
            window_capture: None,
            #[cfg(test)]
            completion_sound_enabled: true,
        };
        if let Some(mode) = mediaa_core::dev::notices_mode() {
            app.notices.push(
                NoticeLevel::Error,
                "Не удалось сохранить WebP — исходники сохранены".into(),
                "WebP encoding failed after 6 attempts: part-000076.webp has no VP8X chunk.\nБыл выполнен переход на линейную конвертацию.".into(),
                Some("webp-33868-0001".into()),
                vec![
                    "C:\\Users\\wmachine\\Documents\\MediaA\\Recording part 1.mkv".into(),
                    "C:\\Users\\wmachine\\Documents\\MediaA\\Recording part 2.mkv".into(),
                ],
                true,
            );
            if mode == "expanded" {
                if let Some(notice) = app.notices.items.last_mut() {
                    notice.expanded = true;
                }
            }
        }
        app.refresh_catalogs(ctx);
        app.lib.refresh(&app.settings);
        app.start_recovery();
        if let Some(error) = tray_problem {
            app.warning(t!("toast.tray_unavailable", error = error));
        }
        if let Some(error) = hotkey_manager_problem {
            app.warning(t!("toast.hotkeys_unavailable", error = error));
        }
        for p in app.hotkey_problems.clone() {
            app.warning(p);
        }
        let wants_tray = startup == StartupAction::Tray || app.settings.start_in_tray;
        app.startup = if startup == StartupAction::None && wants_tray {
            StartupAction::Tray
        } else {
            startup
        };
        if wants_tray && app.tray.is_none() {
            app.show_window(ctx);
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
        let stale_previews = previews;
        match crate::jobs::Job::spawn("startup-cleanup", Arc::default(), move || {
            for dir in output_dirs {
                paths::remove_stale_temporary_files(&dir, Duration::from_secs(3600));
            }
            paths::remove_stale_entries(&stale_previews, Duration::from_secs(30 * 24 * 3600));
        }) {
            Ok(job) => app.housekeeping_jobs.push(job),
            Err(error) => log::warn!("startup cleanup: {error}"),
        }
        match crate::jobs::Job::spawn("font-warmup", Arc::default(), || {
            let _ = mediaa_render::annotate::fonts().len();
        }) {
            Ok(job) => app.housekeeping_jobs.push(job),
            Err(error) => log::warn!("font warmup: {error}"),
        }
        app
    }

    pub fn settings_changed(&mut self) {
        self.settings_dirty = Some(Instant::now());
    }

    pub fn preview_options_changed(&mut self) {
        let animated = self.settings.animated_previews;
        let full = self.settings.full_preview_videos;
        mediaa_media::preview_cache::set_animated_previews_enabled(animated);
        mediaa_media::preview_cache::set_full_previews_enabled(full);
        self.lib.stop_previews();
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
        match crate::jobs::Job::spawn("catalog-load", Arc::default(), move || {
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
        self.recovery_job.is_some()
    }

    pub fn reapply_hotkeys(&mut self) {
        if let Some(h) = self.hotkeys.as_mut() {
            self.hotkey_problems = h.apply(&self.settings.hotkeys);
        }
    }

    pub fn suspend_hotkeys(&mut self) {
        if let Some(h) = self.hotkeys.as_mut() {
            let off = mediaa_core::settings::HotkeySettings {
                enabled: false,
                ..Default::default()
            };
            h.apply(&off);
        }
        let _ = mediaa_platform::keys::print_screen_down();
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.run_startup_action(ctx);
        self.handle_tray(ctx);
        self.handle_hotkeys(ctx);
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
        if let Some(error) = self.settings_writer.take_error() {
            self.error(t!("err.save_settings", error = error));
        }
        self.poll_conversion(ctx);
        self.poll_recovery(ctx);
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
        if self.last_cache_sweep.elapsed() >= CACHE_SWEEP_EVERY {
            self.last_cache_sweep = Instant::now();
            let media_dir = self.settings.media_dir.clone();
            let ctx = ctx.clone();
            match crate::jobs::Job::spawn("cache-sweep", Arc::default(), move || {
                let report = features::cache_maintenance::sweep(
                    &media_dir,
                    std::time::SystemTime::now(),
                    features::cache_maintenance::CACHE_TTL,
                );
                if report.previews > 0 || report.atlas_pages > 0 {
                    log::info!(
                        "cache sweep removed previews={} atlas_pages={}",
                        report.previews,
                        report.atlas_pages
                    );
                }
                ctx.request_repaint();
            }) {
                Ok(job) => self.housekeeping_jobs.push(job),
                Err(error) => log::warn!("cache sweep: {error}"),
            }
        }
        features::library::poll_cleanup(self, ctx);
        self.update_tray();
        self.show_recording_region(ctx);

        let closing = ctx.input(|i| i.viewport().close_requested()) && !self.quit_when_idle;
        if closing {
            match close_request(
                self.settings.close_to_tray,
                self.tray.is_some(),
                self.has_active_work(),
            ) {
                CloseRequest::Allow => {}
                CloseRequest::Hide => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.hide_window(ctx);
                }
                CloseRequest::KeepVisible => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    self.show_window(ctx);
                    self.warning(t!("toast.finish_current"));
                }
            }
        }
        if self.quit_when_idle && !self.has_active_work() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if let Some(t) = self.settings_dirty {
            if t.elapsed() > SETTINGS_SAVE_DELAY {
                self.settings_dirty = None;
                if let Err(e) = self.settings_writer.request(self.settings.clone()) {
                    self.error(t!("err.save_settings", error = format!("{e:#}")));
                }
            } else {
                ctx.request_repaint_after(SETTINGS_SAVE_DELAY + Duration::from_millis(50));
            }
        }
        let ai_dir = mediaa_core::catalogs::ai_dir(&self.settings.media_dir);
        let language = crate::i18n::current();
        let active = self.page.section() == Some(pages::Section::Agent);
        self.agent
            .tick(&ai_dir, &self.settings.agent, &language, active, ctx);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.thumbs.poll(&ctx);
        self.prewarm_region_overlay(&ctx);
        self.handle_overlay(&ctx);
        pages::shell(self, ui);
        let interactive = ctx.memory(|memory| memory.allows_interaction(ui.layer_id()));
        if !self.lib.viewer_open() {
            self.show_notices(&ctx, interactive);
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.settings.highlight_recording_region
            && self.recording.is_recording()
            && self.recording.region().is_some()
        {
            [0.0; 4]
        } else {
            visuals.panel_fill.to_normalized_gamma_f32()
        }
    }

    fn on_exit(&mut self) {
        self.thumbs.flush();
        self.lib.flush();
        if let Err(e) = self
            .settings_writer
            .flush(self.settings.clone(), SETTINGS_FLUSH_TIMEOUT)
        {
            log::error!("saving settings on exit: {e:#}");
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloseRequest {
    Allow,
    Hide,
    KeepVisible,
}

fn close_request(close_to_tray: bool, has_tray: bool, busy: bool) -> CloseRequest {
    match (close_to_tray && has_tray, busy) {
        (true, _) => CloseRequest::Hide,
        (false, true) => CloseRequest::KeepVisible,
        (false, false) => CloseRequest::Allow,
    }
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
fn recording_completion_sound(_output: OutputKind, source: StopSource) -> bool {
    matches!(source, StopSource::Tray | StopSource::Hotkey)
}

fn completion_sound_after_action(requested: bool, action_succeeded: bool) -> bool {
    requested && action_succeeded
}

fn recording_tray_state(recording: &features::recording::RecordingController) -> TrayState {
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
