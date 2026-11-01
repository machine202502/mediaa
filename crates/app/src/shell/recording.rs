use super::*;
use crate::adapters::clipboard_worker::Purpose;
use crate::logic::recording::{plan, PlanError};
use crate::logic::recording::{RecordTarget, RecordingEvent, RecordingStartEvent, StopSource};

impl App {
    pub(crate) fn dispatch_command(&mut self, ctx: &egui::Context, command: AppCommand) {
        match command {
            AppCommand::Recording(command) => self.dispatch_recording_command(ctx, command),
            AppCommand::Quit => self.quit(ctx),
        }
    }

    fn dispatch_recording_command(&mut self, ctx: &egui::Context, command: RecordingCommand) {
        match command {
            RecordingCommand::Start {
                target,
                profile,
                hide,
            } => self.request_recording(ctx, target, profile, hide),
            RecordingCommand::Stop { action, source } => {
                self.stop_recording_from_source(ctx, action, source)
            }
            RecordingCommand::TogglePause => self.recording.toggle_pause(),
        }
    }

    pub(super) fn request_recording(
        &mut self,
        ctx: &egui::Context,
        target: RecordTarget,
        profile: RecordProfile,
        hide: Option<bool>,
    ) {
        if !self.recording.is_idle() {
            return;
        }
        if self.recovery_active() {
            self.show_window(ctx);
            self.warning(t!("record.recovery_wait"));
            return;
        }
        let hide = hide.unwrap_or_else(|| {
            !matches!(target, RecordTarget::Audio) && self.settings.profile(profile).0.hide_to_tray
        });
        self.schedule(ctx, Pending::BeginRecording(target, profile), hide);
    }

    pub(super) fn begin_recording(
        &mut self,
        ctx: &egui::Context,
        target: RecordTarget,
        profile: RecordProfile,
    ) {
        if !self.recording.is_idle() || self.recovery_job.is_some() {
            return;
        }
        if self.mic_devices.is_empty() {
            self.refresh_mics();
        }
        let planned = plan(
            &self.settings,
            &target,
            profile,
            &self.mic_devices,
            self.default_mic_device.as_deref(),
        );
        let plan = match planned {
            Ok(plan) => plan,
            Err(PlanError::Unavailable(label)) => {
                self.recording.clear_region();
                if !self.presence.is_quiet() {
                    self.show_window(ctx);
                }
                self.error(t!("toast.target_unavailable", name = label));
                return;
            }
            Err(PlanError::NoAudioSource) => {
                self.recording.clear_region();
                self.error(t!("audio.no_source"));
                return;
            }
            Err(PlanError::Request(error)) => {
                self.recording.clear_region();
                if !self.presence.is_quiet() {
                    self.show_window(ctx);
                }
                self.error(t!("toast.recording_start_failed", error = error));
                return;
            }
        };
        if let Some(label) = plan.allowed_warning {
            self.warning(t!("toast.source_allowed", name = label));
        }
        self.recording.set_region_for_target(&target);
        let page = if plan.audio_only {
            Page::RecordAudio
        } else {
            Page::Record
        };
        if plan.audio_only {
            self.page = page;
        }
        if let Err(error) = self.recording.start(plan.request, Instant::now(), page) {
            self.recording.clear_region();
            if !self.presence.is_quiet() {
                self.show_window(ctx);
            }
            self.error(t!("toast.recording_start_failed", error = error));
        }
    }

    pub fn recording_page(&self) -> Page {
        self.recording.page()
    }

    fn stop_recording_from_source(
        &mut self,
        ctx: &egui::Context,
        action: StopAction,
        source: StopSource,
    ) {
        if let Some(result) = self.recording.stop(action, source) {
            if action == StopAction::Save {
                self.page = result.page;
            }
        }
        ctx.request_repaint();
    }

    pub(super) fn poll_recording_start(&mut self, ctx: &egui::Context) {
        let Some(event) = self.recording.poll_start(self.quit_when_idle) else {
            if self.recording.is_starting() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            return;
        };
        match event {
            RecordingStartEvent::Started {
                samples,
                discarded: false,
            } => {
                log::info!("recording started: {samples:?}");
                if !self.presence.is_hidden() {
                    self.info(t!("toast.recording_started"));
                }
            }
            RecordingStartEvent::Started {
                discarded: true, ..
            } => {}
            RecordingStartEvent::Cancelled => {
                if !self.quit_when_idle {
                    if !self.presence.is_quiet() {
                        self.show_window(ctx);
                    }
                    self.info(t!("toast.discarded"));
                }
            }
            RecordingStartEvent::Failed { kind, message } => {
                if !self.presence.is_quiet() {
                    self.show_window(ctx);
                }
                self.start_recovery();
                self.error_with_kind(kind, message);
            }
            RecordingStartEvent::WorkerStopped => {
                if !self.presence.is_quiet() {
                    self.show_window(ctx);
                }
                self.start_recovery();
                self.error(t!("toast.recording_worker_stopped"));
            }
        }
    }

