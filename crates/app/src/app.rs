use crate::hotkeys::{HotkeyAction, Hotkeys};
use crate::overlay::{self, Outcome, Overlay};
use crate::thumbs::Thumbnails;
use crate::tray::{Tray, TrayCommand, TrayState};
use crate::{pages, theme};
use crossbeam_channel::Receiver;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_core::geometry::Rect;
use mediaa_core::settings::{
    AfterCapture, AfterCompletionAction, AnimSettings, RecordProfile, ScreenChoice, TrayClickAction,
};
use mediaa_core::source::{AudioSource, SourceFactory};
use mediaa_core::{library, naming, paths, screenshot, Frame, ImageFormat, OutputKind, Settings};
use mediaa_media::recorder::{
    RecordingRequest, RecordingSession, SessionEvent, SessionFailure, SessionNotice,
};
use mediaa_platform::audio::{CpalSource, CpalTarget};
use mediaa_platform::screen::{self, MonitorInfo, WindowInfo};
use mediaa_platform::window::NativeWindow;
use mediaa_platform::{clipboard, files};
use raw_window_handle::HasWindowHandle;
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HIDE_DELAY: Duration = Duration::from_millis(300);
const NOTICE_SUMMARY_CHARS: usize = 72;

fn one_line(message: impl Into<String>) -> String {
    message
        .into()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn notice_summary(message: &str, fallback: &str) -> String {
    let message = one_line(message);
    if message.chars().count() <= NOTICE_SUMMARY_CHARS {
        return message;
    }
    let prefix = message
        .char_indices()
        .find(|(_, ch)| matches!(ch, ':' | '—'))
        .map(|(index, ch)| message[..index + ch.len_utf8()].trim())
        .filter(|prefix| prefix.chars().count() >= 8);
    prefix.unwrap_or(fallback).to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Screenshot,
    Record,
    RecordAudio,
    Library,
    Convert,
    Settings,
    About,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordTarget {
    Audio,
    Screens(Vec<MonitorInfo>),
    Region(MonitorInfo, Rect),
    Windows(Vec<WindowInfo>),
}

#[derive(Debug, Clone)]
enum Pending {
    RegionScreenshot {
        purpose: overlay::Purpose,
        after_completion: bool,
    },
    FullscreenScreenshot {
        monitor: MonitorInfo,
        after_completion: bool,
    },
    WindowScreenshot(u32),
    BeginRecording(RecordTarget, RecordProfile),
}

pub enum RecState {
    Idle,
    Recording {
        session: Box<RecordingSession>,
        events: Receiver<SessionEvent>,
    },
    Saving {
        events: Receiver<SessionEvent>,
        progress: Option<f32>,
        started: Instant,
        cancel: Arc<AtomicBool>,
        sound_on_completion: bool,
    },
}

impl RecState {
    pub fn is_idle(&self) -> bool {
        matches!(self, RecState::Idle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StopSource {
    Ui,
    Tray,
    Hotkey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeLevel {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone)]
struct AppNotice {
    id: u64,
    level: NoticeLevel,
    summary: String,
    detail: String,
    log_ref: String,
    files: Vec<PathBuf>,
    expanded: bool,
    expires_at: Option<Instant>,
}

#[derive(Default, Clone)]
struct Notices {
    items: Vec<AppNotice>,
    next_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NoticeAction {
    CopyLog(String),
    CopyFiles(Vec<PathBuf>),
    CopyFile(PathBuf),
    RevealFile(PathBuf),
}

impl Notices {
    fn push(
        &mut self,
        level: NoticeLevel,
        summary: String,
        detail: String,
        log_ref: Option<String>,
        files: Vec<PathBuf>,
        persistent: bool,
    ) {
        self.next_id += 1;
        let id = self.next_id;
        let log_ref = log_ref.unwrap_or_else(|| format!("notice-{id:04}"));
        match level {
            NoticeLevel::Info => log::info!("[{log_ref}] {detail}"),
            NoticeLevel::Success => log::info!("[{log_ref}] {detail}"),
            NoticeLevel::Warning => log::warn!("[{log_ref}] {detail}"),
            NoticeLevel::Error => log::error!("[{log_ref}] {detail}"),
        }
        let lifetime = match level {
            NoticeLevel::Info | NoticeLevel::Success => Duration::from_secs(5),
            NoticeLevel::Warning => Duration::from_secs(8),
            NoticeLevel::Error => Duration::from_secs(10),
        };
        self.items.push(AppNotice {
            id,
            level,
            summary,
            detail,
            log_ref,
            files,
            expanded: false,
            expires_at: (!persistent).then(|| Instant::now() + lifetime),
        });
        while self.items.len() > 5 {
            self.items.remove(0);
        }
    }

    fn remove(&mut self, id: u64) {
        self.items.retain(|notice| notice.id != id);
    }

    fn expire(&mut self) {
        let now = Instant::now();
        self.items.retain(|notice| {
            notice.expanded || notice.expires_at.is_none_or(|expires| expires > now)
        });
    }

    fn show(&mut self, ctx: &egui::Context) -> Option<NoticeAction> {
        let mut close = None;
        let mut copy_log = None;
        let mut copy_files = None;
        let mut copy_file = None;
        let mut reveal_file = None;
        self.expire();
        egui::Area::new(egui::Id::new("app-notices"))
            .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
            .order(egui::Order::Foreground)
            .show(ctx, |ui| {
                let p = theme::palette(ui);
                ui.set_max_width(460.0);
                ui.spacing_mut().item_spacing.y = 6.0;
                for notice in &mut self.items {
                    let color = match notice.level {
                        NoticeLevel::Info => p.accent,
                        NoticeLevel::Success => p.success,
                        NoticeLevel::Warning => p.warning,
                        NoticeLevel::Error => p.danger,
                    };
                    let icon = match notice.level {
                        NoticeLevel::Info => icon::INFO,
                        NoticeLevel::Success => icon::CHECK_CIRCLE,
                        NoticeLevel::Warning => icon::WARNING,
                        NoticeLevel::Error => icon::WARNING_OCTAGON,
                    };
                    let estimated = notice.summary.chars().count() as f32 * 6.6 + 88.0;
                    let width = if notice.expanded {
                        460.0
                    } else {
                        estimated.clamp(280.0, 480.0)
                    };
                    let mut toggle = false;
                    let mut card_close = false;
                    let mut card_copy_log = false;
                    let mut card_copy_files = false;
                    let mut card_copy_file = None;
                    let mut card_reveal_file = None;
                    let response = egui::Frame::new()
                        .fill(p.card)
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(egui::CornerRadius::same(4))
                        .inner_margin(egui::Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_width(width - 20.0);
                            ui.horizontal(|ui| {
                                ui.label(egui::RichText::new(icon).color(color).size(16.0));
                                let text_width = (ui.available_width() - 54.0).max(120.0);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(text_width, 0.0),
                                    egui::Layout::left_to_right(egui::Align::Center),
                                    |ui| {
                                        let summary = ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&notice.summary).size(13.0),
                                            )
                                            .wrap()
                                            .sense(egui::Sense::click()),
                                        );
                                        if summary.clicked() {
                                            toggle = true;
                                        }
                                    },
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(icon::X)
                                                        .color(p.text_weak)
                                                        .size(13.0),
                                                )
                                                .frame(false),
                                            )
                                            .clicked()
                                        {
                                            card_close = true;
                                        }
                                        if ui
                                            .add(
                                                egui::Button::new(
                                                    egui::RichText::new(if notice.expanded {
                                                        "▴"
                                                    } else {
                                                        "▾"
                                                    })
                                                    .color(p.text_weak)
                                                    .size(13.0),
                                                )
                                                .frame(false),
                                            )
                                            .clicked()
                                        {
                                            toggle = true;
                                        }
                                    },
                                );
                            });
                            if notice.expanded {
                                ui.add_space(4.0);
                                ui.separator();
                                egui::ScrollArea::vertical()
                                    .id_salt(("notice-detail", notice.id))
                                    .max_height(260.0)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&notice.detail).size(12.0),
                                            )
                                            .wrap()
                                            .selectable(false),
                                        );
                                    });
                                ui.horizontal_wrapped(|ui| {
                                    let log_id = ui
                                        .add(
                                            egui::Label::new(
                                                egui::RichText::new(format!(
                                                    "{} {}",
                                                    t!("notice.log_id"),
                                                    notice.log_ref
                                                ))
                                                .monospace()
                                                .color(p.text_weak)
                                                .size(10.5),
                                            )
                                            .sense(egui::Sense::click()),
                                        )
                                        .on_hover_text(t!("notice.copy_log_id"));
                                    if log_id.clicked() {
                                        card_copy_log = true;
                                    }
                                    if ui
                                        .small_button(t!("notice.copy"))
                                        .on_hover_text(t!("notice.copy_log_id"))
                                        .clicked()
                                    {
                                        card_copy_log = true;
                                    }
                                });
                                if !notice.files.is_empty() {
                                    ui.add_space(4.0);
                                    ui.separator();
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(t!(
                                                "notice.mkv_links",
                                                count = notice.files.len()
                                            ))
                                            .color(p.accent)
                                            .size(10.5),
                                        );
                                        if ui
                                            .small_button(t!("notice.copy_links"))
                                            .on_hover_text(t!("toast.recovery_hint"))
                                            .clicked()
                                        {
                                            card_copy_files = true;
                                        }
                                    });
                                    egui::ScrollArea::vertical()
                                        .id_salt(("notice-files", notice.id))
                                        .max_height(130.0)
                                        .show(ui, |ui| {
                                            for (index, file) in notice.files.iter().enumerate() {
                                                let name = file
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy();
                                                ui.push_id(
                                                    ("notice-file", notice.id, index),
                                                    |ui| {
                                                        ui.horizontal(|ui| {
                                                            let actions_width = 140.0;
                                                            let text_width = (ui.available_width()
                                                                - actions_width)
                                                                .max(100.0);
                                                            ui.allocate_ui_with_layout(
                                                                egui::vec2(text_width, 0.0),
                                                                egui::Layout::left_to_right(
                                                                    egui::Align::Center,
                                                                ),
                                                                |ui| {
                                                                    ui.add(
                                                                        egui::Label::new(
                                                                            egui::RichText::new(
                                                                                name.as_ref(),
                                                                            )
                                                                            .size(10.5),
                                                                        )
                                                                        .wrap()
                                                                        .selectable(false),
                                                                    );
                                                                },
                                                            );
                                                            if ui
                                                                .small_button(t!(
                                                                    "notice.copy_link"
                                                                ))
                                                                .clicked()
                                                            {
                                                                card_copy_file = Some(file.clone());
                                                            }
                                                            if ui
                                                                .small_button(t!(
                                                                    "notice.show_file"
                                                                ))
                                                                .clicked()
                                                            {
                                                                card_reveal_file =
                                                                    Some(file.clone());
                                                            }
                                                        });
                                                    },
                                                );
                                            }
                                        });
                                }
                            }
                        });
                    if card_close {
                        close = Some(notice.id);
                    } else if card_copy_log {
                        copy_log = Some(notice.log_ref.clone());
                    } else if card_copy_files {
                        copy_files = Some(notice.files.clone());
                    } else if card_copy_file.is_some() {
                        copy_file = card_copy_file;
                    } else if card_reveal_file.is_some() {
                        reveal_file = card_reveal_file;
                    } else if toggle {
                        notice.expanded = !notice.expanded;
                        notice.expires_at = if notice.expanded {
                            None
                        } else {
                            Some(Instant::now() + Duration::from_secs(8))
                        };
                    } else if response.response.hovered() {
                        if let Some(expires) = notice.expires_at.as_mut() {
                            *expires = Instant::now() + Duration::from_secs(3);
                        }
                    }
                }
            });
        if let Some(id) = close {
            self.remove(id);
        }
        if let Some(log_ref) = copy_log {
            Some(NoticeAction::CopyLog(log_ref))
        } else if copy_file.is_some() {
            copy_file.map(NoticeAction::CopyFile)
        } else if reveal_file.is_some() {
            reveal_file.map(NoticeAction::RevealFile)
        } else {
            copy_files.map(NoticeAction::CopyFiles)
        }
    }
}

