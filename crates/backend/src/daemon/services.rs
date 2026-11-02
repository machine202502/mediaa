use super::*;

impl Daemon {
    pub(super) fn publish_library(&mut self, _paths: Vec<String>, _rescan: bool) {
        self.library_version += 1;
        let page = self
            .library
            .as_ref()
            .and_then(|store| store.snapshot().ok())
            .unwrap_or_else(|| mediaa_protocol::backend::LibraryPageWire {
                entries: Vec::new(),
                next_cursor: None,
                version: self.library_version,
            });
        let payload = mediaa_protocol::backend::encode_library_page(&page);
        self.sync
            .hub_mut()
            .publish(mediaa_protocol::LIBRARY, self.library_version, payload);
        self.flush_sync();
    }

    pub(super) fn poll_library(&mut self) {
        let Some(job) = self.library_job.as_mut() else {
            return;
        };
        match job.poll() {
            Some(Ok(Ok(store))) => {
                self.library = Some(store);
                self.library_job = None;
                self.publish_library(Vec::new(), true);
            }
            Some(Ok(Err(error))) => {
                self.library_job = None;
                log::warn!("library store: {error:#}");
            }
            Some(Err(error)) => {
                self.library_job = None;
                log::warn!("library index: {error:#}");
            }
            None => {}
        }
    }

    pub(super) fn index_library_paths(&mut self, paths: &[std::path::PathBuf]) {
        if let Some(store) = self.library.as_mut() {
            for path in paths {
                if let Err(error) = store.upsert_path(path) {
                    log::warn!("library index {}: {error:#}", path.display());
                }
            }
        }
        self.publish_library(
            paths
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            false,
        );
    }

    pub(super) fn poll_tray(&mut self) {
        self.tray_event(TrayUiEvent::Tick, Instant::now());
    }

    pub(super) fn poll_dev_tray(&mut self) {
        let Some(action) = self.dev_tray.clone() else {
            return;
        };
        if self.dev_tray_at.is_some_and(|at| Instant::now() >= at) {
            self.dev_tray = None;
            let command = match action.as_str() {
                "show" | "window" => TrayCommand::ShowWindow,
                "region" => TrayCommand::RegionScreenshot,
                "fullscreen" => TrayCommand::FullscreenScreenshot,
                "start" => TrayCommand::StartRecording,
                "audio" => TrayCommand::StartAudio,
                "pause" | "resume" => TrayCommand::PauseResume,
                "stop" => TrayCommand::StopAndSave,
                "quit" => TrayCommand::Quit,
                _ => TrayCommand::PrimaryClick,
            };
            log::info!("dev tray: {action}");
            self.handle_tray_command(command);
        }
    }

    pub(super) fn poll_dev_record(&mut self) {
        let Some(seconds) = self.dev_record else {
            return;
        };
        let now = Instant::now();
        if self.dev_record_started.is_none() {
            if self.dev_record_at.is_some_and(|at| now >= at) {
                match mediaa_platform::screen::monitors() {
                    Ok(monitors) => {
                        if let Some(monitor) = monitors.into_iter().next() {
                            log::info!("dev record: starting");
                            self.command_recording(RecordingCommandWire::Start {
                                target: RecordTargetWire::Region(
                                    monitor,
                                    Rect::new(0, 0, 320, 200),
                                ),
                                profile: RecordProfile::Region,
                                hide: Some(true),
                            });
                            self.dev_record_started = Some(now);
                        } else {
                            self.dev_record = None;
                        }
                    }
                    Err(error) => {
                        log::warn!("dev record monitors: {error:#}");
                        self.dev_record = None;
                    }
                }
            }
            return;
        }
        let started = self.dev_record_started.expect("dev record started");
        if now.duration_since(started) >= Duration::from_secs(seconds) {
            log::info!("dev record: stopping");
            self.command_recording(RecordingCommandWire::Stop {
                action: StopAction::Save,
                source: StopSourceWire::Ui,
            });
            self.dev_record = None;
        }
    }

