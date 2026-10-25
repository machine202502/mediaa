mod actions;
pub(crate) mod capture;
mod clipboard_events;
pub(crate) mod command;
mod desktop;
mod notices;
mod recording;
mod recovery;
pub(crate) mod state;
pub(crate) mod tasks;
mod window;

use crate::clipboard_service::ClipboardService;
use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::overlay::{self, Outcome, Overlay};
use crate::settings_writer::SettingsWriter;
use crate::thumbs::Thumbnails;
use crate::tray::{Tray, TrayCommand, TrayState};
use crate::{pages, theme};
use command::{AppCommand, RecordingCommand};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use mediaa_core::geometry::Rect;
use mediaa_core::limits::FPS;
use mediaa_core::settings::{
    AfterCapture, AfterCompletionAction, AnimSettings, RecordProfile, ScreenChoice, TrayClickAction,
};
use mediaa_core::source::{AudioSource, SourceFactory};
use mediaa_core::{library, naming, paths, Frame, ImageFormat, OutputKind, Settings};
use mediaa_media::recorder::{
    recover_incomplete_sessions, AudioLayout, RecordingRequest, RecordingSession,
    RecordingStartCancelled, RecoveryReport, SessionEvent, SessionFailure, SessionNotice,
    StopAction, WebpMode,
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
use state::{
    CaptureJob, CatalogEvent, CatalogJob, CopyImageJob, DesktopEvent, Page, Pending, RecState,
    RecordTarget, RecordingStartFailure, ScreenshotJob, StopSource,
};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tasks::{ActiveTasks, TaskId, TaskKind, TaskSupervisor, WorkKind};

const HIDE_DELAY: Duration = Duration::from_millis(300);
const CATALOG_TIMEOUT: Duration = Duration::from_secs(15);
const SETTINGS_SAVE_DELAY: Duration = Duration::from_millis(600);
const SETTINGS_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);
pub struct App {
    pub settings: Settings,
    settings_dirty: Option<Instant>,
    settings_writer: SettingsWriter,
    pub page: Page,
    pub last_capture_page: Page,
    pub last_main_page: Page,
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
    overlay: Option<Overlay>,
    pending: Option<(Instant, Pending)>,
    hidden_by_us: bool,
    ghosted: bool,
    pub rec: pages::record::RecordUi,
    pub recorder: RecState,
    screenshot_job: Option<ScreenshotJob>,
    capture_job: Option<CaptureJob>,
    pub(crate) copy_image_job: Option<CopyImageJob>,
    pub(crate) clipboard: ClipboardService,
    recording_region: Option<(MonitorInfo, Rect)>,
    region_outline: mediaa_platform::outline::RegionOutline,
    pub lib: pages::library::LibraryUi,
    pub thumbs: Thumbnails,
    pub conv: pages::convert::ConvertUi,
    quit_when_idle: bool,
    pub(crate) tasks: TaskSupervisor,
    recovery_rx: Option<Receiver<RecoveryReport>>,
    recovery_task: Option<TaskId>,
    _instance: Option<SingleInstance>,
    native_window: Option<NativeWindow>,
    startup: StartupAction,
    logo: Option<egui::TextureHandle>,
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
        let settings = Settings::load();
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