    pub(super) fn poll_recorder(&mut self, ctx: &egui::Context) {
        for event in self.recording.poll_events() {
            match event {
                RecordingEvent::Warning(warning) => self.warning(warning),
                RecordingEvent::SessionEvent(SessionEvent::Notice(
                    SessionNotice::WebpFallback { trace_id, detail },
                )) => {
                    let summary = t!("toast.webp_fallback").into_owned();
                    let detail = format!(
                        "{}.\n\ntrace={trace_id}\n\n{detail}",
                        t!("toast.webp_fallback")
                    );
                    self.notices.push(
                        NoticeLevel::Warning,
                        summary,
                        detail,
                        Some(trace_id),
                        Vec::new(),
                        false,
                    );
                }
                RecordingEvent::SessionEvent(SessionEvent::CaptureFailed(error)) => {
                    self.error(t!("toast.recording_stopped", error = error));
                    self.stop_recording_from_source(ctx, StopAction::Save, StopSource::Ui);
                }
                RecordingEvent::SessionEvent(SessionEvent::SourceLost(name)) => {
                    self.warning(t!("toast.source_lost", name = name));
                }
                RecordingEvent::SessionEvent(SessionEvent::SourceRestored(name)) => {
                    self.info(t!("toast.source_restored", name = name));
                }
                RecordingEvent::SessionEvent(
                    SessionEvent::Warning(_)
                    | SessionEvent::Finished(_)
                    | SessionEvent::Cancelled
                    | SessionEvent::Failed(_),
                ) => {}
                RecordingEvent::Finished {
                    paths,
                    sound,
                    finalizer_panicked,
                } => {
                    if finalizer_panicked {
                        log::warn!("recording finalizer panicked after reporting completion");
                    }
                    self.recording.clear_region();
                    self.lib.files_changed(&paths);
                    let mut sound_deferred = false;
                    let names: Vec<_> = paths
                        .iter()
                        .map(|path| {
                            path.file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .into_owned()
                        })
                        .collect();
                    let success = t!("toast.saved", names = names.join(", ")).into_owned();
                    let action_succeeded = match self
                        .clipboard
                        .handle()
                        .copy_files(paths, Purpose::Recording { success, sound })
                    {
                        Ok(()) => {
                            sound_deferred = sound;
                            if !self.quit_when_idle && !self.presence.is_quiet() {
                                self.show_window(ctx);
                            }
                            true
                        }
                        Err(error) => {
                            if !self.presence.is_quiet() {
                                self.show_window(ctx);
                            }
                            self.error(t!("err.copy_failed", error = error));
                            false
                        }
                    };
                    if !sound_deferred
                        && completion_sound_after_action(sound, action_succeeded)
                        && self.completion_sound_enabled()
                    {
                        mediaa_platform::audio::confirmation_tone();
                    }
                }
                RecordingEvent::Cancelled { finalizer_panicked } => {
                    if finalizer_panicked {
                        log::warn!("recording finalizer panicked after cancellation");
                    }
                    self.recording.clear_region();
                    if !self.quit_when_idle {
                        if !self.presence.is_quiet() {
                            self.show_window(ctx);
                        }
                        self.info(t!("toast.discarded"));
                    }
                }
                RecordingEvent::Failed {
                    failure:
                        SessionFailure {
                            error,
                            kind,
                            recovery_files,
                            log_ref,
                        },
                    finalizer_panicked,
                } => {
                    if finalizer_panicked {
                        log::warn!("recording finalizer panicked after reporting failure");
                    }
                    self.recording.clear_region();
                    if !self.presence.is_quiet() {
                        self.show_window(ctx);
                    }
                    if recovery_files.is_empty() {
                        let detail = t!("toast.saving_failed", error = error).into_owned();
                        let summary = notice_summary(&detail, &t!("notice.error"));
                        self.notices.push(
                            NoticeLevel::Error,
                            summary,
                            detail,
                            log_ref,
                            Vec::new(),
                            kind.requires_attention(),
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
                        self.quit_when_idle = false;
                    }
                }
                RecordingEvent::Lost { finalizer_panicked } => {
                    self.recording.clear_region();
                    self.show_window(ctx);
                    self.start_recovery();
                    self.error(if finalizer_panicked {
                        t!("toast.recording_worker_stopped")
                    } else {
                        t!("toast.recording_lost")
                    });
                }
            }
        }
        if !self.recording.is_idle() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn completion_sound_enabled(&self) -> bool {
        #[cfg(test)]
        {
            self.completion_sound_enabled
        }
        #[cfg(not(test))]
        {
            true
        }
    }
}
