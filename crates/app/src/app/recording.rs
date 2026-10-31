use super::*;
use crate::features::recording::{RecordTarget, RecordingEvent, RecordingStartEvent, StopSource};
use crate::services::clipboard::Purpose;
use mediaa_core::limits::DEFAULT_VIDEO_AUDIO_KBPS;
use mediaa_core::settings::{AnimSettings, RecordSettings};
use mediaa_core::{AnimFormat, OutputKind};

fn allow_unavailable_sources(profile: RecordProfile, record: &RecordSettings) -> bool {
    profile != RecordProfile::Region && record.allow_unavailable_sources
}

fn unavailable_target(target: &RecordTarget) -> Option<(String, bool)> {
    use mediaa_platform::screen::{self, MonitorState, WindowState};
    match target {
        RecordTarget::Audio => None,
        RecordTarget::Screens(monitors) => monitors.iter().find_map(|monitor| {
            (screen::monitor_state(monitor.id) == MonitorState::Disconnected)
                .then(|| (monitor.label(), true))
        }),
        RecordTarget::Region(monitor, _) => (screen::monitor_state(monitor.id)
            == MonitorState::Disconnected)
            .then(|| (monitor.label(), true)),
        RecordTarget::Windows(windows) => windows.iter().find_map(|window| {
            match screen::window_status(window.id, window.pid, &window.app) {
                WindowState::Available => None,
                WindowState::Minimized => Some((window.label(), true)),
                WindowState::Hidden | WindowState::Missing | WindowState::Mismatch => {
                    Some((window.label(), false))
                }
            }
        }),
    }
}

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
        if let Some((name, allowed)) = unavailable_target(&target) {
            let allow = allow_unavailable_sources(profile, self.settings.profile(profile).0);
            if !allowed || !allow {
                self.recording.clear_region();
                if !self.recording_quiet {
                    self.show_window(ctx);
                }
                self.error(t!("toast.target_unavailable", name = name));
                return;
            }
            self.warning(t!("toast.source_allowed", name = name));
        }
        if self.mic_devices.is_empty() {
            self.refresh_mics();
        }
        let default_mic = self.default_mic_device.clone();
        let (record, quality) = self.settings.profile(profile);
        let allow_unavailable = allow_unavailable_sources(profile, record);
        let webp = matches!(record.output, OutputKind::Anim(AnimFormat::Webp));
        let (fps, max_height) = if webp {
            (record.animation_fps(), record.animation_height())
        } else {
            (record.fps, record.max_height())
        };
        let animation = AnimSettings::for_recording(fps, max_height, quality);
        let cursor = record.record_cursor;
        self.recording.set_region_for_target(&target);
        let sources: Vec<SourceFactory> = match &target {
            RecordTarget::Audio => Vec::new(),
            RecordTarget::Screens(monitors) => monitors
                .iter()
                .map(|monitor| {
                    let id = monitor.id;
                    let size = (monitor.width, monitor.height);
                    Box::new(move || {
                        if allow_unavailable
                            && mediaa_platform::screen::monitor_state(id)
                                == mediaa_platform::screen::MonitorState::Disconnected
                        {
                            mediaa_platform::screen::deferred_monitor_source(id, size, None, cursor)
                        } else {
                            screen::monitor_source(id, None, cursor)
                        }
                    }) as SourceFactory
                })
                .collect(),
            RecordTarget::Region(monitor, rect) => {
                let (id, rect) = (monitor.id, *rect);
                vec![Box::new(move || {
                    if allow_unavailable
                        && mediaa_platform::screen::monitor_state(id)
                            == mediaa_platform::screen::MonitorState::Disconnected
                    {
                        mediaa_platform::screen::deferred_monitor_source(
                            id,
                            (rect.w as u32, rect.h as u32),
                            Some(rect),
                            cursor,
                        )
                    } else {
                        screen::monitor_source(id, Some(rect), cursor)
                    }
                })]
            }
            RecordTarget::Windows(windows) => windows
                .iter()
                .map(|window| {
                    let id = window.id;
                    let pid = window.pid;
                    let app = window.app.clone();
                    let size = (window.width, window.height);
                    Box::new(move || {
                        if allow_unavailable
                            && mediaa_platform::screen::window_status(id, pid, &app)
                                == mediaa_platform::screen::WindowState::Minimized
                        {
                            mediaa_platform::screen::deferred_window_source(
                                id,
                                size,
                                pid,
                                app.clone(),
                                cursor,
                            )
                        } else {
                            screen::window_source_checked(id, pid, &app, cursor)
                        }
                    }) as SourceFactory
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
        .no_signal(record.show_no_signal)
        .build()
        {
            Ok(request) => request,
            Err(error) => {
                self.recording.clear_region();
                if !self.recording_quiet {
                    self.show_window(ctx);
                }
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
            if !self.recording_quiet {
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
                if !self.hidden_by_us {
                    self.info(t!("toast.recording_started"));
                }
            }
            RecordingStartEvent::Started {
                discarded: true, ..
            } => {}
            RecordingStartEvent::Cancelled => {
                if !self.quit_when_idle {
                    if !self.recording_quiet {
                        self.show_window(ctx);
                    }
                    self.info(t!("toast.discarded"));
                }
            }
            RecordingStartEvent::Failed { kind, message } => {
                if !self.recording_quiet {
                    self.show_window(ctx);
                }
                self.start_recovery();
                self.error_with_kind(kind, message);
            }
            RecordingStartEvent::WorkerStopped => {
                if !self.recording_quiet {
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
                            if !self.quit_when_idle {
                                if !self.recording_quiet {
                                    self.show_window(ctx);
                                }
                                self.page = Page::Library;
                            }
                            true
                        }
                        Err(error) => {
                            if !self.recording_quiet {
                                self.show_window(ctx);
                            }
                            self.error(t!("err.copy_failed", error = error));
                            if !self.quit_when_idle {
                                self.page = Page::Library;
                            }
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
                        if !self.recording_quiet {
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
                    if !self.recording_quiet {
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

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_platform::screen::{CaptureWindowId, MonitorId, MonitorInfo, WindowInfo};

    #[test]
    fn an_unavailable_window_aborts_the_whole_target() {
        let window = WindowInfo {
            id: CaptureWindowId::new(0),
            title: "Closed window".into(),
            app: String::new(),
            pid: 0,
            width: 800,
            height: 600,
        };
        assert_eq!(
            unavailable_target(&RecordTarget::Windows(vec![window])),
            Some(("Closed window".into(), false))
        );
    }

    #[test]
    fn region_never_allows_unavailable_sources() {
        let mut settings = mediaa_core::Settings::default();
        settings.region_record.allow_unavailable_sources = true;
        assert!(!allow_unavailable_sources(
            RecordProfile::Region,
            &settings.region_record
        ));
        settings.record.allow_unavailable_sources = true;
        assert!(allow_unavailable_sources(
            RecordProfile::App,
            &settings.record
        ));
    }

    #[test]
    fn a_disconnected_monitor_can_be_deferred() {
        let monitor = MonitorInfo {
            id: MonitorId::new(u32::MAX),
            device: "gone".into(),
            index: 0,
            name: "Gone monitor".into(),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: false,
        };
        assert_eq!(
            unavailable_target(&RecordTarget::Screens(vec![monitor])),
            Some(("Gone monitor (1920×1080)".into(), true))
        );
    }
}
