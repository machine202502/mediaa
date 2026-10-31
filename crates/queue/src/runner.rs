use crate::{Error, Result, Task};
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Busy,
    Chunk,
    Done(Value),
    Failed(String),
    Stopped,
    Hang { idle: Duration },
}

pub trait RunCtx {
    fn checkpoint(&mut self, cursor: &Value, partial: &Value, progress: f32) -> Result<()>;
    fn should_stop(&self) -> bool;
    fn progress(&mut self, progress: f32) -> Result<()>;
    fn stale(&self) -> Duration;
}

pub trait Runner: Send {
    fn handles(&self, kind: &str) -> bool;
    fn step(&mut self, task: &Task, ctx: &mut dyn RunCtx) -> std::result::Result<Step, String>;
}

impl From<Error> for String {
    fn from(value: Error) -> String {
        value.to_string()
    }
}