pub struct App {
    pub settings: Settings,
    settings_dirty: Option<Instant>,
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
    overlay: Option<Overlay>,
    pending: Option<(Instant, Pending)>,
    hidden_by_us: bool,
    ghosted: bool,
    pub rec: pages::record::RecordUi,
    pub recorder: RecState,
    recording_region: Option<(MonitorInfo, Rect)>,
    region_outline: mediaa_platform::outline::RegionOutline,
    pub lib: pages::library::LibraryUi,
    pub thumbs: Thumbnails,
    pub conv: pages::convert::ConvertUi,
    pub capabilities: Vec<(&'static str, bool)>,
    quit_after_save: bool,
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
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        startup: StartupAction,
        page: Option<Page>,
    ) -> Self {
        let ctx = &cc.egui_ctx;
        theme::install(ctx);
        let settings = Settings::load();
        std::thread::spawn(|| mediaa_core::annotate::fonts().len());
        theme::apply_preference(ctx, settings.theme);
        crate::i18n::apply(settings.language.as_deref());
        let previews = mediaa_core::library_index::cache_dir(&settings.media_dir).join("previews");
        mediaa_core::paths::remove_stale_entries(&previews, Duration::from_secs(30 * 24 * 3600));

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

        let mut app = Self {
            settings,
            settings_dirty: None,
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
            mic_devices: mediaa_platform::audio::input_devices(),
            overlay: None,
            pending: None,
            hidden_by_us: false,
            ghosted: false,
            rec: Default::default(),
            recorder: RecState::Idle,
            recording_region: None,
            region_outline: Default::default(),
            lib: Default::default(),
            thumbs: Thumbnails::new(ctx),
            conv: Default::default(),
            capabilities: mediaa_media::capabilities(),
            quit_after_save: false,
            native_window: cc
                .window_handle()
                .ok()
                .and_then(|h| NativeWindow::from_raw(h.as_raw())),
            startup: StartupAction::None,
            logo: None,
        };
        #[cfg(debug_assertions)]
        if let Ok(mode) = std::env::var("MEDIAA_DEV_NOTICES") {
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
        app.refresh_monitors();
        app.refresh_windows();
        app.lib.refresh(&app.settings);
        for p in app.hotkey_problems.clone() {
            app.warning(p);
        }
        app.startup = startup;
        if let Some(p) = page {
            app.page = p;
            if let Some(pages::Section::Capture) = p.section() {
                app.last_capture_page = p;
            }
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

    pub fn error(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.error"));
        self.notices
            .push(NoticeLevel::Error, summary, detail, None, Vec::new(), false);
    }

    pub fn warning(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.warning"));
        self.notices.push(
            NoticeLevel::Warning,
            summary,
            detail,
            None,
            Vec::new(),
            false,
        );
    }

    fn show_notices(&mut self, ctx: &egui::Context) {
        match self.notices.show(ctx) {
            Some(NoticeAction::CopyLog(log_ref)) => match clipboard::copy_text(&log_ref) {
                Ok(()) => self.success(t!("notice.log_id_copied", id = log_ref)),
                Err(error) => self.error(t!("err.copy_failed", error = format!("{error:#}"))),
            },
            Some(NoticeAction::CopyFiles(files)) => {
                let links = recovery_links(&files);
                match clipboard::copy_text(&links) {
                    Ok(()) => self.success(t!("toast.recovery_copied")),
                    Err(error) => self.error(t!("err.copy_failed", error = format!("{error:#}"))),
                }
            }
            Some(NoticeAction::CopyFile(file)) => {
                match clipboard::copy_text(&library::file_url(&file)) {
                    Ok(()) => self.success(t!("toast.recovery_copied")),
                    Err(error) => self.error(t!("err.copy_failed", error = format!("{error:#}"))),
                }
            }
            Some(NoticeAction::RevealFile(file)) => {
                if let Err(error) = files::reveal(&file) {
                    self.error(format!("{error:#}"));
                }
            }
            None => {}
        }
    }

    pub fn info(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.info"));
        self.notices
            .push(NoticeLevel::Info, summary, detail, None, Vec::new(), false);
    }

    pub fn success(&mut self, msg: impl Into<String>) {
        let detail = msg.into().trim().to_string();
        let summary = notice_summary(&detail, &t!("notice.success"));
        self.notices.push(
            NoticeLevel::Success,
            summary,
            detail,
            None,
            Vec::new(),
            false,
        );
    }

    pub fn logo(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        self.logo
            .get_or_insert_with(|| {
                let img =
                    egui::ColorImage::from_rgba_unmultiplied([64, 64], &crate::icon::rgba(64));
                ctx.load_texture("logo", img, egui::TextureOptions::LINEAR)
            })
            .clone()
    }

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

    pub fn start_region_screenshot(&mut self, ctx: &egui::Context, purpose: overlay::Purpose) {
        self.start_region_screenshot_with_completion(ctx, purpose, false);
    }

    fn start_region_screenshot_with_completion(
        &mut self,
        ctx: &egui::Context,
        purpose: overlay::Purpose,
        after_completion: bool,
    ) {
        if self.overlay.is_some() {
            return;
        }
        self.refresh_monitors();
        if ctx.input(|i| i.viewport().minimized == Some(true)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        }
        self.schedule(
            ctx,
            Pending::RegionScreenshot {
                purpose,
                after_completion,
            },
            false,
        );
    }

    pub fn start_fullscreen_screenshot(
        &mut self,
        ctx: &egui::Context,
        monitor: Option<MonitorInfo>,
    ) {
        self.start_fullscreen_screenshot_with_completion(ctx, monitor, false);
    }

    fn start_fullscreen_screenshot_with_completion(
        &mut self,
        ctx: &egui::Context,
        monitor: Option<MonitorInfo>,
        after_completion: bool,
    ) {
        self.refresh_monitors();
        let monitor = monitor
            .or_else(|| self.monitors.get(self.current_monitor_index(ctx)).cloned())
            .or_else(|| self.monitors.first().cloned());
        match monitor {
            Some(m) => {
                let hide = self.settings.screenshot.hide_window;
                self.schedule(
                    ctx,
                    Pending::FullscreenScreenshot {
                        monitor: m,
                        after_completion,
                    },
                    hide,
                );
            }
            None => self.error(t!("err.no_display")),
        }
    }

    pub fn start_window_screenshot(&mut self, ctx: &egui::Context, window_id: u32) {
        let hide = self.settings.screenshot.hide_window;
        self.schedule(ctx, Pending::WindowScreenshot(window_id), hide);
    }

    pub fn deliver_image(
        &mut self,
        ctx: &egui::Context,
        frame: Frame,
        action: AfterCapture,
        format: Option<ImageFormat>,
        after_completion: bool,
    ) {
        let mut copied = false;
        let mut saved = None;
        let copies_file =
            after_completion && self.settings.after_completion == AfterCompletionAction::CopyFile;
        if action.copies() && !copies_file {
            match clipboard::copy_image(&frame) {
                Ok(()) => copied = true,
                Err(e) => self.error(t!("err.copy_failed", error = format!("{e:#}"))),
            }
        }
        if action.saves() || after_completion {
            let fmt = format.unwrap_or(self.settings.screenshot.format);
            let path = naming::media_path(
                &self.settings.media_dir,
                "Screenshot",
                chrono::Local::now(),
                fmt.extension(),
            );
            match screenshot::save(&frame, fmt, &path) {
                Ok(()) => {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    self.lib.files_changed(std::slice::from_ref(&path));
                    saved = Some((path, name));
                }
                Err(e) => self.error(t!("err.save_failed", error = format!("{e:#}"))),
            }
        }
        let size = format!("{}×{}", frame.width, frame.height);
        if after_completion {
            match self.settings.after_completion {
                AfterCompletionAction::OpenLibrary => {
                    self.show_window(ctx);
                    if !self.quit_after_save {
                        self.page = Page::Library;
                    }
                }
                AfterCompletionAction::CopyFile => match saved.as_ref() {
                    Some((path, _)) => match clipboard::copy_files(&[path.as_path()]) {
                        Ok(()) => {
                            if !self.hidden_by_us {
                                self.success(t!("toast.shot_file_copied"));
                            }
                        }
                        Err(e) => {
                            self.show_window(ctx);
                            self.error(t!("err.copy_failed", error = format!("{e:#}")));
                            if !self.quit_after_save {
                                self.page = Page::Library;
                            }
                        }
                    },
                    None => {
                        self.show_window(ctx);
                        if !self.quit_after_save {
                            self.page = Page::Library;
                        }
                    }
                },
            }
        }
        let msg = match (copied, saved.as_ref().map(|(_, name)| name)) {
            (true, Some(name)) => t!("toast.shot_copied_saved", size = size, name = name),
            (false, Some(name)) => t!("toast.shot_saved", size = size, name = name),
            (true, None) => t!("toast.shot_copied", size = size),
            (false, None) => return,
        };
        if !copies_file {
            self.success(msg);
        }
    }

    fn run_pending(&mut self, ctx: &egui::Context, pending: Pending) {
        log::info!("running {pending:?}");
        match pending {
            Pending::RegionScreenshot {
                purpose,
                after_completion,
            } => {
                let start = self
                    .current_monitor_index(ctx)
                    .min(self.monitors.len().saturating_sub(1));
                let Some(monitor) = self.monitors.get(start).cloned() else {
                    self.error(t!("err.no_display"));
                    return;
                };
                match screen::capture_monitor(monitor.id) {
                    Ok(first) => {
                        let after = self.settings.screenshot.after_capture;
                        let purpose = match purpose {
                            overlay::Purpose::Area if !self.recorder.is_idle() => {
                                overlay::Purpose::Screenshot
                            }
                            p => p,
                        };
                        if self.hidden_by_us {
                            self.ghosted = self.native_window.is_some_and(|w| w.set_ghost(true));
                            if !self.ghosted {
                                self.show_window(ctx);
                            }
                        }
                        self.overlay = Some(Overlay::new(
                            ctx,
                            purpose,
                            self.monitors.clone(),
                            start,
                            first,
                            after,
                            after_completion,
                        ));
                        ctx.request_repaint();
                    }
                    Err(e) => {
                        self.restore_after_capture(ctx);
                        self.error(t!("err.capture_failed_detail", error = format!("{e:#}")));
                    }
                }
            }
            Pending::FullscreenScreenshot {
                monitor,
                after_completion,
            } => {
                let shot = screen::capture_monitor(monitor.id);
                if !after_completion {
                    self.restore_after_capture(ctx);
                }
                match shot {
                    Ok(f) => self.deliver_image(
                        ctx,
                        f,
                        self.settings.screenshot.after_capture,
                        None,
                        after_completion,
                    ),
                    Err(e) => {
                        self.show_window(ctx);
                        self.error(t!("err.capture_failed_detail", error = format!("{e:#}")));
                    }
                }
            }
            Pending::WindowScreenshot(id) => {
                let shot = screen::capture_window(id);
                self.restore_after_capture(ctx);
                match shot {
                    Ok(f) => self.deliver_image(
                        ctx,
                        f,
                        self.settings.screenshot.after_capture,
                        None,
                        false,
                    ),
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

    fn handle_overlay(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.overlay.as_ref().and_then(|o| o.show(ctx)) else {
            return;
        };
        self.overlay = None;
        if std::mem::take(&mut self.ghosted) {
            if let Some(w) = self.native_window {
                w.set_ghost(false);
            }
        }
        match outcome {
            Outcome::Cancel => {}
            Outcome::Image {
                frame,
                action,
                format,
                after_completion,
            } => self.deliver_image(ctx, frame, action, format, after_completion),
            Outcome::SelectedRegion { monitor, rect } => {
                self.rec.target = pages::record::TargetKind::Region;
                self.rec.region = Some((monitor, rect));
                self.page = Page::Record;
                self.last_capture_page = Page::Record;
            }
            Outcome::RecordArea { monitor, rect } => {
                self.request_recording(
                    ctx,
                    RecordTarget::Region(monitor, rect),
                    RecordProfile::Region,
                );
            }
        }
    }

    pub fn request_recording(
        &mut self,
        ctx: &egui::Context,
        target: RecordTarget,
        profile: RecordProfile,
    ) {
        if !self.recorder.is_idle() {
            return;
        }
        let hide =
            !matches!(target, RecordTarget::Audio) && self.settings.profile(profile).0.hide_to_tray;
        self.schedule(ctx, Pending::BeginRecording(target, profile), hide);
    }

    fn begin_recording(
        &mut self,
        ctx: &egui::Context,
        target: RecordTarget,
        profile: RecordProfile,
    ) {
        if !self.recorder.is_idle() {
            return;
        }
        self.refresh_mics();
        let default_mic = mediaa_platform::audio::default_input_device();
        let (r, anim) = self.settings.profile(profile);
        let fps = r.fps;
        let max_height = r.max_height();
        let anim = recording_animation_settings(r.output, fps, max_height, anim);
        let cursor = r.record_cursor;
        self.recording_region = match &target {
            RecordTarget::Region(monitor, rect) => Some((monitor.clone(), *rect)),
            RecordTarget::Audio | RecordTarget::Screens(_) | RecordTarget::Windows(_) => None,
        };
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
                vec![Box::new(move || {
                    screen::monitor_source(id, Some(rect), cursor)
                })]
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
            (
                r.output.supports_audio() && r.record_audio,
                &r.microphone,
                r.system_audio,
            )
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
            output: if audio_only {
                OutputKind::Audio(a.format)
            } else {
                r.output
            },
            fps,
            video_quality: r.video_quality,
            max_height,
            audio_in_every_file: r.audio_in_every_file,
            audio_kbps: a.kbps,
            anim,
            segmented_webp: r.segmented_webp,
            parallel_webp: r.parallel_webp,
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
                self.recorder = RecState::Recording {
                    session: Box::new(session),
                    events,
                };
                if !self.hidden_by_us {
                    self.info(t!("toast.recording_started"));
                }
            }
            Err(e) => {
                self.recording_region = None;
                self.show_window(ctx);
                self.error(format!("{e:#}"));
            }
        }
    }

    pub fn recording_page(&self) -> Page {
        match &self.recorder {
            RecState::Recording { session, .. }
                if matches!(session.output(), OutputKind::Audio(_)) =>
            {
                Page::RecordAudio
            }
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
        self.stop_recording_from_source(ctx, save, StopSource::Ui);
    }

    fn stop_recording_from_tray(&mut self, ctx: &egui::Context, save: bool) {
        self.stop_recording_from_source(ctx, save, StopSource::Tray);
    }

    fn stop_recording_from_hotkey(&mut self, ctx: &egui::Context, save: bool) {
        self.stop_recording_from_source(ctx, save, StopSource::Hotkey);
    }

    fn stop_recording_from_source(&mut self, ctx: &egui::Context, save: bool, source: StopSource) {
        self.recording_region = None;
        match std::mem::replace(&mut self.recorder, RecState::Idle) {
            RecState::Recording { session, events } => {
                let cancel = Arc::new(AtomicBool::new(false));
                let audio_only = matches!(session.output(), OutputKind::Audio(_));
                let sound_on_completion = recording_completion_sound(session.output(), source);
                session.stop(save, cancel.clone());
                self.recorder = RecState::Saving {
                    events,
                    progress: None,
                    started: Instant::now(),
                    cancel,
                    sound_on_completion,
                };
                if save {
                    self.page = if audio_only {
                        Page::RecordAudio
                    } else {
                        Page::Record
                    };
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
                    self.warning(w);
                }
                SessionEvent::Notice(notice) => {
                    let (summary, detail, trace_id) = match notice {
                        SessionNotice::WebpFallback { trace_id, detail } => (
                            t!("toast.webp_fallback").into_owned(),
                            format!(
                                "{}.\n\ntrace={trace_id}\n\n{detail}",
                                t!("toast.webp_fallback")
                            ),
                            trace_id,
                        ),
                    };
                    self.notices.push(
                        NoticeLevel::Warning,
                        summary,
                        detail,
                        Some(trace_id),
                        Vec::new(),
                        false,
                    );
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
                    let sound_on_completion = matches!(
                        &self.recorder,
                        RecState::Saving {
                            sound_on_completion: true,
                            ..
                        }
                    );
                    self.recorder = RecState::Idle;
                    self.recording_region = None;
                    self.lib.files_changed(&paths);
                    match self.settings.after_completion {
                        AfterCompletionAction::OpenLibrary => {
                            self.show_window(ctx);
                            let names: Vec<_> = paths
                                .iter()
                                .map(|p| {
                                    p.file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                })
                                .collect();
                            self.success(t!("toast.saved", names = names.join(", ")));
                            if !self.quit_after_save {
                                self.page = Page::Library;
                            }
                        }
                        AfterCompletionAction::CopyFile => {
                            let files: Vec<_> = paths.iter().map(PathBuf::as_path).collect();
                            match clipboard::copy_files(&files) {
                                Ok(()) => {
                                    if sound_on_completion {
                                        mediaa_platform::audio::confirmation_tone();
                                    }
                                    if !self.hidden_by_us {
                                        self.success(t!("toast.recording_copied"));
                                    }
                                }
                                Err(e) => {
                                    self.show_window(ctx);
                                    self.error(t!("err.copy_failed", error = format!("{e:#}")));
                                    if !self.quit_after_save {
                                        self.page = Page::Library;
                                    }
                                }
                            }
                        }
                    }
                }
                SessionEvent::Cancelled => {
                    self.recorder = RecState::Idle;
                    self.recording_region = None;
                    self.show_window(ctx);
                    self.info(t!("toast.discarded"));
                }
                SessionEvent::Failed(SessionFailure {
                    error,
                    recovery_files,
                    log_ref,
                }) => {
                    self.recorder = RecState::Idle;
                    self.recording_region = None;
                    self.show_window(ctx);
                    if recovery_files.is_empty() {
                        let detail = t!("toast.saving_failed", error = error).into_owned();
                        let summary = notice_summary(&detail, &t!("notice.error"));
                        self.notices.push(
                            NoticeLevel::Error,
                            summary,
                            detail,
                            log_ref,
                            Vec::new(),
                            false,
                        );
                    } else {
                        self.lib.files_changed(&recovery_files);
                        self.notices.push(
                            NoticeLevel::Error,
                            t!("toast.recovery_saved").into_owned(),
                            error,
                            log_ref,
                            recovery_files,
                            true,
                        );
                        self.quit_after_save = false;
                    }
                }
            }
        }
        if !matches!(self.recorder, RecState::Idle) {
            ctx.request_repaint_after(Duration::from_millis(250));
        } else if self.quit_after_save {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub fn record_target_from_ui(&self) -> Result<RecordTarget, String> {
        use pages::record::TargetKind;
        match self.rec.target {
            TargetKind::Screen => {
                let mut ms: Vec<_> = self
                    .monitors
                    .iter()
                    .filter(|m| self.rec.monitors.contains(&m.id))
                    .cloned()
                    .collect();
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
                    Err(String::new())
                } else {
                    Ok(RecordTarget::Windows(ws))
                }
            }
        }
    }

    pub fn start_recording_from_ui(&mut self, ctx: &egui::Context) {
        match self.record_target_from_ui() {
            Ok(t) => self.request_recording(ctx, t, RecordProfile::App),
            Err(e) if !e.is_empty() => self.error(e),
            Err(_) => {}
        }
    }

    pub fn start_audio_recording(&mut self, ctx: &egui::Context, profile: RecordProfile) {
        self.request_recording(ctx, RecordTarget::Audio, profile);
    }

    fn hotkey_screen(&mut self) -> Option<MonitorInfo> {
        self.refresh_monitors();
        let primary = self.monitors.first().cloned();
        match self.settings.hotkey_screen.clone() {
            ScreenChoice::Primary => primary,
            ScreenChoice::UnderCursor => mediaa_platform::cursor::position()
                .and_then(|(x, y)| screen::monitor_at(x, y))
                .and_then(|id| self.monitors.iter().find(|m| m.id == id).cloned())
                .or(primary),
            ScreenChoice::Display(device) => {
                match self.monitors.iter().find(|m| m.device == device) {
                    Some(m) => Some(m.clone()),
                    None => {
                        self.settings.hotkey_screen = ScreenChoice::Primary;
                        self.settings_changed();
                        primary
                    }
                }
            }
        }
    }

    fn start_hotkey_recording(&mut self, ctx: &egui::Context) {
        match self.hotkey_screen() {
            Some(m) => {
                self.request_recording(ctx, RecordTarget::Screens(vec![m]), RecordProfile::Hotkey)
            }
            None => self.error(t!("err.no_display")),
        }
    }

    fn handle_tray(&mut self, ctx: &egui::Context) {
        let cmds: Vec<_> = self
            .tray_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for cmd in cmds {
            match cmd {
                TrayCommand::PrimaryClick => {
                    let recording = matches!(self.recorder, RecState::Recording { .. });
                    if tray_click_stops_recording(
                        self.settings.stop_recording_on_left_click,
                        recording,
                    ) {
                        self.stop_recording_from_tray(ctx, true);
                    } else {
                        match self.settings.tray_left_click {
                            TrayClickAction::OpenWindow => self.show_window(ctx),
                            TrayClickAction::ScreenArea => self
                                .start_region_screenshot_with_completion(
                                    ctx,
                                    overlay::Purpose::Area,
                                    true,
                                ),
                        }
                    }
                }
                TrayCommand::ShowWindow => self.show_window(ctx),
                TrayCommand::RegionScreenshot => {
                    self.start_region_screenshot_with_completion(ctx, overlay::Purpose::Area, true)
                }
                TrayCommand::FullscreenScreenshot => {
                    self.start_fullscreen_screenshot_with_completion(ctx, None, true)
                }
                TrayCommand::StartRecording => self.start_hotkey_recording(ctx),
                TrayCommand::StartAudio => self.start_audio_recording(ctx, RecordProfile::Hotkey),
                TrayCommand::PauseResume => self.toggle_pause(),
                TrayCommand::StopAndSave => self.stop_recording_from_tray(ctx, true),
                TrayCommand::CancelRecording => self.stop_recording(ctx, false),
                TrayCommand::Quit => self.quit(ctx),
            }
        }
    }

    fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        let ids: Vec<u32> = self
            .hotkey_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for id in ids {
            let Some(action) = self.hotkeys.as_ref().and_then(|h| h.action_for(id)) else {
                continue;
            };
            match action {
                HotkeyAction::RegionScreenshot => {
                    self.start_region_screenshot(ctx, overlay::Purpose::Area)
                }
                HotkeyAction::FullscreenScreenshot => self.start_fullscreen_screenshot(ctx, None),
                HotkeyAction::StartVideo if self.recorder.is_idle() => {
                    self.start_hotkey_recording(ctx)
                }
                HotkeyAction::StartAudio if self.recorder.is_idle() => {
                    self.start_audio_recording(ctx, RecordProfile::Hotkey)
                }
                HotkeyAction::StartVideo | HotkeyAction::StartAudio => {}
                HotkeyAction::StopRecording => {
                    if matches!(self.recorder, RecState::Recording { .. }) {
                        self.stop_recording_from_hotkey(ctx, true);
                    }
                }
            }
        }
    }

    pub fn quit(&mut self, ctx: &egui::Context) {
        self.quit_after_save = true;
        if matches!(self.recorder, RecState::Recording { .. }) {
            self.stop_recording(ctx, true);
        } else if !matches!(self.recorder, RecState::Saving { .. }) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn update_tray(&mut self) {
        let state = recording_tray_state(&self.recorder);
        if let Some(t) = self.tray.as_mut() {
            t.set_state(state);
        }
    }

    fn show_recording_region(&mut self, ctx: &egui::Context) {
        if !self.settings.highlight_recording_region {
            self.region_outline.hide();
            return;
        }
        let (Some((monitor, rect)), RecState::Recording { session, .. }) =
            (&self.recording_region, &self.recorder)
        else {
            self.region_outline.hide();
            return;
        };
        let blink_on = ((ctx.input(|i| i.time) * 2.0) as u64).is_multiple_of(2);
        crate::region_marker::show(
            ctx,
            monitor,
            *rect,
            session.is_paused(),
            blink_on,
            self.settings.recording_region_outline_width,
            &mut self.region_outline,
        );
        ctx.request_repaint_after(Duration::from_millis(100));
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
        self.show_recording_region(ctx);

        let closing = ctx.input(|i| i.viewport().close_requested()) && !self.quit_after_save;
        if closing
            && close_goes_to_tray(
                self.settings.close_to_tray,
                self.tray.is_some(),
                !self.recorder.is_idle(),
            )
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.hide_window(ctx);
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
        if let Err(e) = self.settings.save() {
            log::error!("saving settings on exit: {e:#}");
        }
    }
}

fn close_goes_to_tray(close_to_tray: bool, has_tray: bool, busy: bool) -> bool {
    busy || (close_to_tray && has_tray)
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
        animation.fps = fps.clamp(1, 60);
        animation.max_height = max_height.unwrap_or(0);
    }
    animation
}

fn recording_completion_sound(_output: OutputKind, source: StopSource) -> bool {
    matches!(source, StopSource::Tray | StopSource::Hotkey)
}

fn recording_tray_state(recorder: &RecState) -> TrayState {
    match recorder {
        RecState::Idle => TrayState::Idle,
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
mod tests {
    use super::{
        close_goes_to_tray, notice_summary, page_from_args, recording_animation_settings,
        recording_completion_sound, recording_tray_state, recovery_links,
        tray_click_stops_recording, NoticeAction, NoticeLevel, Notices, Page, RecState,
        StartupAction, StopSource,
    };
    use eframe::egui;
    use egui_kittest::kittest::Queryable;
    use mediaa_core::{AnimFormat, OutputKind, Settings, VideoContainer};

    fn parse(args: &[&str]) -> StartupAction {
        StartupAction::from_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn closing_the_window_hides_to_the_tray_unless_turned_off() {
        assert!(
            close_goes_to_tray(true, true, false),
            "the default hides the window"
        );
        assert!(
            !close_goes_to_tray(false, true, false),
            "turned off: the app quits"
        );
        assert!(
            !close_goes_to_tray(true, false, false),
            "no tray icon: nothing to hide to"
        );
        assert!(
            close_goes_to_tray(false, true, true),
            "a recording keeps the app alive"
        );
        assert!(
            close_goes_to_tray(false, false, true),
            "a recording keeps the app alive without a tray"
        );
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
    fn tray_stays_in_processing_state_until_saving_finishes() {
        assert_eq!(
            recording_tray_state(&RecState::Idle),
            crate::tray::TrayState::Idle
        );
        let (_tx, events) = crossbeam_channel::unbounded();
        let saving = RecState::Saving {
            events,
            progress: None,
            started: std::time::Instant::now(),
            cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sound_on_completion: false,
        };
        assert_eq!(
            recording_tray_state(&saving),
            crate::tray::TrayState::Saving
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
    }

    #[test]
    fn page_from_args_picks_known_pages() {
        let args = |a: &str| vec!["mediaa".to_string(), a.to_string()];
        assert_eq!(page_from_args(&args("--page=record")), Some(Page::Record));
        assert_eq!(page_from_args(&args("--page=nope")), None);
        assert_eq!(page_from_args(&args("--area")), None);
    }
}
