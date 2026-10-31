pub mod clock;
pub mod events;
pub mod runner;
pub mod scheduler;
pub mod snapshot;
pub mod state;
pub mod store;

pub use clock::{Clock, FakeClock, SystemClock};
pub use events::{Event, EventKind};
pub use runner::{RunCtx, Runner, Step};
pub use scheduler::{LiveRun, Pool, Scheduler, Turn};
pub use snapshot::{Live, ModelNeed, PoolLoad, Snapshot, TaskView, WaitReason};
pub use state::{
    composite_error, composite_status, effective_status, elapsed, estimate, estimate_tree,
    failure_note, hang_note, models_needed, participant, pause_note, Child, ChildState, DepHold,
    Filter, KindGroup, KindStat, NewTask, Note, Resource, Role, Stats, Status, StopIntent, Task,
};
pub use store::{MemStore, SqliteStore, Store};

use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Bad(String),
}

pub type Result<T> = std::result::Result<T, Error>;
pub type TaskId = i64;

pub const HISTORY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
