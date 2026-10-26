use crossbeam_channel::{bounded, Receiver, Sender};
use mediaa_core::Frame;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

const WAIT_TIMEOUT: Duration = Duration::from_secs(30);
const CLIPBOARD_OPERATION_TIMEOUT: Duration = Duration::from_secs(25);

#[derive(Clone)]
pub(crate) enum Purpose {
    Toast(String),
    Recording { success: String, sound: bool },
}

pub(crate) struct Event {
    pub purpose: Purpose,
    pub result: Result<(), String>,
}

enum Action {
    Text(String),
    Files(Vec<PathBuf>),
    Image(Frame),
}

struct Command {
    action: Action,
    purpose: Option<Purpose>,
    reply: Option<Sender<Result<(), String>>>,
}

#[derive(Clone)]
pub(crate) struct ClipboardHandle {
    sender: Sender<Command>,
    in_flight: Arc<AtomicUsize>,
    available: Arc<std::sync::atomic::AtomicBool>,
}

impl ClipboardHandle {
    pub(crate) fn copy_text(&self, text: String, purpose: Purpose) -> anyhow::Result<()> {
        self.submit(Command {
            action: Action::Text(text),
            purpose: Some(purpose),
            reply: None,
        })
    }

    pub(crate) fn copy_files(&self, files: Vec<PathBuf>, purpose: Purpose) -> anyhow::Result<()> {
        self.submit(Command {
            action: Action::Files(files),
            purpose: Some(purpose),
            reply: None,
        })
    }

    pub(crate) fn copy_image_wait(&self, frame: Frame) -> Result<(), String> {
        self.wait(Action::Image(frame))
    }

    pub(crate) fn copy_files_wait(&self, files: Vec<PathBuf>) -> Result<(), String> {
        self.wait(Action::Files(files))
    }

    fn wait(&self, action: Action) -> Result<(), String> {
        let (reply, done) = bounded(1);
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        if self
            .sender
            .try_send(Command {
                action,
                purpose: None,
                reply: Some(reply),
            })
            .is_err()
        {
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            return Err("clipboard queue is full".into());
        }
        match done.recv_timeout(WAIT_TIMEOUT) {
            Ok(result) => result,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                Err("clipboard did not respond within 30 seconds".into())
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                Err("clipboard worker stopped".into())
            }
        }
    }

    fn submit(&self, command: Command) -> anyhow::Result<()> {
        if !self.available.load(Ordering::Acquire) {
            anyhow::bail!("clipboard worker is unavailable");
        }
        self.in_flight.fetch_add(1, Ordering::Relaxed);
        if self.sender.try_send(command).is_err() {
            self.in_flight.fetch_sub(1, Ordering::Relaxed);
            anyhow::bail!("clipboard queue is full");
        }
        Ok(())
    }
}

pub(crate) struct ClipboardService {
    ctx: eframe::egui::Context,
    recoverable: bool,
    handle: ClipboardHandle,
    events: Receiver<Event>,
    join: Option<JoinHandle<()>>,
}

impl ClipboardService {
    pub(crate) fn new(context: eframe::egui::Context) -> Self {
        Self::with_runner_mode(context, run, true)
    }

    #[cfg(test)]
    fn with_runner(
        ctx: eframe::egui::Context,
        runner: impl FnMut(Action) -> Result<(), String> + Send + 'static,
    ) -> Self {
        Self::with_runner_mode(ctx, runner, false)
    }

    #[cfg(test)]
    fn with_runner_timeout(
        ctx: eframe::egui::Context,
        runner: impl FnMut(Action) -> Result<(), String> + Send + 'static,
        operation_timeout: Duration,
    ) -> Self {
        Self::with_runner_mode_timeout(ctx, runner, operation_timeout, false)
    }

    fn with_runner_mode(
        ctx: eframe::egui::Context,
        runner: impl FnMut(Action) -> Result<(), String> + Send + 'static,
        recoverable: bool,
    ) -> Self {
        Self::with_runner_mode_timeout(ctx, runner, CLIPBOARD_OPERATION_TIMEOUT, recoverable)
    }

