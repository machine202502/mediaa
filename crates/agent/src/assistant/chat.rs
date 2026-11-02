use super::{Answer, Event, Memory};
use crate::scope::Scope;
pub use mediaa_core::assistant::{Live, State};
use mediaa_core::chat::{Author, ChatId};
use mediaa_core::transcript::Transcript;
use mediaa_queue::{Store, TaskId};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

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
        if live.closed() {
            return Ok(());
        }
        live.apply(&Event::Stopped);
        live.close();
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
        if live.closed() {
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
    if live.closed() {
        return Outcome::Stopped;
    }
    live.close();
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