    pub(super) fn command_recording(&mut self, command: RecordingCommandWire) {
        match command {
            RecordingCommandWire::Start {
                target, profile, ..
            } => {
                let target = crate::features::recording::wire::target_from_wire(target);
                let mut settings = self.settings.clone();
                if let Some(dir) = crate::dev_mode::mode().library.clone() {
                    settings.media_dir = dir;
                }
                let devices = mediaa_platform::audio::input_devices();
                let default = mediaa_platform::audio::default_input_device();
                let mut selection = RecordUi::default();
                let flags = StageFlags {
                    quiet: false,
                    hidden: false,
                    quit_when_idle: self.recording_quit_when_idle,
                };
                let effects = host::begin(
                    &mut self.recording,
                    &settings,
                    &devices,
                    default.as_deref(),
                    BeginRequest {
                        target,
                        profile,
                        now: Instant::now(),
                    },
                    flags,
                    &mut selection,
                );
                self.apply_recording_effects(effects);
            }
            RecordingCommandWire::Stop { action, source } => {
                let source = crate::features::recording::wire::source_from_wire(source);
                self.recording.stop(action, source);
            }
            RecordingCommandWire::TogglePause => self.recording.toggle_pause(),
            RecordingCommandWire::CancelStart => {
                self.recording.cancel_start();
            }
            RecordingCommandWire::CancelFinalize => self.recording.cancel_finalize(),
        }
        self.recording_dirty = true;
    }

    pub(super) fn poll_conversion(&mut self) {
        self.conversion.poll_probe(&self.conversion_wake);
        let mut state = mediaa_protocol::backend::ConversionStateWire {
            running: self.conversion.is_busy(),
            progress: None,
            done: 0,
            total: 0,
            error: None,
            finished: None,
        };
        if let Some(status) = self.conversion.status() {
            state.progress = status.progress;
            state.done = status.done;
            state.total = status.total;
        }
        if let Some(outcome) = self.conversion.poll_job(&self.conversion_wake) {
            match outcome {
                crate::features::conversion::ConversionOutcome::Finished(path) => {
                    state.finished = Some(path.to_string_lossy().into_owned());
                }
                crate::features::conversion::ConversionOutcome::Failed { message, .. } => {
                    state.error = Some(message);
                }
                crate::features::conversion::ConversionOutcome::Cancelled => {}
            }
            state.running = false;
        }
        let payload = mediaa_protocol::backend::encode_conversion_state(&state);
        self.push_event(mediaa_protocol::backend::CONVERSION_EVENT, payload);
    }

    pub(super) fn poll_previews(&mut self) {
        let Some(handle) = self.previews.as_ref() else {
            return;
        };
        let mut events = Vec::new();
        while let Some(event) = handle.try_event() {
            events.push(event);
        }
        let had_events = !events.is_empty();
        for event in events {
            let payload = mediaa_protocol::backend::encode_preview_changed(&event);
            self.sync
                .events_mut()
                .push(mediaa_protocol::backend::PREVIEW_EVENT, payload);
        }
        if had_events {
            self.flush_sync();
        }
    }

    pub(super) fn poll_recording(&mut self) {
        let flags = StageFlags {
            quiet: false,
            hidden: false,
            quit_when_idle: self.recording_quit_when_idle,
        };
        let effects = host::start_event(&mut self.recording, flags);
        self.apply_recording_effects(effects);
        let effects = host::poll_events(&mut self.recording, flags);
        self.apply_recording_effects(effects);
        let phase = self.recording.phase();
        if recording_state_due(self.recording_dirty, phase, self.recording_last_phase) {
            self.send_recording_state();
        }
    }

