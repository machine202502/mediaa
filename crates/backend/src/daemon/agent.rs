use super::*;

impl Daemon {
    pub(super) fn send_agent_message(
        &mut self,
        send: mediaa_protocol::backend::AgentSendWire,
    ) -> Result<i64, String> {
        use mediaa_agent::assistant::queued;
        use mediaa_core::chat::Author;
        use mediaa_queue::Resource;

        self.ensure_agent_host();
        let services = self
            .agent_host
            .as_ref()
            .and_then(|host| host.services())
            .ok_or_else(|| "agent is not running".to_string())?;
        let transcript = mediaa_core::transcript::Transcript::open(&services.ai_dir, &send.store)
            .map_err(|error| format!("{error:#}"))?;
        let chat = send
            .chat
            .ok_or_else(|| "agent chat is missing".to_string())?;
        if !send.stored {
            transcript
                .add_message(chat, Author::User, &send.question, &serde_json::Value::Null)
                .map_err(|error| format!("{error:#}"))?;
            let mut open = transcript
                .open_chats()
                .map_err(|error| format!("{error:#}"))?;
            if !open.contains(&chat) {
                open.push(chat);
                transcript
                    .set_open_chats(&open)
                    .map_err(|error| format!("{error:#}"))?;
            }
        }
        let input = if let Some(path) = &send.path {
            serde_json::json!({
                "chat": chat,
                "store": send.store,
                "question": send.question,
                "locale": send.locale,
                "path": path,
                "file_id": send.store,
                "duration": send.duration,
                "position": send.position,
            })
        } else {
            serde_json::json!({
                "chat": chat,
                "store": send.store,
                "question": send.question,
                "locale": send.locale,
                "files": send.files,
            })
        };
        let settings = services.settings.read().expect("settings lock").clone();
        let (resource, model) = match settings.models.agent.as_ref() {
            Some(source) => (Resource::Llm, Some(source.model.clone())),
            None => (Resource::Llm, None),
        };
        let task = queued::answer_task(
            &send.store,
            chat,
            &send.question,
            input,
            resource,
            model.as_deref(),
        );
        services
            .sched
            .enqueue(task)
            .map_err(|error| format!("{error:#}"))?;
        let needs_title = transcript
            .chats()
            .map(|chats| {
                chats
                    .iter()
                    .find(|info| info.id == chat)
                    .is_some_and(|info| info.title.is_empty())
            })
            .unwrap_or(false);
        if needs_title {
            let model = settings
                .models
                .titles
                .as_ref()
                .map(|source| source.model.clone());
            let title = queued::title_task(
                &send.store,
                chat,
                &send.question,
                &send.question,
                model.as_deref(),
            );
            services
                .sched
                .enqueue(title)
                .map_err(|error| format!("{error:#}"))?;
        }
        Ok(chat)
    }

    pub(super) fn agent_services(
        &mut self,
    ) -> Result<crate::features::agent::host::AgentServices, String> {
        self.ensure_agent_host();
        let ai_dir = mediaa_core::catalogs::ai_dir(&self.settings.media_dir);
        let settings = self.settings.agent.clone();
        let language = crate::i18n::current();
        if let Some(host) = self.agent_host.as_mut() {
            host.tick(&ai_dir, &settings, &language, true);
        }
        self.agent_host
            .as_ref()
            .and_then(|host| host.services())
            .ok_or_else(|| "agent is not running".to_string())
    }

    pub(super) fn agent_op(
        &mut self,
        request: mediaa_protocol::backend::AgentOpRequestWire,
    ) -> Result<(), String> {
        use mediaa_protocol::backend::AgentOpWire;
        let services = self.agent_services()?;
        let task = |request: &mediaa_protocol::backend::AgentOpRequestWire| {
            request
                .task
                .ok_or_else(|| "agent task is missing".to_string())
        };
        match request.op {
            AgentOpWire::Pause => services
                .sched
                .pause(task(&request)?)
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
            AgentOpWire::Resume => services
                .sched
                .resume(task(&request)?)
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
            AgentOpWire::Cancel => services
                .sched
                .cancel(task(&request)?)
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
            AgentOpWire::Restart => services
                .sched
                .restart(task(&request)?)
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
            AgentOpWire::Remove => services
                .sched
                .remove(task(&request)?)
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
            AgentOpWire::ClearDone => services
                .sched
                .clear_done()
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
        }?;
        self.last_agent_snapshot.clear();
        Ok(())
    }

