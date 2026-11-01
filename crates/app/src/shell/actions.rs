use super::*;
use crate::logic::capture::CaptureKind;
use crate::logic::intent::{CaptureIntent, Intent};
use crate::logic::region_editor;

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
            Intent::Capture(CaptureIntent::Window(id)) => self.start_window_screenshot(ctx, id),
            Intent::RegionEditor { purpose } => self.start_region_screenshot(ctx, purpose),
            Intent::Notice { level, text } => self.push_notice(level, text),
            Intent::OpenPath(path) => self.open_path(ctx, &path),
            Intent::Reveal(path) => self.reveal_path(ctx, path),
            Intent::RenameMedia { path, title } => self.rename_media(ctx, path, title),
            Intent::OpenInConvert(paths) => self.open_in_convert_many(paths),
            Intent::PreviewOptionsChanged { .. } => self.preview_options_changed(),
            Intent::ReapplyHotkeys => self.reapply_hotkeys(),
            Intent::SuspendHotkeys => self.suspend_hotkeys(),
            Intent::StartRecording => self.start_recording_from_ui(ctx),
            Intent::StartAudioRecording { profile, hide } => {
                self.start_audio_recording(ctx, profile, hide)
            }
            Intent::ConversionUiAction(action) => self.apply_conversion_ui_action(action),
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
            StartupAction::Area => {
                self.start_region_screenshot_from_hotkey(ctx, region_editor::Purpose::Area)
            }
            StartupAction::Fullscreen => self.start_fullscreen_screenshot(ctx, None),
            StartupAction::Tray => {
                if self.tray.is_some() {
                    self.hide_window(ctx);
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
                selection,
                shapes,
                action,
                format,
                after_completion,
            } => self.deliver_image(
                ctx,
                super::capture::CapturedImage {
                    frame,
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
                self.lib.files_changed(std::slice::from_ref(&path));
                self.success(t!("toast.shot_saved", size = size, name = name));
            }
            Outcome::SelectedRegion { monitor, rect } => {
                self.recording.ui.target = crate::logic::recording::TargetKind::Region;
                self.recording.ui.region = Some((monitor, rect));
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

    pub(super) fn poll_conversion(&mut self) {
        let Some(outcome) = self.conversion.poll_job(&self.wake) else {
            return;
        };
        match outcome {
            logic::conversion::ConversionOutcome::Finished(output) => {
                let name = output
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.lib.files_changed(&[output.into_path()]);
                self.success(t!("convert.done", name = name));
            }
            logic::conversion::ConversionOutcome::Failed { kind, message } => {
                self.error_with_kind(kind, t!("convert.failed", error = message));
            }
            logic::conversion::ConversionOutcome::Cancelled => {
                self.info(t!("convert.cancelled"));
            }
        }
    }

    pub(crate) fn apply_conversion_ui_action(
        &mut self,
        action: logic::conversion::ConversionUiAction,
    ) {
        match action {
            logic::conversion::ConversionUiAction::Error(error) => self.error(error),
        }
    }

    pub(super) fn has_active_work(&self) -> bool {
        !self.recording.is_idle() || self.has_other_active_work()
    }

    fn has_other_active_work(&self) -> bool {
        self.background_processing() || self.capture.busy() || self.clipboard.is_busy()
    }

    fn background_processing(&self) -> bool {
        self.conversion.is_busy() || self.recovery_job.is_some() || self.lib.cleanup_active()
    }

    pub(crate) fn record_target_from_ui(&self) -> Result<RecordTarget, String> {
        crate::logic::recording::plan::target_from_ui(
            &self.settings,
            &self.recording,
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

    pub(super) fn handle_tray(&mut self, ctx: &egui::Context) {
        let cmds: Vec<_> = self
            .tray_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for cmd in cmds {
            match cmd {
                TrayCommand::PrimaryClick => {
                    let recording = self.recording.is_recording();
                    if tray_click_stops_recording(
                        self.settings.stop_recording_on_left_click,
                        recording,
                    ) {
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
                TrayCommand::ShowWindow => self.show_window(ctx),
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
    }

    pub(super) fn handle_hotkeys(&mut self, ctx: &egui::Context) {
        let ids: Vec<u32> = self
            .hotkey_rx
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for id in ids {
            let actions = crate::logic::hotkeys::ordered_actions(
                self.hotkeys
                    .as_ref()
                    .map(|h| h.actions_for(id))
                    .unwrap_or_default(),
            );
            for action in actions {
                self.handle_hotkey_action(ctx, action);
            }
        }
    }

    pub(crate) fn handle_hotkey_action(&mut self, ctx: &egui::Context, action: HotkeyAction) {
        match action {
            HotkeyAction::RegionScreenshot => {
                self.start_region_screenshot_from_hotkey(ctx, region_editor::Purpose::Area)
            }
            HotkeyAction::FullscreenScreenshot => self.start_fullscreen_screenshot(ctx, None),
            HotkeyAction::StartVideo if self.recording.is_idle() => {
                self.start_hotkey_recording(ctx, None)
            }
            HotkeyAction::StartAudio if self.recording.is_idle() => {
                self.start_audio_recording(ctx, RecordProfile::Hotkey, None)
            }
            HotkeyAction::StartVideo | HotkeyAction::StartAudio => {}
            HotkeyAction::StopRecording => {
                if self.recording.is_recording() || self.recording.is_starting() {
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
        self.quit_when_idle = true;
        self.recording.cancel_start();
        self.recording.cancel_finalize();
        if self.recording.is_recording() {
            self.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Stop {
                    action: StopAction::Save,
                    source: StopSource::Ui,
                }),
            );
        } else if !self.has_active_work() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    pub(super) fn update_tray(&mut self) {
        let state = if self.recording.is_idle() && self.background_processing() {
            TrayState::Saving
        } else {
            recording_tray_state(&self.recording)
        };
        if let Some(t) = self.tray.as_mut() {
            t.set_state(state, self.settings.tray_progress);
        }
    }

    pub(super) fn show_recording_region(&mut self, ctx: &egui::Context) {
        if !self.settings.highlight_recording_region {
            self.region_outline.hide();
            return;
        }
        let Some((monitor, rect)) = self.recording.region() else {
            self.region_outline.hide();
            return;
        };
        if !self.recording.is_recording() {
            self.region_outline.hide();
            return;
        }
        let blink_on = ((ctx.input(|i| i.time) * 2.0) as u64).is_multiple_of(2);
        crate::ui::region_marker::show(
            ctx,
            monitor,
            *rect,
            self.recording.is_paused(),
            blink_on,
            self.settings.recording_region_outline_width,
            &mut self.region_outline,
        );
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    pub fn open_in_convert(&mut self, path: PathBuf) {
        self.open_in_convert_many(vec![path]);
    }

    pub fn open_in_convert_many(&mut self, paths: Vec<PathBuf>) {
        if self.conversion.is_busy() {
            self.warning(t!("convert.busy"));
            return;
        }
        self.conversion.add_inputs(paths);
        self.page = Page::Convert;
    }
}
