use crate::state::{self, effective_status, estimate_tree, DepHold, Resource, Status, Task, World};
use crate::TaskId;
use serde_json::Value;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitReason {
    Dependency,
    Resource(Resource),
    Memory,
    Paused,
    Turn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskView {
    pub task: Task,
    pub wait: Option<WaitReason>,
    pub elapsed_ms: i64,
    pub estimate_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Live {
    pub id: TaskId,
    pub kind: String,
    pub title: String,
    pub scope: Option<String>,
    pub resource: Resource,
    pub chunk: Value,
    pub progress: f32,
    pub elapsed_ms: i64,
    pub stopping: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelNeed {
    pub model: String,
    pub tasks: Vec<TaskId>,
}

#[derive(Debug, Clone)]
pub struct PoolLoad {
    pub resource: Resource,
    pub free_slots: usize,
    pub memory_left: u64,
    pub winners: Vec<TaskId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub tasks: Vec<TaskView>,
    pub running: Vec<Live>,
    pub now_ms: i64,
}

pub fn build(world: &World, runs: &[(TaskId, Resource)], loads: &[PoolLoad], now: i64) -> Snapshot {
    let mut tasks = world.tasks.clone();
    state::sort_tasks(&mut tasks);
    let tasks = tasks
        .into_iter()
        .map(|task| {
            let wait = explain(world, loads, &task);
            let elapsed_ms = ms(state::elapsed(&task, now));
            let estimate_ms = estimate_tree(&task, &world.tasks, now, &world.kind_stats).map(ms);
            TaskView {
                task,
                wait,
                elapsed_ms,
                estimate_ms,
            }
        })
        .collect();
    let mut running = Vec::new();
    for (id, resource) in runs {
        let Some(task) = world.task(*id) else {
            continue;
        };
        running.push(Live {
            id: *id,
            kind: task.kind.clone(),
            title: task.title.clone(),
            scope: task.scope.clone(),
            resource: *resource,
            chunk: task.cursor.clone(),
            progress: task.progress,
            elapsed_ms: ms(state::elapsed(task, now)),
            stopping: task.status == Status::Stopping,
        });
    }
    running.sort_by_key(|live| live.id);
    Snapshot {
        tasks,
        running,
        now_ms: now,
    }
}

fn ms(duration: Duration) -> i64 {
    duration.as_millis() as i64
}

fn rank(reason: &WaitReason) -> u8 {
    match reason {
        WaitReason::Paused => 0,
        WaitReason::Dependency => 1,
        WaitReason::Memory => 2,
        WaitReason::Resource(_) => 3,
        WaitReason::Turn => 4,
    }
}

pub fn explain(world: &World, loads: &[PoolLoad], task: &Task) -> Option<WaitReason> {
    if task.status.is_finished() || task.status == Status::Paused {
        return None;
    }
    let children: Vec<&Task> = world
        .tasks
        .iter()
        .filter(|item| item.parent == Some(task.id))
        .collect();
    if !children.is_empty() {
        if !matches!(task.status, Status::Waiting | Status::Queued) {
            return None;
        }
        if children
            .iter()
            .any(|child| effective_status(child) == Status::Paused)
        {
            return Some(WaitReason::Paused);
        }
        let mut worst: Option<WaitReason> = None;
        for child in children {
            if effective_status(child).is_finished() {
                continue;
            }
            let Some(reason) = explain(world, loads, child) else {
                continue;
            };
            worst = Some(match worst {
                Some(prev) if rank(&prev) <= rank(&reason) => prev,
                _ => reason,
            });
        }
        return worst;
    }
    if matches!(task.status, Status::Running | Status::Stopping) {
        return None;
    }
    match state::dep_hold(world, task.id) {
        Some(DepHold::Paused(_)) => return Some(WaitReason::Paused),
        Some(DepHold::Dependency) => return Some(WaitReason::Dependency),
        None => {}
    }
    let Some(resource) = task.resource else {
        return Some(WaitReason::Resource(Resource::Llm));
    };
    let Some(load) = loads.iter().find(|load| load.resource == resource) else {
        return Some(if task.role.is_chat() {
            WaitReason::Turn
        } else {
            WaitReason::Resource(resource)
        });
    };
    if load.free_slots == 0 {
        return Some(if task.role.is_chat() {
            WaitReason::Turn
        } else {
            WaitReason::Resource(resource)
        });
    }
    if task.memory > load.memory_left {
        return Some(WaitReason::Memory);
    }
    if !load.winners.contains(&task.id) {
        Some(WaitReason::Turn)
    } else {
        None
    }
}
