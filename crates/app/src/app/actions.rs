use super::*;
use crate::features::capture::CaptureKind;

impl App {
    pub(super) fn run_startup_action(&mut self, ctx: &egui::Context) {
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
            StartupAction::Tray => {
                if self.tray.is_some() {
                    self.hide_window(ctx);
                }
            }
        }
    }

    pub(super) fn handle_overlay(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.capture.overlay.as_ref().and_then(|o| o.show(ctx)) else {
            return;
        };
        let outcome = match outcome {
            Outcome::CaptureMonitor { index, monitor } => {
                self.queue_monitor_capture(ctx, CaptureKind::OverlayMonitor { index }, monitor.id);
                return;
            }
            outcome => outcome,
        };
        self.capture.overlay = None;
        if std::mem::take(&mut self.ghosted) {
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
            Outcome::SelectedRegion { monitor, rect } => {
                self.recording.ui.target = crate::features::recording::TargetKind::Region;
                self.recording.ui.region = Some((monitor, rect));
                self.page = Page::Record;
                self.last_capture_page = Page::Record;
            }
            Outcome::RecordArea { monitor, rect } => {
                self.dispatch_command(
                    ctx,
                    AppCommand::Recording(RecordingCommand::Start {
                        target: RecordTarget::Region(monitor, rect),
                        profile: RecordProfile::Region,
                    }),
                );
            }
            Outcome::CaptureMonitor { .. } => {}
        }
    }

    pub(super) fn poll_conversion(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.conversion.poll_job(ctx) else {
            return;
        };
        match outcome {
            features::conversion::ConversionOutcome::Finished(output) => {
                let name = output
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                self.lib.files_changed(&[output.into_path()]);
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
        ctx: &egui::Context,
        action: features::conversion::ConversionUiAction,
    ) {
        match action {
            features::conversion::ConversionUiAction::Open(path) => self.open_path(ctx, &path),
            features::conversion::ConversionUiAction::Reveal(path) => self.reveal_path(ctx, path),
            features::conversion::ConversionUiAction::Error(error) => self.error(error),
        }
    }

    pub(super) fn has_active_work(&self) -> bool {
        !self.recording.is_idle() || self.has_other_active_work()
    }

    fn has_other_active_work(&self) -> bool {
        self.conversion.is_busy()
            || self.recovery_job.is_some()
            || self.lib.cleanup_active()
            || self.capture.busy()
            || self.clipboard.is_busy()
    }

    pub(crate) fn record_target_from_ui(&self) -> Result<RecordTarget, String> {
        use crate::features::recording::TargetKind;
        match self.recording.ui.target {
            TargetKind::Screen => {
                let mut ms: Vec<_> = self
                    .monitors
                    .iter()
                    .filter(|m| self.recording.ui.monitors.contains(&m.id))
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
                .recording
                .ui
                .region
                .clone()
                .map(|(m, r)| RecordTarget::Region(m, r))
                .ok_or_else(|| t!("err.select_area").into()),
            TargetKind::Windows => {
                let ws: Vec<_> = self
                    .windows
                    .iter()
                    .filter(|w| self.recording.ui.windows.contains(&w.id))
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
            Ok(t) => self.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Start {
                    target: t,
                    profile: RecordProfile::App,
                }),
            ),
            Err(e) if !e.is_empty() => self.error(e),
            Err(_) => {}
        }
    }

    pub fn start_audio_recording(&mut self, ctx: &egui::Context, profile: RecordProfile) {
        self.dispatch_command(
            ctx,
            AppCommand::Recording(RecordingCommand::Start {
                target: RecordTarget::Audio,
                profile,
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

    fn start_hotkey_recording(&mut self, ctx: &egui::Context) {
        match self.hotkey_screen() {
            Some(m) => self.dispatch_command(
                ctx,
                AppCommand::Recording(RecordingCommand::Start {
                    target: RecordTarget::Screens(vec![m]),
                    profile: RecordProfile::Hotkey,
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
                TrayCommand::CancelRecording => self.dispatch_command(
                    ctx,
                    AppCommand::Recording(RecordingCommand::Stop {
                        action: StopAction::Discard,
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
            let Some(action) = self.hotkeys.as_ref().and_then(|h| h.action_for(id)) else {
                continue;
            };
            match action {
                HotkeyAction::RegionScreenshot => {
                    self.start_region_screenshot(ctx, overlay::Purpose::Area)
                }
                HotkeyAction::FullscreenScreenshot => self.start_fullscreen_screenshot(ctx, None),
                HotkeyAction::StartVideo if self.recording.is_idle() => {
                    self.start_hotkey_recording(ctx)
                }
                HotkeyAction::StartAudio if self.recording.is_idle() => {
                    self.start_audio_recording(ctx, RecordProfile::Hotkey)
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
    }

    pub fn quit(&mut self, ctx: &egui::Context) {
        self.quit_when_idle = true;
        self.conversion.cancel_estimate();
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
        let state = if self.recording.is_idle() && self.has_other_active_work() {
            TrayState::Saving
        } else {
            recording_tray_state(&self.recording)
        };
        if let Some(t) = self.tray.as_mut() {
            t.set_state(state);
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
        crate::region_marker::show(
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
        if self.conversion.is_busy() {
            self.warning(t!("convert.busy"));
            return;
        }
        self.conversion.set_input(path);
        self.page = Page::Convert;
    }
}