    pub(super) fn agent_task_details(&mut self, id: i64) -> Result<Vec<u8>, String> {
        let services = self.agent_services()?;
        let task = services
            .sched
            .store()
            .get(id)
            .map_err(|error| format!("{error:#}"))?
            .ok_or_else(|| "agent task is missing".to_string())?;
        Ok(mediaa_protocol::backend::encode_agent_task_details(
            &crate::features::agent::snapshot::details(&task),
        ))
    }

    pub(super) fn agent_stop(&mut self, id: i64) -> Result<Vec<u8>, String> {
        use mediaa_core::assistant::Event;
        use mediaa_core::chat::Author;
        let services = self.agent_services()?;
        let task = services
            .sched
            .store()
            .get(id)
            .map_err(|error| format!("{error:#}"))?
            .ok_or_else(|| "agent task is missing".to_string())?;
        let store = task
            .input
            .get("store")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        let chat = task.input.get("chat").and_then(|value| value.as_i64());
        if let Some(mut live) = services.streams.get(id) {
            live.apply(&Event::Stopped);
            live.close();
            let mut details = live.details(false, true);
            details["task"] = serde_json::json!(id);
            if let (Some(store), Some(chat)) = (store, chat) {
                let transcript =
                    mediaa_core::transcript::Transcript::open(&services.ai_dir, &store)
                        .map_err(|error| format!("{error:#}"))?;
                transcript
                    .add_message(chat, Author::Agent, live.timeline.draft(), &details)
                    .map_err(|error| format!("{error:#}"))?;
            }
            services.streams.apply_event(id, Event::Stopped);
        }
        services
            .sched
            .cancel(id)
            .map_err(|error| format!("{error:#}"))?;
        Ok(Vec::new())
    }

