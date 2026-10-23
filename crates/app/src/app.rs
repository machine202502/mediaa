//! Application state and the event loop glue: tray, hotkeys, timers, captures, recordings.
//! Page UIs live in `pages/`.

use rust_i18n::t;
use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::overlay::{self, Overlay, Outcome};
use crate::thumbs::Thumbnails;
use crate::tray::{Tray, TrayCommand, TrayState};
use crate::{pages, theme};
use crossbeam_channel::Receiver;
use eframe::egui;
use mediaa_core::geometry::Rect;
use mediaa_core::settings::{AfterCapture, RecordProfile, ScreenChoice};
use mediaa_core::source::{AudioSource, SourceFactory};
use mediaa_core::{naming, paths, screenshot, Frame, ImageFormat, OutputKind, Settings};
use mediaa_media::recorder::{RecordingRequest, RecordingSession, SessionEvent};
use mediaa_platform::audio::{CpalSource, CpalTarget};
use mediaa_platform::screen::{self, MonitorInfo, WindowInfo};
use mediaa_platform::window::NativeWindow;
use raw_window_handle::HasWindowHandle;
use mediaa_platform::{clipboard, files};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Delay between hiding the window and grabbing the screen (window fade-out animations).
const HIDE_DELAY: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Screenshot,
    Record,
    RecordAudio,
    Library,
    Convert,
    Settings,
    /// Agent section: virtual catalogs of files.
    Catalogs,
    /// Agent section: LLM providers.
    Providers,
    /// Agent section: models per role.
    Models,
    /// Agent section: the queue of neural-network tasks.
    Queue,
    /// Agent section: memory and concurrency budget.
    Budget,
    About,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordTarget {
    /// Sound only (the "Record audio" page; its own settings).
    Audio,
    Screens(Vec<MonitorInfo>),
    Region(MonitorInfo, Rect),
    Windows(Vec<WindowInfo>),
}

#[derive(Debug, Clone)]
enum Pending {
    RegionScreenshot { purpose: overlay::Purpose },
    FullscreenScreenshot(MonitorInfo),
    WindowScreenshot(u32),
    BeginRecording(RecordTarget, RecordProfile),
}

pub enum RecState {
    Idle,
    Recording { session: Box<RecordingSession>, events: Receiver<SessionEvent> },
    Saving { events: Receiver<SessionEvent>, progress: f32, cancel: Arc<AtomicBool> },
}

impl RecState {
    pub fn is_idle(&self) -> bool {
        matches!(self, RecState::Idle)
    }
}

pub struct App {
    pub settings: Settings,
    settings_dirty: Option<Instant>,
    pub page: Page,
    /// Page to return to when switching back to the Capture section.
    pub last_capture_page: Page,
    /// Page to return to when switching back to the Agent section.
    pub last_agent_page: Page,
    /// Last section page (Settings / About toggle back to it).
    pub last_main_page: Page,
    pub toasts: egui_notify::Toasts,
    tray: Option<Tray>,
    tray_rx: Option<Receiver<TrayCommand>>,
    hotkeys: Option<Hotkeys>,
    hotkey_rx: Option<Receiver<u32>>,
    pub hotkey_problems: Vec<String>,
    pub monitors: Vec<MonitorInfo>,
    pub windows: Vec<WindowInfo>,
    pub mic_devices: Vec<String>,
    overlay: Option<Overlay>,
    pending: Option<(Instant, Pending)>,
    /// The window was hidden by us and should come back afterwards.
    hidden_by_us: bool,
    /// The hidden window is temporarily "ghosted" (see `NativeWindow::set_ghost`) so the area
    /// picker can run while the app sits in the tray.
    ghosted: bool,
    pub rec: pages::record::RecordUi,
    pub recorder: RecState,
    pub lib: pages::library::LibraryUi,
    pub thumbs: Thumbnails,
    pub conv: pages::convert::ConvertUi,
    pub agent: pages::agent::AgentUi,
    pub capabilities: Vec<(&'static str, bool)>,
    quit_after_save: bool,
    /// Native handle of the main window (Windows), for reliable hide/show.
    native_window: Option<NativeWindow>,
    /// Command-line action, run once the window has been shown (see `logic`).
    startup: StartupAction,
    logo: Option<egui::TextureHandle>,
}

/// Action requested on the command line (`mediaa --area`, `mediaa --fullscreen`), handy for
/// binding MediaA to any system shortcut.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StartupAction {
    #[default]
    None,
    Area,
    Fullscreen,
}

