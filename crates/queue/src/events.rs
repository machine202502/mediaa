//! Changes a subscriber (the studio, the queue page) re-reads on.

use crate::TaskId;

/// What changed. `scope` is the recording or folder the task belongs to, when it has one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Started,
    Chunk,
    Paused,
    Resumed,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub task: TaskId,
    pub scope: Option<String>,
    pub kind: EventKind,
}
