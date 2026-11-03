use crate::domain::conversion::{Conversion, ConversionEffect, ConversionEvent};
use crate::domain::JobId;
use crate::services::{LibraryHandle, NoticeRing};
use crate::state::StateStore;
use mediaa2_core::convert::{ConversionOptions, ConvertTarget};
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::paths::{MediaDir, OutputPath, SourcePath};
use mediaa2_media::convert;
use mediaa2_protocol::state::ConversionState;
use mediaa2_protocol::{CommandError, CommandOk, ErrorCode};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
const COMMAND_CAPACITY: usize = 8;

#[derive(Clone)]
pub struct ConversionHandle {
    commands: mpsc::Sender<Command>,
}

impl ConversionHandle {
    pub fn new(state: Arc<StateStore>, library: LibraryHandle, notices: NoticeRing) -> Self {
        let (commands, receiver) = mpsc::channel(COMMAND_CAPACITY);
        let actor = Actor {
            state,
            library,
            notices,
            conversion: Conversion::new(),
            active: None,
            updates: None,
            added: HashSet::new(),
            next_job: 1,
            last_published: Instant::now(),
        };
        tokio::spawn(actor.run(receiver));
        Self { commands }
    }

    pub async fn start(
        &self,
        inputs: Vec<PathBuf>,
        options: ConversionOptions,
        output_dir: PathBuf,
    ) -> Result<CommandOk, CommandError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Start {
                inputs,
                options,
                output_dir,
                reply,
            })
            .await
            .map_err(actor_gone)?;
        answer.await.map_err(actor_gone)?
    }

    pub async fn cancel(&self) -> Result<CommandOk, CommandError> {
        let (reply, answer) = oneshot::channel();
        self.commands
            .send(Command::Cancel { reply })
            .await
            .map_err(actor_gone)?;
        answer.await.map_err(actor_gone)?
    }
}

enum Command {
    Start {
        inputs: Vec<PathBuf>,
        options: ConversionOptions,
        output_dir: PathBuf,
        reply: oneshot::Sender<Result<CommandOk, CommandError>>,
    },
    Cancel {
        reply: oneshot::Sender<Result<CommandOk, CommandError>>,
    },
}

enum Update {
    Progress {
        job: JobId,
        done: u32,
        current: Option<String>,
        progress: Option<f32>,
    },
    ItemDone {
        job: JobId,
        path: PathBuf,
    },
    ItemFailed {
        job: JobId,
        name: String,
        message: String,
    },
    LibraryFailed {
        job: JobId,
        message: String,
    },
    Done {
        job: JobId,
        outcome: BatchOutcome,
    },
}

struct BatchOutcome {
    cancelled: bool,
    last: Option<PathBuf>,
    failures: u32,
    first_error: Option<String>,
}

struct ActiveJob {
    cancel: Arc<AtomicBool>,
    total: u32,
}

enum Step {
    Command(Option<Command>),
    Update(Option<Update>),
}

