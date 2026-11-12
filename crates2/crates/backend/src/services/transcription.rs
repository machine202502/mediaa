use crate::state::StateStore;
use mediaa2_agent_transcription::host::AgentHost;
use mediaa2_core::agent::AgentSettings;
use mediaa2_core::paths;
use mediaa2_model_eval::slots::Slots;
use mediaa2_model_eval::Connections;
use mediaa2_protocol::{
    CommandError, CommandOk, ErrorCode, TranscriptionCommand, TranscriptionState,
};
use std::collections::BTreeMap;
use std::sync::mpsc;
use std::sync::Arc;

const COMMAND_CAPACITY: usize = 8;
const TICK_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

#[derive(Clone)]
pub struct TranscriptionHandle {
    commands: mpsc::SyncSender<Command>,
}

struct Command {
    command: TranscriptionCommand,
    reply: mpsc::Sender<Result<CommandOk, CommandError>>,
}

impl TranscriptionHandle {
    pub fn new(state: Arc<StateStore>) -> Self {
        let (commands, receiver) = mpsc::sync_channel(COMMAND_CAPACITY);
        let worker = Worker::new(state);
        std::thread::Builder::new()
            .name("mediaa2-transcription".into())
            .spawn(move || worker.run(receiver))
            .expect("transcription thread");
        Self { commands }
    }

    pub async fn dispatch(&self, command: TranscriptionCommand) -> Result<CommandOk, CommandError> {
        let (tx, rx) = mpsc::channel();
        let sender = self.commands.clone();
        tokio::task::spawn_blocking(move || {
            let _ = sender.send(Command { command, reply: tx });
            rx.recv()
        })
        .await
        .map_err(|_| CommandError {
            code: ErrorCode::Internal,
            message: "transcription service stopped".into(),
        })?
        .map_err(|_| CommandError {
            code: ErrorCode::Internal,
            message: "transcription service dropped the reply".into(),
        })?
    }
}

struct Worker {
    state: Arc<StateStore>,
    host: AgentHost,
    snapshot_version: u64,
    event_cursor: usize,
    streaming_text: String,
    streaming_task: Option<i64>,
    /// the last failed model call, explained for the chat view
    chat_error: Option<mediaa2_protocol::TranscriptionChatError>,
    /// provider id -> api key, kept in memory only (`Connection::api_key` is
    /// `#[serde(skip)]`, so the state snapshot never carries keys)
    keys: BTreeMap<String, String>,
}

impl Worker {
    fn new(state: Arc<StateStore>) -> Self {
        let connections = Arc::new(Connections::new(Vec::new()));
        let slots = Slots::new();
        let host = AgentHost::with_connections(connections, slots);
        let mut agent = state.snapshot().settings.agent.clone();
        mediaa2_model_eval::secrets::load(&mut agent);
        let keys = mediaa2_model_eval::secrets::keys(&agent);
        Self {
            state,
            host,
            snapshot_version: 0,
            event_cursor: 0,
            streaming_text: String::new(),
            streaming_task: None,
            chat_error: None,
            keys,
        }
    }

