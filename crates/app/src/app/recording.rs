use super::*;
use crate::clipboard_service::Purpose;
use mediaa_core::limits::DEFAULT_VIDEO_AUDIO_KBPS;

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
            RecordingCommand::TogglePause => self.toggle_pause(),
        }
    }

    pub(super) fn request_recording(
        &mut self,
        ctx: &egui::Context,
        target: RecordTarget,
        profile: RecordProfile,
    ) {
        if !self.recorder.is_idle() {
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
        if !self.recorder.is_idle() || self.recovery_rx.is_some() {
            return;
        }
        if self.mic_devices.is_empty() {
            self.refresh_mics();
        }
        let default_mic = self.default_mic_device.clone();
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
        let audio_settings = self.settings.audio_profile(profile);
        if audio_only && audio_settings.is_none() {
            self.recording_region = None;
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
            (r.records_audio(), &r.microphone, r.system_audio)
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
            r.output
        };
        let webp_mode = if !r.segmented_webp {
            WebpMode::Direct
        } else if r.parallel_webp {
            WebpMode::SegmentedParallel
        } else {
            WebpMode::Segmented
        };
        let req = match RecordingRequest::builder(
            output,
            sources,
            audio,
            paths::MediaDir::new(self.settings.media_dir.clone()),
            paths::WorkDir::new(work_dir.clone()),
            if audio_only { "Audio" } else { "Recording" }.into(),
        )
        .video(fps, r.video_quality, max_height)
        .audio_layout(if r.audio_in_every_file {
            AudioLayout::EveryFile
        } else {
            AudioLayout::FirstFileOnly
        })
        .audio_kbps(if audio_only {
            audio_settings.kbps
        } else {
            DEFAULT_VIDEO_AUDIO_KBPS
        })
        .animation(anim)
        .webp_mode(webp_mode)
        .build()
        {
            Ok(request) => request,
            Err(error) => {
                self.recording_region = None;
                self.show_window(ctx);
                self.error(t!("toast.recording_start_failed", error = error));
                return;
            }
        };
        if audio_only {
            self.page = Page::RecordAudio;
        }
        let (tx, rx) = crossbeam_channel::bounded(1);
        let start_cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = start_cancel.clone();
        let started = Instant::now();
        let spawn = self.tasks.spawn(
            TaskKind::RecordingStart,
            "recording-start",
            Some(start_cancel.clone()),
            move || {
                let result =
                    RecordingSession::start_with_cancel(req, &worker_cancel).map_err(|error| {
                        if error.is::<RecordingStartCancelled>() {
                            RecordingStartFailure::Cancelled
                        } else {
                            RecordingStartFailure::Failed {
                                kind: mediaa_media::error::classify_media_error(&error),
                                message: format!("{error:#}"),
                            }
                        }
                    });
                if let Err(error) = tx.send(result) {
                    if let Ok(session) = error.0 {
                        let cancel = Arc::new(AtomicBool::new(false));
                        if let Ok(handle) = session.stop(StopAction::Discard, cancel) {
                            let _ = handle.join();
                        }
                    }
                }
            },
        );
        match spawn {
            Ok(task) => {
                self.recorder = RecState::Starting {
                    rx,
                    task,
                    started,
                    page: if audio_only {
                        Page::RecordAudio
                    } else {
                        Page::Record
                    },
                    cancel: start_cancel,
                }
            }
            Err(error) => {
                self.recording_region = None;
                self.show_window(ctx);
                self.error(t!("toast.recording_start_failed", error = error));
            }
        }
    }

    pub fn recording_page(&self) -> Page {
        match &self.recorder {
            RecState::Starting { page, .. } => *page,
            RecState::Recording { session, .. }
                if matches!(session.output(), OutputKind::Audio(_)) =>
            {
                Page::RecordAudio
            }
            _ => Page::Record,
        }
    }

    fn toggle_pause(&mut self) {
        if let RecState::Recording { session, .. } = &self.recorder {
            if session.is_paused() {
                session.resume();
            } else {
                session.pause();
            }
        }
    }

    fn stop_recording_from_source(
        &mut self,
        ctx: &egui::Context,
        action: StopAction,
        source: StopSource,
    ) {
        self.recording_region = None;
        match std::mem::replace(&mut self.recorder, RecState::Idle) {
            RecState::Starting {
                rx,
                task,
                started,
                page,
                cancel,
            } => {
                self.tasks.cancel(task);
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                self.recorder = RecState::Starting {
                    rx,
                    task,
                    started,
                    page,
                    cancel,
                };
            }
            RecState::Recording { session, events } => {
                let cancel = Arc::new(AtomicBool::new(false));
                let audio_only = matches!(session.output(), OutputKind::Audio(_));
                let sound_on_completion = recording_completion_sound(session.output(), source);
                let progress_state = session.progress();
                let task = match session.stop(action, cancel.clone()) {
                    Ok(handle) => Some(self.tasks.manage_with_deadline(
                        TaskKind::RecordingFinalize,
                        handle,
                        Some(cancel.clone()),
                        None,
                    )),
                    Err(error) => {
                        log::warn!("finalizer ran inline: {error:#}");
                        None
                    }
                };
                self.recorder = RecState::Saving {
                    events,
                    progress_state,
                    progress: None,
                    started: Instant::now(),
                    cancel,
                    sound_on_completion,
                    task,
                };
                if action == StopAction::Save {
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

    pub(super) fn poll_recording_start(&mut self, ctx: &egui::Context) {
        let (rx, started, task) = match &self.recorder {
            RecState::Starting {
                rx, started, task, ..
            } => (rx.clone(), *started, *task),
            _ => return,
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                Err(RecordingStartFailure::Failed {
                    kind: mediaa_media::error::MediaErrorKind::Internal,
                    message: t!("toast.recording_worker_stopped").into(),
                })
            }
        };
        let result = match self.tasks.finish(task) {
            Some(exit) if exit.panicked => Err(RecordingStartFailure::Failed {
                kind: mediaa_media::error::MediaErrorKind::Internal,
                message: t!("toast.recording_worker_stopped").into(),
            }),
            _ => result,
        };
        match result {
            Ok(session) => {
                let events = session.events();
                if self.quit_when_idle {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let progress_state = session.progress();
                    let task = match session.stop(StopAction::Discard, cancel.clone()) {
                        Ok(handle) => Some(self.tasks.manage_with_deadline(
                            TaskKind::RecordingFinalize,
                            handle,
                            Some(cancel.clone()),
                            None,
                        )),
                        Err(error) => {
                            log::warn!("finalizer ran inline: {error:#}");
                            None
                        }
                    };
                    self.recorder = RecState::Saving {
                        events,
                        progress_state,
                        progress: None,
                        started,
                        cancel,
                        sound_on_completion: false,
                        task,
                    };
                } else {
                    log::info!("recording started: {:?}", session.source_names());
                    self.recorder = RecState::Recording {
                        session: Box::new(session),
                        events,
                    };
                    if !self.hidden_by_us {
                        self.info(t!("toast.recording_started"));
                    }
                }
            }
            Err(RecordingStartFailure::Cancelled) => {
                self.recorder = RecState::Idle;
                self.recording_region = None;
                if !self.quit_when_idle {
                    self.show_window(ctx);
                    self.info(t!("toast.discarded"));
                }
            }
            Err(RecordingStartFailure::Failed { kind, message }) => {
                self.recorder = RecState::Idle;
                self.recording_region = None;
                self.show_window(ctx);
                self.start_recovery();
                self.error_with_kind(kind, message);
            }
        }
    }

    pub(super) fn poll_recorder(&mut self, ctx: &egui::Context) {
        if let RecState::Saving {
            progress,
            progress_state,
            ..
        } = &mut self.recorder
        {
            *progress = progress_state.fraction();
        }
        let events = match &self.recorder {
            RecState::Recording { events, .. } | RecState::Saving { events, .. } => events.clone(),
            RecState::Idle | RecState::Starting { .. } => return,
        };
        loop {
            let event = match events.try_recv() {
                Ok(event) => event,
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    let worker_error = self.finish_saving_task().is_some_and(|panicked| panicked);
                    self.recorder = RecState::Idle;
                    self.recording_region = None;
                    self.show_window(ctx);
                    self.start_recovery();
                    let error = if worker_error {
                        t!("toast.recording_worker_stopped").into_owned()
                    } else {
                        t!("toast.recording_lost").into_owned()
                    };
                    self.error(error);
                    break;
                }
            };
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
                    self.stop_recording_from_source(ctx, StopAction::Save, StopSource::Ui);
                }
                SessionEvent::Finished(paths) => {
                    let sound_on_completion = matches!(
                        &self.recorder,
                        RecState::Saving {
                            sound_on_completion: true,
                            ..
                        }
                    );
                    if self.finish_saving_task().is_some_and(|panicked| panicked) {
                        log::warn!("recording finalizer panicked after reporting completion");
                    }
                    self.recorder = RecState::Idle;
                    self.recording_region = None;
                    self.lib.files_changed(&paths);
                    let mut sound_deferred = false;
                    let action_succeeded = match self.settings.after_completion {
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
                                    sound: sound_on_completion,
                                },
                            ) {
                                Ok(()) => {
                                    sound_deferred = sound_on_completion;
                                    if !self.quit_when_idle {
                                        self.page = Page::Library;
                                    }
                                    true
                                }
                                Err(e) => {
                                    self.show_window(ctx);
                                    self.error(t!("err.copy_failed", error = e));
                                    if !self.quit_when_idle {
                                        self.page = Page::Library;
                                    }
                                    false
                                }
                            }
                        }
                    };
                    if !sound_deferred
                        && completion_sound_after_action(sound_on_completion, action_succeeded)
                    {
                        mediaa_platform::audio::confirmation_tone();
                    }
                    break;
                }
                SessionEvent::Cancelled => {
                    if self.finish_saving_task().is_some_and(|panicked| panicked) {
                        log::warn!("recording finalizer panicked after cancellation");
                    }
                    self.recorder = RecState::Idle;
                    self.recording_region = None;
                    if self.quit_when_idle {
                        break;
                    }
                    self.show_window(ctx);
                    self.info(t!("toast.discarded"));
                    break;
                }
                SessionEvent::Failed(SessionFailure {
                    error,
                    kind,
                    recovery_files,
                    log_ref,
                }) => {
                    if self.finish_saving_task().is_some_and(|panicked| panicked) {
                        log::warn!("recording finalizer panicked after reporting failure");
                    }
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
                    break;
                }
            }
        }
        if !matches!(self.recorder, RecState::Idle) {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
    }

    fn finish_saving_task(&mut self) -> Option<bool> {
        let task = match &mut self.recorder {
            RecState::Saving { task, .. } => task.take(),
            _ => None,
        }?;
        self.tasks.finish(task).map(|exit| exit.panicked)
    }
}
