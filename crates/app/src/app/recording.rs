use super::*;
use crate::features::recording::{RecordTarget, RecordingEvent, RecordingStartEvent, StopSource};
use crate::services::clipboard::Purpose;
use mediaa_core::limits::DEFAULT_VIDEO_AUDIO_KBPS;
use mediaa_core::settings::AnimSettings;

impl App {
    pub(crate) fn dispatch_command(&mut self, ctx: &egui::Context, command: AppCommand) {
        match command {
            AppCommand::Recording(command) => self.dispatch_recording_command(ctx, command),
            AppCommand::Quit => self.quit(ctx),
        }
    }

    fn dispatch_recording_command(&mut self, ctx: &egui::Context, command: RecordingCommand) {
        match command {
            RecordingCommand::Start { target, profile } => {
                self.request_recording(ctx, target, profile)
            }
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
    ) {
        if !self.recording.is_idle() {
            return;
        }
        if self.recovery_active() {
            self.show_window(ctx);
            self.warning(t!("record.recovery_wait"));
            return;
        }
        let hide =
            !matches!(target, RecordTarget::Audio) && self.settings.profile(profile).0.hide_to_tray;
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
        let default_mic = self.default_mic_device.clone();
        let (record, quality) = self.settings.profile(profile);
        let fps = record.fps;
        let max_height = record.max_height();
        let animation = AnimSettings::for_recording(fps, max_height, quality);
        let cursor = record.record_cursor;
        self.recording.set_region_for_target(&target);
        let sources: Vec<SourceFactory> = match &target {
            RecordTarget::Audio => Vec::new(),
            RecordTarget::Screens(monitors) => monitors
                .iter()
                .map(|monitor| {
                    let id = monitor.id;
                    Box::new(move || screen::monitor_source(id, None, cursor)) as SourceFactory
                })
                .collect(),
            RecordTarget::Region(monitor, rect) => {
                let (id, rect) = (monitor.id, *rect);
                vec![Box::new(move || {
                    screen::monitor_source(id, Some(rect), cursor)
                })]
            }
            RecordTarget::Windows(windows) => windows
                .iter()
                .map(|window| {
                    let id = window.id;
                    Box::new(move || screen::window_source(id, cursor)) as SourceFactory
                })
                .collect(),
        };
        let audio_only = matches!(target, RecordTarget::Audio);
        let audio_settings = self.settings.audio_profile(profile);
        if audio_only && audio_settings.is_none() {
            self.recording.clear_region();
            self.error(t!("audio.no_source"));
            return;
        }
        let audio_settings = audio_settings.unwrap_or(&self.settings.audio_record);
        let (want_sound, microphone, system_audio) = if audio_only {
            (
                true,
                &audio_settings.microphone,
                audio_settings.system_audio,
            )
        } else {
            (
                record.records_audio(),
                &record.microphone,
                record.system_audio,
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
        let work_dir = paths::work_dir().join(format!(
            "{}-{}",
            now.format("%Y%m%d-%H%M%S"),
            std::process::id()
        ));
        let output = if audio_only {
            OutputKind::Audio(audio_settings.format)
        } else {
            record.output
        };
        let request = match RecordingRequest::builder(
            output,
            sources,
            audio,
            paths::MediaDir::new(self.settings.media_dir.clone()),
            paths::WorkDir::new(work_dir),
            if audio_only { "Audio" } else { "Recording" }.into(),
        )
        .video(fps, record.video_quality, max_height)
        .audio_layout(if record.audio_in_every_file {
            AudioLayout::EveryFile
        } else {
            AudioLayout::FirstFileOnly
        })
        .audio_kbps(if audio_only {
            audio_settings.kbps
        } else {
            DEFAULT_VIDEO_AUDIO_KBPS
        })
        .animation(animation)
        .webp_mode(record.webp_mode())
        .build()
        {
            Ok(request) => request,
            Err(error) => {
                self.recording.clear_region();
                self.show_window(ctx);
                self.error(t!("toast.recording_start_failed", error = error));
                return;
            }
        };
        let page = if audio_only {
            Page::RecordAudio
        } else {
            Page::Record
        };
        if audio_only {
            self.page = page;
        }
        if let Err(error) = self.recording.start(request, Instant::now(), page) {
            self.recording.clear_region();
            self.show_window(ctx);
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
                if !self.hidden_by_us {
                    self.info(t!("toast.recording_started"));
                }
            }
            RecordingStartEvent::Started {
                discarded: true, ..
            } => {}
            RecordingStartEvent::Cancelled => {
                if !self.quit_when_idle {
                    self.show_window(ctx);
                    self.info(t!("toast.discarded"));
                }
            }
            RecordingStartEvent::Failed { kind, message } => {
                self.show_window(ctx);
                self.start_recovery();
                self.error_with_kind(kind, message);
            }
            RecordingStartEvent::WorkerStopped => {
                self.show_window(ctx);
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
                    let action_succeeded = match self.settings.after_completion {
                        AfterCompletionAction::OpenLibrary => {
                            self.show_window(ctx);
                            let names: Vec<_> = paths
                                .iter()
                                .map(|path| {
                                    path.file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .into_owned()
                                })
                                .collect();
                            self.success(t!("toast.saved", names = names.join(", ")));
                            if !self.quit_when_idle {
                                self.page = Page::Library;
                            }
                            true
                        }
                        AfterCompletionAction::CopyFile => {
                            match self.clipboard.handle().copy_files(
                                paths,
                                Purpose::Recording {
                                    success: t!("toast.recording_copied").into_owned(),
                                    sound,
                                },
                            ) {
                                Ok(()) => {
                                    sound_deferred = sound;
                                    if !self.quit_when_idle {
                                        self.page = Page::Library;
                                    }
                                    true
                                }
                                Err(error) => {
                                    self.show_window(ctx);
                                    self.error(t!("err.copy_failed", error = error));
                                    if !self.quit_when_idle {
                                        self.page = Page::Library;
                                    }
                                    false
                                }
                            }
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
                        self.show_window(ctx);
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
