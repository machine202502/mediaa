use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkKind {
    Recording,
    Conversion,
    Recovery,
    LibraryCleanup,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct ActiveTasks(u8);

impl ActiveTasks {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn add(mut self, kind: WorkKind, active: bool) -> Self {
        if active {
            self.0 |= bit(kind);
        }
        self
    }

    pub(crate) fn any(self) -> bool {
        self.0 != 0
    }

    pub(crate) fn any_except(self, kind: WorkKind) -> bool {
        self.0 & !bit(kind) != 0
    }
}

fn bit(kind: WorkKind) -> u8 {
    1 << match kind {
        WorkKind::Recording => 0,
        WorkKind::Conversion => 1,
        WorkKind::Recovery => 2,
        WorkKind::LibraryCleanup => 3,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKind {
    RecordingStart,
    RecordingFinalize,
    Conversion,
    Recovery,
    LibraryScan,
    LibraryCleanup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskExit {
    pub id: TaskId,
    pub kind: TaskKind,
    pub panicked: bool,
}

struct ManagedTask {
    kind: TaskKind,
    handle: JoinHandle<()>,
    cancel: Option<Arc<AtomicBool>>,
}

#[derive(Default)]
pub(crate) struct TaskSupervisor {
    next_id: u64,
    tasks: BTreeMap<TaskId, ManagedTask>,
    completed: BTreeMap<TaskId, TaskExit>,
}

impl TaskSupervisor {
    pub(crate) fn spawn(
        &mut self,
        kind: TaskKind,
        name: &str,
        cancel: Option<Arc<AtomicBool>>,
        worker: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<TaskId> {
        let handle = std::thread::Builder::new()
            .name(name.into())
            .spawn(worker)?;
        Ok(self.manage(kind, handle, cancel))
    }

    pub(crate) fn manage(
        &mut self,
        kind: TaskKind,
        handle: JoinHandle<()>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> TaskId {
        self.next_id += 1;
        let id = TaskId(self.next_id);
        self.tasks.insert(
            id,
            ManagedTask {
                kind,
                handle,
                cancel,
            },
        );
        id
    }

    pub(crate) fn join(&mut self, id: TaskId) -> Option<TaskExit> {
        if let Some(exit) = self.completed.remove(&id) {
            return Some(exit);
        }
        let task = self.tasks.remove(&id)?;
        Some(TaskExit {
            id,
            kind: task.kind,
            panicked: task.handle.join().is_err(),
        })
    }

    pub(crate) fn cancel(&self, id: TaskId) {
        if let Some(cancel) = self.tasks.get(&id).and_then(|task| task.cancel.as_ref()) {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
    }

    pub(crate) fn cancel_kind(&self, kind: TaskKind) {
        for task in self.tasks.values().filter(|task| task.kind == kind) {
            if let Some(cancel) = &task.cancel {
                cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
    }

    pub(crate) fn reap_finished(&mut self) -> Vec<TaskExit> {
        let finished: Vec<_> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| task.handle.is_finished().then_some(*id))
            .collect();
        finished
            .into_iter()
            .filter_map(|id| {
                let exit = self.join(id)?;
                self.completed.insert(exit.id, exit);
                Some(exit)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self, id: TaskId) -> bool {
        self.tasks.contains_key(&id)
    }

    pub(crate) fn active_count(&self) -> usize {
        self.tasks.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{ActiveTasks, TaskKind, TaskSupervisor, WorkKind};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn active_work_is_tracked_by_kind() {
        let tasks = ActiveTasks::new()
            .add(WorkKind::Recording, true)
            .add(WorkKind::Conversion, false)
            .add(WorkKind::Recovery, true);
        assert!(tasks.any());
        assert!(tasks.any_except(WorkKind::Recording));
        assert!(!ActiveTasks::new().any());
    }

    #[test]
    fn supervisor_joins_reaps_and_cancels_tasks() {
        let mut supervisor = TaskSupervisor::default();
        let first = supervisor
            .spawn(TaskKind::Conversion, "task-test", None, || {})
            .unwrap();
        while !supervisor.tasks[&first].handle.is_finished() {
            std::thread::yield_now();
        }
        let exits = supervisor.reap_finished();
        assert_eq!(exits.len(), 1);
        assert_eq!(exits[0].id, first);
        assert_eq!(exits[0].kind, TaskKind::Conversion);
        assert!(!exits[0].panicked);

        let cancel = Arc::new(AtomicBool::new(false));
        let second = supervisor
            .spawn(
                TaskKind::Recovery,
                "cancel-test",
                Some(cancel.clone()),
                || {},
            )
            .unwrap();
        supervisor.cancel(second);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(supervisor.is_active(second));
        supervisor.join(second).unwrap();
        assert_eq!(supervisor.active_count(), 0);
    }

    #[test]
    fn reaping_keeps_the_terminal_status_for_the_owner() {
        let mut supervisor = TaskSupervisor::default();
        let task = supervisor
            .spawn(TaskKind::Conversion, "panic-test", None, || {
                panic!("worker panic")
            })
            .unwrap();
        while !supervisor.tasks[&task].handle.is_finished() {
            std::thread::yield_now();
        }

        let reaped = supervisor.reap_finished();
        assert_eq!(reaped.len(), 1);
        assert!(reaped[0].panicked);

        let joined = supervisor.join(task).unwrap();
        assert_eq!(joined.id, task);
        assert!(joined.panicked);
        assert!(supervisor.join(task).is_none());
    }
}
