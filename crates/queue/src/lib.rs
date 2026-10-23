//! Task queue: composites, dependencies, resource pools and chunked runs.
//!
//! The host opens a store, builds a [`Scheduler`], registers a [`Runner`] per kind, and calls
//! [`Scheduler::tick`] for one quantum (reap stale runs, step each live run once, then start at
//! most one wave of new runs). [`Scheduler::snapshot`] is what the UI shows. [`Scheduler::subscribe`]
//! wakes the studio; on an event for its recording it re-reads the recording. [`Scheduler::models_needed`]
//! is what the engine unloads last. [`Scheduler::purge_history`] drops finished top-level tasks older
//! than [`HISTORY`] (30 days).
//!
//! ```ignore
//! let store = SqliteStore::open(path, Arc::new(SystemClock))?;
//! let sched = Scheduler::new(Arc::new(store), vec![
//!     Pool { resource: Resource::Speech, capacity: 1, memory: 0 },
//!     Pool { resource: Resource::Llm, capacity: 1, memory: budget },
//! ]);
//! sched.register(Box::new(my_runner));
//! sched.tick()?;
//! ```
//!
//! A leaf must name [`NewTask::resource`]; enqueue rejects one that does not. A composite or an
//! inline task may omit it. Also set [`NewTask::memory`], [`NewTask::model`], and
//! [`NewTask::chat`] or [`NewTask::analysis`] (the fairness key).
//! [`Scheduler::begin_inline`] is work the chat itself runs: it is never claimed.
//! [`Scheduler::recover`] cancels an inline run that was still going. Pausing a composite pauses
//! its descendant leaves.
//!
//! `Stopping` means stop was asked and the run has not returned. The slot stays taken, `resume`
//! is rejected, and the snapshot shows it on the task and on [`Live::stopping`]. `resume` is only
//! paused → queued. `restart` (заново) requeues that task unless it is running or stopping, clears
//! its cursor, and brings back failed or cancelled work under it and tasks cancelled because of it.
//! Done work under it stays done. [`SqliteStore::open`] takes a file path and creates schema
//! version 2. Any other `user_version` is dropped and created fresh.

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

/// Finished top-level tasks older than this are removed by [`Scheduler::purge_history`].
pub const HISTORY: Duration = Duration::from_secs(30 * 24 * 60 * 60);
