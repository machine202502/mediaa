use crate::domain::recording::{
    Recording, RecordingEffect, RecordingEvent, SessionEvent as DomainSessionEvent,
};
use crate::domain::JobId;
use crate::services::{LibraryHandle, NoticeRing};
use crate::state::StateStore;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::paths;
use mediaa2_core::recording::{RecordTarget, StopAction, StopSource};
use mediaa2_core::settings::RecordProfile;
use mediaa2_media::recorder::{
    recover_incomplete_sessions, RecordingProgress, RecordingSession, SessionEvent, SessionNotice,
    StopAction as RecorderStopAction,
};
use mediaa2_protocol::state::RecordingState;
use mediaa2_protocol::{CommandError, CommandOk, ErrorCode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;
use tokio::sync::{mpsc, oneshot};

pub mod plan;
use plan::{plan, PlanError, RecordingPlan};

const TICK: Duration = Duration::from_millis(100);
const COMMAND_CAPACITY: usize = 32;

type Reply<T> = oneshot::Sender<Result<T, CommandError>>;

#[derive(Clone)]
pub struct RecordingHandle {
    tx: mpsc::Sender<Command>,
    state: Arc<StateStore>,
    hide: Arc<Mutex<Option<bool>>>,
}

impl RecordingHandle {
    pub fn new(state: Arc<StateStore>, library: LibraryHandle, notices: NoticeRing) -> Self {
        let (tx, commands) = mpsc::channel(COMMAND_CAPACITY);
        let published = state.snapshot().recording.clone();
        let actor = Actor {
            state: Arc::clone(&state),
            library,
            notices,
            recording: Recording::new(),
            published,
            next_job: 1,
            start: None,
            active: None,
            finalize: None,
            pending_action: None,
            pending_sound: false,
            tx: tx.clone(),
            commands,
            epoch: tokio::time::Instant::now(),
        };
        tokio::spawn(actor.run());
        Self {
            tx,
            state,
            hide: Arc::new(Mutex::new(None)),
        }
    }

    pub fn hide(&self) -> Option<bool> {
        *self.hide.lock().expect("recording hide lock poisoned")
    }

    pub async fn start(
        &self,
        target: RecordTarget,
        profile: RecordProfile,
        hide: Option<bool>,
    ) -> Result<CommandOk, CommandError> {
        if !matches!(self.state.snapshot().recording, RecordingState::Idle) {
            return Err(busy());
        }
        let hide = hide.unwrap_or_else(|| {
            let snapshot = self.state.snapshot();
            snapshot.settings.profile(profile).0.hide_to_tray
        });
        let result = self
            .send(|reply| Command::Start {
                target,
                profile,
                hide,
                reply,
            })
            .await?;
        *self.hide.lock().expect("recording hide lock poisoned") = Some(hide);
        Ok(result)
    }

    pub async fn stop(
        &self,
        action: StopAction,
        source: StopSource,
    ) -> Result<CommandOk, CommandError> {
        if !matches!(
            self.state.snapshot().recording,
            RecordingState::Active { .. }
        ) {
            return Err(invalid("no active recording"));
        }
        self.send(|reply| Command::Stop {
            action,
            source,
            reply,
        })
        .await
    }

    pub async fn pause(&self) -> Result<CommandOk, CommandError> {
        if !matches!(
            self.state.snapshot().recording,
            RecordingState::Active { .. }
        ) {
            return Err(invalid("no active recording"));
        }
        self.send(|reply| Command::TogglePause { reply }).await
    }

    pub async fn cancel_start(&self) -> Result<CommandOk, CommandError> {
        if !matches!(
            self.state.snapshot().recording,
            RecordingState::Starting { .. }
        ) {
            return Err(invalid("no recording is starting"));
        }
        self.send(|reply| Command::CancelStart { reply }).await
    }

    pub async fn cancel_finalize(&self) -> Result<CommandOk, CommandError> {
        if !matches!(
            self.state.snapshot().recording,
            RecordingState::Finalizing { .. }
        ) {
            return Err(invalid("no recording is finalizing"));
        }
        self.send(|reply| Command::CancelFinalize { reply }).await
    }

    pub async fn recover(&self) -> Result<CommandOk, CommandError> {
        let media_dir = {
            let snapshot = self.state.snapshot();
            if snapshot.settings.media_dir.as_os_str().is_empty() {
                paths::default_media_dir()
            } else {
                snapshot.settings.media_dir.clone()
            }
        };
        self.recover_from(paths::work_dir(), media_dir).await
    }

    pub async fn recover_from(
        &self,
        work_root: PathBuf,
        out_dir: PathBuf,
    ) -> Result<CommandOk, CommandError> {
        self.send(|reply| Command::Recover {
            work_root,
            out_dir,
            reply,
        })
        .await
    }

    async fn send(
        &self,
        build: impl FnOnce(Reply<CommandOk>) -> Command,
    ) -> Result<CommandOk, CommandError> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(build(reply))
            .await
            .map_err(|_| internal("recording actor is not running"))?;
        response
            .await
            .map_err(|_| internal("recording actor dropped the request"))?
    }
}

