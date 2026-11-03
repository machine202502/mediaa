use crate::client::{Client, ClientEvent};
use crate::pages::{self, Actions, Ctx};
use crate::textures::Textures;
use crate::ui::theme;
use crate::ui_state::UiState;
use crate::view::BackendView;
use eframe::egui;
use mediaa2_core::navigation::Page;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::settings::Settings;
use mediaa2_protocol::state::{ConversionState, PickerState, RecordingState, RecoveryState, State};
use mediaa2_protocol::{ClientCommand, LibraryCommand, UiEvent, UiRequest, UiStartup};
use std::time::{Duration, Instant};

pub struct App {
    client: Client,
    view: BackendView,
    ui: UiState,
    textures: Textures,
    actions: Actions,
    monitors: Vec<mediaa2_core::screen::MonitorInfo>,
    windows: Vec<mediaa2_core::screen::WindowInfo>,
    microphones: Vec<String>,
    startup: UiStartup,
    ready_sent: bool,
    settings_started: bool,
    closing: bool,
    command_error: Option<String>,
    requested_library: Option<u64>,
    capture_excluded: bool,
    dev_action: Option<String>,
    dev_done: bool,
    dev_stop_at: Option<f64>,
    picker_window: bool,
    viewport_ready: bool,
    picker_pending: bool,
    picker_deadline: Option<Instant>,
    picker_visible: bool,
    picker_frames: u32,
}

