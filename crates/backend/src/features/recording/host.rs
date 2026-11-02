#![allow(unused_variables)]
use crate::features::recording::{
    plan, PlanError, RecordTarget, RecordUi, RecordingController, RecordingEvent,
    RecordingStartEvent, StopSource,
};
use mediaa_core::navigation::Page;
use mediaa_core::notices::{notice_summary, NoticeLevel};
use mediaa_core::settings::RecordProfile;
use mediaa_core::Settings;
use mediaa_media::error::MediaErrorKind;
use mediaa_media::recorder::{SessionEvent, SessionFailure, SessionNotice, StopAction};
macro_rules! t {
    ($($tt:tt)*) => {
        std::borrow::Cow::Borrowed("backend recording message")
    };
}
use std::path::PathBuf;
use std::time::Instant;

#[allow(
    dead_code,
    reason = "UI applies these in tests; the daemon consumes a subset"
)]
pub enum RecordingEffect {
    ShowWindow,
    ClearRegion,
    SetPage(Page),
    Info(String),
    Warning(String),
    Error(String),
    ErrorWithKind {
        kind: MediaErrorKind,
        message: String,
    },
    StartRecovery,
    Notice {
        level: NoticeLevel,
        summary: String,
        detail: String,
        log_ref: Option<String>,
        files: Vec<PathBuf>,
        attention: bool,
    },
    FilesChanged(Vec<PathBuf>),
    StopRecording {
        action: StopAction,
        source: StopSource,
    },
    Finished {
        paths: Vec<PathBuf>,
        sound: bool,
    },
    QuitWhenIdle(bool),
}

#[derive(Clone, Copy)]
pub struct StageFlags {
    pub quiet: bool,
    pub hidden: bool,
    pub quit_when_idle: bool,
}

pub struct BeginRequest {
    pub target: RecordTarget,
    pub profile: RecordProfile,
    pub now: Instant,
}

pub fn begin(
    controller: &mut RecordingController,
    settings: &Settings,
    microphones: &[String],
    default_microphone: Option<&str>,
    request: BeginRequest,
    flags: StageFlags,
    selection: &mut RecordUi,
) -> Vec<RecordingEffect> {
    let target = &request.target;
    let mut effects = Vec::new();
    if !controller.is_idle() {
        return effects;
    }
    let plan = match plan(
        settings,
        target,
        request.profile,
        microphones,
        default_microphone,
    ) {
        Ok(plan) => plan,
        Err(PlanError::Unavailable(label)) => {
            effects.push(RecordingEffect::ClearRegion);
            if !flags.quiet {
                effects.push(RecordingEffect::ShowWindow);
            }
            effects.push(RecordingEffect::Error(
                t!("toast.target_unavailable", name = label).into_owned(),
            ));
            return effects;
        }
        Err(PlanError::NoAudioSource) => {
            effects.push(RecordingEffect::ClearRegion);
            effects.push(RecordingEffect::Error(t!("audio.no_source").into_owned()));
            return effects;
        }
        Err(PlanError::Request(error)) => {
            effects.push(RecordingEffect::ClearRegion);
            if !flags.quiet {
                effects.push(RecordingEffect::ShowWindow);
            }
            effects.push(RecordingEffect::Error(
                t!("toast.recording_start_failed", error = error).into_owned(),
            ));
            return effects;
        }
    };
    if let Some(label) = plan.allowed_warning {
        effects.push(RecordingEffect::Warning(
            t!("toast.source_allowed", name = label).into_owned(),
        ));
    }
    selection.set_region_for_target(target);
    let page = if plan.audio_only {
        Page::RecordAudio
    } else {
        Page::Record
    };
    if plan.audio_only {
        effects.push(RecordingEffect::SetPage(page));
    }
    if let Err(error) = controller.start(plan.request, request.now, page) {
        effects.push(RecordingEffect::ClearRegion);
        if !flags.quiet {
            effects.push(RecordingEffect::ShowWindow);
        }
        effects.push(RecordingEffect::Error(
            t!("toast.recording_start_failed", error = error).into_owned(),
        ));
    }
    effects
}

