//! A chat answer as a task of the queue, seen from two sides: the thread that writes it and the
//! pane that shows it.
//!
//! The worker runs the answer with [`work`]: every event of the answer is applied to the shared
//! [`Live`] state (the timeline growing downward, what the model waits for, the scan), sections of
//! a long answer are stored as messages as they come, and at the end the answer is stored with
//! its timeline and steps in its `details` and the queue task is finished.
//!
//! The pane reads [`Run::live`] every frame and stops the answer with [`Run::stop`]: at once, with
//! no wait for the worker. The stop closes the state: whatever the worker still produces
//! afterwards (it may take a while to notice, a web page is still being read) is dropped, and
//! the message of the stopped answer keeps the timeline up to the stop.

use super::{Answer, Event, Memory, StepRecord, WaitReason};
use crate::scope::Scope;
use mediaa_core::chat::{Author, ChatId, Timeline, STEP_RUNNING};
use mediaa_queue::{Store, TaskId};
use mediaa_core::transcript::Transcript;
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

/// How far an answer has got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Stopped,
    Failed(String),
}

/// An answer as the pane shows it, built from the events.
#[derive(Debug, Clone)]
pub struct Live {
    /// Steps, reasoning and parts of the answer so far, in order.
    pub timeline: Timeline,
    /// What the model waits for (until any other event comes).
    pub waiting: Option<WaitReason>,
    /// Parts of the recording a running scan has read, of all.
    pub scanning: Option<(usize, usize)>,
    /// The tool steps that are over, with their arguments and results.
    pub steps: Vec<StepRecord>,
    /// Sections of the answer stored as messages so far (the pane re-reads the chat when this
    /// grows).
    pub stored: usize,
    pub state: State,
    /// The answer continues after the process was restarted (the pane says so, once).
    pub continuing: bool,
    /// Nothing more is accepted: the answer is over (or was stopped).
    closed: bool,
}

impl Live {
    /// The pane stopped the answer: later events from the chunk are dropped.
    pub fn close(&mut self) {
        self.closed = true;
    }

    pub fn closed(&self) -> bool {
        self.closed
    }
}

impl Default for Live {
    fn default() -> Live {
        Live::new()
    }
}

impl Live {
    pub fn new() -> Live {
        Live { timeline: Timeline::default(), waiting: None, scanning: None, steps: Vec::new(), stored: 0, state: State::Running, continuing: false, closed: false }
    }

    /// Takes one event of the answer.
    pub fn apply(&mut self, event: &Event) {
        // `work` closes the state before the ending event, so that one still counts.
        // Anything after the answer has already ended is dropped.
        if !matches!(self.state, State::Running) {
            return;
        }
        if self.closed && !matches!(event, Event::Done | Event::Stopped | Event::Failed(_)) {
            return;
        }
        if !matches!(event, Event::Waiting { .. }) {
            self.waiting = None;
        }
        match event {
            Event::Waiting { reason } => self.waiting = Some(reason.clone()),
            Event::StepStarted { title, tool } => {
                self.scanning = None;
                self.timeline.step(title, tool, STEP_RUNNING);
            }
            Event::Step(record) => {
                self.scanning = None;
                self.timeline.finish_step(record.status.as_str());
                self.steps.push(record.clone());
            }
            Event::Thinking(text) => self.timeline.thinking(text),
            Event::AnswerText(text) => self.timeline.answer(text),
            Event::AnswerDiscarded => self.timeline.discard_answer(),
            Event::Say(text) => {
                self.timeline.section(text);
                self.stored += 1;
            }
            Event::Scanning { done, total } => self.scanning = Some((*done, *total)),
            Event::Done => self.state = State::Done,
            Event::Stopped => {
                self.timeline.stop();
                self.state = State::Stopped;
            }
            Event::Failed(error) => {
                self.timeline.stop();
                self.state = State::Failed(error.clone());
            }
        }
    }

    /// The details of the answer's message.
    fn details(&self, forced: bool, stopped: bool) -> serde_json::Value {
        let mut details = json!({ "steps": self.steps, "forced": forced, "timeline": self.timeline.to_value() });
        if stopped {
            details["stopped"] = json!(true);
        }
        details
    }
}

/// An answer being written: shared by the thread that writes it and the pane that shows it.
#[derive(Clone)]
pub struct Run {
    /// Set to stop the answer (the models and tools look at it).
    pub cancel: Arc<AtomicBool>,
    live: Arc<Mutex<Live>>,
    task: TaskId,
    store: Arc<dyn Store>,
}