struct Actor {
    state: Arc<StateStore>,
    library: LibraryHandle,
    notices: NoticeRing,
    conversion: Conversion,
    active: Option<ActiveJob>,
    updates: Option<mpsc::UnboundedReceiver<Update>>,
    added: HashSet<PathBuf>,
    next_job: JobId,
    last_published: Instant,
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        loop {
            let step = tokio::select! {
                command = commands.recv() => Step::Command(command),
                update = next_update(&mut self.updates) => Step::Update(update),
            };
            match step {
                Step::Command(Some(command)) => self.on_command(command),
                Step::Command(None) => {
                    if let Some(active) = &self.active {
                        active.cancel.store(true, Ordering::SeqCst);
                    }
                    break;
                }
                Step::Update(Some(update)) => self.on_update(update),
                Step::Update(None) => self.on_worker_gone(),
            }
        }
    }

    fn on_command(&mut self, command: Command) {
        match command {
            Command::Start {
                inputs,
                options,
                output_dir,
                reply,
            } => {
                let result = self.start(inputs, options, output_dir);
                let _ = reply.send(result);
            }
            Command::Cancel { reply } => {
                self.cancel();
                let _ = reply.send(Ok(CommandOk::Unit));
            }
        }
    }

    fn start(
        &mut self,
        inputs: Vec<PathBuf>,
        options: ConversionOptions,
        output_dir: PathBuf,
    ) -> Result<CommandOk, CommandError> {
        ensure_idle(&self.conversion)?;
        let inputs = unique(inputs);
        if inputs.is_empty() {
            return Err(invalid("no files to convert"));
        }
        let total = u32::try_from(inputs.len()).unwrap_or(u32::MAX);
        let job = self.next_job;
        self.next_job = self.next_job.saturating_add(1);
        self.added.clear();
        let cancel = Arc::new(AtomicBool::new(false));
        let effects = self.conversion.apply(ConversionEvent::Start { job, total });
        if effects.is_empty() {
            return Err(busy());
        }
        self.publish();
        let (updates, receiver) = mpsc::unbounded_channel();
        self.updates = Some(receiver);
        self.active = Some(ActiveJob {
            cancel: Arc::clone(&cancel),
            total,
        });
        self.last_published = Instant::now();
        let library = self.library.clone();
        tokio::task::spawn_blocking(move || {
            run_batch(job, inputs, options, output_dir, cancel, updates, library)
        });
        Ok(CommandOk::Accepted)
    }

    fn cancel(&mut self) {
        let Some(active) = &self.active else {
            return;
        };
        active.cancel.store(true, Ordering::SeqCst);
        if matches!(self.conversion.view, ConversionState::Idle) {
            return;
        }
        let effects = self.conversion.apply(ConversionEvent::Cancel);
        if effects.is_empty() {
            return;
        }
        self.publish();
    }

    fn on_update(&mut self, update: Update) {
        match update {
            Update::Progress {
                job,
                done,
                current,
                progress,
            } => {
                if self.conversion.job != Some(job)
                    || !matches!(self.conversion.view, ConversionState::Running { .. })
                {
                    return;
                }
                self.conversion.apply(ConversionEvent::Progress {
                    job,
                    done,
                    current,
                    progress,
                });
                if self.last_published.elapsed() >= PROGRESS_INTERVAL {
                    self.publish();
                    self.last_published = Instant::now();
                }
            }
            Update::ItemDone { job, path } => {
                if self.conversion.job != Some(job) {
                    return;
                }
                self.added.insert(path);
                self.sync_library_revision();
            }
            Update::ItemFailed { job, name, message } => {
                if self.conversion.job != Some(job) {
                    return;
                }
                self.notices.push(
                    &self.state,
                    NoticeLevel::Error,
                    format!("Conversion failed: {name}"),
                    message,
                );
            }
            Update::LibraryFailed { job, message } => {
                if self.conversion.job != Some(job) {
                    return;
                }
                self.notices
                    .push(&self.state, NoticeLevel::Error, "Library", message);
            }
            Update::Done { job, outcome } => self.finish(job, outcome),
        }
    }

    fn on_worker_gone(&mut self) {
        self.updates = None;
        self.active = None;
        if let Some(job) = self.conversion.job {
            let effects = self.conversion.apply(ConversionEvent::Failed {
                job,
                message: "conversion worker stopped".into(),
            });
            self.publish();
            for effect in effects {
                self.apply_effect(effect);
            }
        }
        self.added.clear();
    }

    fn finish(&mut self, job: JobId, outcome: BatchOutcome) {
        self.updates = None;
        let total = self.active.take().map_or(0, |active| active.total);
        if self.conversion.job != Some(job) {
            self.added.clear();
            return;
        }
        let BatchOutcome {
            cancelled,
            last,
            failures,
            first_error,
        } = outcome;
        let event = if cancelled {
            ConversionEvent::Cancelled { job }
        } else if failures == 0 {
            match last {
                Some(path) => ConversionEvent::Finished { job, path },
                None => ConversionEvent::Failed {
                    job,
                    message: "nothing was converted".into(),
                },
            }
        } else {
            let detail = first_error.unwrap_or_else(|| "unknown error".into());
            ConversionEvent::Failed {
                job,
                message: format!("{failures} of {total} files failed: {detail}"),
            }
        };
        let effects = self.conversion.apply(event);
        self.publish();
        for effect in effects {
            self.apply_effect(effect);
        }
        self.added.clear();
    }

    fn apply_effect(&mut self, effect: ConversionEffect) {
        match effect {
            ConversionEffect::AddLibrary(path) => {
                if self.added.insert(path.clone()) {
                    match self.library.add_paths(std::slice::from_ref(&path)) {
                        Ok(()) => self.sync_library_revision(),
                        Err(error) => {
                            self.notices.push(
                                &self.state,
                                NoticeLevel::Error,
                                "Library",
                                format!("{error:#}"),
                            );
                        }
                    }
                }
            }
            ConversionEffect::Notify {
                level,
                summary,
                detail,
            } => self.notices.push(&self.state, level, summary, detail),
            ConversionEffect::StartJob | ConversionEffect::CancelJob => {}
        }
    }

    fn publish(&self) {
        let view = self.conversion.view.clone();
        self.state.update(|current| current.conversion = view);
    }

    fn sync_library_revision(&self) {
        let revision = self.library.revision();
        self.state
            .update(|current| current.library_revision = revision);
    }
}

