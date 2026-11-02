use super::*;
use crate::features::capture::CaptureKind;
use crate::features::intent::{CaptureIntent, Intent};
use crate::features::region_editor;

impl App {
    pub(crate) fn handle_intents(&mut self, ctx: &egui::Context, intents: Vec<Intent>) {
        for intent in intents {
            self.handle_intent(ctx, intent);
        }
    }

    fn handle_intent(&mut self, ctx: &egui::Context, intent: Intent) {
        match intent {
            Intent::SettingsEdited => self.settings_changed(),
            Intent::RefreshCatalogs => self.refresh_catalogs(ctx),
            Intent::OpenLogs => self.open_path(ctx, &mediaa_core::paths::logs_dir()),
            Intent::OpenUrl(url) => ctx.open_url(egui::OpenUrl::new_tab(&url)),
            Intent::Capture(CaptureIntent::Region) => {
                self.start_region_screenshot(ctx, region_editor::Purpose::Screenshot)
            }
            Intent::Capture(CaptureIntent::Fullscreen(monitor)) => {
                self.start_fullscreen_screenshot(ctx, monitor)
            }
            Intent::Capture(CaptureIntent::Window(window)) => {
                self.start_window_screenshot(ctx, window)
            }
            Intent::RegionEditor { purpose } => self.start_region_screenshot(ctx, purpose),
            Intent::Notice { level, text } => self.push_notice(level, text),
            Intent::OpenPath(path) => self.open_path(ctx, &path),
            Intent::Reveal(path) => self.reveal_path(ctx, path),
            Intent::RenameMedia { path, title } => self.rename_media(ctx, path, title),
            Intent::LibraryRemoved(paths) => self.send_library_removed(&paths),
            Intent::ConversionStart(start) => self.send_conversion_start(start),
            Intent::OpenInConvert(paths) => self.open_in_convert_many(paths),
            Intent::PreviewOptionsChanged { .. } => self.preview_options_changed(),
            Intent::ReapplyHotkeys => self.reapply_hotkeys(),
            Intent::SuspendHotkeys => self.suspend_hotkeys(),
            Intent::StartRecording => self.start_recording_from_ui(ctx),
            Intent::StartAudioRecording { profile, hide } => {
                self.start_audio_recording(ctx, profile, hide)
            }
            Intent::ConversionUiAction(action) => self.apply_conversion_ui_action(action),
            Intent::AgentOp(request) => self.send_sync_command(
                mediaa_protocol::backend::AGENT_OP,
                mediaa_protocol::backend::encode_agent_op(&request),
            ),
            Intent::AgentRequest(request) => self.send_sync_command(
                mediaa_protocol::backend::AGENT_REQUEST,
                mediaa_protocol::backend::encode_agent_request(&request),
            ),
            Intent::AgentAnalyse(analyse) => self.send_sync_command(
                mediaa_protocol::backend::AGENT_ANALYSE,
                mediaa_protocol::backend::encode_agent_analyse(&analyse),
            ),
            Intent::AgentNames(names) => self.send_sync_command(
                mediaa_protocol::backend::AGENT_NAMES,
                mediaa_protocol::backend::encode_agent_names(&names),
            ),
            Intent::Command(command) => self.dispatch_command(ctx, command),
        }
    }

    pub(super) fn run_startup_action(&mut self, ctx: &egui::Context) {
        if self.startup == StartupAction::None || ctx.cumulative_frame_nr() < 2 {
            if self.startup != StartupAction::None {
                ctx.request_repaint();
            }
            return;
        }
        match std::mem::take(&mut self.startup) {
            StartupAction::None => {}
            StartupAction::Show => {}
            StartupAction::Area => {
                self.start_region_screenshot_from_hotkey(ctx, region_editor::Purpose::Area)
            }
            StartupAction::Fullscreen => self.start_fullscreen_screenshot(ctx, None),
            StartupAction::Tray => {
                if self.tray_available {
                    self.hide_window(ctx);
                    ctx.request_repaint_after(Duration::from_millis(500));
                }
            }
        }
    }