impl Run {
    /// The run of the answer that is the task of `scope`.
    pub fn new(scope: &Scope) -> Run {
        Run { cancel: Arc::new(AtomicBool::new(false)), live: Arc::new(Mutex::new(Live::new())), task: scope.task(), store: scope.store().clone() }
    }

    /// The answer so far.
    pub fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().expect("live answer lock")
    }

    /// Whether the answer is over, however it ended.
    pub fn finished(&self) -> bool {
        self.live().state != State::Running
    }

    /// The user stops the answer. It ends here and now: the state is closed (what the worker
    /// produces from now on is dropped), the queue task is cancelled and the answer is stored
    /// with the timeline up to the stop, marked as stopped. Does not wait for the worker.
    pub fn stop(&self, t: &Transcript, chat: ChatId) -> anyhow::Result<()> {
        self.cancel.store(true, Ordering::Relaxed);
        let mut live = self.live();
        if live.closed {
            return Ok(());
        }
        live.apply(&Event::Stopped);
        live.closed = true;
        if let Err(e) = self.store.cancel(self.task) {
            log::warn!("the stopped answer was not cancelled in the queue: {e:#}");
        }
        t.add_message(chat, Author::Agent, live.timeline.draft(), &live.details(false, true))
    }
}

/// How an answer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    /// Stopped by the user; the pane has stored what there was.
    Stopped,
    Failed(String),
}

/// Writes the answer of `run`, the task of `scope`, into the chat `chat` of the transcript store
/// `store` in `ai_dir`. `ask` is the work (`assistant::ask` or `ask_folder` with everything but
/// the memory and the event sink); it hears the events, which also go to [`Run::live`]. Sections
/// are stored as messages as they come; at the end the answer is stored with its steps and
/// timeline, the chat's memory is saved (also after a failure, not after a stop) and the task is
/// finished, failed or (if it was stopped) left as the stop made it.
pub fn work(
    run: &Run,
    scope: &Scope,
    ai_dir: &Path,
    store: &str,
    chat: ChatId,
    memory: &mut Memory,
    ask: impl FnOnce(&mut Memory, &mut (dyn FnMut(&Event) + Send)) -> anyhow::Result<Answer>,
) -> Outcome {
    // The sink owns a handle of its own: it may be called from another thread (the waiting is
    // reported by a watcher).
    let sections = Transcript::open(ai_dir, store);
    let live = run.live.clone();
    let mut sink = move |event: &Event| {
        let mut live = live.lock().expect("live answer lock");
        if live.closed {
            return;
        }
        if let Event::Say(text) = event {
            let stored = sections.as_ref().map_err(|e| anyhow::anyhow!("{e:#}")).and_then(|t| t.add_message(chat, Author::Agent, text, &json!({ "section": true })));
            if let Err(e) = stored {
                return log::warn!("a section of the answer was not stored: {e:#}");
            }
        }
        live.apply(event);
    };
    let answered = ask(memory, &mut sink);
    drop(sink);

    // Stopped: the pane has the say; our result and our memory are dropped.
    if run.cancel.load(Ordering::Relaxed) {
        scope.cancel();
        return Outcome::Stopped;
    }
    let mut live = run.live();
    if live.closed {
        return Outcome::Stopped;
    }
    live.closed = true;
    let t = match Transcript::open(ai_dir, store) {
        Ok(t) => t,
        Err(e) => return fail(scope, &mut live, format!("{e:#}")),
    };
    // What the chat learned is kept even when the answer failed.
    let saved = serde_json::to_string(memory).map_err(anyhow::Error::from).and_then(|m| t.set_chat_memory(chat, &m));
    match (answered, saved) {
        (Ok(answer), saved) => {
            live.timeline.finish_answer(&answer.text);
            let stored = saved.and_then(|()| t.add_message(chat, Author::Agent, &answer.text, &live.details(answer.forced, false)));
            match stored {
                Ok(()) => {
                    live.apply(&Event::Done);
                    scope.finish(&json!({ "chars": answer.text.chars().count(), "steps": answer.steps.len() }));
                    Outcome::Done
                }
                Err(e) => fail(scope, &mut live, format!("{e:#}")),
            }
        }
        (Err(e), _) => fail(scope, &mut live, format!("{e:#}")),
    }
}

fn fail(scope: &Scope, live: &mut Live, error: String) -> Outcome {
    live.apply(&Event::Failed(error.clone()));
    scope.fail(&error);
    Outcome::Failed(error)
}

#[cfg(test)]
mod tests;