async fn next_update(receiver: &mut Option<mpsc::UnboundedReceiver<Update>>) -> Option<Update> {
    match receiver {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

fn run_batch(
    job: JobId,
    inputs: Vec<PathBuf>,
    options: ConversionOptions,
    output_dir: PathBuf,
    cancel: Arc<AtomicBool>,
    updates: mpsc::UnboundedSender<Update>,
    library: LibraryHandle,
) {
    let media_dir = MediaDir::new(output_dir);
    let mut done = 0u32;
    let mut failures = 0u32;
    let mut first_error: Option<String> = None;
    let mut last: Option<PathBuf> = None;
    for input in inputs {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let source = SourcePath::new(input.clone());
        let output = output_for(&input, options.target, &media_dir);
        let name = display_name(&input);
        let _ = updates.send(Update::Progress {
            job,
            done,
            current: Some(name.clone()),
            progress: Some(0.0),
        });
        let progress_updates = updates.clone();
        let progress_name = name.clone();
        let mut report = move |value: Option<f32>| {
            let _ = progress_updates.send(Update::Progress {
                job,
                done,
                current: Some(progress_name.clone()),
                progress: value,
            });
        };
        let result = convert::convert(&source, &output, &options, &mut report, &cancel);
        done = done.saturating_add(1);
        match result {
            Ok(()) => {
                let path = output.into_path();
                last = Some(path.clone());
                match library.add_paths(std::slice::from_ref(&path)) {
                    Ok(()) => {
                        let _ = updates.send(Update::ItemDone {
                            job,
                            path: path.clone(),
                        });
                    }
                    Err(error) => {
                        let _ = updates.send(Update::LibraryFailed {
                            job,
                            message: format!("{}: {error:#}", path.display()),
                        });
                    }
                }
                let _ = updates.send(Update::Progress {
                    job,
                    done,
                    current: None,
                    progress: Some(1.0),
                });
            }
            Err(_) if cancel.load(Ordering::SeqCst) => break,
            Err(error) => {
                failures = failures.saturating_add(1);
                let message = format!("{error:#}");
                if first_error.is_none() {
                    first_error = Some(message.clone());
                }
                let _ = updates.send(Update::ItemFailed {
                    job,
                    name: name.clone(),
                    message,
                });
            }
        }
    }
    let _ = updates.send(Update::Done {
        job,
        outcome: BatchOutcome {
            cancelled: cancel.load(Ordering::SeqCst),
            last,
            failures,
            first_error,
        },
    });
}

fn output_for(input: &Path, target: ConvertTarget, media_dir: &MediaDir) -> OutputPath {
    convert::output_path(
        &SourcePath::new(input.to_path_buf()),
        target,
        None,
        media_dir,
    )
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

fn unique(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = HashSet::with_capacity(paths.len());
    let mut unique = Vec::with_capacity(paths.len());
    for path in paths {
        if seen.insert(path.clone()) {
            unique.push(path);
        }
    }
    unique
}

fn ensure_idle(conversion: &Conversion) -> Result<(), CommandError> {
    if conversion.job.is_none() && matches!(conversion.view, ConversionState::Idle) {
        Ok(())
    } else {
        Err(busy())
    }
}

fn busy() -> CommandError {
    CommandError {
        code: ErrorCode::Busy,
        message: "conversion is already running".into(),
    }
}

fn invalid(message: &str) -> CommandError {
    CommandError {
        code: ErrorCode::Invalid,
        message: message.into(),
    }
}

fn actor_gone<T>(_: T) -> CommandError {
    CommandError {
        code: ErrorCode::Internal,
        message: "conversion worker is not running".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::test_state;
    use mediaa2_core::convert::StillFormat;
    use mediaa2_core::naming;

    fn handle(state: &Arc<StateStore>, dir: &Path) -> ConversionHandle {
        let library = LibraryHandle::open(dir.join("media")).unwrap();
        ConversionHandle::new(Arc::clone(state), library, NoticeRing::new())
    }

    #[test]
    fn output_names_keep_the_title_and_avoid_existing_files() {
        let dir = tempfile::tempdir().unwrap();
        let input = Path::new("/in/My clip_2026-10-13_09-05-07.png");
        let media_dir = MediaDir::new(dir.path());
        let first = output_for(input, ConvertTarget::Still(StillFormat::Jpeg), &media_dir);
        assert_eq!(first.extension().and_then(|ext| ext.to_str()), Some("jpg"));
        let parsed = naming::parse_path(&first).unwrap();
        assert_eq!(parsed.title, "My clip");
        assert_eq!(parsed.counter, None);
        std::fs::write(&first, b"x").unwrap();
        let second = output_for(input, ConvertTarget::Still(StillFormat::Jpeg), &media_dir);
        assert_ne!(first, second);
        assert_eq!(naming::parse_path(&second).unwrap().counter, Some(2));
    }

    #[test]
    fn start_is_rejected_while_a_job_runs() {
        let mut conversion = Conversion::new();
        conversion.apply(ConversionEvent::Start { job: 1, total: 2 });
        let error = ensure_idle(&conversion).unwrap_err();
        assert_eq!(error.code, ErrorCode::Busy);
        assert!(ensure_idle(&Conversion::new()).is_ok());
    }

    #[tokio::test]
    async fn empty_inputs_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateStore::new(test_state()));
        let handle = handle(&state, dir.path());
        let error = handle
            .start(
                Vec::new(),
                ConversionOptions::default(),
                dir.path().join("out"),
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::Invalid);
        assert_eq!(state.snapshot().conversion, ConversionState::Idle);
    }

    #[tokio::test]
    async fn cancel_without_a_job_is_a_noop() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateStore::new(test_state()));
        let handle = handle(&state, dir.path());
        assert_eq!(handle.cancel().await.unwrap(), CommandOk::Unit);
        assert_eq!(state.snapshot().conversion, ConversionState::Idle);
    }

    #[tokio::test]
    async fn a_missing_input_returns_to_idle_with_an_error_notice() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateStore::new(test_state()));
        let handle = handle(&state, dir.path());
        let input = dir.path().join("missing_2026-10-13_09-05-07.png");
        let accepted = handle
            .start(
                vec![input],
                ConversionOptions::for_target(ConvertTarget::Still(StillFormat::Jpeg)),
                dir.path().join("out"),
            )
            .await
            .unwrap();
        assert_eq!(accepted, CommandOk::Accepted);
        let mut notices = Vec::new();
        for _ in 0..400 {
            if matches!(state.snapshot().conversion, ConversionState::Idle) {
                notices = state.snapshot().notices.clone();
                if !notices.is_empty() {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(state.snapshot().conversion, ConversionState::Idle);
        assert!(
            notices
                .iter()
                .any(|notice| notice.level == NoticeLevel::Error),
            "a failed batch notifies the user"
        );
        assert_eq!(state.snapshot().library_revision, 0);
    }
}