    fn with_runner_mode_timeout(
        ctx: eframe::egui::Context,
        runner: impl FnMut(Action) -> Result<(), String> + Send + 'static,
        operation_timeout: Duration,
        recoverable: bool,
    ) -> Self {
        let (sender, commands) = bounded::<Command>(32);
        let (events_tx, events) = bounded(32);
        let in_flight = Arc::new(AtomicUsize::new(0));
        let available = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_in_flight = in_flight.clone();
        let worker_available = available.clone();
        let runner = Arc::new(parking_lot::Mutex::new(runner));
        let worker_ctx = ctx.clone();
        let join = std::thread::Builder::new()
            .name("clipboard".into())
            .spawn(move || {
                while let Ok(command) = commands.recv() {
                    let (done_tx, done) = bounded(1);
                    let action = command.action;
                    let runner = runner.clone();
                    let (result, timed_out) = match std::thread::Builder::new().spawn(move || {
                        let result = runner.lock()(action);
                        let _ = done_tx.send(result);
                    }) {
                        Ok(_) => match done.recv_timeout(operation_timeout) {
                            Ok(result) => (result, false),
                            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                                (Err("clipboard operation timed out".into()), true)
                            }
                            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                                (Err("clipboard operation stopped".into()), false)
                            }
                        },
                        Err(error) => (
                            Err(format!("clipboard worker could not start: {error}")),
                            false,
                        ),
                    };
                    if let Some(reply) = command.reply {
                        let _ = reply.send(result.clone());
                    }
                    if let Some(purpose) = command.purpose {
                        let _ = events_tx.try_send(Event { purpose, result });
                    }
                    worker_in_flight.fetch_sub(1, Ordering::Relaxed);
                    worker_ctx.request_repaint();
                    if timed_out {
                        worker_available.store(false, Ordering::Release);
                        break;
                    }
                }
            })
            .map_err(|error| log::warn!("clipboard worker: {error}"))
            .ok();
        Self {
            ctx,
            recoverable,
            handle: ClipboardHandle {
                sender,
                in_flight,
                available,
            },
            events,
            join,
        }
    }

    pub(crate) fn handle(&mut self) -> ClipboardHandle {
        if self.recoverable && !self.handle.available.load(Ordering::Acquire) {
            let replacement = Self::with_runner_mode(self.ctx.clone(), run, true);
            drop(std::mem::replace(self, replacement));
        }
        self.handle.clone()
    }

    pub(crate) fn try_recv(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    pub(crate) fn is_busy(&self) -> bool {
        self.handle.in_flight.load(Ordering::Relaxed) > 0
    }
}

impl Drop for ClipboardService {
    fn drop(&mut self) {
        if let Some(join) = self.join.take().filter(|join| join.is_finished()) {
            let _ = join.join();
        }
    }
}

fn run(action: Action) -> Result<(), String> {
    match action {
        Action::Text(text) => {
            mediaa_platform::clipboard::copy_text(&text).map_err(|error| format!("{error:#}"))
        }
        Action::Files(files) => {
            let refs: Vec<_> = files.iter().map(PathBuf::as_path).collect();
            mediaa_platform::clipboard::copy_files(&refs).map_err(|error| format!("{error:#}"))
        }
        Action::Image(frame) => {
            mediaa_platform::clipboard::copy_image(frame).map_err(|error| format!("{error:#}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Action, ClipboardService, Purpose};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[test]
    fn production_clipboard_recovers_after_a_timed_out_worker() {
        let mut service = ClipboardService::new(eframe::egui::Context::default());
        service.handle.available.store(false, Ordering::Release);
        assert!(service.handle().available.load(Ordering::Acquire));
    }

    #[test]
    fn commands_run_in_submission_order() {
        let mut service =
            ClipboardService::with_runner(eframe::egui::Context::default(), |action| {
                if let Action::Text(text) = action {
                    assert!(!text.is_empty());
                }
                Ok(())
            });
        for text in ["one", "two", "three"] {
            service
                .handle()
                .copy_text(text.into(), Purpose::Toast(text.into()))
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        let mut messages = Vec::new();
        while messages.len() < 3 && Instant::now() < deadline {
            if let Some(event) = service.try_recv() {
                if let Purpose::Toast(message) = event.purpose {
                    messages.push(message);
                }
            } else {
                std::thread::yield_now();
            }
        }
        assert_eq!(messages, ["one", "two", "three"]);
        while service.is_busy() && Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(!service.is_busy());
    }

    #[test]
    fn a_stuck_clipboard_operation_disables_the_queue() {
        let mut service = ClipboardService::with_runner_timeout(
            eframe::egui::Context::default(),
            |_action| {
                std::thread::sleep(Duration::from_millis(100));
                Ok(())
            },
            Duration::from_millis(10),
        );
        service
            .handle()
            .copy_text("first".into(), Purpose::Toast("first".into()))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(1);
        let event = loop {
            if let Some(event) = service.try_recv() {
                break event;
            }
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        };
        assert!(event.result.is_err());
        while service.is_busy() {
            std::thread::yield_now();
        }
        while Instant::now() < deadline {
            if service
                .handle()
                .copy_text("second".into(), Purpose::Toast("second".into()))
                .is_err()
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("clipboard queue accepted work after its worker stopped");
    }
}