    pub(super) fn handle_overlay(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.overlay.as_ref().and_then(|o| o.show(ctx)) else {
            return;
        };
        let restore = self
            .overlay
            .as_ref()
            .is_some_and(|overlay| overlay.restore());
        let outcome = match outcome {
            Outcome::CaptureMonitor { index, monitor } => {
                self.queue_monitor_capture(ctx, CaptureKind::OverlayMonitor { index }, monitor.id);
                if !self.capture.capture_busy() {
                    if let Some(overlay) = self.overlay.as_ref() {
                        overlay.monitor_capture_failed();
                    }
                }
                return;
            }
            outcome => outcome,
        };
        self.overlay = None;
        if self.presence.take_ghost() {
            if let Some(w) = self.native_window {
                w.set_ghost(false);
            }
        }
        match outcome {
            Outcome::Cancel => {}
            Outcome::Image {
                frame,
                source,
                selection,
                shapes,
                action,
                format,
                after_completion,
            } => self.deliver_image(
                ctx,
                super::capture::CapturedImage {
                    frame,
                    source,
                    selection,
                    shapes,
                    action,
                    format,
                    after_completion,
                },
            ),
            Outcome::Saved { path, size } => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let library = crate::features::library::library_dir(&self.settings);
                if path.parent() == Some(library.as_path()) {
                    self.send_library_paths(std::slice::from_ref(&path));
                }
                self.success(t!("toast.shot_saved", size = size, name = name));
            }
            Outcome::SelectedRegion { monitor, rect } => {
                self.recording_view.ui.target = crate::features::recording::TargetKind::Region;
                self.recording_view.ui.region = Some((monitor, rect));
                self.page = Page::Record;
                self.last_capture_page = Page::Record;
            }
            Outcome::RecordArea { monitor, rect } => {
                self.presence.set_quiet(!restore);
                self.dispatch_command(
                    ctx,
                    AppCommand::Recording(RecordingCommand::Start {
                        target: RecordTarget::Region(monitor, rect),
                        profile: RecordProfile::Region,
                        hide: Some(false),
                    }),
                );
            }
            Outcome::CaptureMonitor { .. } => {}
        }
        if restore {
            self.show_window(ctx);
        }
    }

    pub(super) fn send_library_removed(&mut self, paths: &[std::path::PathBuf]) {
        let edits: Vec<mediaa_protocol::backend::LibraryEditWire> = paths
            .iter()
            .map(|path| mediaa_protocol::backend::LibraryEditWire::Delete {
                path: path.to_string_lossy().into_owned(),
            })
            .collect();
        for edit in edits {
            let id = self.next_command;
            self.next_command += 1;
            let frame = self.sync.command_frame(
                id,
                mediaa_protocol::backend::LIBRARY_EDIT,
                mediaa_protocol::backend::encode_library_edit(&edit),
            );
            self.send_sync_frame(frame);
        }
    }

    pub(super) fn send_library_renamed(&mut self, old: &std::path::Path, new: &std::path::Path) {
        self.send_library_removed(std::slice::from_ref(&old.to_path_buf()));
        self.send_library_paths(std::slice::from_ref(&new.to_path_buf()));
    }
    pub(super) fn send_library_paths(&mut self, paths: &[std::path::PathBuf]) {
        let paths: Vec<String> = paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let id = self.next_command;
        self.next_command += 1;
        let frame = self.sync.command_frame(
            id,
            mediaa_protocol::backend::LIBRARY_ADD,
            mediaa_protocol::backend::encode_library_add(&paths),
        );
        self.send_sync_frame(frame);
    }
    #[cfg(test)]
    pub(super) fn poll_conversion(&mut self) {
        let Some(outcome) = self.conversion.poll_job(&self.wake) else {
            return;
        };
        match outcome {
            features::conversion::ConversionOutcome::Finished(output) => {
                let name = output
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                let path = output.into_path();
                let library = crate::features::library::library_dir(&self.settings);
                if path.parent() == Some(library.as_path()) {
                    self.send_library_paths(&[path]);
                }
                self.success(t!("convert.done", name = name));
            }
            features::conversion::ConversionOutcome::Failed { kind, message } => {
                self.error_with_kind(kind, t!("convert.failed", error = message));
            }
            features::conversion::ConversionOutcome::Cancelled => {
                self.info(t!("convert.cancelled"));
            }
        }
    }

    pub(crate) fn apply_conversion_ui_action(
        &mut self,
        action: features::conversion::ConversionUiAction,
    ) {
        match action {
            features::conversion::ConversionUiAction::Error(error) => self.error(error),
        }
    }

    pub(super) fn has_active_work(&self) -> bool {
        !self.recording_view.is_idle() || self.has_other_active_work() || self.overlay.is_some()
    }

    fn has_other_active_work(&self) -> bool {
        self.background_processing() || self.capture.busy() || self.clipboard.is_busy()
    }

    fn background_processing(&self) -> bool {
        self.conversion.is_busy() || self.recovery_running || self.lib.cleanup_active()
    }

    pub(crate) fn record_target_from_ui(&self) -> Result<RecordTarget, String> {
        crate::features::recording::plan::target_from_ui(
            &self.settings,
            &self.recording_view.ui,
            &self.monitors,
            &self.windows,
        )
    }
    pub fn start_recording_from_ui(&mut self, ctx: &egui::Context) {
        self.presence.set_quiet(false);
        match self.record_target_from_ui() {
            Ok(t) => self.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Start {
                    target: t,
                    profile: RecordProfile::App,
                    hide: None,
                }),
            ),
            Err(e) if !e.is_empty() => self.error(e),
            Err(_) => {}
        }
    }

    pub fn start_audio_recording(
        &mut self,
        ctx: &egui::Context,
        profile: RecordProfile,
        hide: Option<bool>,
    ) {
        self.presence.set_quiet(hide == Some(false));
        self.dispatch_command(
            ctx,
            AppCommand::Recording(RecordingCommand::Start {
                target: RecordTarget::Audio,
                profile,
                hide,
            }),
        );
    }

    fn hotkey_screen(&mut self) -> Option<MonitorInfo> {
        if self.monitors.is_empty() {
            self.refresh_monitors();
        }
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

    fn start_hotkey_recording(&mut self, ctx: &egui::Context, hide: Option<bool>) {
        self.presence.set_quiet(hide == Some(false));
        match self.hotkey_screen() {
            Some(m) => self.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Start {
                    target: RecordTarget::Screens(vec![m]),
                    profile: RecordProfile::Hotkey,
                    hide,
                }),
            ),
            None => self.error(t!("err.no_display")),
        }
    }

    pub(super) fn handle_daemon_events(&mut self, ctx: &egui::Context) {
        let events: Vec<_> = self
            .daemon_events
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                crate::app::DaemonEvent::Sync(frame) => self.handle_sync_frame(ctx, frame),
            }
        }
    }

    fn handle_sync_frame(&mut self, ctx: &egui::Context, frame: mediaa_sync::Frame) {
        use mediaa_sync::ClientUpdate;
        let updates = self.sync.on_frame(frame);
        for update in updates {
            match update {
                ClientUpdate::State { store, payload, .. }
                    if store == mediaa_protocol::SETTINGS =>
                {
                    match mediaa_protocol::decode_settings(&payload) {
                        Ok(settings) => self.settings = settings,
                        Err(error) => log::warn!("settings payload: {error}"),
                    }
                }
                ClientUpdate::State { store, payload, .. }
                    if store == mediaa_protocol::RECORDING =>
                {
                    match mediaa_protocol::decode_recording(&payload) {
                        Ok(state) => {
                            let (view, page) = crate::protocol::recording_from_state(&state);
                            let active =
                                !matches!(view, crate::features::recording::RecordingView::Idle);
                            self.recording_view.apply(view, page);
                            if active {
                                self.page = page;
                            }
                        }
                        Err(error) => log::warn!("recording state payload: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::TRAY_EVENT =>
                {
                    match mediaa_protocol::decode_tray_command(&payload) {
                        Ok(command) => self.handle_tray_command(ctx, command),
                        Err(error) => log::warn!("tray event payload: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::HOTKEYS_EVENT =>
                {
                    match mediaa_protocol::decode_hotkeys(&payload) {
                        Ok(actions) => {
                            for action in crate::features::hotkeys::ordered_actions(actions) {
                                self.handle_hotkey_action(ctx, action);
                            }
                        }
                        Err(error) => log::warn!("hotkeys event payload: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::backend::PREVIEW_EVENT =>
                {
                    match mediaa_protocol::backend::decode_preview_changed(&payload) {
                        Ok(changed) => match changed.kind {
                            mediaa_protocol::backend::PreviewKindWire::Player => {}
                            mediaa_protocol::backend::PreviewKindWire::Thumbnail => {
                                match changed.state {
                                    mediaa_protocol::backend::PreviewStateWire::Ready { path } => {
                                        match image::open(&path) {
                                            Ok(image) => self.thumbs.backend_ready(
                                                std::path::Path::new(&changed.path),
                                                mediaa_core::Frame::from_image(image.to_rgba8()),
                                            ),
                                            Err(error) => {
                                                log::warn!("thumbnail {path}: {error}");
                                                self.thumbs.backend_failed(std::path::Path::new(
                                                    &changed.path,
                                                ));
                                            }
                                        }
                                    }
                                    mediaa_protocol::backend::PreviewStateWire::Failed {
                                        ..
                                    } => {
                                        self.thumbs
                                            .backend_failed(std::path::Path::new(&changed.path));
                                    }
                                    mediaa_protocol::backend::PreviewStateWire::Pending => {}
                                }
                            }
                            mediaa_protocol::backend::PreviewKindWire::Hover => {}
                        },
                        Err(error) => log::warn!("preview event: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::backend::AGENT_REQUEST_EVENT =>
                {
                    match serde_json::from_slice::<mediaa_protocol::backend::AgentResponseWire>(
                        &payload,
                    ) {
                        Ok(response) => crate::features::agent::requests::apply(response),
                        Err(error) => log::warn!("agent response: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::backend::AGENT_EVENT =>
                {
                    match mediaa_protocol::backend::decode_agent_event(&payload) {
                        Ok(event) => crate::ui::pages::agent::live_buffer::apply_event(&event),
                        Err(error) => log::warn!("agent event: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::backend::AGENT_SNAPSHOT =>
                {
                    match mediaa_protocol::backend::decode_agent_snapshot(&payload) {
                        Ok(snapshot) => {
                            crate::ui::pages::agent::live_buffer::apply_snapshot(snapshot)
                        }
                        Err(error) => log::warn!("agent snapshot: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::backend::AGENT_STATE =>
                {
                    match mediaa_protocol::backend::decode_agent_state(&payload) {
                        Ok(state) => crate::ui::pages::agent::live_buffer::apply_state(state),
                        Err(error) => log::warn!("agent state: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::backend::CONVERSION_EVENT =>
                {
                    match mediaa_protocol::backend::decode_conversion_state(&payload) {
                        Ok(state) => self.conversion.apply_backend_state(&state),
                        Err(error) => log::warn!("conversion event: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::RECOVERY_EVENT =>
                {
                    match mediaa_protocol::decode_recovery_report(&payload) {
                        Ok(report) => self.show_recovery(RecoveryReport {
                            files: report
                                .files
                                .into_iter()
                                .map(std::path::PathBuf::from)
                                .collect(),
                            failures: report.failures,
                        }),
                        Err(error) => log::warn!("recovery report payload: {error}"),
                    }
                }
                ClientUpdate::Event { kind, payload, .. }
                    if kind == mediaa_protocol::RECORDING_EVENT =>
                {
                    match mediaa_protocol::decode_recording_event(&payload) {
                        Ok(event) => {
                            log::debug!("recording event received: {event:?}");
                            self.handle_recording_event(ctx, event)
                        }
                        Err(error) => log::warn!("recording event payload: {error}"),
                    }
                }
                ClientUpdate::State { store, payload, .. } if store == mediaa_protocol::LIBRARY => {
                    match mediaa_protocol::backend::decode_library_page(&payload) {
                        Ok(page) => {
                            let dir = crate::features::library::library_dir(&self.settings);
                            let entries = page
                                .entries
                                .into_iter()
                                .filter_map(crate::protocol::library_entry_from_wire)
                                .collect();
                            self.lib.apply_snapshot(dir, entries, page.version);
                        }
                        Err(error) => log::warn!("library page: {error}"),
                    }
                }
                ClientUpdate::State { store, payload, .. } if store == mediaa_protocol::NOTICES => {
                    match mediaa_protocol::decode_notices(&payload) {
                        Ok(notices) => {
                            for notice in notices {
                                self.push_synced_notice(notice);
                            }
                        }
                        Err(error) => log::warn!("notices payload: {error}"),
                    }
                }
                ClientUpdate::State { store, payload, .. }
                    if store == mediaa_protocol::RECOVERY =>
                {
                    match mediaa_protocol::decode_recovery_state(&payload) {
                        Ok(state) => self.recovery_running = state.running,
                        Err(error) => log::warn!("recovery state payload: {error}"),
                    }
                }
                ClientUpdate::Resync => {
                    let frame = self.sync.subscribe_frame();
                    self.send_sync_frame(frame);
                }
                ClientUpdate::Ping => {
                    self.send_sync_frame(mediaa_sync::Frame::Pong);
                }
                ClientUpdate::Gap => {
                    let frame = self.sync.resync_frame();
                    self.send_sync_frame(frame);
                }
                ClientUpdate::Result {
                    ok: true,
                    payload,
                    id,
                    ..
                } if self.pending_agent_sends.remove(&id) => {
                    if let Ok(result) = mediaa_protocol::backend::decode_agent_send_result(&payload)
                    {
                        crate::ui::pages::agent::chat::set_agent_chat(result.chat);
                    }
                }
                ClientUpdate::Result {
                    ok: false,
                    error,
                    id,
                    ..
                } => {
                    self.pending_agent_sends.remove(&id);
                    log::warn!("sync command {id} failed: {error:?}");
                }
                ClientUpdate::Error { code, message } => {
                    log::warn!("sync error {code}: {message}");
                }
                _ => {}
            }
        }
    }

    #[cfg(not(test))]
    pub(super) fn send_library_removal(
        &mut self,
        request: crate::features::library::RemovalRequest,
    ) {
        use crate::features::library::RemovalRequest;
        let paths = match request {
            RemovalRequest::Selected { item, .. } => vec![item.path],
            RemovalRequest::SelectedMany(items) => {
                items.into_iter().map(|item| item.path).collect()
            }
            RemovalRequest::OlderThan(plan) => {
                plan.items.into_iter().map(|item| item.path).collect()
            }
        };
        let remove = mediaa_protocol::backend::LibraryRemoveWire {
            paths: paths
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            permanent: false,
        };
        self.send_sync_command(
            mediaa_protocol::backend::LIBRARY_REMOVE,
            mediaa_protocol::backend::encode_library_remove(&remove),
        );
    }

    pub(super) fn send_sync_command(&mut self, name: &str, payload: Vec<u8>) {
        let id = self.next_command;
        self.next_command += 1;
        let frame = self.sync.command_frame(id, name, payload);
        self.send_sync_frame(frame);
    }
    fn push_synced_notice(&mut self, notice: mediaa_protocol::NoticeWire) {
        if notice.id <= self.last_notice_id {
            return;
        }
        self.last_notice_id = notice.id;
        let level = match notice.level {
            mediaa_protocol::NoticeLevelWire::Info => crate::features::notices::NoticeLevel::Info,
            mediaa_protocol::NoticeLevelWire::Warning => {
                crate::features::notices::NoticeLevel::Warning
            }
            mediaa_protocol::NoticeLevelWire::Error => crate::features::notices::NoticeLevel::Error,
        };
        self.notices.push(
            level,
            notice.summary,
            notice.detail,
            None,
            Vec::new(),
            false,
        );
    }
    pub(super) fn poll_backend_previews(&mut self) {
        while let Ok(request) = self.preview_rx.try_recv() {
            self.send_preview_request(request);
        }
        while let Ok(request) = self.hover_preview_rx.try_recv() {
            self.send_preview_request(request);
        }
    }

    fn send_conversion_start(&mut self, start: mediaa_protocol::backend::ConversionStartWire) {
        let id = self.next_command;
        self.next_command += 1;
        let frame = self.sync.command_frame(
            id,
            mediaa_protocol::backend::CONVERSION_START,
            mediaa_protocol::backend::encode_conversion_start(&start),
        );
        self.send_sync_frame(frame);
    }
    pub(super) fn poll_agent_commands(&mut self) {
        use crate::ui::pages::agent::chat::AgentCommand;
        while let Ok(command) = self.agent_commands_rx.try_recv() {
            let (name, payload) = match command {
                AgentCommand::Send(send) => (
                    mediaa_protocol::backend::AGENT_SEND,
                    mediaa_protocol::backend::encode_agent_send(&send),
                ),
                AgentCommand::Steer(steer) => (
                    mediaa_protocol::backend::AGENT_STEER,
                    mediaa_protocol::backend::encode_agent_steer(&steer),
                ),
                AgentCommand::Stop(id) => (
                    mediaa_protocol::backend::AGENT_STOP,
                    mediaa_protocol::backend::encode_agent_task(
                        &mediaa_protocol::backend::AgentTaskWire { id },
                    ),
                ),
                AgentCommand::Resume(id) => (
                    mediaa_protocol::backend::AGENT_RESUME,
                    mediaa_protocol::backend::encode_agent_task(
                        &mediaa_protocol::backend::AgentTaskWire { id },
                    ),
                ),
            };
            let id = self.next_command;
            self.next_command += 1;
            if name == mediaa_protocol::backend::AGENT_SEND {
                self.pending_agent_sends.insert(id);
            }
            let frame = self.sync.command_frame(id, name, payload);
            self.send_sync_frame(frame);
        }
    }
    pub(super) fn poll_agent_requests(&mut self) {
        while let Ok(request) = self.agent_requests_rx.try_recv() {
            self.send_sync_command(
                mediaa_protocol::backend::AGENT_REQUEST,
                mediaa_protocol::backend::encode_agent_request(&request),
            );
        }
    }

    pub(super) fn poll_clipboard_commands(&mut self) {
        let Some(rx) = self.clipboard_rx.clone() else {
            return;
        };
        while let Ok(copy) = rx.try_recv() {
            let id = self.next_command;
            self.next_command += 1;
            let frame = self.sync.command_frame(
                id,
                mediaa_protocol::backend::CLIPBOARD_COPY,
                mediaa_protocol::backend::encode_clipboard(&copy),
            );
            self.send_sync_frame(frame);
        }
    }
    fn send_preview_request(&mut self, request: mediaa_protocol::backend::PreviewRequestWire) {
        let id = self.next_command;
        self.next_command += 1;
        let payload = mediaa_protocol::backend::encode_preview_request(&request);
        let frame = self
            .sync
            .command_frame(id, mediaa_protocol::backend::PREVIEW_ENSURE, payload);
        self.send_sync_frame(frame);
    }
    fn handle_recording_event(
        &mut self,
        ctx: &egui::Context,
        event: mediaa_protocol::RecordingEventWire,
    ) {
        match event {
            mediaa_protocol::RecordingEventWire::ShowWindow => self.show_window(ctx),
            mediaa_protocol::RecordingEventWire::ClearRegion => {
                self.recording_view.ui.clear_region()
            }
            mediaa_protocol::RecordingEventWire::FilesChanged(_) => {}
            mediaa_protocol::RecordingEventWire::Finished { paths, sound } => {
                let paths: Vec<_> = paths.into_iter().map(std::path::PathBuf::from).collect();
                self.finish_recording(ctx, paths, sound);
            }
        }
    }
    pub(super) fn send_sync_frame(&mut self, frame: mediaa_sync::Frame) {
        let Some(control) = self.daemon_control.as_mut() else {
            return;
        };
        if let Err(error) = control.send_frame(&frame) {
            log::warn!("sync send: {error}");
        }
    }

    pub(super) fn handle_tray_command(&mut self, ctx: &egui::Context, cmd: TrayCommand) {
        match cmd {
            TrayCommand::PrimaryClick => {
                let recording = self.recording_view.is_recording();
                if tray_click_stops_recording(self.settings.stop_recording_on_left_click, recording)
                {
                    self.dispatch_command(
                        ctx,
                        AppCommand::Recording(RecordingCommand::Stop {
                            action: StopAction::Save,
                            source: StopSource::Tray,
                        }),
                    );
                } else {
                    match self.settings.tray_left_click {
                        TrayClickAction::OpenWindow => self.show_window(ctx),
                        TrayClickAction::ScreenArea => self
                            .start_region_screenshot_with_completion(
                                ctx,
                                region_editor::Purpose::Area,
                                true,
                            ),
                    }
                }
            }
            TrayCommand::ShowWindow => {
                self.show_window(ctx);
                self.send_sync_command(mediaa_protocol::TRAY_SHOW_ACK, Vec::new());
            }
            TrayCommand::RegionScreenshot => self.start_region_screenshot_with_completion(
                ctx,
                region_editor::Purpose::Area,
                true,
            ),
            TrayCommand::FullscreenScreenshot => {
                self.start_fullscreen_screenshot_with_completion(ctx, None, true)
            }
            TrayCommand::StartRecording => self.start_hotkey_recording(ctx, Some(false)),
            TrayCommand::StartAudio => {
                self.start_audio_recording(ctx, RecordProfile::Hotkey, Some(false))
            }
            TrayCommand::PauseResume => {
                self.dispatch_command(ctx, AppCommand::Recording(RecordingCommand::TogglePause))
            }
            TrayCommand::StopAndSave => self.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Stop {
                    action: StopAction::Save,
                    source: StopSource::Tray,
                }),
            ),
            TrayCommand::Quit => self.dispatch_command(ctx, AppCommand::Quit),
        }
    }

    pub(crate) fn handle_hotkey_action(&mut self, ctx: &egui::Context, action: HotkeyAction) {
        match action {
            HotkeyAction::RegionScreenshot => {
                self.start_region_screenshot_from_hotkey(ctx, region_editor::Purpose::Area)
            }
            HotkeyAction::FullscreenScreenshot => self.start_fullscreen_screenshot(ctx, None),
            HotkeyAction::StartVideo if self.recording_view.is_idle() => {
                self.start_hotkey_recording(ctx, None)
            }
            HotkeyAction::StartAudio if self.recording_view.is_idle() => {
                self.start_audio_recording(ctx, RecordProfile::Hotkey, None)
            }
            HotkeyAction::StartVideo | HotkeyAction::StartAudio => {}
            HotkeyAction::StopRecording => {
                if self.recording_view.is_recording() || self.recording_view.is_starting() {
                    self.dispatch_command(
                        ctx,
                        AppCommand::Recording(RecordingCommand::Stop {
                            action: StopAction::Save,
                            source: StopSource::Hotkey,
                        }),
                    );
                }
            }
        }
    }
    pub fn quit(&mut self, ctx: &egui::Context) {
        let busy = self.has_active_work() || self.recording_view.is_recording();
        log::info!("quit requested; busy={busy}");
        self.window_event(ctx, WindowEvent::Quit { busy });
        #[cfg(test)]
        {
            self.recording.cancel_start();
            self.recording.cancel_finalize();
        }
        #[cfg(not(test))]
        {
            self.send_recording_command(crate::features::command::RecordingCommand::CancelStart);
            self.send_recording_command(crate::features::command::RecordingCommand::CancelFinalize);
        }
        if self.recording_view.is_recording() {
            self.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Stop {
                    action: StopAction::Save,
                    source: StopSource::Ui,
                }),
            );
        }
    }

    pub(super) fn update_tray(&mut self) {
        let state = if self.recording_view.is_idle() && self.background_processing() {
            TrayState::Saving
        } else {
            recording_tray_state(&self.recording_view)
        };
        let progress = self.settings.tray_progress;
        if self.tray_applied == Some((state, progress)) {
            return;
        }
        self.tray_applied = Some((state, progress));
        let update = mediaa_protocol::TrayUpdateWire { state, progress };
        self.send_sync_command(
            mediaa_protocol::TRAY_STATE,
            mediaa_protocol::encode_tray_update(&update),
        );
    }

    pub(super) fn show_recording_region(&mut self, ctx: &egui::Context) {
        if !self.settings.highlight_recording_region {
            self.region_outline.hide();
            return;
        }
        let Some((monitor, rect)) = self.recording_view.region() else {
            self.region_outline.hide();
            return;
        };
        if !self.recording_view.is_recording() {
            self.region_outline.hide();
            return;
        }
        let blink_on = ((ctx.input(|i| i.time) * 2.0) as u64).is_multiple_of(2);
        crate::ui::region_marker::show(
            ctx,
            monitor,
            *rect,
            self.recording_view.is_paused(),
            blink_on,
            self.settings.recording_region_outline_width,
            &mut self.region_outline,
        );
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    pub fn open_in_convert_many(&mut self, paths: Vec<PathBuf>) {
        if self.conversion.is_busy() {
            self.warning(t!("convert.busy"));
            return;
        }
        if paths.is_empty() {
            return;
        }
        let names: Vec<String> = paths
            .iter()
            .map(|path| {
                path.file_name().map_or_else(
                    || path.to_string_lossy().into_owned(),
                    |name| name.to_string_lossy().into_owned(),
                )
            })
            .collect();
        let summary = t!("convert.queued", count = names.len()).to_string();
        self.conversion.add_inputs(paths);
        self.notices.push(
            features::notices::NoticeLevel::Info,
            summary,
            names.join("\n"),
            None,
            Vec::new(),
            false,
        );
    }
}
