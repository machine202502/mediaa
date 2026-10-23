//! How one kind of task runs. The agent implements this; tests use a fake.
//!
//! A run is cooperative. [`RunCtx::checkpoint`] saves a finished chunk, [`RunCtx::should_stop`]
//! is the pause/cancel flag (it is raised at once, from any thread), and [`RunCtx::progress`]
//! updates the bar without committing a chunk. After a chunk the scheduler takes the resource
//! back. A chunk that was not checkpointed is discarded.

use crate::{Error, Result, Task};
use serde_json::Value;
use std::time::Duration;

/// What the runner did with one poll.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Inside a chunk (a model call). The resource stays taken.
    Busy,
    /// One chunk is saved. The resource is free for the next participant.
    Chunk,
    /// The task finished. `value` is its result.
    Done(Value),
    /// The task failed. The checkpoint stays.
    Failed(String),
    /// Stop was requested and the unfinished chunk was dropped.
    Stopped,
    /// The run is stuck: its work has shown no sign of life (no checkpoint, no progress, no
    /// heartbeat) for longer than the resource's stale limit. `idle` is that time. The call is
    /// cancelled, its model is unloaded and the task goes back to the queue at its cursor.
    Hang { idle: Duration },
}

pub trait RunCtx {
    /// Save a finished chunk. Ignored when the task is no longer running (stop or a stale run);
    /// check [`RunCtx::should_stop`] and return [`Step::Stopped`].
    fn checkpoint(&mut self, cursor: &Value, partial: &Value, progress: f32) -> Result<()>;
    fn should_stop(&self) -> bool;
    fn progress(&mut self, progress: f32) -> Result<()>;
    /// How long a run of this task's resource may show no sign of life (see [`Step::Hang`]).
    fn stale(&self) -> Duration;
}

pub trait Runner: Send {
    /// `true` when this runner performs `kind`. The first match is used.
    fn handles(&self, kind: &str) -> bool;
    fn step(&mut self, task: &Task, ctx: &mut dyn RunCtx) -> std::result::Result<Step, String>;
}

impl From<Error> for String {
    fn from(value: Error) -> String {
        value.to_string()
    }
}