enum Command {
    Start {
        target: RecordTarget,
        profile: RecordProfile,
        hide: bool,
        reply: Reply<CommandOk>,
    },
    Stop {
        action: StopAction,
        source: StopSource,
        reply: Reply<CommandOk>,
    },
    TogglePause {
        reply: Reply<CommandOk>,
    },
    CancelStart {
        reply: Reply<CommandOk>,
    },
    CancelFinalize {
        reply: Reply<CommandOk>,
    },
    Recover {
        work_root: PathBuf,
        out_dir: PathBuf,
        reply: Reply<CommandOk>,
    },
    StartFinished {
        job: JobId,
        result: Result<RecordingSession, StartFailure>,
    },
}

struct StartFailure {
    cancelled: bool,
    message: String,
}

struct StartSlot {
    job: JobId,
    cancel: Arc<AtomicBool>,
}

struct ActiveSlot {
    job: JobId,
    session: RecordingSession,
    pump: EventPump,
}

struct FinalizeSlot {
    job: JobId,
    cancel: Arc<AtomicBool>,
    progress: RecordingProgress,
    events: EventPump,
    finalizer: Option<JoinHandle<()>>,
    terminal: Option<Terminal>,
    sound: bool,
    last_progress: Option<f32>,
}

enum Terminal {
    Finished(Vec<PathBuf>),
    Cancelled,
    Failed {
        message: String,
        files: Vec<PathBuf>,
    },
}

enum EventPoll {
    Event(SessionEvent),
    Empty,
    Disconnected,
}

type EventPump = Box<dyn FnMut() -> EventPoll + Send>;

struct Actor {
    state: Arc<StateStore>,
    library: LibraryHandle,
    notices: NoticeRing,
    recording: Recording,
    published: RecordingState,
    next_job: JobId,
    start: Option<StartSlot>,
    active: Option<ActiveSlot>,
    finalize: Option<FinalizeSlot>,
    pending_action: Option<(JobId, StopAction)>,
    pending_sound: bool,
    tx: mpsc::Sender<Command>,
    commands: mpsc::Receiver<Command>,
    epoch: tokio::time::Instant,
}