    pub(super) fn agent_steer(
        &mut self,
        steer: mediaa_protocol::backend::AgentSteerWire,
    ) -> Result<(), String> {
        use mediaa_core::chat::Author;
        let services = self.agent_services()?;
        let task = services
            .sched
            .store()
            .get(steer.task)
            .map_err(|error| format!("{error:#}"))?
            .ok_or_else(|| "agent task is missing".to_string())?;
        let store = task
            .input
            .get("store")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "agent store is missing".to_string())?;
        let chat = task
            .input
            .get("chat")
            .and_then(|value| value.as_i64())
            .ok_or_else(|| "agent chat is missing".to_string())?;
        let transcript = mediaa_core::transcript::Transcript::open(&services.ai_dir, store)
            .map_err(|error| format!("{error:#}"))?;
        let details = serde_json::json!({ "steer": { "task": steer.task } });
        transcript
            .add_message(chat, Author::User, &steer.text, &details)
            .map_err(|error| format!("{error:#}"))?;
        Ok(())
    }

    pub(super) fn poll_agent_requests(&mut self) {
        for response in self.agent_requests.poll() {
            self.push_event(
                mediaa_protocol::backend::AGENT_REQUEST_EVENT,
                mediaa_protocol::backend::encode_agent_response(&response),
            );
        }
    }

    pub(super) fn agent_analyse(
        &mut self,
        analyse: mediaa_protocol::backend::AgentAnalyseWire,
    ) -> Result<Vec<u8>, String> {
        use mediaa_core::analysis::Stage;
        let services = self.agent_services()?;
        let ai_dir = mediaa_core::catalogs::ai_dir(&self.settings.media_dir);
        let path = std::path::Path::new(&analyse.path);
        let from = analyse
            .from
            .as_deref()
            .map(|stage| {
                Stage::parse(stage).ok_or_else(|| format!("unknown analysis stage: {stage}"))
            })
            .transpose()?;
        match from {
            Some(from) => mediaa_agent::pipeline::restart(
                &services.sched,
                &ai_dir,
                path,
                from,
                analyse.options,
            ),
            None => mediaa_agent::pipeline::start(&services.sched, &ai_dir, path, analyse.options),
        }
        .map(|_| Vec::new())
        .map_err(|error| format!("{error:#}"))
    }

    pub(super) fn agent_names(
        &mut self,
        names: mediaa_protocol::backend::AgentNamesWire,
    ) -> Result<Vec<u8>, String> {
        let services = self.agent_services()?;
        let ai_dir = mediaa_core::catalogs::ai_dir(&self.settings.media_dir);
        let settings = services.settings.read().expect("agent settings").clone();
        mediaa_agent::names::enqueue(
            &services.sched,
            &ai_dir,
            std::path::Path::new(&names.path),
            &settings,
        )
        .map(|_| Vec::new())
        .map_err(|error| format!("{error:#}"))
    }

    pub(super) fn ensure_agent_host(&mut self) {
        if self.agent_host.is_some() {
            return;
        }
        let connections = Arc::new(mediaa_engine::Connections::new(
            self.settings.agent.connections.clone(),
        ));
        let slots = mediaa_core::slots::Slots::new();
        self.agent_host = Some(AgentHost::with_connections(connections, slots));
        log::info!("agent host: started in the backend");
    }

    pub(super) fn poll_agent_host(&mut self) {
        self.ensure_agent_host();
        let ai_dir = mediaa_core::catalogs::ai_dir(&self.settings.media_dir);
        let language = crate::i18n::current();
        if let Some(host) = self.agent_host.as_mut() {
            host.tick(&ai_dir, &self.settings.agent, &language, true);
        }
        let snapshot = self
            .agent_host
            .as_ref()
            .and_then(|host| host.services())
            .and_then(|services| services.sched.snapshot().ok());
        if let Some(snapshot) = snapshot {
            let marker = format!(
                "{:?}",
                snapshot
                    .tasks
                    .iter()
                    .map(|view| (&view.task.id, &view.task.status))
                    .collect::<Vec<_>>()
            );
            if marker != self.last_agent_snapshot {
                self.last_agent_snapshot = marker;
                let services = self.agent_host.as_ref().and_then(|host| host.services());
                let kind_stats = services
                    .as_ref()
                    .and_then(|services| services.sched.store().kind_stats().ok())
                    .unwrap_or_default();
                let slots = services
                    .as_ref()
                    .map(|services| services.slots.status())
                    .unwrap_or_default();
                self.agent_snapshot_version += 1;
                let snapshot = crate::features::agent::snapshot::snapshot(
                    &snapshot,
                    &kind_stats,
                    &slots,
                    self.agent_snapshot_version,
                );
                self.push_event(
                    mediaa_protocol::backend::AGENT_SNAPSHOT,
                    mediaa_protocol::backend::encode_agent_snapshot(&snapshot),
                );
            }
            let marker = snapshot
                .running
                .iter()
                .map(|run| format!("{}:{:.2}", run.title, run.progress))
                .collect::<Vec<_>>()
                .join("|");
            if marker != self.last_agent_live {
                self.last_agent_live = marker;
                let state = mediaa_protocol::backend::AgentStateWire {
                    running: !snapshot.running.is_empty(),
                    task: snapshot.running.first().map(|run| run.title.clone()),
                    status: snapshot.running.first().map(|run| run.kind.clone()),
                    progress: snapshot
                        .running
                        .first()
                        .map(|run| (run.progress * 100.0).round() as u32),
                };
                self.push_event(
                    mediaa_protocol::backend::AGENT_STATE,
                    mediaa_protocol::backend::encode_agent_state(&state),
                );
            }
            if let Some(services) = self.agent_host.as_ref().and_then(|host| host.services()) {
                let (next, events, gap) = services.streams.events_since(self.agent_event_cursor);
                if gap {
                    log::warn!("agent live events gap; resetting the cursor");
                }
                self.agent_event_cursor = next;
                for (task, event) in events {
                    let event = mediaa_protocol::backend::AgentEventWire { task, event };
                    self.push_event(
                        mediaa_protocol::backend::AGENT_EVENT,
                        mediaa_protocol::backend::encode_agent_event(&event),
                    );
                }
            }
        }
    }
}