impl StartupAction {
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Self {
        args.into_iter()
            .find_map(|a| match a.as_str() {
                "--area" | "-a" => Some(StartupAction::Area),
                "--fullscreen" | "-f" => Some(StartupAction::Fullscreen),
                _ => None,
            })
            .unwrap_or_default()
    }
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, startup: StartupAction, page: Option<Page>) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        let settings = Settings::load();
        // Scan the system fonts for text annotations in the background (takes a moment).
        std::thread::spawn(|| mediaa_core::annotate::fonts().len());
        theme::apply_preference(ctx, settings.theme);
        crate::i18n::apply(settings.language.as_deref());

        let (tray, tray_rx) = match Tray::new(ctx) {
            Ok((t, rx)) => (Some(t), Some(rx)),
            Err(e) => {
                log::warn!("tray unavailable: {e:#}");
                (None, None)
            }
        };
        let (mut hotkeys, hotkey_rx) = match Hotkeys::new(ctx) {
            Ok((h, rx)) => (Some(h), Some(rx)),
            Err(e) => {
                log::warn!("global hotkeys unavailable: {e:#}");
                (None, None)
            }
        };
        let hotkey_problems = hotkeys
            .as_mut()
            .map(|h| h.apply(&settings.hotkeys))
            .unwrap_or_default();
        // One engine for the whole process: the agent's services and the pages share it.
        let agent_budget = settings.agent.budget.clone();
        let engine: std::sync::Arc<dyn mediaa_engine::Engine> =
            std::sync::Arc::new(mediaa_engine::LocalEngine::new(mediaa_core::paths::models_dir(), agent_budget.clone().into()));

        let mut app = Self {
            settings,
            settings_dirty: None,
            page: Page::Screenshot,
            last_capture_page: Page::Screenshot,
            last_agent_page: Page::Catalogs,
            last_main_page: Page::Screenshot,
            toasts: egui_notify::Toasts::default().with_anchor(egui_notify::Anchor::BottomRight),
            tray,
            tray_rx,
            hotkeys,
            hotkey_rx,
            hotkey_problems,
            monitors: Vec::new(),
            windows: Vec::new(),
            mic_devices: mediaa_platform::audio::input_devices(),
            overlay: None,
            pending: None,
            hidden_by_us: false,
            ghosted: false,
            rec: Default::default(),
            recorder: RecState::Idle,
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx),
            conv: Default::default(),
            agent: pages::agent::AgentUi::new(engine, &agent_budget),
            capabilities: mediaa_media::capabilities(),
            quit_after_save: false,
            native_window: cc.window_handle().ok().and_then(|h| NativeWindow::from_raw(h.as_raw())),
            startup: StartupAction::None,
            logo: None,
        };
        app.refresh_monitors();
        app.refresh_windows();
        app.lib.refresh(&app.settings);
        for p in app.hotkey_problems.clone() {
            app.toasts.warning(p).duration(Some(Duration::from_secs(8)));
        }
        app.startup = startup;
        if let Some(p) = page {
            app.page = p;
            match p.section() {
                Some(pages::Section::Capture) => app.last_capture_page = p,
                Some(pages::Section::Agent) => app.last_agent_page = p,
                _ => {}
            }
        }
        app
    }

    // ---------------------------------------------------------------- shared helpers

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
    }

    pub fn refresh_windows(&mut self) {
        match screen::windows() {
            Ok(w) => self.windows = w,
            Err(e) => self.error(t!("err.list_windows", error = format!("{e:#}"))),
        }
    }

    pub fn reapply_hotkeys(&mut self) {
        if let Some(h) = self.hotkeys.as_mut() {
            self.hotkey_problems = h.apply(&self.settings.hotkeys);
        }
    }

    /// Unregisters all global hotkeys while a new combination is being captured, so pressing
    /// an existing one reaches the settings field instead of triggering it.
    pub fn suspend_hotkeys(&mut self) {
        if let Some(h) = self.hotkeys.as_mut() {
            let off = mediaa_core::settings::HotkeySettings { enabled: false, ..Default::default() };
            h.apply(&off);
        }
        // Reset the "pressed since last poll" flag of the polled PrintScreen key.
        let _ = mediaa_platform::keys::print_screen_down();
    }

    pub fn error(&mut self, msg: impl Into<String>) {
        let msg = msg.into();
        log::error!("{msg}");
        self.toasts.error(msg).duration(Some(Duration::from_secs(6)));
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        self.toasts.info(msg.into());
    }

    pub fn success(&mut self, msg: impl Into<String>) {
        self.toasts.success(msg.into());
    }

    pub fn logo(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        self.logo
            .get_or_insert_with(|| {
                let img = egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::rgba(64));
                ctx.load_texture("logo", img, egui::TextureOptions::LINEAR)
            })
            .clone()
    }

    /// Hides / shows the main window: natively where supported (see `platform::window`),
    /// otherwise through egui.
    fn set_window_visible(&mut self, ctx: &egui::Context, visible: bool) {
        let native = self.native_window.is_some_and(|w| w.set_visible(visible));
        if !native {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
        }
        if visible {
            if ctx.input(|i| i.viewport().minimized == Some(true)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    }

    fn hide_window(&mut self, ctx: &egui::Context) {
        if !self.hidden_by_us {
            self.hidden_by_us = true;
            self.set_window_visible(ctx, false);
        }
    }

    pub fn show_window(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ghosted) {
            if let Some(w) = self.native_window {
                w.set_ghost(false);
            }
        }
        self.hidden_by_us = false;
        self.set_window_visible(ctx, true);
    }

    fn schedule(&mut self, ctx: &egui::Context, pending: Pending, hide: bool) {
        let delay = if hide {
            self.hide_window(ctx);
            HIDE_DELAY
        } else {
            Duration::ZERO
        };
        log::info!("scheduled {pending:?} in {delay:?} (hide: {hide})");
        self.pending = Some((Instant::now() + delay, pending));
        ctx.request_repaint_after(delay);
    }

    /// Monitor that contains the main window (falls back to the primary one).
    fn current_monitor_index(&self, ctx: &egui::Context) -> usize {
        let pos = ctx.input(|i| {
            let ppp = i.viewport().native_pixels_per_point.unwrap_or(1.0);
            i.viewport().outer_rect.map(|r| r.center() * ppp)
        });
        pos.and_then(|p| {
            self.monitors.iter().position(|m| {
                p.x >= m.x as f32
                    && p.x < (m.x + m.width as i32) as f32
                    && p.y >= m.y as f32
                    && p.y < (m.y + m.height as i32) as f32
            })
        })
        .unwrap_or(0)
    }

    // ---------------------------------------------------------------- screenshots

    pub fn start_region_screenshot(&mut self, ctx: &egui::Context, purpose: overlay::Purpose) {
        if self.overlay.is_some() {
            return;
        }
        self.refresh_monitors();
        // The area picker is its own window; the main window stays as it is.
        // egui only creates child windows while the main one runs its UI, so un-minimize it.
        if ctx.input(|i| i.viewport().minimized == Some(true)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        }
        self.schedule(ctx, Pending::RegionScreenshot { purpose }, false);
    }

    pub fn start_fullscreen_screenshot(&mut self, ctx: &egui::Context, monitor: Option<MonitorInfo>) {
        self.refresh_monitors();
        let monitor = monitor
            .or_else(|| self.monitors.get(self.current_monitor_index(ctx)).cloned())
            .or_else(|| self.monitors.first().cloned());
        match monitor {
            Some(m) => {
                let hide = self.settings.screenshot.hide_window;
                self.schedule(ctx, Pending::FullscreenScreenshot(m), hide);
            }
            None => self.error(t!("err.no_display")),
        }
    }

    pub fn start_window_screenshot(&mut self, ctx: &egui::Context, window_id: u32) {
        let hide = self.settings.screenshot.hide_window;
        self.schedule(ctx, Pending::WindowScreenshot(window_id), hide);
    }

    /// Copies and/or saves a captured image according to `action`.
    pub fn deliver_image(&mut self, frame: Frame, action: AfterCapture, format: Option<ImageFormat>) {
        let mut copied = false;
        let mut saved = None;
        if action.copies() {
            match clipboard::copy_image(&frame) {
                Ok(()) => copied = true,
                Err(e) => self.error(t!("err.copy_failed", error = format!("{e:#}"))),
            }
        }
        if action.saves() {
            let fmt = format.unwrap_or(self.settings.screenshot.format);
            let path = naming::media_path(&self.settings.media_dir, "Screenshot", chrono::Local::now(), fmt.extension());
            match screenshot::save(&frame, fmt, &path) {
                Ok(()) => {
                    saved = Some(path.file_name().unwrap_or_default().to_string_lossy().into_owned());
                    self.lib.files_changed(std::slice::from_ref(&path));
                }
                Err(e) => self.error(t!("err.save_failed", error = format!("{e:#}"))),
            }
        }
        let size = format!("{}×{}", frame.width, frame.height);
        let msg = match (copied, saved) {
            (true, Some(name)) => t!("toast.shot_copied_saved", size = size, name = name),
            (false, Some(name)) => t!("toast.shot_saved", size = size, name = name),
            (true, None) => t!("toast.shot_copied", size = size),
            (false, None) => return,
        };
        self.success(msg);
    }

    fn run_pending(&mut self, ctx: &egui::Context, pending: Pending) {
        log::info!("running {pending:?}");
        match pending {
            Pending::RegionScreenshot { purpose } => {
                // Capture only the display the picker opens on (others on Tab), then hand the
                // frozen picture to the overlay before its window exists.
                let start = self.current_monitor_index(ctx).min(self.monitors.len().saturating_sub(1));
                let Some(monitor) = self.monitors.get(start).cloned() else {
                    self.error(t!("err.no_display"));
                    return;
                };
                match screen::capture_monitor(monitor.id) {
                    Ok(first) => {
                        let after = self.settings.screenshot.after_capture;
                        // One recording at a time: no "Record" button while one is running.
                        let purpose = match purpose {
                            overlay::Purpose::Area if !self.recorder.is_idle() => overlay::Purpose::Screenshot,
                            p => p,
                        };
                        if self.hidden_by_us {
                            // Hidden in the tray: egui renders child windows only while the main
                            // one is shown, so show it fully transparent for the picker's lifetime.
                            self.ghosted = self.native_window.is_some_and(|w| w.set_ghost(true));
                            if !self.ghosted {
                                self.show_window(ctx);
                            }
                        }
                        self.overlay = Some(Overlay::new(ctx, purpose, self.monitors.clone(), start, first, after));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        self.restore_after_capture(ctx);
                        self.error(t!("err.capture_failed_detail", error = format!("{e:#}")));
                    }
                }
            }
            Pending::FullscreenScreenshot(m) => {
                let shot = screen::capture_monitor(m.id);
                self.restore_after_capture(ctx);
                match shot {
                    Ok(f) => self.deliver_image(f, self.settings.screenshot.after_capture, None),
                    Err(e) => self.error(t!("err.capture_failed_detail", error = format!("{e:#}"))),
                }
            }
            Pending::WindowScreenshot(id) => {
                let shot = screen::capture_window(id);
                self.restore_after_capture(ctx);
                match shot {
                    Ok(f) => self.deliver_image(f, self.settings.screenshot.after_capture, None),
                    Err(e) => self.error(t!("err.window_capture_failed", error = format!("{e:#}"))),
                }
            }
            Pending::BeginRecording(target, profile) => self.begin_recording(ctx, target, profile),
        }
    }

    fn restore_after_capture(&mut self, ctx: &egui::Context) {
        if self.hidden_by_us && self.recorder.is_idle() {
            self.show_window(ctx);
        }
    }

    /// Runs the command-line action after the first rendered frame (child windows such as the
    /// area picker can only be created once the main window is up).
    fn run_startup_action(&mut self, ctx: &egui::Context) {
        if self.startup == StartupAction::None || ctx.cumulative_frame_nr() < 2 {
            if self.startup != StartupAction::None {
                ctx.request_repaint();
            }
            return;
        }
        match std::mem::take(&mut self.startup) {
            StartupAction::None => {}
            StartupAction::Area => self.start_region_screenshot(ctx, overlay::Purpose::Area),
            StartupAction::Fullscreen => self.start_fullscreen_screenshot(ctx, None),
        }
    }

    /// Keeps the area-picker window alive and applies its result once the user is done.
    fn handle_overlay(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.overlay.as_ref().and_then(|o| o.show(ctx)) else { return };
        self.overlay = None;
        if std::mem::take(&mut self.ghosted) {
            if let Some(w) = self.native_window {
                w.set_ghost(false);
            }
        }
        match outcome {
            Outcome::Cancel => {}
            Outcome::Image { frame, action, format } => self.deliver_image(frame, action, format),
            // Area picked on the Record page: record it right away with the app settings.
            Outcome::Region { monitor, rect } => {
                self.rec.target = pages::record::TargetKind::Region;
                self.rec.region = Some((monitor.clone(), rect));
                self.page = Page::Record;
                self.last_capture_page = Page::Record;
                self.request_recording(ctx, RecordTarget::Region(monitor, rect), RecordProfile::App);
            }
            // Area picked from the tray / hotkey: the hotkey recording settings apply.
            Outcome::RecordArea { monitor, rect } => {
                self.request_recording(ctx, RecordTarget::Region(monitor, rect), RecordProfile::Hotkey);
            }
        }
    }

    // ---------------------------------------------------------------- recording

    /// Starts recording right away (one recording at a time).
    pub fn request_recording(&mut self, ctx: &egui::Context, target: RecordTarget, profile: RecordProfile) {
        if !self.recorder.is_idle() {
            return;
        }
        // Audio recordings keep the window (there is nothing on screen to keep out of the picture).
        let hide = !matches!(target, RecordTarget::Audio) && self.settings.profile(profile).0.hide_to_tray;
        self.schedule(ctx, Pending::BeginRecording(target, profile), hide);
    }

    fn begin_recording(&mut self, ctx: &egui::Context, target: RecordTarget, profile: RecordProfile) {
        if !self.recorder.is_idle() {
            return;
        }
        // Devices may have been plugged in or out since startup.
        self.refresh_mics();
        let default_mic = mediaa_platform::audio::default_input_device();
        let (r, anim) = self.settings.profile(profile);
        let cursor = r.record_cursor;
        let sources: Vec<SourceFactory> = match &target {
            RecordTarget::Audio => Vec::new(),
            RecordTarget::Screens(ms) => ms
                .iter()
                .map(|m| {
                    let id = m.id;
                    Box::new(move || screen::monitor_source(id, None, cursor)) as SourceFactory
                })
                .collect(),
            RecordTarget::Region(m, rect) => {
                let (id, rect) = (m.id, *rect);
                vec![Box::new(move || screen::monitor_source(id, Some(rect), cursor))]
            }
            RecordTarget::Windows(ws) => ws
                .iter()
                .map(|w| {
                    let id = w.id;
                    Box::new(move || screen::window_source(id, cursor)) as SourceFactory
                })
                .collect(),
        };
        let audio_only = matches!(target, RecordTarget::Audio);
        let a = self.settings.audio_profile(profile);
        let (want_sound, microphone, system_audio) = if audio_only {
            (true, &a.microphone, a.system_audio)
        } else {
            (r.output.supports_audio() && r.record_audio, &r.microphone, r.system_audio)
        };
        let mut audio: Vec<Box<dyn AudioSource>> = Vec::new();
        if want_sound {
            if let Some(mic) = microphone.resolve(&self.mic_devices, default_mic.as_deref()) {
                log::info!("microphone: {mic}");
                audio.push(Box::new(CpalSource::new(CpalTarget::Input(mic))));
            }
            if system_audio && mediaa_platform::audio::system_audio_supported() {
                audio.push(Box::new(CpalSource::new(CpalTarget::System)));
            }
        }
        let now = chrono::Local::now();
        let req = RecordingRequest {
            sources,
            audio,
            output: if audio_only { OutputKind::Audio(a.format) } else { r.output },
            fps: r.fps,
            video_quality: r.video_quality,
            max_height: r.max_height(),
            audio_in_every_file: r.audio_in_every_file,
            audio_kbps: a.kbps,
            anim: anim.clone(),
            out_dir: self.settings.media_dir.clone(),
            work_dir: paths::work_dir().join(format!(
                "{}-{}",
                now.format("%Y%m%d-%H%M%S"),
                std::process::id()
            )),
            title: if audio_only { "Audio" } else { "Recording" }.into(),
        };
        match RecordingSession::start(req) {
            Ok(session) => {
                let events = session.events();
                log::info!("recording started: {:?}", session.source_names());
                self.recorder = RecState::Recording { session: Box::new(session), events };
                if !self.hidden_by_us {
                    self.info(t!("toast.recording_started"));
                }
            }
            Err(e) => {
                self.show_window(ctx);
                self.error(format!("{e:#}"));
            }
        }
    }

    /// Capture page of the running recording (video or audio).
    pub fn recording_page(&self) -> Page {
        match &self.recorder {
            RecState::Recording { session, .. } if matches!(session.output(), OutputKind::Audio(_)) => Page::RecordAudio,
            _ => Page::Record,
        }
    }

    pub fn toggle_pause(&mut self) {
        if let RecState::Recording { session, .. } = &self.recorder {
            if session.is_paused() {
                session.resume();
            } else {
                session.pause();
            }
        }
    }

    pub fn stop_recording(&mut self, ctx: &egui::Context, save: bool) {
        match std::mem::replace(&mut self.recorder, RecState::Idle) {
            RecState::Recording { session, events } => {
                let cancel = Arc::new(AtomicBool::new(false));
                let audio_only = matches!(session.output(), OutputKind::Audio(_));
                session.stop(save, cancel.clone());
                self.recorder = RecState::Saving { events, progress: 0.0, cancel };
                if save {
                    self.page = if audio_only { Page::RecordAudio } else { Page::Record };
                }
            }
            other => self.recorder = other,
        }
        ctx.request_repaint();
    }

    fn poll_recorder(&mut self, ctx: &egui::Context) {
        let events = match &self.recorder {
            RecState::Recording { events, .. } | RecState::Saving { events, .. } => events.clone(),
            RecState::Idle => return,
        };
        for event in events.try_iter() {
            match event {
                SessionEvent::Warning(w) => {
                    self.toasts.warning(w).duration(Some(Duration::from_secs(6)));
                }
                SessionEvent::CaptureFailed(e) => {
                    self.error(t!("toast.recording_stopped", error = e));
                    self.stop_recording(ctx, true);
                }
                SessionEvent::Finalizing(p) => {
                    if let RecState::Saving { progress, .. } = &mut self.recorder {
                        *progress = p;
                    }
                }
                SessionEvent::Finished(paths) => {
                    self.recorder = RecState::Idle;
                    self.lib.files_changed(&paths);
                    self.show_window(ctx);
                    let names: Vec<_> = paths
                        .iter()
                        .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
                        .collect();
                    self.success(t!("toast.saved", names = names.join(", ")));
                    if !self.quit_after_save {
                        self.page = Page::Library;
                    }

                }
                SessionEvent::Cancelled => {
                    self.recorder = RecState::Idle;
                    self.show_window(ctx);
                    self.info(t!("toast.discarded"));

                }
                SessionEvent::Failed(e) => {
                    self.recorder = RecState::Idle;
                    self.show_window(ctx);
                    self.error(t!("toast.saving_failed", error = e));

                }
            }
        }
        if !matches!(self.recorder, RecState::Idle) {
            ctx.request_repaint_after(Duration::from_millis(250));
        } else if self.quit_after_save {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// Current target from the Record page selections.
    pub fn record_target_from_ui(&self) -> Result<RecordTarget, String> {
        use pages::record::TargetKind;
        match self.rec.target {
            TargetKind::Screen => {
                // Several displays at once (one file each); nothing ticked = the primary one.
                let mut ms: Vec<_> = self.monitors.iter().filter(|m| self.rec.monitors.contains(&m.id)).cloned().collect();
                if ms.is_empty() {
                    ms.extend(self.monitors.first().cloned());
                }
                if ms.is_empty() {
                    Err(t!("err.no_display").into())
                } else {
                    Ok(RecordTarget::Screens(ms))
                }
            }
            TargetKind::Region => self
                .rec
                .region
                .clone()
                .map(|(m, r)| RecordTarget::Region(m, r))
                .ok_or_else(|| t!("err.select_area").into()),
            TargetKind::Windows => {
                let ws: Vec<_> = self
                    .windows
                    .iter()
                    .filter(|w| self.rec.windows.contains(&w.id))
                    .cloned()
                    .collect();
                if ws.is_empty() {
                    // No message: the start button is simply disabled until a window is picked.
                    Err(String::new())
                } else {
                    Ok(RecordTarget::Windows(ws))
                }
            }
        }
    }

    /// Records the source chosen on the Record page (the start button).
    pub fn start_recording_from_ui(&mut self, ctx: &egui::Context) {
        match self.record_target_from_ui() {
            Ok(t) => self.request_recording(ctx, t, RecordProfile::App),
            Err(e) if !e.is_empty() => self.error(e),
            Err(_) => {}
        }
    }

    /// Records sound only, with the audio settings of `profile` (the page or the hotkey / tray).
    pub fn start_audio_recording(&mut self, ctx: &egui::Context, profile: RecordProfile) {
        self.request_recording(ctx, RecordTarget::Audio, profile);
    }

    /// Display for hotkey / tray recordings (`Settings::hotkey_screen`). A remembered display
    /// that is gone switches the setting back to the primary one.
    fn hotkey_screen(&mut self) -> Option<MonitorInfo> {
        self.refresh_monitors();
        let primary = self.monitors.first().cloned();
        match self.settings.hotkey_screen.clone() {
            ScreenChoice::Primary => primary,
            ScreenChoice::UnderCursor => mediaa_platform::cursor::position()
                .and_then(|(x, y)| screen::monitor_at(x, y))
                .and_then(|id| self.monitors.iter().find(|m| m.id == id).cloned())
                .or(primary),
            ScreenChoice::Display(device) => match self.monitors.iter().find(|m| m.device == device) {
                Some(m) => Some(m.clone()),
                None => {
                    self.settings.hotkey_screen = ScreenChoice::Primary;
                    self.settings_changed();
                    primary
                }
            },
        }
    }

    /// Hotkey / tray "start recording": the configured display, hotkey settings.
    fn start_hotkey_recording(&mut self, ctx: &egui::Context) {
        match self.hotkey_screen() {
            Some(m) => self.request_recording(ctx, RecordTarget::Screens(vec![m]), RecordProfile::Hotkey),
            None => self.error(t!("err.no_display")),
        }
    }

    // ---------------------------------------------------------------- external events

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let cmds: Vec<_> = self.tray_rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for cmd in cmds {
            match cmd {
                TrayCommand::ShowWindow => self.show_window(ctx),
                TrayCommand::RegionScreenshot => self.start_region_screenshot(ctx, overlay::Purpose::Area),
                TrayCommand::FullscreenScreenshot => self.start_fullscreen_screenshot(ctx, None),
                TrayCommand::StartRecording => self.start_hotkey_recording(ctx),
                TrayCommand::StartAudio => self.start_audio_recording(ctx, RecordProfile::Hotkey),
                TrayCommand::PauseResume => self.toggle_pause(),
                TrayCommand::StopAndSave => self.stop_recording(ctx, true),
                TrayCommand::CancelRecording => self.stop_recording(ctx, false),
                TrayCommand::Quit => self.quit(ctx),
            }
        }
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        let ids: Vec<u32> = self.hotkey_rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for id in ids {
            let Some(action) = self.hotkeys.as_ref().and_then(|h| h.action_for(id)) else {
                continue;
            };
            match action {
                HotkeyAction::RegionScreenshot => self.start_region_screenshot(ctx, overlay::Purpose::Area),
                HotkeyAction::FullscreenScreenshot => self.start_fullscreen_screenshot(ctx, None),
                // One recording at a time: starts are ignored while one runs.
                HotkeyAction::StartVideo if self.recorder.is_idle() => self.start_hotkey_recording(ctx),
                HotkeyAction::StartAudio if self.recorder.is_idle() => self.start_audio_recording(ctx, RecordProfile::Hotkey),
                HotkeyAction::StartVideo | HotkeyAction::StartAudio => {}
                HotkeyAction::StopRecording => {
                    if matches!(self.recorder, RecState::Recording { .. }) {
                        self.stop_recording(ctx, true);
                    }
                }
            }
        }
    }

    pub fn quit(&mut self, ctx: &egui::Context) {
        if matches!(self.recorder, RecState::Recording { .. }) {
            self.quit_after_save = true;
            self.stop_recording(ctx, true);
        } else if matches!(self.recorder, RecState::Saving { .. }) {
            self.quit_after_save = true;
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn update_tray(&mut self) {
        let state = match &self.recorder {
            RecState::Idle => TrayState::Idle,
            RecState::Recording { session, .. } if session.is_paused() => TrayState::Paused,
            RecState::Recording { .. } => TrayState::Recording,
            RecState::Saving { .. } => TrayState::Saving,
        };
        if let Some(t) = self.tray.as_mut() {
            t.set_state(state);
        }
    }

    pub fn open_path(&mut self, path: &std::path::Path) {
        if let Err(e) = files::open(path) {
            self.error(format!("{e:#}"));
        }
    }

    pub fn open_in_convert(&mut self, path: PathBuf) {
        self.conv.set_input(path);
        self.page = Page::Convert;
    }
}

impl eframe::App for App {
    /// Runs every frame, and also while the window is hidden in the tray.
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
        self.poll_recorder(ctx);
        self.update_tray();
        let ai = mediaa_core::catalogs::ai_dir(&crate::pages::library::library_dir(&self.settings));
        let language = crate::i18n::current();
        self.agent.tick(&ai, &self.settings.agent, &language, ctx);

        if ctx.input(|i| i.viewport().close_requested()) && !self.quit_after_save {
            let busy = !self.recorder.is_idle();
            if busy && self.tray.is_some() {
                // Keep recording in the background; the tray icon controls it.
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                self.hide_window(ctx);
            }
        }
        if let Some(t) = self.settings_dirty {
            if t.elapsed() > Duration::from_millis(600) {
                self.settings_dirty = None;
                if let Err(e) = self.settings.save() {
                    self.error(t!("err.save_settings", error = format!("{e:#}")));
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(650));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.thumbs.poll(&ctx);
        self.handle_overlay(&ctx);
        pages::shell(self, ui);
        // Toasts react to the raw pointer, so they would stay clickable over the viewer: they
        // simply wait until it is closed.
        if !self.lib.viewer_open() {
            self.toasts.show(&ctx);
        }
    }

    fn clear_color(&self, visuals: &egui::Visuals) -> [f32; 4] {
        visuals.panel_fill.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.thumbs.flush();
        if let Err(e) = self.settings.save() {
            log::error!("saving settings on exit: {e:#}");
        }
    }
}

/// `--page=<name>` opens the app on a given page (handy for checking layouts).
pub fn page_from_args(args: &[String]) -> Option<Page> {
    args.iter().find_map(|a| match a.strip_prefix("--page=")? {
        "screenshot" => Some(Page::Screenshot),
        "record" => Some(Page::Record),
        "audio" => Some(Page::RecordAudio),
        "library" => Some(Page::Library),
        "convert" => Some(Page::Convert),
        "settings" => Some(Page::Settings),
        "about" => Some(Page::About),
        "catalogs" => Some(Page::Catalogs),
        "providers" => Some(Page::Providers),
        "models" => Some(Page::Models),
        "queue" => Some(Page::Queue),
        "budget" => Some(Page::Budget),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::{page_from_args, Page, StartupAction};

    fn parse(args: &[&str]) -> StartupAction {
        StartupAction::from_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn startup_action_from_args() {
        assert_eq!(parse(&[]), StartupAction::None);
        assert_eq!(parse(&["--area"]), StartupAction::Area);
        assert_eq!(parse(&["-f"]), StartupAction::Fullscreen);
        assert_eq!(parse(&["--unknown", "--fullscreen"]), StartupAction::Fullscreen);
    }

    #[test]
    fn page_from_args_picks_known_pages() {
        let args = |a: &str| vec!["mediaa".to_string(), a.to_string()];
        assert_eq!(page_from_args(&args("--page=record")), Some(Page::Record));
        assert_eq!(page_from_args(&args("--page=nope")), None);
        assert_eq!(page_from_args(&args("--area")), None);
    }
}
