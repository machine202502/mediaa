use super::{Answer, Event, Memory, Metrics, StepRecord, WaitReason};
use crate::scope::Scope;
use mediaa_core::chat::{Author, ChatId, Timeline, STEP_RUNNING};
use mediaa_core::transcript::Transcript;
use mediaa_queue::{Store, TaskId};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    Stopped,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Live {
    pub timeline: Timeline,
    pub waiting: Option<WaitReason>,
    pub scanning: Option<(usize, usize)>,
    pub metrics: Option<Metrics>,
    pub steps: Vec<StepRecord>,
    pub stored: usize,
    pub steers_taken: usize,
    pub state: State,
    pub continuing: bool,
    closed: bool,
}

impl Live {
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
        Live {
            timeline: Timeline::default(),
            waiting: None,
            scanning: None,
            metrics: None,
            steps: Vec::new(),
            stored: 0,
            steers_taken: 0,
            state: State::Running,
            continuing: false,
            closed: false,
        }
    }

    pub fn apply(&mut self, event: &Event) {
        if !matches!(self.state, State::Running) {
            return;
        }
        if self.closed && !matches!(event, Event::Done | Event::Stopped | Event::Failed(_)) {
            return;
        }
        if !matches!(event, Event::Waiting { .. } | Event::Metrics(_)) {
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
            Event::SteerTaken { text } => {
                self.timeline.steer(text);
                self.steers_taken += 1;
            }
            Event::Scanning { done, total } => self.scanning = Some((*done, *total)),
            Event::Metrics(metrics) => self.metrics = Some(*metrics),
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

    fn details(&self, forced: bool, stopped: bool) -> serde_json::Value {
        let mut details =
            json!({ "steps": self.steps, "forced": forced, "timeline": self.timeline.to_value() });
        if let Some(metrics) = self.metrics {
            details["metrics"] = json!(metrics);
        }
        if stopped {
            details["stopped"] = json!(true);
        }
        details
    }
}

#[derive(Clone)]
pub struct Run {
    pub cancel: Arc<AtomicBool>,
    live: Arc<Mutex<Live>>,
    task: TaskId,
    store: Arc<dyn Store>,
}

impl Run {
    pub fn new(scope: &Scope) -> Run {
        Run {
            cancel: Arc::new(AtomicBool::new(false)),
            live: Arc::new(Mutex::new(Live::new())),
            task: scope.task(),
            store: scope.store().clone(),
        }
    }

    pub fn live(&self) -> MutexGuard<'_, Live> {
        self.live.lock().expect("live answer lock")
    }

    pub fn finished(&self) -> bool {
        self.live().state != State::Running
    }

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
        t.add_message(
            chat,
            Author::Agent,
            live.timeline.draft(),
            &live.details(false, true),
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Stopped,
    Failed(String),
}

pub fn work(
    run: &Run,
    scope: &Scope,
    ai_dir: &Path,
    store: &str,
    chat: ChatId,
    memory: &mut Memory,
    ask: impl FnOnce(&mut Memory, &mut (dyn FnMut(&Event) + Send)) -> anyhow::Result<Answer>,
) -> Outcome {
    let sections = Transcript::open(ai_dir, store);
    let live = run.live.clone();
    let mut sink = move |event: &Event| {
        let mut live = live.lock().expect("live answer lock");
        if live.closed {
            return;
        }
        if let Event::Say(text) = event {
            let stored = sections
                .as_ref()
                .map_err(|e| anyhow::anyhow!("{e:#}"))
                .and_then(|t| {
                    t.add_message(chat, Author::Agent, text, &json!({ "section": true }))
                });
            if let Err(e) = stored {
                return log::warn!("a section of the answer was not stored: {e:#}");
            }
        }
        live.apply(event);
    };
    let answered = ask(memory, &mut sink);
    drop(sink);

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
    let saved = serde_json::to_string(memory)
        .map_err(anyhow::Error::from)
        .and_then(|m| t.set_chat_memory(chat, &m));
    match (answered, saved) {
        (Ok(answer), saved) => {
            live.timeline.finish_answer(&answer.text);
            let stored = saved.and_then(|()| {
                t.add_message(
                    chat,
                    Author::Agent,
                    &answer.text,
                    &live.details(answer.forced, false),
                )
            });
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