        let mut app = Self {
            settings,
            settings_dirty: None,
            settings_writer: SettingsWriter::new(),
            page: Page::Screenshot,
            last_capture_page: Page::Screenshot,
            last_main_page: Page::Screenshot,
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
            overlay: None,
            pending: None,
            hidden_by_us: false,
            ghosted: false,
            rec: Default::default(),
            recorder: RecState::Idle,
            screenshot_job: None,
            capture_job: None,
            copy_image_job: None,
            clipboard,
            recording_region: None,
            region_outline: Default::default(),
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx),
            conv: Default::default(),
            quit_when_idle: false,
            tasks: TaskSupervisor::default(),
            recovery_rx: None,
            recovery_task: None,
            _instance: instance,
            native_window: cc
                .window_handle()
                .ok()
                .and_then(|h| NativeWindow::from_raw(h.as_raw())),
            startup: StartupAction::None,
            logo: None,
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
        app.lib.refresh(&app.settings, &mut app.tasks);
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
        }
        let stale_previews = previews;
        if let Err(error) =
            app.tasks
                .spawn(TaskKind::Housekeeping, "startup-cleanup", None, move || {
                    for dir in output_dirs {
                        paths::remove_stale_temporary_files(&dir, Duration::from_secs(3600));
                    }
                    paths::remove_stale_entries(
                        &stale_previews,
                        Duration::from_secs(30 * 24 * 3600),
                    );
                })
        {
            log::warn!("startup cleanup: {error}");
        }
        if let Err(error) = app
            .tasks
            .spawn(TaskKind::Housekeeping, "font-warmup", None, || {
                let _ = mediaa_render::annotate::fonts().len();
            })
        {
            log::warn!("font warmup: {error}");
        }
        app
    }

    pub fn settings_changed(&mut self) {
        self.settings_dirty = Some(Instant::now());
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
        if self.catalog_job.is_some() {
            return;
        }
        let (tx, rx) = crossbeam_channel::bounded(3);
        let ctx = ctx.clone();
        match self.tasks.spawn_with_deadline(
            TaskKind::Catalog,
            "catalog-load",
            None,
            Some(CATALOG_TIMEOUT),
            move || {
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
            },
        ) {
            Ok(task) => self.catalog_job = Some(CatalogJob { rx, task }),
            Err(error) => log::warn!("catalog load thread: {error}"),
        }
    }

    fn poll_catalogs(&mut self, ctx: &egui::Context) {
        let Some(rx) = self.catalog_job.as_ref().map(|job| job.rx.clone()) else {
            return;
        };
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
                        .is_some_and(|job| self.tasks.overdue(job.task))
                    {
                        let job = self.catalog_job.take().expect("catalog job checked");
                        self.tasks.detach(job.task);
                        self.error(t!("err.catalog_timeout"));
                        break;
                    }
                    ctx.request_repaint_after(Duration::from_millis(100));
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let job = self.catalog_job.take().expect("catalog job checked");
                    self.tasks.finish(job.task);
                    break;
                }
            }
        }
    }

    pub fn recovery_active(&self) -> bool {
        self.recovery_rx.is_some()
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
        for exit in self.tasks.reap_finished() {
            if exit.panicked {
                log::error!("background task {:?} panicked", exit.kind);
            } else if exit.timed_out {
                log::warn!("background task {:?} exceeded its deadline", exit.kind);
            }
        }
        pages::library::poll_cleanup(self, ctx);
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
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.thumbs.poll(&ctx);
        self.handle_overlay(&ctx);
        pages::shell(self, ui);
        if !self.lib.viewer_open() {
            self.show_notices(&ctx);
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        if self.settings.highlight_recording_region
            && matches!(self.recorder, RecState::Recording { .. })
            && self.recording_region.is_some()
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

fn recording_animation_settings(
    output: OutputKind,
    fps: u32,
    max_height: Option<u32>,
    animation: &AnimSettings,
) -> AnimSettings {
    let mut animation = animation.clone();
    if matches!(output, OutputKind::Anim(_)) {
        animation.fps = FPS.clamp(fps);
        animation.max_height = max_height.unwrap_or(0);
    }
    animation
}

fn recording_completion_sound(_output: OutputKind, source: StopSource) -> bool {
    matches!(source, StopSource::Tray | StopSource::Hotkey)
}

fn completion_sound_after_action(requested: bool, action_succeeded: bool) -> bool {
    requested && action_succeeded
}

fn recording_tray_state(recorder: &RecState) -> TrayState {
    match recorder {
        RecState::Idle => TrayState::Idle,
        RecState::Starting { .. } => TrayState::Starting,
        RecState::Recording { session, .. } if session.is_paused() => TrayState::Paused,
        RecState::Recording { .. } => TrayState::Recording,
        RecState::Saving { .. } => TrayState::Saving,
    }
}

pub fn page_from_args(args: &[String]) -> Option<Page> {
    args.iter().find_map(|a| match a.strip_prefix("--page=")? {
        "screenshot" => Some(Page::Screenshot),
        "record" => Some(Page::Record),
        "audio" => Some(Page::RecordAudio),
        "library" => Some(Page::Library),
        "convert" => Some(Page::Convert),
        "settings" => Some(Page::Settings),
        "about" => Some(Page::About),
        _ => None,
    })
}

#[cfg(test)]
mod tests;