impl App {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        client: Client,
        startup: UiStartup,
        page: Option<Page>,
    ) -> Self {
        client.set_context(cc.egui_ctx.clone());
        theme::install(&cc.egui_ctx);
        let mut ui = UiState::default();
        if let Some(page) = page {
            ui.select_page(page);
        }
        let settings = Settings::default();
        ui.settings.begin(settings.clone());
        ui.convert.output_dir = settings.convert_dir();
        theme::apply_preference(&cc.egui_ctx, settings.theme);
        let monitors = mediaa2_platform::screen::monitors().unwrap_or_default();
        let windows = mediaa2_platform::screen::windows().unwrap_or_default();
        let microphones = mediaa2_platform::audio::input_devices();
        Self {
            client,
            view: BackendView::new(default_state(settings)),
            ui,
            textures: Textures::default(),
            actions: Actions::default(),
            monitors,
            windows,
            microphones,
            startup,
            ready_sent: false,
            settings_started: false,
            closing: false,
            command_error: None,
            requested_library: None,
            capture_excluded: false,
            dev_action: std::env::var("MEDIAA2_DEV_ACTION").ok(),
            dev_done: false,
            dev_stop_at: None,
            picker_window: false,
            viewport_ready: false,
            picker_pending: false,
            picker_deadline: None,
            picker_visible: false,
            picker_frames: 0,
        }
    }

    fn handle_event(&mut self, ctx: &egui::Context, event: ClientEvent) {
        match event {
            ClientEvent::State(state) => {
                log::info!(
                    "ui: state revision={} notices={}",
                    state.library_revision,
                    state.notices.len()
                );
                self.view.apply(state);
                mediaa2_i18n::set_language(self.view.settings().language.as_deref(), None);
                theme::apply_preference(ctx, self.view.settings().theme);
                if !self.settings_started || !self.ui.settings.dirty {
                    self.ui.settings.begin(self.view.settings().clone());
                    self.settings_started = true;
                }
                if self.ui.convert.output_dir.as_os_str().is_empty() {
                    self.ui.convert.output_dir = self.view.settings().convert_dir();
                }
                if self.requested_library != Some(self.view.library_revision()) {
                    self.requested_library = Some(self.view.library_revision());
                    self.ui.library.query.offset = 0;
                    self.client
                        .send(ClientCommand::Library(LibraryCommand::Query(
                            self.ui.library.query.clone(),
                        )));
                }
                if self.view.picker().is_some() && self.ui.picker.is_none() {
                    if let Some(picker) = self.view.picker() {
                        let picker = picker.clone();
                        self.open_picker(ctx, picker);
                    }
                }
                if !self.dev_done {
                    if let Some(action) = self.dev_action.clone() {
                        self.dev_done = true;
                        match action.as_str() {
                            "shot" => {
                                if let Some(monitor) = self.monitors.first().cloned() {
                                    self.client.send(ClientCommand::Screenshot(
                                        mediaa2_protocol::ScreenshotCommand::Monitor {
                                            monitor,
                                            after:
                                                mediaa2_core::settings::AfterCapture::CopyAndSave,
                                        },
                                    ));
                                }
                            }
                            "record" => {
                                if let Some(monitor) = self.monitors.first().cloned() {
                                    self.client.send(ClientCommand::Recording(
                                        mediaa2_protocol::RecordingCommand::Start {
                                            target:
                                                mediaa2_core::recording::RecordTarget::Screens {
                                                    monitors: vec![monitor],
                                                },
                                            profile: mediaa2_core::settings::RecordProfile::App,
                                            hide: None,
                                        },
                                    ));
                                    self.dev_stop_at = Some(ctx.input(|input| input.time) + 3.0);
                                }
                            }
                            "convert" => {
                                if let Some(input) = std::env::var_os("MEDIAA2_DEV_INPUT") {
                                    self.client.send(ClientCommand::Conversion(
                                        mediaa2_protocol::ConversionCommand::Start {
                                            inputs: vec![std::path::PathBuf::from(input)],
                                            target:
                                                mediaa2_core::convert::ConversionOptions::default(),
                                            output_dir: self.view.settings().convert_dir(),
                                        },
                                    ));
                                }
                            }
                            "audio" => {
                                self.client.send(ClientCommand::Recording(
                                    mediaa2_protocol::RecordingCommand::Start {
                                        target: mediaa2_core::recording::RecordTarget::Audio,
                                        profile: mediaa2_core::settings::RecordProfile::App,
                                        hide: None,
                                    },
                                ));
                                self.dev_stop_at = Some(ctx.input(|input| input.time) + 3.0);
                            }
                            "picker" => {
                                self.client.send(ClientCommand::Screenshot(
                                    mediaa2_protocol::ScreenshotCommand::StartPicker,
                                ));
                            }
                            _ => {}
                        }
                    }
                }
            }
            ClientEvent::Result { id: _, result } => match result {
                Ok(mediaa2_protocol::CommandOk::LibraryPage(page)) => {
                    log::info!(
                        "ui: library page revision={} entries={}",
                        page.revision,
                        page.entries.len()
                    );
                    self.view.apply_library(page);
                }
                Ok(mediaa2_protocol::CommandOk::Shot(shot)) => {
                    let detail = match (&shot.path, shot.copied) {
                        (Some(path), true) => format!("{} · copied", path.display()),
                        (Some(path), false) => path.display().to_string(),
                        (None, true) => "copied to clipboard".to_string(),
                        (None, false) => "captured".to_string(),
                    };
                    self.actions
                        .notice(NoticeLevel::Success, "Screenshot", detail);
                    self.ui.library.query.offset = 0;
                    self.requested_library = None;
                    self.client
                        .send(ClientCommand::Library(LibraryCommand::Query(
                            self.ui.library.query.clone(),
                        )));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                    ctx.request_repaint();
                }
                Ok(mediaa2_protocol::CommandOk::PickerStarted { session }) => {
                    if self.ui.picker.is_none() {
                        if let Some(picker) = self.view.picker() {
                            let picker = picker.clone();
                            if picker.session == session {
                                self.open_picker(ctx, picker);
                            }
                        }
                    }
                }
                Ok(mediaa2_protocol::CommandOk::Cleanup(report)) => {
                    let (level, summary) = if report.failed > 0 {
                        (
                            NoticeLevel::Warning,
                            mediaa2_i18n::t!("lib.cleanup_failed_count", count = report.failed)
                                .to_string(),
                        )
                    } else {
                        (
                            NoticeLevel::Success,
                            mediaa2_i18n::t!("lib.cleanup_done_count", count = report.removed)
                                .to_string(),
                        )
                    };
                    self.actions.notice(level, summary, String::new());
                    self.requested_library = None;
                    self.client
                        .send(ClientCommand::Library(LibraryCommand::Refresh));
                }
                Ok(_) => {}
                Err(error) => {
                    self.command_error = Some(error.message.clone());
                    self.actions.notice(
                        NoticeLevel::Error,
                        error.message,
                        format!("{:?}", error.code),
                    );
                    if self.picker_pending {
                        self.cancel_pending_picker(ctx);
                    }
                }
            },
            ClientEvent::UiRequest(request) => match request {
                UiRequest::ShowWindow => {
                    let _ = mediaa2_platform::window::show_window("MediaA");
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                UiRequest::HideWindow => {
                    let _ = mediaa2_platform::window::hide_window("MediaA");
                }
                UiRequest::OpenPicker { session } => {
                    if self.ui.picker.is_none() {
                        if let Some(picker) = self.view.picker() {
                            let state = picker.clone();
                            if state.session == session {
                                self.open_picker(ctx, state);
                            }
                        }
                    }
                }
                UiRequest::Quit => {
                    self.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            },
            ClientEvent::Preview(preview) => {
                let ready = mediaa2_protocol::PreviewState::Ready(preview.file);
                if preview.kind == mediaa2_protocol::PreviewKind::Hover {
                    self.view.set_hover_preview(preview.path, ready);
                } else {
                    self.view.set_preview(preview.path, ready);
                }
            }
            ClientEvent::Reject(reason) => {
                if reason.contains("UiAlreadyOpen") {
                    self.closing = true;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.fail(ctx, format!("MediaA rejected the window: {reason}"));
                }
            }
            ClientEvent::Closed(error) => {
                if !self.closing {
                    self.fail(ctx, error);
                }
            }
        }
    }

    fn fail(&mut self, ctx: &egui::Context, message: String) {
        log::error!("ui: fatal client error: {message}");
        mediaa2_platform::dialogs::show_error("MediaA", &message);
        self.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn open_picker(&mut self, ctx: &egui::Context, picker: PickerState) {
        if self
            .ui
            .picker
            .as_ref()
            .is_some_and(|current| current.session == picker.session)
        {
            return;
        }
        self.picker_window = true;
        self.picker_pending = false;
        self.picker_deadline = None;
        self.picker_visible = false;
        self.picker_frames = 0;
        let _ = self.textures.get_or_load(ctx, &picker.image);
        self.ui.open_picker(picker_ui(&picker));
        let _ = mediaa2_platform::window::hide_window("MediaA");
    }

    fn finish_picker(&mut self, ctx: &egui::Context) {
        self.picker_window = false;
        self.picker_pending = false;
        self.picker_deadline = None;
        self.picker_visible = false;
        self.picker_frames = 0;
        self.ui.record.picking_region = false;
        ctx.send_viewport_cmd_to(picker_viewport_id(), egui::ViewportCommand::Close);
        if matches!(self.startup, UiStartup::Picker { .. }) {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            let _ = mediaa2_platform::window::show_window("MediaA");
        }
    }

    fn begin_picker(&mut self, ctx: &egui::Context) {
        self.picker_pending = true;
        self.picker_deadline = Some(Instant::now() + Duration::from_secs(6));
        self.picker_frames = 0;
        ctx.request_repaint();
    }

    fn cancel_pending_picker(&mut self, ctx: &egui::Context) {
        if !self.picker_pending {
            return;
        }
        self.picker_pending = false;
        self.picker_deadline = None;
        self.picker_visible = false;
        self.picker_frames = 0;
        if matches!(self.startup, UiStartup::Picker { .. }) {
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            let _ = mediaa2_platform::window::show_window("MediaA");
        }
    }

    fn schedule_screenshot(&self, ctx: &egui::Context, command: ClientCommand) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
        let sender = self.client.command_sender();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _ = sender.send(command);
            std::thread::sleep(Duration::from_millis(600));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let gui = ui.ctx().clone();
        if !self.viewport_ready {
            self.viewport_ready = true;
            if !matches!(self.startup, UiStartup::Picker { .. }) {
                gui.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
                gui.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
                if matches!(self.startup, UiStartup::Show) {
                    gui.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                }
            }
        }
        if self.picker_window && self.ui.picker.is_none() {
            self.finish_picker(&gui);
        }
        if self.picker_pending
            && self
                .picker_deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.cancel_pending_picker(&gui);
        }
        if self.ui.picker.is_some()
            && (self.view.picker().is_none()
                || gui.input(|input| input.key_pressed(egui::Key::Escape)))
        {
            if let Some(picker) = &self.ui.picker {
                self.client.send(ClientCommand::Picker(
                    mediaa2_protocol::PickerCommand::Cancel {
                        session: picker.session,
                    },
                ));
            }
            self.ui.close_picker();
            self.finish_picker(&gui);
        }
        if !self.capture_excluded {
            let capturable =
                std::env::var("MEDIAA2_DEV_CAPTURE_UI").is_ok_and(|value| value == "1");
            if !capturable {
                let _ = mediaa2_platform::window::exclude_from_capture("MediaA");
            }
            self.capture_excluded = true;
        }
        let events = self.client.drain();
        for event in events {
            self.handle_event(&gui, event);
        }
        if let Some(at) = self.dev_stop_at {
            if gui.input(|input| input.time) >= at {
                self.dev_stop_at = None;
                self.client.send(ClientCommand::Recording(
                    mediaa2_protocol::RecordingCommand::Stop {
                        action: mediaa2_core::recording::StopAction::Save,
                        source: mediaa2_core::recording::StopSource::Ui,
                    },
                ));
            }
        }
        if !self.ready_sent {
            self.ready_sent = self.client.send_ui(UiEvent::Ready);
        }

        if self.ui.picker.is_some() {
            if let Some(picker) = self.view.picker().cloned() {
                let id = picker_viewport_id();
                let builder = picker_viewport_builder(&picker.monitor, self.picker_visible);
                gui.show_viewport_immediate(id, builder, |ui, _class| {
                    let mut ctx = Ctx {
                        view: &self.view,
                        ui: &mut self.ui,
                        actions: &mut self.actions,
                        textures: &mut self.textures,
                        monitors: &self.monitors,
                        windows: &self.windows,
                        microphones: &self.microphones,
                        command_error: self.command_error.as_deref(),
                    };
                    pages::picker::show(&mut ctx, ui);
                });
                self.picker_frames = self.picker_frames.saturating_add(1);
                if self.picker_frames == 1 {
                    let _ = mediaa2_platform::window::disable_open_animation("MediaA Region");
                    let _ = mediaa2_platform::window::exclude_from_capture("MediaA Region");
                }
                if self.picker_frames >= 2 && !self.picker_visible {
                    self.picker_visible = true;
                    gui.send_viewport_cmd_to(id, egui::ViewportCommand::Visible(true));
                    gui.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
                    gui.request_repaint();
                } else if !self.picker_visible {
                    gui.request_repaint();
                }
                if self.ui.picker.is_none() {
                    self.finish_picker(&gui);
                } else {
                    let _ = mediaa2_platform::window::hide_window("MediaA");
                }
            }
        } else {
            let mut ctx = Ctx {
                view: &self.view,
                ui: &mut self.ui,
                actions: &mut self.actions,
                textures: &mut self.textures,
                monitors: &self.monitors,
                windows: &self.windows,
                microphones: &self.microphones,
                command_error: self.command_error.as_deref(),
            };
            pages::frame(&mut ctx, ui);
        }

        for command in self.actions.take_commands() {
            match &command {
                ClientCommand::Screenshot(mediaa2_protocol::ScreenshotCommand::StartPicker) => {
                    self.begin_picker(&gui);
                    self.client.send(command);
                }
                ClientCommand::Screenshot(
                    mediaa2_protocol::ScreenshotCommand::Monitor { .. }
                    | mediaa2_protocol::ScreenshotCommand::Window { .. },
                ) => {
                    self.schedule_screenshot(&gui, command);
                }
                _ => {
                    self.client.send(command);
                }
            }
        }
        if self.actions.take_refresh_catalogs() {
            self.monitors = mediaa2_platform::screen::monitors().unwrap_or_default();
            self.windows = mediaa2_platform::screen::windows().unwrap_or_default();
            self.microphones = mediaa2_platform::audio::input_devices();
        }
        if self.actions.quit_requested() {
            self.client.send(ClientCommand::Quit);
            self.closing = true;
            gui.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if gui.input(|input| input.viewport().close_requested()) && !self.closing {
            self.closing = true;
            if self.view.settings().close_to_tray {
                self.client.send_ui(UiEvent::Closing);
            } else {
                self.client.send(ClientCommand::Quit);
            }
            gui.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        if self.view.recording_active() || self.view.conversion_busy() {
            gui.request_repaint_after(Duration::from_millis(100));
        } else {
            gui.request_repaint_after(Duration::from_millis(500));
        }
    }
}

fn picker_viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("mediaa2-picker")
}

fn picker_viewport_builder(
    monitor: &mediaa2_core::screen::MonitorInfo,
    visible: bool,
) -> egui::ViewportBuilder {
    let scale = if monitor.scale > 0.5 {
        monitor.scale
    } else {
        1.0
    };
    egui::ViewportBuilder::default()
        .with_title("MediaA Region")
        .with_decorations(false)
        .with_transparent(false)
        .with_resizable(false)
        .with_visible(visible)
        .with_position(egui::pos2(
            monitor.x as f32 / scale,
            monitor.y as f32 / scale,
        ))
        .with_inner_size(egui::vec2(
            monitor.width as f32 / scale,
            monitor.height as f32 / scale,
        ))
        .with_taskbar(false)
        .with_active(false)
        .with_always_on_top()
}

fn picker_ui(state: &PickerState) -> crate::ui_state::PickerUi {
    crate::ui_state::PickerUi {
        session: state.session,
        tool: mediaa2_core::annotate::Tool::Rect,
        style: mediaa2_core::annotate::Style::new([255, 70, 82, 255], 4.0),
        shapes: Vec::new(),
        selection: None,
        text: None,
    }
}

fn default_state(settings: Settings) -> State {
    State {
        settings,
        recording: RecordingState::Idle,
        conversion: ConversionState::Idle,
        recovery: RecoveryState { running: false },
        notices: Vec::new(),
        picker: None,
        library_revision: 0,
    }
}

pub fn run_ui(args: &[String]) -> anyhow::Result<()> {
    mediaa2_core::logging::init_named(
        &mediaa2_core::paths::logs_dir(),
        "mediaa2-ui.log",
        log::LevelFilter::Info,
    );
    log::info!("MediaA 2 ui starting: {args:?}");
    mediaa2_platform::init();
    let startup = if args.iter().any(|arg| arg == "--picker") {
        UiStartup::Picker { session: 0 }
    } else if args.iter().any(|arg| arg == "--tray") {
        UiStartup::Tray
    } else {
        UiStartup::Show
    };
    let page = page_from_args(args);
    let scope = scope();
    let client = match Client::connect(&scope, startup) {
        Ok(client) => client,
        Err(error) => {
            mediaa2_platform::dialogs::show_error("MediaA", &error.to_string());
            return Ok(());
        }
    };
    let visible = matches!(startup, UiStartup::Show);
    let background = std::env::var("MEDIAA2_DEV_BACKGROUND").is_ok_and(|value| value == "1");
    let window_icon = mediaa2_platform::tray_icon::icon(crate::dev_agent(), 2);
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("MediaA")
        .with_app_id("mediaa")
        .with_inner_size([1120.0, 760.0])
        .with_min_inner_size([480.0, 480.0])
        .with_transparent(true)
        .with_visible(visible)
        .with_icon(std::sync::Arc::new(egui::IconData {
            rgba: window_icon.rgba,
            width: window_icon.width,
            height: window_icon.height,
        }));
    if background {
        viewport = viewport.with_position([-32000.0, -32000.0]);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    let result = eframe::run_native(
        "MediaA",
        options,
        Box::new(move |cc| Ok(Box::new(App::new(cc, client, startup, page)))),
    );
    result.map_err(|error| anyhow::anyhow!("ui: {error}"))
}

fn page_from_args(args: &[String]) -> Option<Page> {
    args.iter().find_map(|arg| {
        let value = arg.strip_prefix("--page=")?;
        match value {
            "screenshot" => Some(Page::Screenshot),
            "record" => Some(Page::Record),
            "audio" | "record_audio" => Some(Page::RecordAudio),
            "library" => Some(Page::Library),
            "convert" => Some(Page::Convert),
            "settings" => Some(Page::Settings),
            "about" => Some(Page::About),
            _ => None,
        }
    })
}

fn scope() -> String {
    match std::env::var_os("MEDIAA2_INSTANCE_ID") {
        Some(value) if !value.is_empty() => format!(
            "{:016x}",
            mediaa2_core::hash::stable_hash(value.to_string_lossy().as_bytes())
        ),
        _ => "default".to_string(),
    }
}
