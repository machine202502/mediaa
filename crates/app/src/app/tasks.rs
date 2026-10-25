use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const COMPLETED_HISTORY: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkKind {
    Recording,
    Conversion,
    Recovery,
    LibraryCleanup,
    ImageIo,
    Clipboard,
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
        WorkKind::ImageIo => 4,
        WorkKind::Clipboard => 5,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskKind {
    RecordingStart,
    RecordingFinalize,
    Conversion,
    Screenshot,
    ScreenCapture,
    ConversionProbe,
    ConversionEstimate,
    Recovery,
    LibraryScan,
    LibraryLoad,
    LibraryCleanup,
    ViewerImage,
    LibraryCopyImage,
    ViewerAudio,
    Housekeeping,
    Catalog,
    ExternalOpen,
    RenameMedia,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TaskExit {
    pub id: TaskId,
    pub kind: TaskKind,
    pub panicked: bool,
    pub timed_out: bool,
}

struct ManagedTask {
    kind: TaskKind,
    handle: JoinHandle<()>,
    cancel: Option<Arc<AtomicBool>>,
    deadline: Option<Instant>,
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
        self.spawn_with_deadline(kind, name, cancel, None, worker)
    }

    pub(crate) fn spawn_with_deadline(
        &mut self,
        kind: TaskKind,
        name: &str,
        cancel: Option<Arc<AtomicBool>>,
        deadline: Option<Duration>,
        worker: impl FnOnce() + Send + 'static,
    ) -> anyhow::Result<TaskId> {
        let handle = std::thread::Builder::new()
            .name(name.into())
            .spawn(worker)?;
        Ok(self.manage_with_deadline(kind, handle, cancel, deadline))
    }

    #[cfg(test)]
    pub(crate) fn manage(
        &mut self,
        kind: TaskKind,
        handle: JoinHandle<()>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> TaskId {
        self.manage_with_deadline(kind, handle, cancel, None)
    }

    pub(crate) fn manage_with_deadline(
        &mut self,
        kind: TaskKind,
        handle: JoinHandle<()>,
        cancel: Option<Arc<AtomicBool>>,
        deadline: Option<Duration>,
    ) -> TaskId {
        self.next_id += 1;
        let id = TaskId(self.next_id);
        self.tasks.insert(
            id,
            ManagedTask {
                kind,
                handle,
                cancel,
                deadline: deadline.map(|duration| Instant::now() + duration),
            },
        );
        id
    }

    pub(crate) fn finish(&mut self, id: TaskId) -> Option<TaskExit> {
        if let Some(exit) = self.completed.remove(&id) {
            return Some(exit);
        }
        if !self.tasks.get(&id)?.handle.is_finished() {
            return None;
        }
        let task = self.tasks.remove(&id)?;
        let timed_out = task
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline);
        Some(TaskExit {
            id,
            kind: task.kind,
            panicked: task.handle.join().is_err(),
            timed_out,
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

    pub(crate) fn detach(&mut self, id: TaskId) -> Option<TaskKind> {
        let task = self.tasks.remove(&id)?;
        if let Some(cancel) = &task.cancel {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Some(task.kind)
    }

    pub(crate) fn detach_kind(&mut self, kind: TaskKind) -> usize {
        let ids: Vec<_> = self
            .tasks
            .iter()
            .filter_map(|(id, task)| (task.kind == kind).then_some(*id))
            .collect();
        ids.into_iter().filter_map(|id| self.detach(id)).count()
    }

    pub(crate) fn overdue(&self, id: TaskId) -> bool {
        self.tasks
            .get(&id)
            .and_then(|task| task.deadline)
            .is_some_and(|deadline| Instant::now() >= deadline)
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
                let exit = self.finish(id)?;
                self.completed.insert(exit.id, exit);
                while self.completed.len() > COMPLETED_HISTORY {
                    self.completed.pop_first();
                }
                Some(exit)
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn wait(&mut self, id: TaskId) -> Option<TaskExit> {
        if let Some(exit) = self.completed.remove(&id) {
            return Some(exit);
        }
        let task = self.tasks.remove(&id)?;
        let timed_out = task
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline);
        Some(TaskExit {
            id,
            kind: task.kind,
            panicked: task.handle.join().is_err(),
            timed_out,
        })
    }

    #[cfg(test)]
    pub(crate) fn is_active(&self, id: TaskId) -> bool {
        self.tasks.contains_key(&id)
    }

    pub(crate) fn active_count(&self) -> usize {
        self.tasks.len()
    }

    #[cfg(test)]
    pub(crate) fn completed_count(&self) -> usize {
        self.completed.len()
    }
}

#[cfg(test)]
mod tests {
    use super::{ActiveTasks, TaskKind, TaskSupervisor, WorkKind, COMPLETED_HISTORY};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

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
        supervisor.wait(second).unwrap();
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

        let joined = supervisor.wait(task).unwrap();
        assert_eq!(joined.id, task);
        assert!(joined.panicked);
        assert!(supervisor.wait(task).is_none());
    }

    #[test]
    fn unfinished_tasks_remain_tracked_without_blocking() {
        let (release, wait) = crossbeam_channel::bounded(1);
        let mut supervisor = TaskSupervisor::default();
        let task = supervisor
            .spawn(TaskKind::Conversion, "blocked-test", None, move || {
                let _ = wait.recv();
            })
            .unwrap();
        assert!(supervisor.finish(task).is_none());
        assert_eq!(supervisor.active_count(), 1);
        release.send(()).unwrap();
        supervisor.wait(task);
        assert_eq!(supervisor.active_count(), 0);
    }

    #[test]
    fn detaching_stops_tracking_an_uncooperative_task() {
        let (release, wait) = crossbeam_channel::bounded(1);
        let mut supervisor = TaskSupervisor::default();
        let task = supervisor
            .spawn(TaskKind::Screenshot, "detach-test", None, move || {
                let _ = wait.recv();
            })
            .unwrap();
        assert_eq!(supervisor.detach(task), Some(TaskKind::Screenshot));
        assert_eq!(supervisor.active_count(), 0);
        release.send(()).unwrap();
    }

    #[test]
    fn detaching_disposable_work_keeps_required_work_tracked() {
        let (release, wait) = crossbeam_channel::bounded(1);
        let mut supervisor = TaskSupervisor::default();
        let disposable = supervisor
            .spawn(TaskKind::Catalog, "catalog", None, move || {
                let _ = wait.recv();
            })
            .unwrap();
        let required = supervisor
            .spawn(TaskKind::Screenshot, "screenshot", None, || {
                std::thread::sleep(Duration::from_millis(20));
            })
            .unwrap();
        assert_eq!(supervisor.detach_kind(TaskKind::Catalog), 1);
        assert_eq!(supervisor.active_count(), 1);
        assert!(!supervisor.is_active(disposable));
        assert!(supervisor.is_active(required));
        release.send(()).unwrap();
    }

    #[test]
    fn completed_history_is_bounded() {
        let mut supervisor = TaskSupervisor::default();
        for _ in 0..COMPLETED_HISTORY + 10 {
            supervisor
                .spawn(TaskKind::Housekeeping, "history-test", None, || {})
                .unwrap();
        }
        while supervisor
            .tasks
            .values()
            .any(|task| !task.handle.is_finished())
        {
            std::thread::yield_now();
        }
        assert_eq!(supervisor.reap_finished().len(), COMPLETED_HISTORY + 10);
        assert_eq!(supervisor.completed_count(), COMPLETED_HISTORY);
    }

    #[test]
    fn overdue_tasks_keep_their_deadline_status() {
        let mut supervisor = TaskSupervisor::default();
        let task = supervisor
            .spawn_with_deadline(
                TaskKind::ConversionEstimate,
                "deadline-test",
                None,
                Some(Duration::from_millis(1)),
                || std::thread::sleep(Duration::from_millis(10)),
            )
            .unwrap();
        while !supervisor.tasks[&task].handle.is_finished() {
            std::thread::yield_now();
        }
        assert!(supervisor.finish(task).unwrap().timed_out);
    }

    #[test]
    fn direct_threads_are_limited_to_owned_long_lived_workers() {
        fn files(dir: &std::path::Path, output: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    files(&path, output);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    let test_file = path.file_name().is_some_and(|name| name == "tests.rs")
                        || path
                            .components()
                            .any(|component| component.as_os_str() == "tests");
                    if !test_file {
                        output.push(path);
                    }
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut source_files = Vec::new();
        files(&root, &mut source_files);
        let allowed = [
            "app/tasks.rs",
            "clipboard_service.rs",
            "latest_worker.rs",
            "settings_writer.rs",
            "thumbs.rs",
        ];
        let mut direct = Vec::new();
        for path in source_files {
            let text = std::fs::read_to_string(&path).unwrap();
            if text.contains("std::thread::spawn") || text.contains("std::thread::Builder::new") {
                let relative = path
                    .strip_prefix(&root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                if !allowed.contains(&relative.as_str()) {
                    direct.push(relative);
                }
            }
        }
        assert!(
            direct.is_empty(),
            "unowned direct threads: {}",
            direct.join(", ")
        );
    }
}
