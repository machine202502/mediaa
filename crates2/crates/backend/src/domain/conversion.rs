use crate::domain::JobId;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_protocol::state::ConversionState;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum ConversionEvent {
    Start {
        job: JobId,
        total: u32,
    },
    Progress {
        job: JobId,
        done: u32,
        current: Option<String>,
        progress: Option<f32>,
    },
    Finished {
        job: JobId,
        path: PathBuf,
    },
    Failed {
        job: JobId,
        message: String,
    },
    Cancel,
    Cancelled {
        job: JobId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConversionEffect {
    StartJob,
    CancelJob,
    AddLibrary(PathBuf),
    Notify {
        level: NoticeLevel,
        summary: String,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conversion {
    pub view: ConversionState,
    pub job: Option<JobId>,
}

impl Default for Conversion {
    fn default() -> Self {
        Self {
            view: ConversionState::Idle,
            job: None,
        }
    }
}

impl Conversion {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: ConversionEvent) -> Vec<ConversionEffect> {
        match event {
            ConversionEvent::Start { job, total } => {
                if !matches!(&self.view, ConversionState::Idle) || self.job.is_some() {
                    return Vec::new();
                }
                self.job = Some(job);
                self.view = ConversionState::Running {
                    job,
                    done: 0,
                    total,
                    progress: None,
                    current: None,
                };
                vec![ConversionEffect::StartJob]
            }
            ConversionEvent::Progress {
                job,
                done,
                current,
                progress,
            } => {
                let ConversionState::Running {
                    job: active,
                    done: current_done,
                    current: current_name,
                    progress: current_progress,
                    ..
                } = &mut self.view
                else {
                    return Vec::new();
                };
                if *active != job || self.job != Some(job) {
                    return Vec::new();
                }
                *current_done = done;
                *current_name = current;
                *current_progress = progress;
                Vec::new()
            }
            ConversionEvent::Finished { job, path } => {
                if self.job != Some(job) || !self.accepts_terminal(job) {
                    return Vec::new();
                }
                self.view = ConversionState::Idle;
                self.job = None;
                vec![
                    ConversionEffect::AddLibrary(path.clone()),
                    notice(
                        NoticeLevel::Success,
                        "Conversion finished",
                        path.display().to_string(),
                    ),
                ]
            }
            ConversionEvent::Failed { job, message } => {
                if self.job != Some(job) || !self.accepts_terminal(job) {
                    return Vec::new();
                }
                self.view = ConversionState::Idle;
                self.job = None;
                vec![notice(NoticeLevel::Error, "Conversion failed", message)]
            }
            ConversionEvent::Cancel => {
                if !matches!(&self.view, ConversionState::Running { .. }) {
                    return Vec::new();
                }
                self.view = ConversionState::Cancelling;
                vec![ConversionEffect::CancelJob]
            }
            ConversionEvent::Cancelled { job } => {
                if self.job != Some(job) || !self.accepts_terminal(job) {
                    return Vec::new();
                }
                self.view = ConversionState::Idle;
                self.job = None;
                vec![notice(
                    NoticeLevel::Warning,
                    "Conversion cancelled",
                    String::new(),
                )]
            }
        }
    }

    fn accepts_terminal(&self, job: JobId) -> bool {
        match &self.view {
            ConversionState::Running { job: active, .. } => *active == job,
            ConversionState::Cancelling => self.job == Some(job),
            ConversionState::Idle => false,
        }
    }
}

fn notice(
    level: NoticeLevel,
    summary: impl Into<String>,
    detail: impl Into<String>,
) -> ConversionEffect {
    ConversionEffect::Notify {
        level,
        summary: summary.into(),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn running(conversion: &mut Conversion, job: JobId) {
        conversion.apply(ConversionEvent::Start { job, total: 3 });
    }

    #[test]
    fn start_runs_the_job() {
        let mut conversion = Conversion::new();
        let effects = conversion.apply(ConversionEvent::Start { job: 5, total: 4 });
        assert_eq!(conversion.job, Some(5));
        assert_eq!(
            conversion.view,
            ConversionState::Running {
                job: 5,
                done: 0,
                total: 4,
                progress: None,
                current: None,
            }
        );
        assert_eq!(effects, vec![ConversionEffect::StartJob]);
    }

    #[test]
    fn start_is_rejected_while_running() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let before = conversion.clone();
        assert!(conversion
            .apply(ConversionEvent::Start { job: 6, total: 1 })
            .is_empty());
        assert_eq!(conversion, before);
    }

    #[test]
    fn progress_updates_the_current_job() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let effects = conversion.apply(ConversionEvent::Progress {
            job: 5,
            done: 2,
            current: Some("clip.mp4".into()),
            progress: Some(0.5),
        });
        assert!(effects.is_empty());
        assert_eq!(
            conversion.view,
            ConversionState::Running {
                job: 5,
                done: 2,
                total: 3,
                progress: Some(0.5),
                current: Some("clip.mp4".into()),
            }
        );
    }

    #[test]
    fn progress_from_a_stale_job_is_ignored() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let before = conversion.clone();
        assert!(conversion
            .apply(ConversionEvent::Progress {
                job: 6,
                done: 3,
                current: None,
                progress: Some(1.0),
            })
            .is_empty());
        assert_eq!(conversion, before);
    }

    #[test]
    fn finished_adds_to_the_library_and_notifies() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let effects = conversion.apply(ConversionEvent::Finished {
            job: 5,
            path: "C:\\media\\clip.gif".into(),
        });
        assert_eq!(conversion.view, ConversionState::Idle);
        assert_eq!(conversion.job, None);
        assert_eq!(
            effects,
            vec![
                ConversionEffect::AddLibrary("C:\\media\\clip.gif".into()),
                ConversionEffect::Notify {
                    level: NoticeLevel::Success,
                    summary: "Conversion finished".into(),
                    detail: "C:\\media\\clip.gif".into(),
                },
            ]
        );
    }

    #[test]
    fn finished_from_a_stale_job_is_ignored() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let before = conversion.clone();
        assert!(conversion
            .apply(ConversionEvent::Finished {
                job: 6,
                path: "C:\\media\\clip.gif".into(),
            })
            .is_empty());
        assert_eq!(conversion, before);
    }

    #[test]
    fn failed_returns_to_idle_with_a_notice() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let effects = conversion.apply(ConversionEvent::Failed {
            job: 5,
            message: "codec error".into(),
        });
        assert_eq!(conversion.view, ConversionState::Idle);
        assert_eq!(conversion.job, None);
        assert_eq!(
            effects,
            vec![ConversionEffect::Notify {
                level: NoticeLevel::Error,
                summary: "Conversion failed".into(),
                detail: "codec error".into(),
            }]
        );
    }

    #[test]
    fn failed_from_a_stale_job_is_ignored() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let before = conversion.clone();
        assert!(conversion
            .apply(ConversionEvent::Failed {
                job: 6,
                message: "codec error".into(),
            })
            .is_empty());
        assert_eq!(conversion, before);
    }

    #[test]
    fn cancel_moves_running_to_cancelling() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        let effects = conversion.apply(ConversionEvent::Cancel);
        assert_eq!(conversion.view, ConversionState::Cancelling);
        assert_eq!(conversion.job, Some(5));
        assert_eq!(effects, vec![ConversionEffect::CancelJob]);
    }

    #[test]
    fn cancel_is_idempotent() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        conversion.apply(ConversionEvent::Cancel);
        let before = conversion.clone();
        assert!(conversion.apply(ConversionEvent::Cancel).is_empty());
        assert_eq!(conversion, before);
    }

    #[test]
    fn cancel_in_idle_is_ignored() {
        let mut conversion = Conversion::new();
        assert!(conversion.apply(ConversionEvent::Cancel).is_empty());
        assert_eq!(conversion.view, ConversionState::Idle);
    }

    #[test]
    fn cancelled_returns_to_idle() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        conversion.apply(ConversionEvent::Cancel);
        let effects = conversion.apply(ConversionEvent::Cancelled { job: 5 });
        assert_eq!(conversion.view, ConversionState::Idle);
        assert_eq!(conversion.job, None);
        assert_eq!(
            effects,
            vec![ConversionEffect::Notify {
                level: NoticeLevel::Warning,
                summary: "Conversion cancelled".into(),
                detail: String::new(),
            }]
        );
    }

    #[test]
    fn cancelled_from_a_stale_job_is_ignored() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        conversion.apply(ConversionEvent::Cancel);
        let before = conversion.clone();
        assert!(conversion
            .apply(ConversionEvent::Cancelled { job: 6 })
            .is_empty());
        assert_eq!(conversion, before);
    }

    #[test]
    fn progress_is_ignored_while_cancelling() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        conversion.apply(ConversionEvent::Cancel);
        let before = conversion.clone();
        assert!(conversion
            .apply(ConversionEvent::Progress {
                job: 5,
                done: 2,
                current: Some("clip.mp4".into()),
                progress: Some(0.7),
            })
            .is_empty());
        assert_eq!(conversion, before);
    }

    #[test]
    fn a_late_finish_during_cancel_is_accepted_once() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        conversion.apply(ConversionEvent::Cancel);
        let effects = conversion.apply(ConversionEvent::Finished {
            job: 5,
            path: "C:\\media\\clip.gif".into(),
        });
        assert_eq!(conversion.view, ConversionState::Idle);
        assert!(matches!(
            &effects[0],
            ConversionEffect::AddLibrary(path) if path.as_path() == std::path::Path::new("C:\\media\\clip.gif")
        ));
        let after = conversion.clone();
        assert!(conversion
            .apply(ConversionEvent::Cancelled { job: 5 })
            .is_empty());
        assert_eq!(conversion, after);
    }

    #[test]
    fn a_new_conversion_can_start_after_the_previous_one() {
        let mut conversion = Conversion::new();
        running(&mut conversion, 5);
        conversion.apply(ConversionEvent::Finished {
            job: 5,
            path: "C:\\media\\clip.gif".into(),
        });
        let effects = conversion.apply(ConversionEvent::Start { job: 6, total: 1 });
        assert_eq!(effects, vec![ConversionEffect::StartJob]);
        assert_eq!(conversion.job, Some(6));
    }

    #[test]
    fn events_in_idle_are_noops() {
        let mut conversion = Conversion::new();
        for event in [
            ConversionEvent::Progress {
                job: 1,
                done: 1,
                current: None,
                progress: Some(0.1),
            },
            ConversionEvent::Finished {
                job: 1,
                path: "x".into(),
            },
            ConversionEvent::Failed {
                job: 1,
                message: "x".into(),
            },
            ConversionEvent::Cancelled { job: 1 },
        ] {
            let before = conversion.clone();
            assert!(conversion.apply(event).is_empty());
            assert_eq!(conversion, before);
        }
    }
}
