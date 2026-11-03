use crate::domain::JobId;
use mediaa2_core::formats::OutputKind;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::recording::{RecordTarget, StopAction, StopSource};
use mediaa2_core::settings::RecordProfile;
use mediaa2_protocol::state::RecordingState;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum RecordingEvent {
    Start {
        job: JobId,
        target: RecordTarget,
        profile: RecordProfile,
        hide: bool,
    },
    Started {
        job: JobId,
        sources: Vec<String>,
        has_audio: bool,
        output: OutputKind,
    },
    StartFailed {
        job: JobId,
        message: String,
    },
    Pause,
    Resume,
    Stop {
        action: StopAction,
        source: StopSource,
    },
    CancelStart,
    CancelFinalize,
    Tick {
        now_ms: u64,
    },
    Session {
        job: JobId,
        event: SessionEvent,
    },
    FinalizeProgress {
        job: JobId,
        progress: Option<f32>,
    },
    Finished {
        job: JobId,
        paths: Vec<PathBuf>,
        sound: bool,
    },
    Failed {
        job: JobId,
        message: String,
    },
    Cancelled {
        job: JobId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionEvent {
    Warning(String),
    SourceLost(String),
    SourceRestored(String),
    CaptureFailed(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum RecordingEffect {
    Spawn {
        job: JobId,
        target: RecordTarget,
        profile: RecordProfile,
        hide: bool,
    },
    Stop {
        job: JobId,
        action: StopAction,
        source: StopSource,
    },
    Cancel {
        job: JobId,
    },
    Finalize {
        job: JobId,
        discard: bool,
    },
    AddLibrary(Vec<PathBuf>),
    CopyFiles(Vec<PathBuf>),
    PlaySound,
    Notify {
        level: NoticeLevel,
        summary: String,
        detail: String,
    },
    Tray,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub view: RecordingState,
    pub job: Option<JobId>,
}

impl Default for Recording {
    fn default() -> Self {
        Self {
            view: RecordingState::Idle,
            job: None,
        }
    }
}

impl Recording {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: RecordingEvent, now_ms: u64) -> Vec<RecordingEffect> {
        match event {
            RecordingEvent::Start {
                job,
                target,
                profile,
                hide,
            } => {
                if !matches!(&self.view, RecordingState::Idle) || self.job.is_some() {
                    return Vec::new();
                }
                self.job = Some(job);
                self.view = RecordingState::Starting {
                    started_ms: now_ms,
                    target: target.clone(),
                };
                vec![RecordingEffect::Spawn {
                    job,
                    target,
                    profile,
                    hide,
                }]
            }
            RecordingEvent::Started {
                job,
                sources,
                has_audio,
                output,
            } => {
                let target = match &self.view {
                    RecordingState::Starting { target, .. } if self.job == Some(job) => {
                        target.clone()
                    }
                    _ => return Vec::new(),
                };
                self.view = RecordingState::Active {
                    job,
                    target,
                    started_ms: now_ms,
                    paused: false,
                    elapsed_ms: 0,
                    sources,
                    has_audio,
                    output,
                };
                Vec::new()
            }
            RecordingEvent::StartFailed { job, message } => {
                if !matches!(&self.view, RecordingState::Starting { .. }) || self.job != Some(job) {
                    return Vec::new();
                }
                self.view = RecordingState::Idle;
                self.job = None;
                vec![
                    notice(NoticeLevel::Error, "Recording failed", message),
                    RecordingEffect::Tray,
                ]
            }
            RecordingEvent::Pause => {
                let RecordingState::Active {
                    started_ms,
                    paused,
                    elapsed_ms,
                    ..
                } = &mut self.view
                else {
                    return Vec::new();
                };
                if *paused {
                    return Vec::new();
                }
                *elapsed_ms = now_ms.saturating_sub(*started_ms);
                *paused = true;
                vec![RecordingEffect::Tray]
            }
            RecordingEvent::Resume => {
                let RecordingState::Active {
                    started_ms,
                    paused,
                    elapsed_ms,
                    ..
                } = &mut self.view
                else {
                    return Vec::new();
                };
                if !*paused {
                    return Vec::new();
                }
                *started_ms = now_ms.saturating_sub(*elapsed_ms);
                *paused = false;
                vec![RecordingEffect::Tray]
            }
            RecordingEvent::Stop { action, source } => {
                let (job, recorded_ms) = match &self.view {
                    RecordingState::Active {
                        job,
                        started_ms,
                        paused,
                        elapsed_ms,
                        ..
                    } if self.job == Some(*job) => (
                        *job,
                        if *paused {
                            *elapsed_ms
                        } else {
                            now_ms.saturating_sub(*started_ms)
                        },
                    ),
                    _ => return Vec::new(),
                };
                self.view = RecordingState::Finalizing {
                    job,
                    recorded_ms,
                    progress: None,
                };
                vec![
                    RecordingEffect::Stop {
                        job,
                        action,
                        source,
                    },
                    RecordingEffect::Finalize {
                        job,
                        discard: action == StopAction::Discard,
                    },
                ]
            }
            RecordingEvent::CancelStart => {
                if !matches!(&self.view, RecordingState::Starting { .. }) {
                    return Vec::new();
                }
                match self.job.take() {
                    Some(job) => {
                        self.view = RecordingState::Idle;
                        vec![RecordingEffect::Cancel { job }]
                    }
                    None => Vec::new(),
                }
            }
            RecordingEvent::CancelFinalize => match &self.view {
                RecordingState::Finalizing { job, .. } if self.job == Some(*job) => {
                    vec![RecordingEffect::Cancel { job: *job }]
                }
                _ => Vec::new(),
            },
            RecordingEvent::Tick { now_ms } => {
                if let RecordingState::Active {
                    started_ms,
                    paused,
                    elapsed_ms,
                    ..
                } = &mut self.view
                {
                    if !*paused {
                        *elapsed_ms = now_ms.saturating_sub(*started_ms);
                    }
                }
                Vec::new()
            }
            RecordingEvent::Session { job, event } => {
                let current = matches!(
                    &self.view,
                    RecordingState::Active { job: active, .. } if *active == job
                ) && self.job == Some(job);
                if !current {
                    return Vec::new();
                }
                let effect = match event {
                    SessionEvent::Warning(detail) => {
                        notice(NoticeLevel::Warning, "Recording warning", detail)
                    }
                    SessionEvent::SourceLost(detail) => {
                        notice(NoticeLevel::Warning, "Source lost", detail)
                    }
                    SessionEvent::SourceRestored(detail) => {
                        notice(NoticeLevel::Info, "Source restored", detail)
                    }
                    SessionEvent::CaptureFailed(detail) => {
                        notice(NoticeLevel::Error, "Capture failed", detail)
                    }
                };
                vec![effect]
            }
            RecordingEvent::FinalizeProgress { job, progress } => {
                let RecordingState::Finalizing {
                    job: active,
                    progress: current_progress,
                    ..
                } = &mut self.view
                else {
                    return Vec::new();
                };
                if *active != job || self.job != Some(job) {
                    return Vec::new();
                }
                *current_progress = progress;
                Vec::new()
            }
            RecordingEvent::Finished { job, paths, sound } => {
                if !matches!(
                    &self.view,
                    RecordingState::Finalizing { job: active, .. } if *active == job
                ) || self.job != Some(job)
                {
                    return Vec::new();
                }
                self.view = RecordingState::Idle;
                self.job = None;
                let detail = paths
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut effects = Vec::new();
                if !paths.is_empty() {
                    effects.push(RecordingEffect::AddLibrary(paths.clone()));
                    effects.push(RecordingEffect::CopyFiles(paths));
                }
                if sound {
                    effects.push(RecordingEffect::PlaySound);
                }
                effects.push(notice(NoticeLevel::Success, "Recording saved", detail));
                effects.push(RecordingEffect::Tray);
                effects
            }
            RecordingEvent::Failed { job, message } => {
                if !matches!(
                    &self.view,
                    RecordingState::Finalizing { job: active, .. } if *active == job
                ) || self.job != Some(job)
                {
                    return Vec::new();
                }
                self.view = RecordingState::Idle;
                self.job = None;
                vec![
                    notice(NoticeLevel::Error, "Recording failed", message),
                    RecordingEffect::Tray,
                ]
            }
            RecordingEvent::Cancelled { job } => {
                if !matches!(
                    &self.view,
                    RecordingState::Finalizing { job: active, .. } if *active == job
                ) || self.job != Some(job)
                {
                    return Vec::new();
                }
                self.view = RecordingState::Idle;
                self.job = None;
                vec![
                    notice(NoticeLevel::Warning, "Recording cancelled", String::new()),
                    RecordingEffect::Tray,
                ]
            }
        }
    }
}

fn notice(
    level: NoticeLevel,
    summary: impl Into<String>,
    detail: impl Into<String>,
) -> RecordingEffect {
    RecordingEffect::Notify {
        level,
        summary: summary.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::formats::VideoContainer;
    use mediaa2_core::geometry::Rect;
    use mediaa2_core::screen::{MonitorId, MonitorInfo};

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(1),
            device: r"\\.\DISPLAY1".into(),
            index: 0,
            name: "DELL U2720Q".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            scale: 1.5,
            primary: true,
        }
    }

    fn target() -> RecordTarget {
        RecordTarget::Region {
            monitor: monitor(),
            region: Rect::new(10, 20, 300, 200),
        }
    }

    fn start(recording: &mut Recording, job: JobId) -> Vec<RecordingEffect> {
        recording.apply(
            RecordingEvent::Start {
                job,
                target: target(),
                profile: RecordProfile::App,
                hide: true,
            },
            1_000,
        )
    }

    fn started(recording: &mut Recording, job: JobId) -> Vec<RecordingEffect> {
        recording.apply(
            RecordingEvent::Started {
                job,
                sources: vec!["DELL U2720Q".into()],
                has_audio: true,
                output: OutputKind::Video(VideoContainer::Mkv),
            },
            1_100,
        )
    }

    fn active(recording: &mut Recording, job: JobId) {
        start(recording, job);
        started(recording, job);
    }

    fn finalizing(recording: &mut Recording, job: JobId) {
        active(recording, job);
        recording.apply(
            RecordingEvent::Stop {
                action: StopAction::Save,
                source: StopSource::Ui,
            },
            2_000,
        );
    }

    fn notify() -> RecordingEffect {
        RecordingEffect::Notify {
            level: NoticeLevel::Error,
            summary: "Recording failed".into(),
            detail: "no monitor".into(),
        }
    }

    #[test]
    fn start_from_idle_spawns_capture() {
        let mut recording = Recording::new();
        let effects = start(&mut recording, 7);
        assert_eq!(recording.job, Some(7));
        assert!(matches!(
            &recording.view,
            RecordingState::Starting {
                started_ms: 1_000,
                ..
            }
        ));
        assert_eq!(
            effects,
            vec![RecordingEffect::Spawn {
                job: 7,
                target: target(),
                profile: RecordProfile::App,
                hide: true,
            }]
        );
    }

    #[test]
    fn start_is_rejected_when_busy() {
        for view in [
            RecordingState::Starting {
                started_ms: 0,
                target: target(),
            },
            RecordingState::Active {
                job: 7,
                target: target(),
                started_ms: 0,
                paused: false,
                elapsed_ms: 0,
                sources: Vec::new(),
                has_audio: false,
                output: OutputKind::Video(VideoContainer::Mkv),
            },
            RecordingState::Finalizing {
                job: 7,
                recorded_ms: 10,
                progress: None,
            },
        ] {
            let mut recording = Recording { view, job: Some(7) };
            let before = recording.clone();
            assert!(start(&mut recording, 8).is_empty());
            assert_eq!(recording, before);
        }
    }

    #[test]
    fn started_moves_starting_to_active() {
        let mut recording = Recording::new();
        start(&mut recording, 7);
        let effects = started(&mut recording, 7);
        assert!(effects.is_empty());
        assert_eq!(recording.job, Some(7));
        assert!(matches!(
            &recording.view,
            RecordingState::Active {
                job: 7,
                started_ms: 1_100,
                paused: false,
                elapsed_ms: 0,
                has_audio: true,
                ..
            }
        ));
    }

    #[test]
    fn started_with_stale_job_is_ignored() {
        let mut recording = Recording::new();
        start(&mut recording, 7);
        let before = recording.clone();
        assert!(started(&mut recording, 8).is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn start_failed_returns_to_idle() {
        let mut recording = Recording::new();
        start(&mut recording, 7);
        let effects = recording.apply(
            RecordingEvent::StartFailed {
                job: 7,
                message: "no monitor".into(),
            },
            1_100,
        );
        assert_eq!(recording.view, RecordingState::Idle);
        assert_eq!(recording.job, None);
        assert_eq!(effects, vec![notify(), RecordingEffect::Tray]);
    }

    #[test]
    fn start_failed_with_stale_job_is_ignored() {
        let mut recording = Recording::new();
        start(&mut recording, 7);
        let before = recording.clone();
        let effects = recording.apply(
            RecordingEvent::StartFailed {
                job: 8,
                message: "no monitor".into(),
            },
            1_100,
        );
        assert!(effects.is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn cancel_start_cancels_the_current_job() {
        let mut recording = Recording::new();
        start(&mut recording, 7);
        let effects = recording.apply(RecordingEvent::CancelStart, 1_100);
        assert_eq!(recording.view, RecordingState::Idle);
        assert_eq!(recording.job, None);
        assert_eq!(effects, vec![RecordingEffect::Cancel { job: 7 }]);
    }

    #[test]
    fn cancel_start_is_ignored_outside_starting() {
        let mut recording = Recording::new();
        assert!(recording.apply(RecordingEvent::CancelStart, 0).is_empty());
        assert_eq!(recording.view, RecordingState::Idle);
        active(&mut recording, 7);
        let before = recording.clone();
        assert!(recording.apply(RecordingEvent::CancelStart, 0).is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn pause_and_resume_track_elapsed_time() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        recording.apply(RecordingEvent::Tick { now_ms: 1_600 }, 1_600);
        assert!(matches!(
            &recording.view,
            RecordingState::Active {
                paused: false,
                elapsed_ms: 500,
                ..
            }
        ));
        assert_eq!(
            recording.apply(RecordingEvent::Pause, 1_700),
            vec![RecordingEffect::Tray]
        );
        assert!(matches!(
            &recording.view,
            RecordingState::Active {
                paused: true,
                elapsed_ms: 600,
                ..
            }
        ));
        recording.apply(RecordingEvent::Tick { now_ms: 2_000 }, 2_000);
        assert!(matches!(
            &recording.view,
            RecordingState::Active {
                paused: true,
                elapsed_ms: 600,
                ..
            }
        ));
        assert_eq!(
            recording.apply(RecordingEvent::Resume, 2_500),
            vec![RecordingEffect::Tray]
        );
        recording.apply(RecordingEvent::Tick { now_ms: 2_900 }, 2_900);
        assert!(matches!(
            &recording.view,
            RecordingState::Active {
                paused: false,
                elapsed_ms: 1_000,
                ..
            }
        ));
    }

    #[test]
    fn pause_and_resume_are_idempotent() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        assert_eq!(
            recording.apply(RecordingEvent::Pause, 1_200),
            vec![RecordingEffect::Tray]
        );
        let after_pause = recording.clone();
        assert!(recording.apply(RecordingEvent::Pause, 1_300).is_empty());
        assert_eq!(recording, after_pause);
        recording.apply(RecordingEvent::Resume, 1_400);
        let after_resume = recording.clone();
        assert!(recording.apply(RecordingEvent::Resume, 1_500).is_empty());
        assert_eq!(recording, after_resume);
    }

    #[test]
    fn stop_save_starts_finalizing() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        recording.apply(RecordingEvent::Tick { now_ms: 1_900 }, 1_900);
        let effects = recording.apply(
            RecordingEvent::Stop {
                action: StopAction::Save,
                source: StopSource::Hotkey,
            },
            2_000,
        );
        assert!(matches!(
            &recording.view,
            RecordingState::Finalizing {
                job: 7,
                recorded_ms: 900,
                progress: None,
            }
        ));
        assert_eq!(
            effects,
            vec![
                RecordingEffect::Stop {
                    job: 7,
                    action: StopAction::Save,
                    source: StopSource::Hotkey,
                },
                RecordingEffect::Finalize {
                    job: 7,
                    discard: false,
                },
            ]
        );
    }

    #[test]
    fn stop_discard_starts_discarding_finalize() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        let effects = recording.apply(
            RecordingEvent::Stop {
                action: StopAction::Discard,
                source: StopSource::Tray,
            },
            1_500,
        );
        assert_eq!(
            effects,
            vec![
                RecordingEffect::Stop {
                    job: 7,
                    action: StopAction::Discard,
                    source: StopSource::Tray,
                },
                RecordingEffect::Finalize {
                    job: 7,
                    discard: true,
                },
            ]
        );
    }

    #[test]
    fn stop_outside_active_is_ignored() {
        let mut idle = Recording::new();
        assert!(idle
            .apply(
                RecordingEvent::Stop {
                    action: StopAction::Save,
                    source: StopSource::Ui,
                },
                10,
            )
            .is_empty());
        let mut recording = Recording::new();
        start(&mut recording, 7);
        let before = recording.clone();
        assert!(recording
            .apply(
                RecordingEvent::Stop {
                    action: StopAction::Save,
                    source: StopSource::Ui,
                },
                10,
            )
            .is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn finalize_progress_updates_only_the_current_job() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        assert!(recording
            .apply(
                RecordingEvent::FinalizeProgress {
                    job: 7,
                    progress: Some(0.4),
                },
                2_100,
            )
            .is_empty());
        assert!(matches!(
            &recording.view,
            RecordingState::Finalizing {
                progress: Some(value),
                ..
            } if (*value - 0.4).abs() < f32::EPSILON
        ));
        let before = recording.clone();
        recording.apply(
            RecordingEvent::FinalizeProgress {
                job: 8,
                progress: Some(0.9),
            },
            2_200,
        );
        assert_eq!(recording, before);
    }

    #[test]
    fn finished_publishes_results() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        let effects = recording.apply(
            RecordingEvent::Finished {
                job: 7,
                paths: vec!["C:\\media\\clip.mkv".into()],
                sound: true,
            },
            3_000,
        );
        assert_eq!(recording.view, RecordingState::Idle);
        assert_eq!(recording.job, None);
        assert_eq!(effects.len(), 5);
        assert_eq!(
            effects[0],
            RecordingEffect::AddLibrary(vec!["C:\\media\\clip.mkv".into()])
        );
        assert_eq!(
            effects[1],
            RecordingEffect::CopyFiles(vec!["C:\\media\\clip.mkv".into()])
        );
        assert_eq!(effects[2], RecordingEffect::PlaySound);
        assert!(matches!(
            &effects[3],
            RecordingEffect::Notify {
                level: NoticeLevel::Success,
                ..
            }
        ));
        assert_eq!(effects[4], RecordingEffect::Tray);
    }

    #[test]
    fn finished_without_paths_still_notifies() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        let effects = recording.apply(
            RecordingEvent::Finished {
                job: 7,
                paths: Vec::new(),
                sound: false,
            },
            3_000,
        );
        assert_eq!(
            effects,
            vec![
                RecordingEffect::Notify {
                    level: NoticeLevel::Success,
                    summary: "Recording saved".into(),
                    detail: String::new(),
                },
                RecordingEffect::Tray,
            ]
        );
    }

    #[test]
    fn finished_with_stale_job_is_ignored() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        let before = recording.clone();
        assert!(recording
            .apply(
                RecordingEvent::Finished {
                    job: 8,
                    paths: Vec::new(),
                    sound: false,
                },
                3_000,
            )
            .is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn failed_returns_to_idle_with_notice() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        let effects = recording.apply(
            RecordingEvent::Failed {
                job: 7,
                message: "ffmpeg failed".into(),
            },
            3_000,
        );
        assert_eq!(recording.view, RecordingState::Idle);
        assert_eq!(recording.job, None);
        assert_eq!(
            effects,
            vec![
                RecordingEffect::Notify {
                    level: NoticeLevel::Error,
                    summary: "Recording failed".into(),
                    detail: "ffmpeg failed".into(),
                },
                RecordingEffect::Tray,
            ]
        );
    }

    #[test]
    fn failed_with_stale_job_is_ignored() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        let before = recording.clone();
        assert!(recording
            .apply(
                RecordingEvent::Failed {
                    job: 8,
                    message: "ffmpeg failed".into(),
                },
                3_000,
            )
            .is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn cancelled_returns_to_idle() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        let effects = recording.apply(RecordingEvent::Cancelled { job: 7 }, 3_000);
        assert_eq!(recording.view, RecordingState::Idle);
        assert_eq!(recording.job, None);
        assert!(matches!(
            &effects[0],
            RecordingEffect::Notify {
                level: NoticeLevel::Warning,
                ..
            }
        ));
        assert_eq!(effects[1], RecordingEffect::Tray);
    }

    #[test]
    fn cancel_finalize_requests_cancellation_without_leaving_finalizing() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        let effects = recording.apply(RecordingEvent::CancelFinalize, 3_000);
        assert_eq!(effects, vec![RecordingEffect::Cancel { job: 7 }]);
        assert!(matches!(
            &recording.view,
            RecordingState::Finalizing { job: 7, .. }
        ));
    }

    #[test]
    fn cancel_finalize_ignores_non_finalizing_states() {
        let mut recording = Recording::new();
        assert!(recording
            .apply(RecordingEvent::CancelFinalize, 3_000)
            .is_empty());
        active(&mut recording, 7);
        let before = recording.clone();
        assert!(recording
            .apply(RecordingEvent::CancelFinalize, 3_000)
            .is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn tick_updates_active_elapsed() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        recording.apply(RecordingEvent::Tick { now_ms: 1_250 }, 1_250);
        assert!(matches!(
            &recording.view,
            RecordingState::Active {
                elapsed_ms: 150,
                ..
            }
        ));
    }

    #[test]
    fn tick_is_ignored_while_paused_and_outside_active() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        recording.apply(RecordingEvent::Pause, 1_200);
        let paused = recording.clone();
        recording.apply(RecordingEvent::Tick { now_ms: 9_000 }, 9_000);
        assert_eq!(recording, paused);
        let mut idle = Recording::new();
        assert!(idle
            .apply(RecordingEvent::Tick { now_ms: 9_000 }, 9_000)
            .is_empty());
        assert_eq!(idle.view, RecordingState::Idle);
    }

    #[test]
    fn session_events_notify_without_changing_the_view() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        for (event, level) in [
            (
                SessionEvent::Warning("low disk".into()),
                NoticeLevel::Warning,
            ),
            (
                SessionEvent::SourceLost("monitor".into()),
                NoticeLevel::Warning,
            ),
            (
                SessionEvent::SourceRestored("monitor".into()),
                NoticeLevel::Info,
            ),
            (
                SessionEvent::CaptureFailed("frame".into()),
                NoticeLevel::Error,
            ),
        ] {
            let before = recording.clone();
            let effects = recording.apply(RecordingEvent::Session { job: 7, event }, 1_200);
            assert_eq!(recording, before);
            assert!(matches!(
                &effects[0],
                RecordingEffect::Notify { level: actual, .. } if *actual == level
            ));
        }
    }

    #[test]
    fn session_events_from_a_stale_job_are_ignored() {
        let mut recording = Recording::new();
        active(&mut recording, 7);
        let before = recording.clone();
        assert!(recording
            .apply(
                RecordingEvent::Session {
                    job: 8,
                    event: SessionEvent::Warning("low disk".into()),
                },
                1_200,
            )
            .is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn session_events_before_active_are_ignored() {
        let mut recording = Recording::new();
        start(&mut recording, 7);
        let before = recording.clone();
        assert!(recording
            .apply(
                RecordingEvent::Session {
                    job: 7,
                    event: SessionEvent::Warning("low disk".into()),
                },
                1_200,
            )
            .is_empty());
        assert_eq!(recording, before);
    }

    #[test]
    fn a_new_recording_can_start_after_the_previous_one_finished() {
        let mut recording = Recording::new();
        finalizing(&mut recording, 7);
        recording.apply(
            RecordingEvent::Finished {
                job: 7,
                paths: Vec::new(),
                sound: false,
            },
            3_000,
        );
        let effects = start(&mut recording, 8);
        assert_eq!(recording.job, Some(8));
        assert_eq!(
            effects,
            vec![RecordingEffect::Spawn {
                job: 8,
                target: target(),
                profile: RecordProfile::App,
                hide: true,
            }]
        );
    }

    #[test]
    fn events_in_idle_are_noops() {
        let mut recording = Recording::new();
        for event in [
            RecordingEvent::Pause,
            RecordingEvent::Resume,
            RecordingEvent::CancelStart,
            RecordingEvent::CancelFinalize,
            RecordingEvent::Finished {
                job: 1,
                paths: Vec::new(),
                sound: false,
            },
            RecordingEvent::Failed {
                job: 1,
                message: "x".into(),
            },
            RecordingEvent::Cancelled { job: 1 },
            RecordingEvent::FinalizeProgress {
                job: 1,
                progress: None,
            },
        ] {
            let before = recording.clone();
            assert!(recording.apply(event, 5).is_empty());
            assert_eq!(recording, before);
        }
    }
}