    fn run(mut self, commands: mpsc::Receiver<Command>) {
        let mut last_tick = std::time::Instant::now();
        loop {
            let timeout = TICK_INTERVAL.saturating_sub(last_tick.elapsed());
            match commands.recv_timeout(timeout) {
                Ok(command) => {
                    let _ = command.reply.send(self.handle(command.command));
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
            self.tick();
            last_tick = std::time::Instant::now();
        }
    }

    fn tick(&mut self) {
        let settings = self.current_settings();
        let ai_dir = paths::data_dir().join("ai");
        self.host.tick(&ai_dir, &settings, "en", true);
        self.poll_events();
        self.publish_snapshot();
    }

    fn poll_events(&mut self) {
        let Some(services) = self.host.services() else {
            return;
        };
        let (next, events, _gap) = services.streams.events_since(self.event_cursor);
        self.event_cursor = next;
        for (task_id, event) in events {
            match event {
                mediaa2_core::assistant::Event::AnswerText(text) => {
                    self.streaming_text.push_str(&text);
                    self.streaming_task = Some(task_id);
                    self.chat_error = None;
                }
                mediaa2_core::assistant::Event::StepStarted { .. } => {
                    self.chat_error = None;
                }
                mediaa2_core::assistant::Event::Failed(error) => {
                    self.streaming_text.clear();
                    self.streaming_task = None;
                    self.chat_error = Some(mediaa2_protocol::TranscriptionChatError {
                        kind: mediaa2_agent_transcription::model_errors::model_error_kind(&error)
                            .into(),
                        detail: error,
                    });
                }
                mediaa2_core::assistant::Event::Done | mediaa2_core::assistant::Event::Stopped => {
                    self.streaming_text.clear();
                    self.streaming_task = None;
                }
                _ => {}
            }
        }
    }

    fn current_settings(&self) -> AgentSettings {
        let mut agent = self.state.snapshot().settings.agent.clone();
        for connection in &mut agent.connections {
            if let Some(key) = self.keys.get(&connection.id) {
                connection.api_key = key.clone();
            }
        }
        if let Some(key) = self.keys.get(mediaa2_model_eval::secrets::WEB_KEY) {
            agent.web.api_key = key.clone();
        }
        agent
    }

    fn publish_snapshot(&mut self) {
        let Some(services) = self.host.services() else {
            return;
        };
        let Ok(snapshot) = services.sched.snapshot() else {
            return;
        };
        let marker = format!(
            "{:?}",
            snapshot
                .tasks
                .iter()
                .map(|view| (&view.task.id, &view.task.status))
                .collect::<Vec<_>>()
        );
        if marker.is_empty() && self.snapshot_version == 0 {
            return;
        }
        self.snapshot_version += 1;
        let running = !snapshot.running.is_empty();
        let task = snapshot.running.first().map(|run| run.title.clone());
        let status = snapshot.running.first().map(|run| run.kind.clone());
        let progress = snapshot
            .running
            .first()
            .map(|run| (run.progress * 100.0).round() as u32);
        let tasks = snapshot
            .tasks
            .iter()
            .map(|view| {
                use mediaa2_agent_transcription::queue as q;
                let t = &view.task;
                mediaa2_protocol::transcription::TranscriptionTaskView {
                    id: t.id,
                    parent: t.parent,
                    kind: t.kind.clone(),
                    title: t.title.clone(),
                    scope: t.scope.clone(),
                    status: match t.status {
                        q::Status::Queued => mediaa2_protocol::transcription::TranscriptionTaskStatus::Queued,
                        q::Status::Running => mediaa2_protocol::transcription::TranscriptionTaskStatus::Running,
                        q::Status::Stopping => mediaa2_protocol::transcription::TranscriptionTaskStatus::Stopping,
                        q::Status::Paused => mediaa2_protocol::transcription::TranscriptionTaskStatus::Paused,
                        q::Status::Waiting => mediaa2_protocol::transcription::TranscriptionTaskStatus::Waiting,
                        q::Status::Done => mediaa2_protocol::transcription::TranscriptionTaskStatus::Done,
                        q::Status::Failed => mediaa2_protocol::transcription::TranscriptionTaskStatus::Failed,
                        q::Status::Cancelled => mediaa2_protocol::transcription::TranscriptionTaskStatus::Cancelled,
                    },
                    progress: t.progress,
                    waiting: view.wait.as_ref().map(|w| match w {
                        q::WaitReason::Dependency => mediaa2_protocol::transcription::TranscriptionWaitReason::Dependency,
                        q::WaitReason::Resource(r) => mediaa2_protocol::transcription::TranscriptionWaitReason::Resource(match r {
                            q::Resource::Speech => mediaa2_protocol::transcription::TranscriptionResource::Speech,
                            q::Resource::Embed => mediaa2_protocol::transcription::TranscriptionResource::Embed,
                            q::Resource::Llm => mediaa2_protocol::transcription::TranscriptionResource::Llm,
                            q::Resource::Provider => mediaa2_protocol::transcription::TranscriptionResource::Provider,
                        }),
                        q::WaitReason::Memory => mediaa2_protocol::transcription::TranscriptionWaitReason::Memory,
                        q::WaitReason::Paused => mediaa2_protocol::transcription::TranscriptionWaitReason::Paused,
                        q::WaitReason::Turn => mediaa2_protocol::transcription::TranscriptionWaitReason::Turn,
                    }),
                    created_ms: t.created_ms,
                    updated_ms: t.updated_ms,
                    started_ms: t.started_ms,
                    finished_ms: t.finished_ms,
                    elapsed_ms: view.elapsed_ms,
                    estimate_ms: view.estimate_ms,
                    error: t.error.clone(),
                    note: t.note.as_ref().map(|n| match n {
                        q::Note::PausedBy { title } => mediaa2_protocol::transcription::TranscriptionNote::PausedBy { title: title.clone() },
                        q::Note::FailedBy { title } => mediaa2_protocol::transcription::TranscriptionNote::FailedBy { title: title.clone() },
                        q::Note::Hang { resource, idle_s } => mediaa2_protocol::transcription::TranscriptionNote::Hang {
                            resource: match resource {
                                q::Resource::Speech => mediaa2_protocol::transcription::TranscriptionResource::Speech,
                                q::Resource::Embed => mediaa2_protocol::transcription::TranscriptionResource::Embed,
                                q::Resource::Llm => mediaa2_protocol::transcription::TranscriptionResource::Llm,
                                q::Resource::Provider => mediaa2_protocol::transcription::TranscriptionResource::Provider,
                            },
                            idle_s: *idle_s,
                        },
                    }),
                    stats: mediaa2_protocol::transcription::TranscriptionStats {
                        model_calls: t.stats.model_calls,
                        prompt_tokens: t.stats.prompt_tokens,
                        completion_tokens: t.stats.completion_tokens,
                        model_ms: t.stats.model_ms,
                        models: t.stats.models.clone(),
                    },
                    units: t.units,
                    resource: t.resource.map(|r| match r {
                        q::Resource::Speech => mediaa2_protocol::transcription::TranscriptionResource::Speech,
                        q::Resource::Embed => mediaa2_protocol::transcription::TranscriptionResource::Embed,
                        q::Resource::Llm => mediaa2_protocol::transcription::TranscriptionResource::Llm,
                        q::Resource::Provider => mediaa2_protocol::transcription::TranscriptionResource::Provider,
                    }),
                    model: t.model.clone(),
                    role: t.role.code(),
                    inline: t.inline,
                    attempts: t.attempts,
                    input: t.input.clone(),
                    cursor: t.cursor.clone(),
                }
            })
            .collect();
        let kind_stats = services
            .sched
            .store()
            .kind_stats()
            .unwrap_or_default()
            .into_iter()
            .map(|s| mediaa2_protocol::transcription::TranscriptionKindStat {
                kind: s.kind,
                runs: s.runs,
                ms: s.ms,
                units: s.units,
            })
            .collect();
        let slots_status = services.slots.status();
        let slots = mediaa2_protocol::transcription::TranscriptionSlots {
            groups: slots_status
                .groups
                .iter()
                .map(
                    |g| mediaa2_protocol::transcription::TranscriptionGroupLoad {
                        name: g.name.clone(),
                        busy: g.busy,
                        limit: g.limit,
                        waiting: g.waiting,
                    },
                )
                .collect(),
            web: mediaa2_protocol::transcription::TranscriptionToolLoad {
                busy: slots_status.web.busy,
                limit: slots_status.web.limit,
                waiting: slots_status.web.waiting,
            },
        };
        let scopes = snapshot
            .tasks
            .iter()
            .filter_map(|view| view.task.scope.as_ref().map(|s| (s.clone(), &view.task)))
            .fold(BTreeMap::new(), |mut acc, (key, task)| {
                acc.entry(key).or_insert_with(|| task.title.clone());
                acc
            })
            .into_iter()
            .map(
                |(key, name)| mediaa2_protocol::transcription::TranscriptionScope {
                    name: name.chars().take(8).collect(),
                    key,
                },
            )
            .collect();
        let state = TranscriptionState {
            snapshot: Some(mediaa2_protocol::transcription::TranscriptionSnapshot {
                version: self.snapshot_version,
                now_ms: 0,
                tasks,
                kind_stats,
                scopes,
                slots,
            }),
            running,
            task,
            status,
            progress,
            streaming_text: if self.streaming_text.is_empty() {
                None
            } else {
                Some(self.streaming_text.clone())
            },
            streaming_task: self.streaming_task,
            chat_error: self.chat_error.clone(),
        };
        self.state.update(|current| current.transcription = state);
    }

    fn handle(&mut self, command: TranscriptionCommand) -> Result<CommandOk, CommandError> {
        match command {
            TranscriptionCommand::Start {
                path,
                options,
                from,
            } => {
                let Some(services) = self.host.services() else {
                    return Err(busy("agent is not running"));
                };
                let ai_dir = mediaa2_agent_transcription::catalogs::ai_dir(
                    &self.state.snapshot().settings.media_dir,
                );
                let path = std::path::Path::new(&path);
                match from {
                    Some(stage) => {
                        let from = mediaa2_agent_transcription::stages::Stage::parse(&stage)
                            .ok_or_else(|| invalid(&format!("unknown analysis stage: {stage}")))?;
                        mediaa2_agent_transcription::pipeline::restart(
                            &services.sched,
                            &ai_dir,
                            path,
                            from,
                            options,
                        )
                        .map_err(|error| CommandError {
                            code: ErrorCode::Internal,
                            message: format!("{error:#}"),
                        })?;
                    }
                    None => {
                        mediaa2_agent_transcription::pipeline::start(
                            &services.sched,
                            &ai_dir,
                            path,
                            options,
                        )
                        .map_err(|error| CommandError {
                            code: ErrorCode::Internal,
                            message: format!("{error:#}"),
                        })?;
                    }
                }
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::CancelTask { task } => {
                let Some(services) = self.host.services() else {
                    return Err(busy("agent is not running"));
                };
                let _ = services.sched.cancel(task);
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::LoadTopics { path } => {
                let media_dir = self.state.snapshot().settings.media_dir.clone();
                let ai_dir = mediaa2_agent_transcription::catalogs::ai_dir(&media_dir);
                let file_id = mediaa2_agent_transcription::transcript_db::file_id(
                    std::path::Path::new(&path),
                )
                .map_err(|e| CommandError {
                    code: ErrorCode::NotFound,
                    message: format!("{e:#}"),
                })?;
                let t =
                    mediaa2_agent_transcription::transcript_db::Transcript::open(&ai_dir, &file_id)
                        .map_err(|e| CommandError {
                            code: ErrorCode::NotFound,
                            message: format!("{e:#}"),
                        })?;
                let chapters = t.chapters().map_err(|e| CommandError {
                    code: ErrorCode::Internal,
                    message: format!("{e:#}"),
                })?;
                let topics: Vec<mediaa2_protocol::TopicChapter> = chapters
                    .into_iter()
                    .map(|c| mediaa2_protocol::TopicChapter {
                        start: c.start,
                        end: c.end,
                        title: c.digest.title,
                        summary: c.digest.summary,
                        decisions: c.digest.decisions,
                        actions: c.digest.actions,
                        sections: c
                            .digest
                            .sections
                            .into_iter()
                            .map(|s| mediaa2_protocol::TopicSection {
                                title: s.title,
                                kind: format!("{:?}", s.kind),
                                items: s
                                    .items
                                    .into_iter()
                                    .map(|i| mediaa2_protocol::TopicItem {
                                        text: i.text,
                                        time: i.time,
                                    })
                                    .collect(),
                            })
                            .collect(),
                    })
                    .collect();
                Ok(CommandOk::Topics(topics))
            }
            TranscriptionCommand::LoadChat { store, chat } => {
                let media_dir = self.state.snapshot().settings.media_dir.clone();
                let ai_dir = mediaa2_agent_transcription::catalogs::ai_dir(&media_dir);
                let t =
                    mediaa2_agent_transcription::transcript_db::Transcript::open(&ai_dir, &store)
                        .map_err(|e| CommandError {
                        code: ErrorCode::NotFound,
                        message: format!("{e:#}"),
                    })?;
                let messages = t.messages(chat).map_err(|e| CommandError {
                    code: ErrorCode::Internal,
                    message: format!("{e:#}"),
                })?;
                let entries: Vec<mediaa2_core::chat::TimelineEntry> = messages
                    .into_iter()
                    .map(|m| match m.author {
                        mediaa2_core::chat::Author::User => {
                            mediaa2_core::chat::TimelineEntry::Answer {
                                text: m.text,
                                section: false,
                            }
                        }
                        _ => mediaa2_core::chat::TimelineEntry::Answer {
                            text: m.text,
                            section: false,
                        },
                    })
                    .collect();
                let model =
                    t.chat_model(chat)
                        .ok()
                        .flatten()
                        .map(|s| mediaa2_protocol::ChatModelInfo {
                            connection: s.connection,
                            model: s.model,
                        });
                Ok(CommandOk::ChatView(mediaa2_protocol::ChatView {
                    chat,
                    model,
                    timeline: mediaa2_core::chat::Timeline { entries },
                }))
            }
            TranscriptionCommand::Send {
                store,
                chat,
                question,
                locale,
                path,
                files,
                duration,
                position,
                stored,
                switch_model,
            } => {
                let Some(services) = self.host.services() else {
                    return Err(busy("agent is not running"));
                };
                let media_dir = self.state.snapshot().settings.media_dir.clone();
                let ai_dir = mediaa2_agent_transcription::catalogs::ai_dir(&media_dir);
                let t =
                    mediaa2_agent_transcription::transcript_db::Transcript::open(&ai_dir, &store)
                        .map_err(|e| CommandError {
                        code: ErrorCode::NotFound,
                        message: format!("{e:#}"),
                    })?;
                let chat = match chat {
                    Some(chat) => chat,
                    None => t.create_chat("Chat").map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("{e:#}"),
                    })?,
                };
                if !stored {
                    t.add_message(
                        chat,
                        mediaa2_core::chat::Author::User,
                        &question,
                        &serde_json::Value::Null,
                    )
                    .map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("{e:#}"),
                    })?;
                    let mut open = t.open_chats().map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("{e:#}"),
                    })?;
                    if !open.contains(&chat) {
                        open.push(chat);
                        t.set_open_chats(&open).map_err(|e| CommandError {
                            code: ErrorCode::Internal,
                            message: format!("{e:#}"),
                        })?;
                    }
                }
                let input = if let Some(path) = &path {
                    serde_json::json!({
                        "chat": chat, "store": store, "question": question,
                        "locale": locale, "path": path, "file_id": store,
                        "duration": duration, "position": position,
                    })
                } else {
                    serde_json::json!({
                        "chat": chat, "store": store, "question": question,
                        "locale": locale, "files": files,
                    })
                };
                let settings = services.settings.read().unwrap().clone();
                if switch_model {
                    if let Some(current) = &settings.models.agent {
                        t.set_chat_model(chat, current).map_err(|e| CommandError {
                            code: ErrorCode::Internal,
                            message: format!("{e:#}"),
                        })?;
                    }
                } else if t.chat_model(chat).ok().flatten().is_none() {
                    // a fresh chat keeps the model it started with
                    if let Some(current) = &settings.models.agent {
                        let _ = t.set_chat_model(chat, current);
                    }
                }
                let pinned = t.chat_model(chat).ok().flatten();
                let model = pinned.as_ref().map(|s| s.model.clone());
                let task = mediaa2_agent_transcription::assistant::queued::answer_task(
                    &store,
                    chat,
                    &question,
                    input,
                    mediaa2_agent_transcription::queue::Resource::Llm,
                    model.as_deref(),
                );
                self.chat_error = None;
                services.sched.enqueue(task).map_err(|e| CommandError {
                    code: ErrorCode::Internal,
                    message: format!("{e:#}"),
                })?;
                let needs_title = t
                    .chats()
                    .ok()
                    .and_then(|chats| {
                        chats
                            .iter()
                            .find(|c| c.id == chat)
                            .map(|c| c.title.is_empty())
                    })
                    .unwrap_or(false);
                if needs_title {
                    let title_model = settings.models.titles.as_ref().map(|s| s.model.clone());
                    let title = mediaa2_agent_transcription::assistant::queued::title_task(
                        &store,
                        chat,
                        &question,
                        &question,
                        title_model.as_deref(),
                    );
                    services.sched.enqueue(title).map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("{e:#}"),
                    })?;
                }
                Ok(CommandOk::Chat {
                    chat,
                    model: pinned.map(|s| mediaa2_protocol::ChatModelInfo {
                        connection: s.connection,
                        model: s.model,
                    }),
                })
            }
            TranscriptionCommand::ProviderSave { connection, key } => {
                let mut agent = self.state.snapshot().settings.agent.clone();
                if let Some(existing) = agent.connections.iter_mut().find(|c| c.id == connection.id)
                {
                    existing.name = connection.name.clone();
                    existing.kind = connection.kind;
                    existing.base_url = connection.base_url.clone();
                    existing.proxy = connection.proxy.clone();
                    existing.group = connection.group.clone();
                } else {
                    agent.connections.push(connection.clone());
                }
                if let Some(key) = &key {
                    mediaa2_model_eval::secrets::save_key(&connection.id, key).map_err(|e| {
                        CommandError {
                            code: ErrorCode::Internal,
                            message: format!("keychain: {e:#}"),
                        }
                    })?;
                    self.keys.insert(connection.id.clone(), key.clone());
                } else if self.keys.contains_key(&connection.id) {
                    // keep the previously stored key; the UI never echoes it back
                    if let Some(existing) =
                        agent.connections.iter_mut().find(|c| c.id == connection.id)
                    {
                        existing.api_key =
                            self.keys.get(&connection.id).cloned().unwrap_or_default();
                    }
                }
                self.state.update(|current| {
                    current.settings.agent.connections = agent.connections;
                });
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::ProviderDelete { id } => {
                self.state
                    .update(|current| current.settings.agent.connections.retain(|c| c.id != id));
                self.keys.remove(&id);
                let _ = mediaa2_model_eval::secrets::delete_key(&id);
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::ProviderCheck { connection, key } => {
                let mut conn = connection.clone();
                if let Some(key) = &key {
                    conn.api_key = key.clone();
                } else if let Some(stored) = self.keys.get(&connection.id) {
                    conn.api_key = stored.clone();
                }
                let connections = Arc::new(mediaa2_model_eval::Connections::new(vec![conn]));
                let engine: Arc<dyn mediaa2_model_eval::Engine> =
                    Arc::clone(&connections) as Arc<dyn mediaa2_model_eval::Engine>;
                let list = engine.models(&connection.id).map_err(|e| CommandError {
                    code: ErrorCode::Unavailable,
                    message: format!("{e:#}"),
                })?;
                let mut models = BTreeMap::new();
                models.insert(
                    connection.id.clone(),
                    list.into_iter().map(|m| m.id).collect(),
                );
                Ok(CommandOk::Models(models))
            }
            TranscriptionCommand::ModelsSet { settings: agent } => {
                self.state.update(|current| {
                    current.settings.agent.models = agent.models;
                    current.settings.agent.groups = agent.groups;
                    current.settings.agent.web_limit = agent.web_limit;
                    current.settings.agent.web = agent.web;
                    current.settings.agent.retries = agent.retries;
                });
                // the host applies the new attempt count on its next tick
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::ModelsCheck => {
                let agent = self.state.snapshot().settings.agent.clone();
                let connections = Arc::new(mediaa2_model_eval::Connections::new(
                    self.current_settings().connections,
                ));
                let engine: Arc<dyn mediaa2_model_eval::Engine> =
                    Arc::clone(&connections) as Arc<dyn mediaa2_model_eval::Engine>;
                let mut models = BTreeMap::new();
                for connection in agent.connections {
                    let Ok(list) = engine.models(&connection.id) else {
                        continue;
                    };
                    models.insert(connection.id, list.into_iter().map(|m| m.id).collect());
                }
                Ok(CommandOk::Models(models))
            }
            TranscriptionCommand::OllamaPull { model } => {
                let keys = self.keys.clone();
                let connection = self
                    .current_settings()
                    .connections
                    .into_iter()
                    .find(|c| c.kind == mediaa2_core::agent::ConnectionKind::Ollama)
                    .ok_or_else(|| CommandError {
                        code: ErrorCode::NotFound,
                        message: "no Ollama connection".into(),
                    })?;
                std::thread::Builder::new()
                    .name("mediaa2-ollama-pull".into())
                    .spawn(move || {
                        let mut connection = connection;
                        if connection.api_key.is_empty() {
                            connection.api_key =
                                keys.get(&connection.id).cloned().unwrap_or_default();
                        }
                        let cancel = std::sync::atomic::AtomicBool::new(false);
                        let mut last_log = 0u64;
                        let result = mediaa2_model_eval::ollama::pull(
                            &connection,
                            &model,
                            &mut |done, total| {
                                if total > 0 && done / (total / 10 + 1) > last_log {
                                    last_log = done / (total / 10 + 1);
                                    log::info!("ollama pull {model}: {done}/{total}");
                                }
                            },
                            &cancel,
                        );
                        match result {
                            Ok(()) => log::info!("ollama pull {model}: done"),
                            Err(e) => log::error!("ollama pull {model}: {e:#}"),
                        }
                    })
                    .map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("spawn: {e}"),
                    })?;
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::OllamaServe => {
                let connection = self
                    .current_settings()
                    .connections
                    .into_iter()
                    .find(|c| c.kind == mediaa2_core::agent::ConnectionKind::Ollama)
                    .ok_or_else(|| CommandError {
                        code: ErrorCode::NotFound,
                        message: "no Ollama connection".into(),
                    })?;
                mediaa2_model_eval::ollama::serve(&mediaa2_model_eval::ollama::program_path(
                    &connection,
                ))
                .map_err(|e| CommandError {
                    code: ErrorCode::Unavailable,
                    message: format!("{e:#}"),
                })?;
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::WebCheck => {
                let agent = self.current_settings();
                let hits = mediaa2_agent_transcription::web::search(&agent.web, "mediaa", 3)
                    .map_err(|e| CommandError {
                        code: ErrorCode::Unavailable,
                        message: format!("{e:#}"),
                    })?;
                log::info!("web check: {} hits", hits.len());
                Ok(CommandOk::Web(hits.len() as u32))
            }
            TranscriptionCommand::WebSearchSet { enabled, key } => {
                if let Some(key) = &key {
                    mediaa2_model_eval::secrets::save_key(
                        mediaa2_model_eval::secrets::WEB_KEY,
                        key,
                    )
                    .map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("keychain: {e:#}"),
                    })?;
                    self.keys
                        .insert(mediaa2_model_eval::secrets::WEB_KEY.into(), key.clone());
                }
                self.state
                    .update(|current| current.settings.agent.web.enabled = enabled);
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::RestartStage { task, .. } => {
                let Some(services) = self.host.services() else {
                    return Err(CommandError {
                        code: ErrorCode::Unavailable,
                        message: "agent host is not running".into(),
                    });
                };
                services.sched.restart(task).map_err(|e| CommandError {
                    code: ErrorCode::Internal,
                    message: format!("{e:#}"),
                })?;
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::Pause | TranscriptionCommand::Resume => Ok(CommandOk::Unit),
            TranscriptionCommand::LoadTranscript { path } => {
                let media_dir = self.state.snapshot().settings.media_dir.clone();
                let ai_dir = mediaa2_agent_transcription::catalogs::ai_dir(&media_dir);
                let file_id = mediaa2_agent_transcription::transcript_db::file_id(
                    std::path::Path::new(&path),
                )
                .map_err(|e| CommandError {
                    code: ErrorCode::NotFound,
                    message: format!("{e:#}"),
                })?;
                let t =
                    mediaa2_agent_transcription::transcript_db::Transcript::open(&ai_dir, &file_id)
                        .map_err(|e| CommandError {
                            code: ErrorCode::NotFound,
                            message: format!("{e:#}"),
                        })?;
                let replies = t
                    .dialog()
                    .map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("{e:#}"),
                    })?
                    .into_iter()
                    .map(|r| mediaa2_protocol::command::TranscriptReply {
                        start: r.start,
                        end: r.end,
                        speaker: r.speaker,
                        text: r
                            .words
                            .iter()
                            .map(|w| w.text.as_str())
                            .collect::<Vec<_>>()
                            .join(" "),
                    })
                    .collect();
                let speakers = t
                    .speaker_entries()
                    .map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("{e:#}"),
                    })?
                    .into_iter()
                    .map(|s| mediaa2_protocol::command::TranscriptSpeaker {
                        index: s.0,
                        name: s.1.name.clone(),
                        source: format!("{:?}", s.1.source),
                        suggested: t
                            .speaker_suggestion(s.0)
                            .ok()
                            .flatten()
                            .map(|(name, _)| name),
                    })
                    .collect();
                // the index status of the current embedding model
                let embed_key = self
                    .state
                    .snapshot()
                    .settings
                    .agent
                    .models
                    .embedding
                    .as_ref()
                    .map(|s| s.key());
                let index_ready = match &embed_key {
                    Some(key) => {
                        t.embed_index_ok(key).unwrap_or(false)
                            && t.chunks_todo(key)
                                .map(|todo| todo.is_empty())
                                .unwrap_or(false)
                    }
                    None => false,
                };
                let index_building = self
                    .host
                    .services()
                    .map(|s| {
                        s.sched
                            .snapshot()
                            .map(|snap| {
                                snap.running.iter().any(|run| {
                                    run.kind == "index"
                                        && run.scope.as_deref() == Some(file_id.as_str())
                                })
                            })
                            .unwrap_or(false)
                    })
                    .unwrap_or(false);
                Ok(CommandOk::Transcript(mediaa2_protocol::TranscriptView {
                    file_id,
                    path,
                    replies,
                    speakers,
                    index_ready,
                    index_building,
                }))
            }
            TranscriptionCommand::RenameSpeaker {
                store,
                speaker,
                name,
            } => {
                let media_dir = self.state.snapshot().settings.media_dir.clone();
                let ai_dir = mediaa2_agent_transcription::catalogs::ai_dir(&media_dir);
                let t =
                    mediaa2_agent_transcription::transcript_db::Transcript::open(&ai_dir, &store)
                        .map_err(|e| CommandError {
                        code: ErrorCode::NotFound,
                        message: format!("{e:#}"),
                    })?;
                t.rename_speaker(speaker, &name).map_err(|e| CommandError {
                    code: ErrorCode::Internal,
                    message: format!("{e:#}"),
                })?;
                let _ = t.clear_speaker_suggestion(speaker);
                // the same voice in other recordings gets the name too
                let carried = mediaa2_agent_transcription::names::propagate_rename(
                    &ai_dir, &store, speaker, &name,
                );
                if carried.applied > 0 || carried.suggested > 0 {
                    log::info!(
                        "rename: the name reached {} speakers, {} more got a suggestion",
                        carried.applied,
                        carried.suggested
                    );
                }
                Ok(CommandOk::Unit)
            }
            TranscriptionCommand::DismissSuggestion { store, speaker } => {
                let media_dir = self.state.snapshot().settings.media_dir.clone();
                let ai_dir = mediaa2_agent_transcription::catalogs::ai_dir(&media_dir);
                let t =
                    mediaa2_agent_transcription::transcript_db::Transcript::open(&ai_dir, &store)
                        .map_err(|e| CommandError {
                        code: ErrorCode::NotFound,
                        message: format!("{e:#}"),
                    })?;
                t.clear_speaker_suggestion(speaker)
                    .map_err(|e| CommandError {
                        code: ErrorCode::Internal,
                        message: format!("{e:#}"),
                    })?;
                Ok(CommandOk::Unit)
            }
            _ => Err(busy("transcription: not wired yet")),
        }
    }
}

fn invalid(message: &str) -> CommandError {
    CommandError {
        code: ErrorCode::Invalid,
        message: message.into(),
    }
}

fn busy(message: &str) -> CommandError {
    CommandError {
        code: ErrorCode::Busy,
        message: message.into(),
    }
}
