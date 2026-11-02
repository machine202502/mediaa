use mediaa_protocol::backend::{
    AgentKindStatWire, AgentNoteWire, AgentResourceWire, AgentScopeWire, AgentSnapshotWire,
    AgentStatsWire, AgentTaskStatusWire, AgentTaskViewWire, AgentWaitReasonWire,
};
use mediaa_queue::{KindStat, Note, Resource, Snapshot, Stats, Status, Task, TaskView, WaitReason};
use std::collections::BTreeMap;

pub fn snapshot(
    world: &Snapshot,
    kind_stats: &[KindStat],
    slots: &mediaa_core::slots::Status,
    version: u64,
) -> AgentSnapshotWire {
    let tasks = world.tasks.iter().map(view).collect();
    let mut scopes: BTreeMap<String, Option<String>> = BTreeMap::new();
    for task in &world.tasks {
        let task = &task.task;
        let Some(scope) = &task.scope else {
            continue;
        };
        let name = scopes.entry(scope.clone()).or_default();
        if task.kind == "analysis" && name.is_none() {
            *name = Some(task.title.clone());
        }
    }
    let mut scopes: Vec<AgentScopeWire> = scopes
        .into_iter()
        .map(|(key, name)| AgentScopeWire {
            name: name.unwrap_or_else(|| format!("{}…", key.chars().take(8).collect::<String>())),
            key,
        })
        .collect();
    scopes.sort_by_key(|scope| scope.name.to_lowercase());
    AgentSnapshotWire {
        version,
        now_ms: world.now_ms,
        tasks,
        kind_stats: kind_stats.iter().map(kind_stat).collect(),
        scopes,
        slots: slots.clone(),
    }
}

pub fn details(task: &Task) -> mediaa_protocol::backend::AgentTaskDetailsWire {
    mediaa_protocol::backend::AgentTaskDetailsWire {
        task: view(&TaskView {
            task: task.clone(),
            wait: None,
            elapsed_ms: 0,
            estimate_ms: None,
        }),
        result: task.result.clone().unwrap_or(serde_json::Value::Null),
        detail: task.detail.clone(),
        partial: task.partial.clone(),
    }
}

fn view(view: &TaskView) -> AgentTaskViewWire {
    let task = &view.task;
    AgentTaskViewWire {
        id: task.id,
        parent: task.parent,
        kind: task.kind.clone(),
        title: task.title.clone(),
        scope: task.scope.clone(),
        status: status(task.status),
        progress: task.progress,
        waiting: view.wait.as_ref().map(wait),
        created_ms: task.created_ms,
        updated_ms: task.updated_ms,
        started_ms: task.started_ms,
        finished_ms: task.finished_ms,
        elapsed_ms: view.elapsed_ms,
        estimate_ms: view.estimate_ms,
        error: task.error.clone(),
        note: task.note.as_ref().map(note),
        stats: stats(&task.stats),
        units: task.units,
        resource: task.resource.map(resource_wire),
        model: task.model.clone(),
        role: task.role.code(),
        inline: task.inline,
        attempts: task.attempts,
        input: task.input.clone(),
        cursor: task.cursor.clone(),
    }
}

fn status(status: Status) -> AgentTaskStatusWire {
    match status {
        Status::Queued => AgentTaskStatusWire::Queued,
        Status::Running => AgentTaskStatusWire::Running,
        Status::Stopping => AgentTaskStatusWire::Stopping,
        Status::Paused => AgentTaskStatusWire::Paused,
        Status::Waiting => AgentTaskStatusWire::Waiting,
        Status::Done => AgentTaskStatusWire::Done,
        Status::Failed => AgentTaskStatusWire::Failed,
        Status::Cancelled => AgentTaskStatusWire::Cancelled,
    }
}

fn resource_wire(resource: Resource) -> AgentResourceWire {
    match resource {
        Resource::Speech => AgentResourceWire::Speech,
        Resource::Embed => AgentResourceWire::Embed,
        Resource::Llm => AgentResourceWire::Llm,
        Resource::Provider => AgentResourceWire::Provider,
    }
}

fn wait(wait: &WaitReason) -> AgentWaitReasonWire {
    match wait {
        WaitReason::Dependency => AgentWaitReasonWire::Dependency,
        WaitReason::Resource(resource) => AgentWaitReasonWire::Resource(resource_wire(*resource)),
        WaitReason::Memory => AgentWaitReasonWire::Memory,
        WaitReason::Paused => AgentWaitReasonWire::Paused,
        WaitReason::Turn => AgentWaitReasonWire::Turn,
    }
}

fn note(note: &Note) -> AgentNoteWire {
    match note {
        Note::PausedBy { title } => AgentNoteWire::PausedBy {
            title: title.clone(),
        },
        Note::FailedBy { title } => AgentNoteWire::FailedBy {
            title: title.clone(),
        },
        Note::Hang { resource, idle_s } => AgentNoteWire::Hang {
            resource: resource_wire(*resource),
            idle_s: *idle_s,
        },
    }
}

fn stats(stats: &Stats) -> AgentStatsWire {
    AgentStatsWire {
        model_calls: stats.model_calls,
        prompt_tokens: stats.prompt_tokens,
        completion_tokens: stats.completion_tokens,
        model_ms: stats.model_ms,
        models: stats.models.clone(),
    }
}

fn kind_stat(stat: &KindStat) -> AgentKindStatWire {
    AgentKindStatWire {
        kind: stat.kind.clone(),
        runs: stat.runs,
        ms: stat.ms,
        units: stat.units,
    }
}
