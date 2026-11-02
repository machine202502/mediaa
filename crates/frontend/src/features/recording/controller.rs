use super::types::*;

#[cfg(test)]
impl ViewState {
    pub(crate) fn refresh(&mut self, controller: &RecordingController) {
        self.apply(controller.view(), controller.page());
    }
}
use crate::features::jobs::Job;
use crate::features::navigation::Page;
use crossbeam_channel::Receiver;
use mediaa_media::error::MediaErrorKind;
use mediaa_media::recorder::{
    RecordingProgress, RecordingRequest, RecordingSession, SessionEvent, StopAction,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Instant;

pub(crate) enum RecordingStartFailure {
    Cancelled,
    Failed {
        kind: MediaErrorKind,
        message: String,
    },
}

pub(crate) enum RecState {
    Idle,
    Starting {
        job: Job<Result<RecordingSession, RecordingStartFailure>>,
        started: Instant,
        page: Page,
    },
    Recording {
        session: Box<RecordingSession>,
        events: Receiver<SessionEvent>,
    },
    Saving {
        page: Page,
        events: Receiver<SessionEvent>,
        progress_state: RecordingProgress,
        progress: Option<f32>,
        started: Instant,
        recorded: std::time::Duration,
        discarding: bool,
        cancel: Arc<AtomicBool>,
        sound_on_completion: bool,
        finalizer: Option<JoinHandle<()>>,
        terminal: Option<RecordingTerminal>,
    },
}

pub(crate) enum RecordingTerminal {
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed(mediaa_media::recorder::SessionFailure),
    Lost,
}

pub(crate) enum RecordingStartEvent {
    Started {
        samples: Vec<String>,
        discarded: bool,
    },
    Cancelled,
    Failed {
        kind: MediaErrorKind,
        message: String,
    },
    WorkerStopped,
}

pub(crate) enum RecordingEvent {
    Warning(String),
    SessionEvent(SessionEvent),
    Finished {
        paths: Vec<PathBuf>,
        sound: bool,
        finalizer_panicked: bool,
    },
    Cancelled {
        finalizer_panicked: bool,
    },
    Failed {
        failure: mediaa_media::recorder::SessionFailure,
        finalizer_panicked: bool,
    },
    Lost {
        finalizer_panicked: bool,
    },
}

pub(crate) struct StopResult {
    #[allow(dead_code, reason = "the daemon sends the page in the snapshot")]
    pub page: Page,
}

pub(crate) struct RecordingController {
    state: RecState,
}

impl Default for RecordingController {
    fn default() -> Self {
        Self {
            state: RecState::Idle,
        }
    }
}

impl RecordingController {
    pub(crate) fn is_idle(&self) -> bool {
        matches!(self.state, RecState::Idle)
    }

    #[cfg(test)]
    pub(crate) fn is_paused(&self) -> bool {
        matches!(
            &self.state,
            RecState::Recording { session, .. } if session.is_paused()
        )
    }

    pub(crate) fn view(&self) -> RecordingView {
        match &self.state {
            RecState::Idle => RecordingView::Idle,
            RecState::Starting { started, .. } => RecordingView::Starting(started.elapsed()),
            RecState::Recording { session, .. } => RecordingView::Recording(RecordingInfo {
                output: session.output(),
                paused: session.is_paused(),
                elapsed: session.elapsed(),
                has_audio: session.has_audio(),
                sources: session.source_names(),
                frames: session.frames_captured(),
            }),
            RecState::Saving {
                progress,
                started,
                recorded,
                discarding,
                ..
            } => RecordingView::Saving {
                progress: *progress,
                elapsed: started.elapsed(),
                recorded: *recorded,
                discarding: *discarding,
            },
        }
    }

    pub(crate) fn page(&self) -> Page {
        match &self.state {
            RecState::Starting { page, .. } => *page,
            RecState::Saving { page, .. } => *page,
            RecState::Recording { session, .. }
                if matches!(session.output(), mediaa_core::OutputKind::Audio(_)) =>
            {
                Page::RecordAudio
            }
            _ => Page::Record,
        }
    }

    pub(crate) fn start(
        &mut self,
        request: RecordingRequest,
        started: Instant,
        page: Page,
    ) -> anyhow::Result<()> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let job = Job::spawn("recording-start", cancel, move || {
            RecordingSession::start_with_cancel(request, &worker_cancel).map_err(|error| {
                if error.is::<mediaa_media::recorder::RecordingStartCancelled>() {
                    RecordingStartFailure::Cancelled
                } else {
                    RecordingStartFailure::Failed {
                        kind: mediaa_media::error::classify_media_error(&error),
                        message: format!("{error:#}"),
                    }
                }
            })
        })?;
        self.state = RecState::Starting { job, started, page };
        Ok(())
    }

    pub(crate) fn cancel_start(&mut self) -> bool {
        let RecState::Starting { job, .. } = &self.state else {
            return false;
        };
        job.cancel();
        true
    }

    pub(crate) fn toggle_pause(&mut self) {
        if let RecState::Recording { session, .. } = &self.state {
            if session.is_paused() {
                session.resume();
            } else {
                session.pause();
            }
        }
    }

    pub(crate) fn stop(&mut self, action: StopAction, source: StopSource) -> Option<StopResult> {
        match std::mem::replace(&mut self.state, RecState::Idle) {
            RecState::Starting { job, started, page } => {
                job.cancel();
                self.state = RecState::Starting { job, started, page };
                None
            }
            RecState::Recording { session, events } => {
                let cancel = Arc::new(AtomicBool::new(false));
                let output = session.output();
                let page = if matches!(output, mediaa_core::OutputKind::Audio(_)) {
                    Page::RecordAudio
                } else {
                    Page::Record
                };
                let sound = matches!(source, StopSource::Tray | StopSource::Hotkey);
                let recorded = session.elapsed();
                let progress_state = session.progress();
                let finalizer = match session.stop(action, cancel.clone()) {
                    Ok(handle) => Some(handle),
                    Err(error) => {
                        log::warn!("finalizer ran inline: {error:#}");
                        None
                    }
                };
                self.state = RecState::Saving {
                    page,
                    events,
                    progress_state,
                    progress: None,
                    started: Instant::now(),
                    recorded,
                    discarding: matches!(action, StopAction::Discard),
                    cancel,
                    sound_on_completion: sound,
                    finalizer,
                    terminal: None,
                };
                Some(StopResult { page })
            }
            other => {
                self.state = other;
                None
            }
        }
    }

    pub(crate) fn poll_start(&mut self, discard: bool) -> Option<RecordingStartEvent> {
        let RecState::Starting { job, started, page } = &mut self.state else {
            return None;
        };
        let result = job.poll()?;
        let started = *started;
        let page = *page;
        let result = match result {
            Ok(result) => result,
            Err(_) => {
                self.state = RecState::Idle;
                return Some(RecordingStartEvent::WorkerStopped);
            }
        };
        match result {
            Ok(session) => {
                let samples = session.source_names();
                let events = session.events();
                if discard {
                    let cancel = Arc::new(AtomicBool::new(false));
                    let recorded = session.elapsed();
                    let progress_state = session.progress();
                    let finalizer = session.stop(StopAction::Discard, cancel.clone()).ok();
                    self.state = RecState::Saving {
                        page,
                        events,
                        progress_state,
                        progress: None,
                        started,
                        recorded,
                        discarding: discard,
                        cancel,
                        sound_on_completion: false,
                        finalizer,
                        terminal: None,
                    };
                    Some(RecordingStartEvent::Started {
                        samples,
                        discarded: true,
                    })
                } else {
                    self.state = RecState::Recording {
                        session: Box::new(session),
                        events,
                    };
                    Some(RecordingStartEvent::Started {
                        samples,
                        discarded: false,
                    })
                }
            }
            Err(RecordingStartFailure::Cancelled) => {
                self.state = RecState::Idle;
                Some(RecordingStartEvent::Cancelled)
            }
            Err(RecordingStartFailure::Failed { kind, message }) => {
                self.state = RecState::Idle;
                Some(RecordingStartEvent::Failed { kind, message })
            }
        }
    }

    pub(crate) fn poll_events(&mut self) -> Vec<RecordingEvent> {
        if let RecState::Saving {
            progress,
            progress_state,
            ..
        } = &mut self.state
        {
            *progress = progress_state.fraction();
        }
        let events = match &self.state {
            RecState::Recording { events, .. } | RecState::Saving { events, .. } => events.clone(),
            _ => return Vec::new(),
        };
        let mut result = Vec::new();
        loop {
            match events.try_recv() {
                Ok(SessionEvent::Warning(warning)) => {
                    result.push(RecordingEvent::Warning(warning));
                }
                Ok(SessionEvent::Notice(notice)) => {
                    result.push(RecordingEvent::SessionEvent(SessionEvent::Notice(notice)));
                }
                Ok(SessionEvent::CaptureFailed(error)) => {
                    result.push(RecordingEvent::SessionEvent(SessionEvent::CaptureFailed(
                        error,
                    )));
                }
                Ok(SessionEvent::SourceLost(name)) => {
                    result.push(RecordingEvent::SessionEvent(SessionEvent::SourceLost(name)));
                }
                Ok(SessionEvent::SourceRestored(name)) => {
                    result.push(RecordingEvent::SessionEvent(SessionEvent::SourceRestored(
                        name,
                    )));
                }
                Ok(SessionEvent::Finished(paths)) => {
                    self.set_terminal(RecordingTerminal::Finished(paths));
                    break;
                }
                Ok(SessionEvent::Cancelled) => {
                    self.set_terminal(RecordingTerminal::Cancelled);
                    break;
                }
                Ok(SessionEvent::Failed(failure)) => {
                    self.set_terminal(RecordingTerminal::Failed(failure));
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.set_terminal(RecordingTerminal::Lost);
                    break;
                }
            }
        }
        if let Some(event) = self.complete_terminal() {
            result.push(event);
        }
        result
    }

    fn set_terminal(&mut self, terminal: RecordingTerminal) {
        if let RecState::Saving { terminal: slot, .. } = &mut self.state {
            *slot = Some(terminal);
        }
    }

    fn complete_terminal(&mut self) -> Option<RecordingEvent> {
        let RecState::Saving {
            sound_on_completion,
            finalizer,
            terminal,
            ..
        } = &mut self.state
        else {
            return None;
        };
        let sound = *sound_on_completion;
        if finalizer
            .as_ref()
            .is_some_and(|handle| !handle.is_finished())
        {
            return None;
        }
        let finalizer_panicked = finalizer
            .take()
            .is_some_and(|handle| handle.join().is_err());
        let terminal = terminal.take()?;
        self.state = RecState::Idle;
        Some(match terminal {
            RecordingTerminal::Finished(paths) => RecordingEvent::Finished {
                paths,
                sound,
                finalizer_panicked,
            },
            RecordingTerminal::Cancelled => RecordingEvent::Cancelled { finalizer_panicked },
            RecordingTerminal::Failed(failure) => RecordingEvent::Failed {
                failure,
                finalizer_panicked,
            },
            RecordingTerminal::Lost => RecordingEvent::Lost { finalizer_panicked },
        })
    }

    pub(crate) fn cancel_finalize(&self) {
        if let RecState::Saving { cancel, .. } = &self.state {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    #[cfg(test)]
    pub(crate) fn test_recording(
        &mut self,
        session: RecordingSession,
        events: Receiver<SessionEvent>,
    ) {
        self.state = RecState::Recording {
            session: Box::new(session),
            events,
        };
    }

    #[cfg(test)]
    pub(crate) fn test_starting(&mut self, page: Page) {
        let job = Job::spawn("test-starting", Arc::default(), || {
            std::thread::sleep(std::time::Duration::from_secs(30));
            Err(RecordingStartFailure::Cancelled)
        })
        .expect("starting test job");
        self.state = RecState::Starting {
            job,
            started: Instant::now(),
            page,
        };
    }
    #[cfg(test)]
    pub(crate) fn test_saving(
        &mut self,
        events: Receiver<SessionEvent>,
        recorded: std::time::Duration,
    ) {
        self.test_saving_on(events, recorded, Page::Record);
    }

    #[cfg(test)]
    pub(crate) fn test_saving_on(
        &mut self,
        events: Receiver<SessionEvent>,
        recorded: std::time::Duration,
        page: Page,
    ) {
        self.state = RecState::Saving {
            page,
            events,
            progress_state: RecordingProgress::default(),
            progress: None,
            started: Instant::now(),
            recorded,
            discarding: false,
            cancel: Arc::default(),
            sound_on_completion: false,
            finalizer: None,
            terminal: None,
        };
    }

    #[cfg(test)]
    pub(crate) fn test_saving_progress(
        &mut self,
        events: Receiver<SessionEvent>,
        recorded: std::time::Duration,
        progress: Option<f32>,
    ) {
        self.state = RecState::Saving {
            page: Page::Record,
            events,
            progress_state: RecordingProgress::default(),
            progress,
            started: Instant::now(),
            recorded,
            discarding: false,
            cancel: Arc::default(),
            sound_on_completion: false,
            finalizer: None,
            terminal: None,
        };
    }
    #[cfg(test)]
    pub(crate) fn saving_sound(&self) -> Option<bool> {
        match &self.state {
            RecState::Saving {
                sound_on_completion,
                ..
            } => Some(*sound_on_completion),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_recording_view_survives_the_wire() {
        let view = RecordingView::Saving {
            progress: Some(0.25),
            elapsed: Duration::from_millis(120),
            recorded: Duration::from_secs(3),
            discarding: false,
        };
        let line = serde_json::to_string(&view).expect("serialize");
        let parsed: RecordingView = serde_json::from_str(&line).expect("deserialize");
        assert_eq!(serde_json::to_string(&parsed).expect("serialize"), line);
    }

    #[test]
    fn applying_a_view_updates_the_phase() {
        let mut state = ViewState::default();
        assert!(state.is_idle());
        state.apply(
            RecordingView::Starting(Duration::from_millis(40)),
            Page::Record,
        );
        assert!(state.is_starting());
    }
}
