use crate::TaskId;

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