impl Actor {
    async fn run(mut self) {
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                command = self.commands.recv() => {
                    let Some(command) = command else { break };
                    self.handle(command).await;
                }
                _ = ticker.tick() => self.tick().await,
            }
        }
    }

    async fn handle(&mut self, command: Command) {
        match command {
            Command::Start {
                target,
                profile,
                hide,
                reply,
            } => self.start(target, profile, hide, reply).await,
            Command::Stop {
                action,
                source,
                reply,
            } => self.stop(action, source, reply).await,
            Command::TogglePause { reply } => self.toggle_pause(reply).await,
            Command::CancelStart { reply } => self.cancel_start(reply).await,
            Command::CancelFinalize { reply } => self.cancel_finalize(reply).await,
            Command::Recover {
                work_root,
                out_dir,
                reply,
            } => self.recover(work_root, out_dir, reply).await,
            Command::StartFinished { job, result } => self.start_finished(job, result).await,
        }
    }

    async fn start(
        &mut self,
        target: RecordTarget,
        profile: RecordProfile,
        hide: bool,
        reply: Reply<CommandOk>,
    ) {
        if !matches!(self.recording.view, RecordingState::Idle) || self.recording.job.is_some() {
            let _ = reply.send(Err(busy()));
            return;
        }
        let settings = self.state.snapshot().settings.clone();
        let planning = {
            let target = target.clone();
            tokio::task::spawn_blocking(move || plan(&settings, &target, profile, hide)).await
        };
        let plan = match planning {
            Ok(Ok(plan)) => plan,
            Ok(Err(error)) => {
                let _ = reply.send(Err(plan_error(error)));
                return;
            }
            Err(error) => {
                let _ = reply.send(Err(internal(format!(
                    "recording plan task failed: {error}"
                ))));
                return;
            }
        };
        let job = self.next_job;
        self.next_job = self.next_job.saturating_add(1);
        let effects = self.apply(RecordingEvent::Start {
            job,
            target,
            profile,
            hide,
        });
        if effects.is_empty() {
            let _ = reply.send(Err(busy()));
            return;
        }
        if let Some(label) = plan.allowed_warning.clone() {
            self.notices.push(
                &self.state,
                NoticeLevel::Warning,
                "Recording source is not available",
                label,
            );
        }
        self.begin_capture(job, plan);
        let _ = reply.send(Ok(CommandOk::Accepted));
        self.run_effects(effects).await;
    }

    fn begin_capture(&mut self, job: JobId, plan: RecordingPlan) {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let seen_cancel = Arc::clone(&cancel);
        self.start = Some(StartSlot { job, cancel });
        let request = plan.request;
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let started = tokio::task::spawn_blocking(move || {
                RecordingSession::start_with_cancel(request, worker_cancel.as_ref())
            })
            .await;
            let result = match started {
                Ok(Ok(session)) => Ok(session),
                Ok(Err(error)) => Err(StartFailure {
                    cancelled: seen_cancel.load(Ordering::Relaxed),
                    message: format!("{error:#}"),
                }),
                Err(error) => Err(StartFailure {
                    cancelled: seen_cancel.load(Ordering::Relaxed),
                    message: format!("recording start task failed: {error}"),
                }),
            };
            let _ = tx.send(Command::StartFinished { job, result }).await;
        });
    }

    async fn start_finished(&mut self, job: JobId, result: Result<RecordingSession, StartFailure>) {
        let Some(slot) = self.start.take() else {
            discard(result);
            return;
        };
        if slot.job != job
            || self.recording.job != Some(job)
            || !matches!(self.recording.view, RecordingState::Starting { .. })
        {
            discard(result);
            return;
        }
        match result {
            Ok(session) => {
                let sources = session.source_names();
                let has_audio = session.has_audio();
                let output = session.output();
                let pump = pump_from(&session);
                self.active = Some(ActiveSlot { job, session, pump });
                let effects = self.apply(RecordingEvent::Started {
                    job,
                    sources,
                    has_audio,
                    output,
                });
                self.run_effects(effects).await;
            }
            Err(failure) => {
                let effects = if failure.cancelled || slot.cancel.load(Ordering::Relaxed) {
                    self.apply(RecordingEvent::CancelStart)
                } else {
                    self.apply(RecordingEvent::StartFailed {
                        job,
                        message: failure.message,
                    })
                };
                self.run_effects(effects).await;
            }
        }
    }

    async fn stop(&mut self, action: StopAction, source: StopSource, reply: Reply<CommandOk>) {
        if !matches!(self.recording.view, RecordingState::Active { .. }) {
            let _ = reply.send(Err(invalid("no active recording")));
            return;
        }
        let effects = self.apply(RecordingEvent::Stop { action, source });
        if effects.is_empty() {
            let _ = reply.send(Err(invalid("no active recording")));
            return;
        }
        let _ = reply.send(Ok(CommandOk::Accepted));
        self.run_effects(effects).await;
    }

    async fn toggle_pause(&mut self, reply: Reply<CommandOk>) {
        let paused = match &self.recording.view {
            RecordingState::Active { paused, .. } => *paused,
            _ => {
                let _ = reply.send(Err(invalid("no active recording")));
                return;
            }
        };
        let effects = self.apply(if paused {
            RecordingEvent::Resume
        } else {
            RecordingEvent::Pause
        });
        if let Some(slot) = &self.active {
            if paused {
                slot.session.resume();
            } else {
                slot.session.pause();
            }
        }
        if effects.is_empty() {
            let _ = reply.send(Err(invalid("recording is not running")));
            return;
        }
        let _ = reply.send(Ok(CommandOk::Accepted));
        self.run_effects(effects).await;
    }

    async fn cancel_start(&mut self, reply: Reply<CommandOk>) {
        if !matches!(self.recording.view, RecordingState::Starting { .. }) {
            let _ = reply.send(Err(invalid("no recording is starting")));
            return;
        }
        let effects = self.apply(RecordingEvent::CancelStart);
        if effects.is_empty() {
            let _ = reply.send(Err(invalid("no recording is starting")));
            return;
        }
        let _ = reply.send(Ok(CommandOk::Accepted));
        self.run_effects(effects).await;
    }

    async fn cancel_finalize(&mut self, reply: Reply<CommandOk>) {
        if !matches!(self.recording.view, RecordingState::Finalizing { .. }) {
            let _ = reply.send(Err(invalid("no recording is finalizing")));
            return;
        }
        let effects = self.apply(RecordingEvent::CancelFinalize);
        if effects.is_empty() {
            let _ = reply.send(Err(invalid("no recording is finalizing")));
            return;
        }
        let _ = reply.send(Ok(CommandOk::Accepted));
        self.run_effects(effects).await;
    }

    async fn recover(&mut self, work_root: PathBuf, out_dir: PathBuf, reply: Reply<CommandOk>) {
        self.state.update(|state| state.recovery.running = true);
        let report =
            tokio::task::spawn_blocking(move || recover_incomplete_sessions(&work_root, &out_dir))
                .await;
        match report {
            Ok(report) => {
                self.add_library(&report.files);
                if !report.files.is_empty() {
                    let detail = report
                        .files
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    self.notices.push(
                        &self.state,
                        NoticeLevel::Success,
                        "Recordings recovered",
                        detail,
                    );
                }
                if !report.failures.is_empty() {
                    self.notices.push(
                        &self.state,
                        NoticeLevel::Warning,
                        "Recovery is incomplete",
                        report.failures.join("\n"),
                    );
                }
                self.state.update(|state| state.recovery.running = false);
                let _ = reply.send(Ok(CommandOk::Unit));
            }
            Err(error) => {
                self.state.update(|state| state.recovery.running = false);
                let _ = reply.send(Err(internal(format!("recovery task failed: {error}"))));
            }
        }
    }

    async fn tick(&mut self) {
        let now = self.now_ms();
        self.recording
            .apply(RecordingEvent::Tick { now_ms: now }, now);
        self.publish();
        self.poll_progress();
        self.poll_events().await;
    }

    async fn poll_events(&mut self) {
        let mut collected = Vec::new();
        let mut lost = false;
        if let Some(slot) = &mut self.active {
            loop {
                match (slot.pump)() {
                    EventPoll::Event(event) => collected.push(event),
                    EventPoll::Empty => break,
                    EventPoll::Disconnected => {
                        lost = true;
                        break;
                    }
                }
            }
        } else if let Some(slot) = &mut self.finalize {
            loop {
                match (slot.events)() {
                    EventPoll::Event(event) => collected.push(event),
                    EventPoll::Empty => break,
                    EventPoll::Disconnected => {
                        lost = true;
                        break;
                    }
                }
            }
        }
        for event in collected {
            self.handle_event(event).await;
        }
        if lost {
            self.mark_lost();
        }
        self.complete_finalize().await;
    }

    async fn handle_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Warning(message) => {
                self.session_notice(DomainSessionEvent::Warning(message))
                    .await
            }
            SessionEvent::Notice(SessionNotice::WebpFallback { trace_id, detail }) => {
                self.notices.push(
                    &self.state,
                    NoticeLevel::Warning,
                    "WebP encoding fell back",
                    format!("trace={trace_id}\n{detail}"),
                );
            }
            SessionEvent::SourceLost(name) => {
                self.session_notice(DomainSessionEvent::SourceLost(name))
                    .await
            }
            SessionEvent::SourceRestored(name) => {
                self.session_notice(DomainSessionEvent::SourceRestored(name))
                    .await
            }
            SessionEvent::CaptureFailed(message) => {
                self.session_notice(DomainSessionEvent::CaptureFailed(message))
                    .await
            }
            SessionEvent::Finished(paths) => self.set_terminal(Terminal::Finished(paths)),
            SessionEvent::Cancelled => self.set_terminal(Terminal::Cancelled),
            SessionEvent::Failed(failure) => self.set_terminal(Terminal::Failed {
                message: failure.error,
                files: failure.recovery_files,
            }),
        }
    }

    async fn session_notice(&mut self, event: DomainSessionEvent) {
        let active = match (&self.recording.view, self.recording.job) {
            (RecordingState::Active { job, .. }, Some(current)) => *job == current,
            _ => false,
        };
        if active {
            let job = self.recording.job.expect("active recording owns a job");
            let effects = self.apply(RecordingEvent::Session { job, event });
            self.run_effects(effects).await;
        } else {
            let (level, summary) = match &event {
                DomainSessionEvent::Warning(_) => (NoticeLevel::Warning, "Recording warning"),
                DomainSessionEvent::SourceLost(_) => (NoticeLevel::Warning, "Source lost"),
                DomainSessionEvent::SourceRestored(_) => (NoticeLevel::Info, "Source restored"),
                DomainSessionEvent::CaptureFailed(_) => (NoticeLevel::Error, "Capture failed"),
            };
            let detail = match event {
                DomainSessionEvent::Warning(detail)
                | DomainSessionEvent::SourceLost(detail)
                | DomainSessionEvent::SourceRestored(detail)
                | DomainSessionEvent::CaptureFailed(detail) => detail,
            };
            self.notices.push(&self.state, level, summary, detail);
        }
    }

    fn set_terminal(&mut self, terminal: Terminal) {
        if let Some(slot) = &mut self.finalize {
            if slot.terminal.is_none() {
                slot.terminal = Some(terminal);
            }
        }
    }

    fn mark_lost(&mut self) {
        if let Some(slot) = &mut self.finalize {
            if slot.terminal.is_none() {
                slot.terminal = Some(Terminal::Failed {
                    message: "recording finalizer stopped unexpectedly".into(),
                    files: Vec::new(),
                });
            }
        }
    }

    fn poll_progress(&mut self) {
        let update = {
            let Some(slot) = &mut self.finalize else {
                return;
            };
            let progress = slot.progress.fraction();
            if progress == slot.last_progress {
                None
            } else {
                slot.last_progress = progress;
                Some((slot.job, progress))
            }
        };
        if let Some((job, progress)) = update {
            self.apply(RecordingEvent::FinalizeProgress { job, progress });
        }
    }

    async fn complete_finalize(&mut self) {
        let ready = match &self.finalize {
            Some(slot) => {
                slot.terminal.is_some()
                    && slot
                        .finalizer
                        .as_ref()
                        .is_none_or(|handle| handle.is_finished())
            }
            None => false,
        };
        if !ready {
            return;
        }
        let Some(mut slot) = self.finalize.take() else {
            return;
        };
        if let Some(handle) = slot.finalizer.take() {
            if handle.join().is_err() {
                log::warn!("recording finalizer panicked");
            }
        }
        let job = slot.job;
        let now = self.now_ms();
        let (effects, files) = match slot.terminal.take() {
            Some(Terminal::Finished(paths)) => (
                self.recording.apply(
                    RecordingEvent::Finished {
                        job,
                        paths,
                        sound: slot.sound,
                    },
                    now,
                ),
                Vec::new(),
            ),
            Some(Terminal::Cancelled) => (
                self.recording.apply(RecordingEvent::Cancelled { job }, now),
                Vec::new(),
            ),
            Some(Terminal::Failed { message, files }) => (
                self.recording
                    .apply(RecordingEvent::Failed { job, message }, now),
                files,
            ),
            None => (Vec::new(), Vec::new()),
        };
        self.publish();
        self.run_effects(effects).await;
        if !files.is_empty() {
            let detail = files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join("\n");
            self.add_library(&files);
            self.notices.push(
                &self.state,
                NoticeLevel::Warning,
                "Recording files were preserved",
                detail,
            );
        }
    }

    async fn begin_finalize(&mut self, job: JobId, discard: bool) {
        let Some(slot) = self.active.take() else {
            return;
        };
        if slot.job != job {
            self.active = Some(slot);
            return;
        }
        let action = match self.pending_action.take() {
            Some((pending_job, action)) if pending_job == job => action,
            _ if discard => StopAction::Discard,
            _ => StopAction::Save,
        };
        let sound = std::mem::take(&mut self.pending_sound);
        let progress = slot.session.progress();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        self.finalize = Some(FinalizeSlot {
            job,
            cancel,
            progress,
            events: slot.pump,
            finalizer: None,
            terminal: None,
            sound,
            last_progress: None,
        });
        let session = slot.session;
        let stopped =
            tokio::task::spawn_blocking(move || session.stop(media_action(action), worker_cancel))
                .await;
        match stopped {
            Ok(Ok(handle)) => {
                if let Some(slot) = self.finalize.as_mut() {
                    slot.finalizer = Some(handle);
                }
            }
            Ok(Err(error)) => self.fail_finalize(job, format!("{error:#}")).await,
            Err(error) => {
                self.fail_finalize(job, format!("recording finalizer task failed: {error}"))
                    .await
            }
        }
    }

    async fn fail_finalize(&mut self, job: JobId, message: String) {
        self.finalize = None;
        let effects = self.apply(RecordingEvent::Failed { job, message });
        Box::pin(self.run_effects(effects)).await;
    }

    async fn run_effects(&mut self, effects: Vec<RecordingEffect>) {
        for effect in effects {
            self.run_effect(effect).await;
        }
    }

    async fn run_effect(&mut self, effect: RecordingEffect) {
        match effect {
            RecordingEffect::Spawn { .. } => {}
            RecordingEffect::Stop {
                job,
                action,
                source,
            } => {
                self.pending_action = Some((job, action));
                self.pending_sound = matches!(source, StopSource::Tray | StopSource::Hotkey);
            }
            RecordingEffect::Finalize { job, discard } => {
                self.begin_finalize(job, discard).await;
            }
            RecordingEffect::Cancel { job } => self.cancel_job(job),
            RecordingEffect::AddLibrary(paths) => self.add_library(&paths),
            RecordingEffect::CopyFiles(paths) => self.copy_files(paths),
            RecordingEffect::PlaySound => mediaa2_platform::audio::confirmation_tone(),
            RecordingEffect::Notify {
                level,
                summary,
                detail,
            } => self.notices.push(&self.state, level, summary, detail),
            RecordingEffect::Tray => {}
        }
    }

    fn cancel_job(&mut self, job: JobId) {
        if let Some(slot) = &self.start {
            if slot.job == job {
                slot.cancel.store(true, Ordering::Relaxed);
            }
        }
        if let Some(slot) = &self.finalize {
            if slot.job == job {
                slot.cancel.store(true, Ordering::Relaxed);
            }
        }
    }

    fn add_library(&mut self, paths: &[PathBuf]) {
        if paths.is_empty() {
            return;
        }
        match self.library.add_paths(paths) {
            Ok(()) => {
                let revision = self.library.revision();
                self.state.update(|state| state.library_revision = revision);
            }
            Err(error) => self.notices.push(
                &self.state,
                NoticeLevel::Error,
                "Library update failed",
                format!("{error:#}"),
            ),
        }
    }

    fn copy_files(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let state = Arc::clone(&self.state);
        let notices = self.notices.clone();
        std::mem::drop(tokio::task::spawn_blocking(move || {
            let refs: Vec<&std::path::Path> = paths.iter().map(PathBuf::as_path).collect();
            if let Err(error) = mediaa2_platform::clipboard::copy_files(&refs) {
                notices.push(
                    &state,
                    NoticeLevel::Warning,
                    "Could not copy recording",
                    format!("{error:#}"),
                );
            }
        }));
    }

    fn apply(&mut self, event: RecordingEvent) -> Vec<RecordingEffect> {
        let now = self.now_ms();
        let effects = self.recording.apply(event, now);
        self.publish();
        effects
    }

    fn publish(&mut self) {
        if self.recording.view == self.published {
            return;
        }
        self.published = self.recording.view.clone();
        let view = self.recording.view.clone();
        self.state.update(|state| state.recording = view);
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

fn pump_from(session: &RecordingSession) -> EventPump {
    let events = session.events();
    Box::new(move || match events.try_recv() {
        Ok(event) => EventPoll::Event(event),
        Err(error) if error.is_empty() => EventPoll::Empty,
        Err(_) => EventPoll::Disconnected,
    })
}

fn discard(result: Result<RecordingSession, StartFailure>) {
    if let Ok(session) = result {
        std::mem::drop(tokio::task::spawn_blocking(move || {
            let cancel = Arc::new(AtomicBool::new(false));
            let _ = session.stop(RecorderStopAction::Discard, cancel);
        }));
    }
}

fn media_action(action: StopAction) -> RecorderStopAction {
    match action {
        StopAction::Save => RecorderStopAction::Save,
        StopAction::Discard => RecorderStopAction::Discard,
    }
}

fn plan_error(error: PlanError) -> CommandError {
    let code = match &error {
        PlanError::Unavailable(_) => ErrorCode::Unavailable,
        PlanError::NoAudioSource | PlanError::Request(_) => ErrorCode::Invalid,
    };
    CommandError {
        code,
        message: error.message(),
    }
}

fn busy() -> CommandError {
    CommandError {
        code: ErrorCode::Busy,
        message: "a recording is already running".into(),
    }
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError {
        code: ErrorCode::Invalid,
        message: message.into(),
    }
}

fn internal(message: impl Into<String>) -> CommandError {
    CommandError {
        code: ErrorCode::Internal,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::test_state;
    use mediaa2_core::formats::{MediaKind, OutputKind, VideoContainer};
    use mediaa2_core::query::LibraryQuery;
    use mediaa2_core::screen::{MonitorId, MonitorInfo};

    fn monitor(id: u32) -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(id),
            device: format!(r"\\.\DISPLAY{id}"),
            index: 0,
            name: format!("Display {id}"),
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: false,
        }
    }

    fn active() -> RecordingState {
        RecordingState::Active {
            job: 7,
            target: RecordTarget::Audio,
            started_ms: 0,
            paused: false,
            elapsed_ms: 0,
            sources: Vec::new(),
            has_audio: false,
            output: OutputKind::Video(VideoContainer::Mkv),
        }
    }

    fn fixture() -> (tempfile::TempDir, Arc<StateStore>, RecordingHandle) {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateStore::new(test_state()));
        let library = LibraryHandle::open(dir.path().join("media")).unwrap();
        let handle = RecordingHandle::new(Arc::clone(&state), library, NoticeRing::new());
        (dir, state, handle)
    }

    #[tokio::test]
    async fn start_is_busy_while_a_recording_is_active() {
        let (_dir, state, handle) = fixture();
        state.update(|state| state.recording = active());
        let error = handle
            .start(RecordTarget::Audio, RecordProfile::App, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Busy);
    }

    #[tokio::test]
    async fn commands_require_a_running_recording() {
        let (_dir, _state, handle) = fixture();
        assert_eq!(handle.pause().await.unwrap_err().code, ErrorCode::Invalid);
        assert_eq!(
            handle
                .stop(StopAction::Save, StopSource::Ui)
                .await
                .unwrap_err()
                .code,
            ErrorCode::Invalid
        );
        assert_eq!(
            handle.cancel_start().await.unwrap_err().code,
            ErrorCode::Invalid
        );
        assert_eq!(
            handle.cancel_finalize().await.unwrap_err().code,
            ErrorCode::Invalid
        );
    }

    #[tokio::test]
    async fn start_reports_an_unavailable_target_and_stays_idle() {
        let (_dir, state, handle) = fixture();
        let target = RecordTarget::Screens {
            monitors: vec![monitor(u32::MAX)],
        };
        let error = handle
            .start(target, RecordProfile::Region, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Unavailable);
        assert_eq!(state.snapshot().recording, RecordingState::Idle);
    }

    #[tokio::test]
    async fn audio_only_without_an_audio_profile_is_invalid() {
        let (_dir, state, handle) = fixture();
        let error = handle
            .start(RecordTarget::Audio, RecordProfile::Region, None)
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Invalid);
        assert_eq!(state.snapshot().recording, RecordingState::Idle);
    }

    #[tokio::test]
    async fn hide_is_unknown_before_a_recording_starts() {
        let (_dir, _state, handle) = fixture();
        assert_eq!(handle.hide(), None);
    }

    #[tokio::test]
    async fn recovery_moves_incomplete_sessions_into_media_and_library() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        let media = dir.path().join("media");
        let session = work.join("20260101-000000-42");
        std::fs::create_dir_all(&session).unwrap();
        std::fs::write(session.join("video-0.mkv"), b"video").unwrap();
        std::fs::write(session.join("audio-0.wav"), b"audio").unwrap();

        let state = Arc::new(StateStore::new(test_state()));
        let library = LibraryHandle::open(media.clone()).unwrap();
        let handle = RecordingHandle::new(Arc::clone(&state), library.clone(), NoticeRing::new());

        let result = handle.recover_from(work, media).await.unwrap();

        assert_eq!(result, CommandOk::Unit);
        let snapshot = state.snapshot();
        assert!(!snapshot.recovery.running);
        assert!(snapshot
            .notices
            .iter()
            .any(|notice| notice.level == NoticeLevel::Success));
        let page = library.query(&LibraryQuery::default()).unwrap();
        assert_eq!(page.total, 2);
        assert!(page.entries.iter().all(|entry| entry.path.exists()));
        assert!(page
            .entries
            .iter()
            .any(|entry| entry.kind == MediaKind::Video));
        assert!(page
            .entries
            .iter()
            .any(|entry| entry.kind == MediaKind::Audio));
    }

    #[tokio::test]
    async fn recovery_without_work_files_is_a_no_op() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        let state = Arc::new(StateStore::new(test_state()));
        let library = LibraryHandle::open(media.clone()).unwrap();
        let handle = RecordingHandle::new(Arc::clone(&state), library.clone(), NoticeRing::new());

        let result = handle
            .recover_from(dir.path().join("missing"), media)
            .await
            .unwrap();

        assert_eq!(result, CommandOk::Unit);
        let snapshot = state.snapshot();
        assert!(!snapshot.recovery.running);
        assert!(snapshot.notices.is_empty());
        assert_eq!(library.query(&LibraryQuery::default()).unwrap().total, 0);
    }
}
