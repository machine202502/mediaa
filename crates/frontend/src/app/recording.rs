use super::*;
#[cfg(test)]
use crate::features::recording::host::{self, BeginRequest, RecordingEffect, StageFlags};
use crate::features::recording::RecordTarget;
#[cfg(test)]
use crate::features::recording::StopSource;
use crate::infra::clipboard_worker::Purpose;

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
            #[cfg(test)]
            RecordingCommand::Stop { action, source } => {
                self.stop_recording_from_source(ctx, action, source)
            }
            #[cfg(not(test))]
            RecordingCommand::Stop { action, source } => {
                self.send_recording_command(RecordingCommand::Stop { action, source });
            }
            RecordingCommand::TogglePause => {
                #[cfg(test)]
                self.recording.toggle_pause();
                #[cfg(not(test))]
                self.send_recording_command(RecordingCommand::TogglePause);
            }
            RecordingCommand::CancelStart => {
                #[cfg(test)]
                self.recording.cancel_start();
                #[cfg(not(test))]
                self.send_recording_command(RecordingCommand::CancelStart);
            }
            RecordingCommand::CancelFinalize => {
                #[cfg(test)]
                self.recording.cancel_finalize();
                #[cfg(not(test))]
                self.send_recording_command(RecordingCommand::CancelFinalize);
            }
        }
    }

    #[cfg(not(test))]
    pub(super) fn send_recording_command(&mut self, command: RecordingCommand) {
        let id = self.next_command;
        self.next_command += 1;
        let name = match &command {
            RecordingCommand::Start { .. } => mediaa_protocol::RECORDING_START,
            RecordingCommand::Stop { .. } => mediaa_protocol::RECORDING_STOP,
            RecordingCommand::TogglePause => mediaa_protocol::RECORDING_TOGGLE,
            RecordingCommand::CancelStart => mediaa_protocol::RECORDING_CANCEL_START,
            RecordingCommand::CancelFinalize => mediaa_protocol::RECORDING_CANCEL_FINALIZE,
        };
        let wire = crate::protocol::recording_command_to_wire(command);
        let payload = mediaa_protocol::encode_recording_command(&wire);
        let frame = self.sync.command_frame(id, name, payload);
        self.send_sync_frame(frame);
    }
    pub(super) fn request_recording(
        &mut self,
        ctx: &egui::Context,
        target: RecordTarget,
        profile: RecordProfile,
        hide: Option<bool>,
    ) {
        if !self.recording_view.is_idle() {
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
        if !self.recording_view.is_idle() || self.recovery_running {
            return;
        }
        #[cfg(test)]
        {
            if self.mic_devices.is_empty() {
                self.refresh_mics();
            }
            let flags = self.stage_flags();
            let effects = host::begin(
                &mut self.recording,
                &self.settings,
                &self.mic_devices,
                self.default_mic_device.as_deref(),
                BeginRequest {
                    target,
                    profile,
                    now: Instant::now(),
                },
                flags,
                &mut self.recording_view.ui,
            );
            self.apply_recording_effects(ctx, effects);
        }
        #[cfg(not(test))]
        {
            let _ = ctx;
            self.send_recording_command(RecordingCommand::Start {
                target,
                profile,
                hide: None,
            });
        }
    }

    #[cfg(test)]
    fn stage_flags(&self) -> StageFlags {
        StageFlags {
            quiet: self.presence.is_quiet(),
            hidden: self.window.is_hidden(),
            quit_when_idle: self.window.is_quitting(),
        }
    }

    #[cfg(test)]
    fn apply_recording_effects(&mut self, ctx: &egui::Context, effects: Vec<RecordingEffect>) {
        for effect in effects {
            match effect {
                RecordingEffect::ShowWindow => self.show_window(ctx),
                RecordingEffect::ClearRegion => self.recording_view.ui.clear_region(),
                RecordingEffect::SetPage(page) => self.page = page,
                RecordingEffect::Info(text) => self.info(text),
                RecordingEffect::Warning(text) => self.warning(text),
                RecordingEffect::Error(text) => self.error(text),
                RecordingEffect::ErrorWithKind { kind, message } => {
                    self.error_with_kind(kind, message)
                }
                RecordingEffect::StartRecovery => {}
                RecordingEffect::Notice {
                    level,
                    summary,
                    detail,
                    log_ref,
                    files,
                    attention,
                } => self
                    .notices
                    .push(level, summary, detail, log_ref, files, attention),
                RecordingEffect::FilesChanged(_) => {}
                RecordingEffect::StopRecording { action, source } => {
                    self.stop_recording_from_source(ctx, action, source)
                }
                RecordingEffect::Finished { paths, sound } => {
                    self.finish_recording(ctx, paths, sound)
                }
                RecordingEffect::QuitWhenIdle(_) => {}
            }
        }
    }

    pub fn recording_page(&self) -> Page {
        self.recording_view.page()
    }

    #[cfg(test)]
    fn stop_recording_from_source(
        &mut self,
        ctx: &egui::Context,
        action: StopAction,
        source: StopSource,
    ) {
        #[cfg(test)]
        {
            if let Some(result) = self.recording.stop(action, source) {
                if action == StopAction::Save {
                    self.page = result.page;
                }
            }
            ctx.request_repaint();
        }
        #[cfg(not(test))]
        {
            let _ = ctx;
            self.send_recording_command(RecordingCommand::Stop { action, source });
        }
    }

    pub(super) fn poll_recording_start(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        {
            let flags = self.stage_flags();
            let effects = host::start_event(&mut self.recording, flags);
            if effects.is_empty() && self.recording_view.is_starting() {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
            self.apply_recording_effects(ctx, effects);
        }
        #[cfg(not(test))]
        {
            let _ = ctx;
        }
    }

    pub(super) fn poll_recorder(&mut self, ctx: &egui::Context) {
        #[cfg(test)]
        {
            let flags = self.stage_flags();
            let effects = host::poll_events(&mut self.recording, flags);
            self.apply_recording_effects(ctx, effects);
            if !self.recording_view.is_idle() {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        #[cfg(not(test))]
        {
            let _ = ctx;
        }
    }
    pub(super) fn finish_recording(
        &mut self,
        ctx: &egui::Context,
        paths: Vec<std::path::PathBuf>,
        sound: bool,
    ) {
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
                if !self.window.is_quitting() && !self.presence.is_quiet() {
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