pub fn start_event(
    controller: &mut RecordingController,
    flags: StageFlags,
) -> Vec<RecordingEffect> {
    let mut effects = Vec::new();
    let Some(event) = controller.poll_start(flags.quit_when_idle) else {
        return effects;
    };
    match event {
        RecordingStartEvent::Started {
            samples,
            discarded: false,
        } => {
            log::info!("recording started: {samples:?}");
            if !flags.hidden {
                effects.push(RecordingEffect::Info(
                    t!("toast.recording_started").into_owned(),
                ));
            }
        }
        RecordingStartEvent::Started {
            discarded: true, ..
        } => {}
        RecordingStartEvent::Cancelled => {
            effects.push(RecordingEffect::ClearRegion);
            if !flags.quit_when_idle {
                if !flags.quiet {
                    effects.push(RecordingEffect::ShowWindow);
                }
                effects.push(RecordingEffect::Info(t!("toast.discarded").into_owned()));
            }
        }
        RecordingStartEvent::Failed { kind, message } => {
            effects.push(RecordingEffect::ClearRegion);
            if !flags.quiet {
                effects.push(RecordingEffect::ShowWindow);
            }
            effects.push(RecordingEffect::StartRecovery);
            effects.push(RecordingEffect::ErrorWithKind { kind, message });
        }
        RecordingStartEvent::WorkerStopped => {
            effects.push(RecordingEffect::ClearRegion);
            if !flags.quiet {
                effects.push(RecordingEffect::ShowWindow);
            }
            effects.push(RecordingEffect::StartRecovery);
            effects.push(RecordingEffect::Error(
                t!("toast.recording_worker_stopped").into_owned(),
            ));
        }
    }
    effects
}

pub fn poll_events(
    controller: &mut RecordingController,
    flags: StageFlags,
) -> Vec<RecordingEffect> {
    let mut effects = Vec::new();
    for event in controller.poll_events() {
        match event {
            RecordingEvent::Warning(warning) => effects.push(RecordingEffect::Warning(warning)),
            RecordingEvent::SessionEvent(SessionEvent::Notice(SessionNotice::WebpFallback {
                trace_id,
                detail,
            })) => {
                let summary = t!("toast.webp_fallback").into_owned();
                let detail = format!(
                    "{}.\n\ntrace={trace_id}\n\n{detail}",
                    t!("toast.webp_fallback")
                );
                effects.push(RecordingEffect::Notice {
                    level: NoticeLevel::Warning,
                    summary,
                    detail,
                    log_ref: Some(trace_id),
                    files: Vec::new(),
                    attention: false,
                });
            }
            RecordingEvent::SessionEvent(SessionEvent::CaptureFailed(error)) => {
                effects.push(RecordingEffect::Error(
                    t!("toast.recording_stopped", error = error).into_owned(),
                ));
                effects.push(RecordingEffect::StopRecording {
                    action: StopAction::Save,
                    source: StopSource::Ui,
                });
            }
            RecordingEvent::SessionEvent(SessionEvent::SourceLost(name)) => {
                effects.push(RecordingEffect::Warning(
                    t!("toast.source_lost", name = name).into_owned(),
                ));
            }
            RecordingEvent::SessionEvent(SessionEvent::SourceRestored(name)) => {
                effects.push(RecordingEffect::Info(
                    t!("toast.source_restored", name = name).into_owned(),
                ));
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
                effects.push(RecordingEffect::ClearRegion);
                effects.push(RecordingEffect::FilesChanged(paths.clone()));
                effects.push(RecordingEffect::Finished { paths, sound });
            }
            RecordingEvent::Cancelled { finalizer_panicked } => {
                if finalizer_panicked {
                    log::warn!("recording finalizer panicked after cancellation");
                }
                effects.push(RecordingEffect::ClearRegion);
                if !flags.quit_when_idle {
                    if !flags.quiet {
                        effects.push(RecordingEffect::ShowWindow);
                    }
                    effects.push(RecordingEffect::Info(t!("toast.discarded").into_owned()));
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
                effects.push(RecordingEffect::ClearRegion);
                if !flags.quiet {
                    effects.push(RecordingEffect::ShowWindow);
                }
                if recovery_files.is_empty() {
                    let summary = notice_summary(&error, &t!("notice.error"));
                    effects.push(RecordingEffect::Notice {
                        level: NoticeLevel::Error,
                        summary,
                        detail: error,
                        log_ref,
                        files: Vec::new(),
                        attention: kind.requires_attention(),
                    });
                } else {
                    effects.push(RecordingEffect::FilesChanged(recovery_files.clone()));
                    effects.push(RecordingEffect::Notice {
                        level: NoticeLevel::Error,
                        summary: t!("toast.recovery_saved").into_owned(),
                        detail: error,
                        log_ref,
                        files: recovery_files,
                        attention: true,
                    });
                    effects.push(RecordingEffect::QuitWhenIdle(false));
                }
            }
            RecordingEvent::Lost { finalizer_panicked } => {
                effects.push(RecordingEffect::ClearRegion);
                effects.push(RecordingEffect::ShowWindow);
                effects.push(RecordingEffect::StartRecovery);
                effects.push(RecordingEffect::Error(if finalizer_panicked {
                    t!("toast.recording_worker_stopped").into_owned()
                } else {
                    t!("toast.recording_lost").into_owned()
                }));
            }
        }
    }
    effects
}