    pub(super) fn send_recording_state(&mut self) {
        self.recording_view.refresh(&self.recording);
        let state = crate::features::recording::wire::recording_state(
            &self.recording_view.view(),
            self.recording_view.page(),
        );
        self.recording_version += 1;
        let payload = mediaa_protocol::encode_recording(&state);
        self.sync
            .hub_mut()
            .publish(mediaa_protocol::RECORDING, self.recording_version, payload);
        self.recording_last_phase = Some(self.recording.phase());
        self.recording_dirty = false;
        self.flush_sync();
    }

    pub(super) fn push_recording_event(&mut self, event: mediaa_protocol::RecordingEventWire) {
        log::debug!("recording event pushed: {event:?}");
        let payload = mediaa_protocol::encode_recording_event(&event);
        self.sync
            .events_mut()
            .push(mediaa_protocol::RECORDING_EVENT, payload);
        self.flush_sync();
    }

    pub(super) fn apply_recording_effects(
        &mut self,
        effects: Vec<crate::features::recording::host::RecordingEffect>,
    ) {
        use crate::features::recording::host::RecordingEffect;
        for effect in effects {
            match effect {
                RecordingEffect::ShowWindow => {
                    self.push_recording_event(mediaa_protocol::RecordingEventWire::ShowWindow);
                }
                RecordingEffect::ClearRegion => {
                    self.push_recording_event(mediaa_protocol::RecordingEventWire::ClearRegion);
                }
                RecordingEffect::SetPage(_) => {}
                RecordingEffect::Info(text) => {
                    self.note(ipc::NoticeLevel::Info, notice_summary(&text, "info"), text)
                }
                RecordingEffect::Warning(text) => self.note(
                    ipc::NoticeLevel::Warning,
                    notice_summary(&text, "warning"),
                    text,
                ),
                RecordingEffect::Error(text) => self.note(
                    ipc::NoticeLevel::Error,
                    notice_summary(&text, "error"),
                    text,
                ),
                RecordingEffect::ErrorWithKind { kind: _, message } => self.note(
                    ipc::NoticeLevel::Error,
                    notice_summary(&message, "error"),
                    message,
                ),
                RecordingEffect::StartRecovery => self.start_recovery(),
                RecordingEffect::Notice {
                    level,
                    summary,
                    detail,
                    ..
                } => self.note(notice_level(level), summary, detail),
                RecordingEffect::FilesChanged(paths) => {
                    self.index_library_paths(&paths);
                    self.push_recording_event(mediaa_protocol::RecordingEventWire::FilesChanged(
                        paths
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect(),
                    ));
                }
                RecordingEffect::StopRecording { action, source } => {
                    self.recording.stop(action, source);
                }
                RecordingEffect::Finished { paths, sound } => {
                    log::info!("recording finished: {paths:?}");
                    self.index_library_paths(&paths);
                    self.push_recording_event(mediaa_protocol::RecordingEventWire::Finished {
                        paths: paths
                            .iter()
                            .map(|path| path.display().to_string())
                            .collect(),
                        sound,
                    });
                }
                RecordingEffect::QuitWhenIdle(value) => {
                    self.recording_quit_when_idle = value;
                }
            }
        }
    }

    pub(super) fn start_recovery(&mut self) {
        if self.recovery.is_some() {
            return;
        }
        self.recovery = spawn_recovery(&self.settings);
        if self.recovery.is_some() {
            self.publish_recovery();
        }
    }

    pub(super) fn poll_recovery(&mut self) {
        let Some(job) = self.recovery.as_mut() else {
            return;
        };
        match job.poll() {
            Some(Ok(report)) => {
                self.recovery = None;
                self.publish_recovery();
                self.push_recovery_report(report);
                self.send_recording_state();
            }
            Some(Err(error)) => {
                self.recovery = None;
                self.publish_recovery();
                self.note(
                    ipc::NoticeLevel::Error,
                    "recording recovery",
                    format!("{error:#}"),
                );
            }
            None => {}
        }
    }
}
