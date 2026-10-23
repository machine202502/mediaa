//! Chat answers and chat titles as queue tasks. One chunk of an answer is one agent step.
//! Tokens go through [`crate::run::Streams`], keyed by the task, and are not stored per token.

use super::chat::Live;
use super::folder::Item;
use super::title::{self, task_title};
use super::tools::Recording;
use super::{answer_step, answer_step_folder, Advance, Event, Session, Turn};
use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use mediaa_core::agent::ModelRole;
use mediaa_core::chat::{Author, ChatId, ChatMessage, Timeline};
use mediaa_core::transcript::Transcript;
use mediaa_queue::{Resource, Status, Task};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const ANSWER_KIND: &str = "chat-answer";
pub const TITLE_KIND: &str = "chat-title";

/// A crash while a step was running: recover bumps gen, then the next claim bumps it again,
/// so the claim is three above the gen stored with the last finished step. A pause does not
/// bump gen, so resume is only two above and does not count.
///
/// A crash between chunks leaves the task queued, so gen is not bumped. That one is told by
/// the cursor (it already has steps) together with `fresh`: this process has no live stream
/// for the task yet. A yield in the same process still has the stream, so it does not count.
fn show_restart(noticed: bool, had_cursor: bool, gen: i64, gen_at_save: i64, fresh: bool, steps: usize) -> bool {
    if noticed {
        return false;
    }
    if had_cursor && gen >= gen_at_save.saturating_add(3) {
        return true;
    }
    if !had_cursor && gen >= 3 {
        return true;
    }
    fresh && had_cursor && steps > 0
}

fn history_of(messages: &[ChatMessage]) -> Vec<Turn> {
    let mut turns: Vec<Turn> = Vec::new();
    let mut question: Option<&str> = None;
    for m in messages {
        match m.author {
            // A correction is not a question of the conversation: it is taken while an answer runs.
            Author::User if is_steer(m) => {}
            Author::User => question = Some(&m.text),
            Author::Agent => match (question.take(), turns.last_mut()) {
                (Some(q), _) => turns.push(Turn { question: q.to_string(), answer: m.text.clone() }),
                (None, Some(t)) => {
                    t.answer.push_str("\n\n");
                    t.answer.push_str(&m.text);
                }
                (None, None) => {}
            },
        }
    }
    turns
}

/// Whether the message is a correction the user sent for the answer running at that moment.
fn is_steer(m: &ChatMessage) -> bool {
    m.details.get("steer").and_then(|s| s.get("task")).and_then(Value::as_i64).is_some()
}

/// The corrections of the answer `answer` that it has not taken yet, in the order the user sent
/// them: the index of each message in `messages` and its text.
pub fn pending_steers(messages: &[ChatMessage], answer: i64, taken: usize) -> Vec<(usize, String)> {
    messages
        .iter()
        .enumerate()
        .filter(|(_, m)| m.author == Author::User && m.details["steer"]["task"].as_i64() == Some(answer))
        .skip(taken)
        .map(|(index, m)| (index, m.text.clone()))
        .collect()
}

fn recording_of(ai: &Path, id: &str, path: &Path, timing: Option<(f64, f64)>) -> anyhow::Result<Recording> {
    let t = Transcript::open(ai, id)?;
    let replies = t.dialog()?;
    let duration = timing.map_or_else(|| replies.last().map_or(0.0, |r| r.end), |(d, _)| d);
    Ok(Recording {
        title: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(),
        duration,
        diarized: replies.iter().any(|r| r.speaker.is_some()),
        names: t.speaker_names()?,
        replies,
        chapters: t.chapters()?,
        position: timing.map(|(_, p)| p),
    })
}

fn items_of(ai: &Path, files: &[PathBuf]) -> Vec<Item> {
    files
        .iter()
        .filter_map(|path| {
            let id = mediaa_core::transcript::file_id(path).ok()?;
            let recording = if Transcript::exists(ai, &id) {
                recording_of(ai, &id, path, None).ok()?
            } else {
                Recording { title: path.file_stem().unwrap_or_default().to_string_lossy().into_owned(), ..Recording::default() }
            };
            Some(Item { recording, file_id: id, path: path.clone() })
        })
        .collect()
}

fn chat_id(task: &Task) -> Option<ChatId> {
    task.input.get("chat").and_then(Value::as_i64)
}

/// The answer task. The cursor is a [`Session`].
pub struct AnswerStage;

impl Stage for AnswerStage {
    fn handles(&self, kind: &str) -> bool {
        kind == ANSWER_KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let store_key = task.input.get("store").and_then(Value::as_str).unwrap_or("").to_string();
        let question = task.input.get("question").and_then(Value::as_str).unwrap_or("").to_string();
        let chat = chat_id(task).ok_or_else(|| anyhow::anyhow!("chat-answer has no chat"))?;
        let had_cursor = !task.cursor.is_null();
        let mut session: Session = if had_cursor { serde_json::from_value(task.cursor.clone()).unwrap_or_else(|_| Session::new(Default::default())) } else { Session::new(Default::default()) };
        let settings = (host.settings)();
        session.titles = settings.models.get(ModelRole::Titles).is_some();
        session.ui_english = task.input.get("locale").and_then(Value::as_str) == Some("en");
        session.store = store_key.clone();
        session.chat = chat;
        let fresh = host.streams.get(task.id).is_none();
        if show_restart(session.noticed, had_cursor, task.gen, session.gen_at_save, fresh, session.steps.len()) {
            session.noticed = true;
        }
        if let Ok(tasks) = host.store.list() {
            super::step_title::overlay(&tasks, task.id, &mut session);
        }
        // What was shown is the saved timeline. Tokens of a step that did not finish are dropped.
        let mut live = Live::new();
        live.timeline = Timeline::from_details(&json!({ "timeline": session.timeline }));
        live.steps = session.steps.clone();
        live.steers_taken = session.steers_taken;
        live.continuing = session.noticed;
        host.streams.insert(task.id, live);
        let scope = Scope::attach(host.store.clone(), task.id, host.engine.clone(), Some(store_key.clone()));
        let messages = Transcript::open(&host.ai_dir, &store_key).and_then(|t| t.messages(chat)).unwrap_or_default();
        // What the user sent while the previous step ran: it joins the context after that step.
        let steers: Vec<String> = pending_steers(&messages, task.id, session.steers_taken).into_iter().map(|(_, text)| text).collect();
        let history = history_of(&messages);
        let streams = host.streams.clone();
        let ai = host.ai_dir.clone();
        let id = task.id;
        let key = store_key.clone();
        let mut emit = move |event: &Event| {
            if let Event::Say(text) = event {
                if let Ok(t) = Transcript::open(&ai, &key) {
                    if let Err(e) = t.add_message(chat, Author::Agent, text, &json!({ "section": true })) {
                        log::warn!("a section of the answer was not stored: {e:#}");
                    }
                }
            }
            streams.update(id, |live| {
                live.apply(event);
            });
        };
        let stepped = if let Some(files) = task.input.get("files").and_then(Value::as_array) {
            let files: Vec<PathBuf> = files.iter().filter_map(|v| v.as_str().map(PathBuf::from)).collect();
            let items = items_of(&host.ai_dir, &files);
            answer_step_folder(&scope, &host.slots, &settings, &host.ai_dir, &items, &history, &mut session, &question, &steers, cancel, &mut emit)
        } else {
            let path = PathBuf::from(task.input.get("path").and_then(Value::as_str).unwrap_or(""));
            let file_id = task.input.get("file_id").and_then(Value::as_str).unwrap_or(&store_key).to_string();
            let duration = task.input.get("duration").and_then(Value::as_f64);
            let position = task.input.get("position").and_then(Value::as_f64);
            let timing = duration.map(|d| (d, position.unwrap_or(0.0)));
            let recording = recording_of(&host.ai_dir, &file_id, &path, timing)?;
            answer_step(&scope, &host.slots, &settings, &host.ai_dir, &file_id, &recording, &history, &mut session, &question, &steers, cancel, &mut emit)
        };
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        match stepped {
            Err(e) if e.to_string().contains("cancelled") => Ok(Work::Stopped),
            Err(e) => {
                save_memory(&host.ai_dir, &store_key, chat, &session.memory);
                host.streams.update(task.id, |live| live.apply(&Event::Failed(format!("{e:#}"))));
                Err(e)
            }
            Ok(Advance::More) => {
                if let Some(live) = host.streams.get(task.id) {
                    session.timeline = live.timeline.to_value();
                    session.steps = live.steps;
                    session.steers_taken = live.steers_taken;
                }
                session.gen_at_save = task.gen;
                let progress = (session.n as f32 / (super::MAX_STEPS as f32 + 4.0)).clamp(0.0, 0.99);
                Ok(Work::More { cursor: serde_json::to_value(&session)?, partial: Value::Null, progress })
            }
            Ok(Advance::Done(answer)) => {
                save_memory(&host.ai_dir, &store_key, chat, &session.memory);
                host.streams.update(task.id, |live| {
                    live.timeline.finish_answer(&answer.text);
                    live.continuing = session.noticed;
                    live.apply(&Event::Done);
                });
                let live = host.streams.get(task.id).unwrap_or_default();
                let mut details = json!({ "steps": answer.steps, "forced": answer.forced, "timeline": live.timeline.to_value(), "task": task.id });
                if let Some(metrics) = live.metrics {
                    details["metrics"] = json!(metrics);
                }
                if session.noticed {
                    details["continuing"] = json!(true);
                }
                if let Ok(t) = Transcript::open(&host.ai_dir, &store_key) {
                    t.add_message(chat, Author::Agent, &answer.text, &details)?;
                }
                // A correction that came after the last step becomes the chat's next question.
                if let Err(e) = ask_pending(host, task, chat, &store_key, session.steers_taken) {
                    log::warn!("the next question after the answer was not started: {e:#}");
                }
                Ok(Work::Done(json!({ "chars": answer.text.chars().count(), "steps": answer.steps.len() })))
            }
        }
    }
}

/// The first correction the answer did not take becomes the chat's next question, asked at once.
/// The corrections after it become corrections of that next answer, in order.
fn ask_pending(host: &Host, task: &Task, chat: ChatId, store: &str, taken: usize) -> anyhow::Result<()> {
    let t = Transcript::open(&host.ai_dir, store)?;
    let messages = t.messages(chat)?;
    let pending = pending_steers(&messages, task.id, taken);
    let Some((_, question)) = pending.first() else { return Ok(()) };
    let mut input = task.input.clone();
    input["question"] = json!(question);
    let resource = task.resource.unwrap_or(Resource::Llm);
    let id = host.store.enqueue(answer_task(store, chat, question, input, resource, task.model.as_deref()))?;
    // It is a question of the chat now, not a correction of the old answer.
    t.unsteer(chat, task.id, taken)?;
    if pending.len() > 1 {
        t.retag_steers(chat, task.id, id, taken)?;
    }    Ok(())
}

fn save_memory(ai: &Path, store: &str, chat: ChatId, memory: &super::Memory) {
    let saved = serde_json::to_string(memory).map_err(anyhow::Error::from).and_then(|m| Transcript::open(ai, store)?.set_chat_memory(chat, &m));
    if let Err(e) = saved {
        log::warn!("chat memory: {e:#}");
    }
}

/// The titles model names a chat. One chunk, the same fairness key as the chat's answers.
pub struct TitleStage;

impl Stage for TitleStage {
    fn handles(&self, kind: &str) -> bool {
        kind == TITLE_KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let store_key = task.input.get("store").and_then(Value::as_str).unwrap_or("").to_string();
        let question = task.input.get("question").and_then(Value::as_str).unwrap_or("").to_string();
        let Some(chat) = chat_id(task) else {
            anyhow::bail!("chat-title has no chat");
        };
        let scope = Scope::attach(host.store.clone(), task.id, host.engine.clone(), Some(store_key.clone()));
        let settings = (host.settings)();
        match title::work(&scope, &settings, &host.ai_dir, &store_key, chat, &question, cancel) {
            None => Ok(Work::Stopped),
            Some(title) => Ok(Work::Done(json!({ "title": title }))),
        }
    }
}

/// One phrase for a tool step. [`super::step_title::work`] finishes or fails the task itself.
pub struct StepTitleStage;

impl Stage for StepTitleStage {
    fn handles(&self, kind: &str) -> bool {
        kind == super::step_title::KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let key = task.input.get("store").and_then(Value::as_str).filter(|s| !s.is_empty()).map(str::to_string);
        let scope = Scope::attach(host.store.clone(), task.id, host.engine.clone(), key);
        let settings = (host.settings)();
        match super::step_title::work(&scope, &settings, task, cancel) {
            Some(phrase) => {
                // The answer row may already be stored (the phrase finished after the answer).
                // The live timeline is updated too, so a stop before the next chunk stores it.
                super::step_title::save_phrase(&host.ai_dir, task, &phrase);
                if let (Some(answer), Some(index)) = (task.input.get("answer").and_then(Value::as_i64), task.input.get("index").and_then(Value::as_u64)) {
                    let index = index as usize;
                    host.streams.update(answer, |live| {
                        live.timeline.set_step_title(index, &phrase);
                        if let Some(step) = live.steps.get_mut(index) {
                            step.title = phrase.clone();
                        }
                    });
                }
                Ok(Work::Done(json!({ "phrase": phrase })))
            }
            None => Ok(Work::Stopped),
        }
    }
}

pub fn stages() -> Vec<Arc<dyn Stage>> {
    vec![Arc::new(AnswerStage), Arc::new(TitleStage), Arc::new(StepTitleStage)]
}

/// Fairness key of one chat: the store and the chat, so two chats do not share a turn.
pub fn chat_key(store: &str, chat: ChatId) -> String {
    format!("{store}:{chat}")
}

pub fn answer_task(store: &str, chat: ChatId, question: &str, input: Value, resource: mediaa_queue::Resource, model: Option<&str>) -> mediaa_queue::NewTask {
    let mut task = mediaa_queue::NewTask::new(ANSWER_KIND, &task_title(question), &input)
        .resource(resource)
        .scope(store)
        .chat(&chat_key(store, chat));
    if let Some(model) = model {
        task = task.model(model);
    }
    task
}

pub fn title_task(store: &str, chat: ChatId, question: &str, for_model: &str, model: Option<&str>) -> mediaa_queue::NewTask {
    let input = json!({ "chat": chat, "store": store, "question": for_model });
    let mut task = mediaa_queue::NewTask::new(TITLE_KIND, &task_title(question), &input)
        .resource(mediaa_queue::Resource::Llm)
        .scope(store)
        .chat(&chat_key(store, chat));
    if let Some(model) = model {
        task = task.model(model);
    }
    task
}

/// Unfinished answer of `chat` in `store`, the newest one.
pub fn running_answer<'a>(tasks: &'a [Task], store: &str, chat: ChatId) -> Option<&'a Task> {
    tasks
        .iter()
        .filter(|t| t.kind == ANSWER_KIND && t.scope.as_deref() == Some(store) && t.input.get("chat").and_then(Value::as_i64) == Some(chat) && !t.status.is_finished())
        .max_by_key(|t| t.id)
}

pub fn answer_busy(task: &Task) -> bool {
    matches!(task.status, Status::Running | Status::Stopping)
}

#[cfg(test)]
mod tests {
    use super::show_restart;

    #[test]
    fn a_restart_is_a_crash_or_a_queued_cursor_with_no_live_stream() {
        // Crash while running: recover bumps gen, the next claim bumps it again (+3).
        assert!(show_restart(false, true, 5, 2, false, 1));
        // The line was already shown.
        assert!(!show_restart(true, true, 5, 2, true, 1));
        // A yield in this process: gen is only +1 and the live stream is still here.
        assert!(!show_restart(false, true, 3, 2, false, 2));
        // A crash between chunks: the task stayed queued (gen +1) but this process has no stream.
        assert!(show_restart(false, true, 3, 2, true, 2));
        // A new answer has no steps yet, even with no stream.
        assert!(!show_restart(false, true, 3, 2, true, 0));
        assert!(!show_restart(false, false, 1, 0, true, 0));
        // A crash before the first step was saved.
        assert!(show_restart(false, false, 3, 0, true, 0));
    }

    #[test]
    fn a_finished_phrase_is_stored_on_the_answer() {
        use super::super::chat::Live;
        use super::super::{StepRecord, StepStatus};
        use crate::run::{open_store, Host, Stage, Streams, Work};
        use crate::slots::Slots;
        use mediaa_core::agent::AgentSettings;
        use mediaa_core::chat::{Author, Timeline, TimelineEntry};
        use mediaa_core::transcript::Transcript;
        use mediaa_engine::testing::FakeEngine;
        use mediaa_engine::Engine;
        use serde_json::json;
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let chat = {
            let t = Transcript::open(dir.path(), "rec").unwrap();
            let chat = t.create_chat("Сроки").unwrap();
            let mut tl = Timeline::default();
            tl.thinking("хм");
            tl.step("Поиск", "search", "found");
            t.add_message(chat, Author::Agent, "В пятницу", &json!({"task": 42, "steps": [{"title": "Поиск", "tool": "search"}], "timeline": tl.to_value()})).unwrap();
            chat
        };
        let fake = FakeEngine::new();
        fake.chat_answers(&["«Ищу срок отчёта»."]);
        let engine: Arc<dyn Engine> = Arc::new(fake);
        let store = open_store(dir.path()).unwrap();
        let input = json!({
            "answer": 42,
            "index": 0,
            "question": "Когда отчёт?",
            "tool": "search",
            "purpose": "Finds places.",
            "arguments": "{\"query\":\"отчёт\"}",
            "language": "Russian",
            "store": "rec",
            "chat": chat,
        });
        let id = store.begin_inline(super::super::step_title::KIND, "Поиск", &input, None, Some("rec")).unwrap();
        let task = store.get(id).unwrap().unwrap();
        let host = Host {
            ai_dir: dir.path().to_path_buf(),
            engine,
            store,
            slots: Slots::default(),
            settings: Arc::new(AgentSettings::default),
            streams: Arc::new(Streams::default()),
        };
        let mut live = Live::new();
        live.timeline.step("Поиск", "search", "found");
        live.steps.push(StepRecord { title: "Поиск".into(), tool: "search".into(), args: json!({}), result: String::new(), status: StepStatus::Found });
        host.streams.insert(42, live);

        match super::StepTitleStage.chunk(&task, &Arc::new(AtomicBool::new(false)), &host).unwrap() {
            Work::Done(value) => assert_eq!(value["phrase"], "Ищу срок отчёта"),
            other => panic!("the phrase was not finished: {other:?}"),
        }

        let saved = Transcript::open(dir.path(), "rec").unwrap().messages(chat).unwrap();
        assert_eq!(
            saved[0].timeline().entries,
            [
                TimelineEntry::Thinking { text: "хм".into() },
                TimelineEntry::Step { title: "Ищу срок отчёта".into(), tool: "search".into(), status: "found".into() },
            ],
            "an old chat reads the phrase from the stored timeline"
        );
        assert_eq!(saved[0].details["steps"][0]["title"], "Ищу срок отчёта");
        let live = host.streams.get(42).unwrap();
        assert_eq!(live.timeline.entries[0], TimelineEntry::Step { title: "Ищу срок отчёта".into(), tool: "search".into(), status: "found".into() });
    }

    #[test]
    fn a_correction_survives_a_crash_and_is_taken_once() {
        use mediaa_core::chat::Author;
        use mediaa_core::transcript::Transcript;
        use serde_json::json;

        let dir = tempfile::tempdir().unwrap();
        let chat = {
            let t = Transcript::open(dir.path(), "rec").unwrap();
            let chat = t.create_chat("Сроки").unwrap();
            t.add_message(chat, Author::User, "Когда отчёт?", &json!(null)).unwrap();
            t.add_message(chat, Author::Agent, "Ищу.", &json!({ "task": 5 })).unwrap();
            t.add_message(chat, Author::User, "Ещё про бюджет.", &json!({ "steer": { "task": 5 } })).unwrap();
            chat
        };
        // The cursor of the first step was saved before the correction came.
        let session = super::super::Session::new(Default::default());
        let cursor = serde_json::to_value(&session).unwrap();
        // A crash: another process reads the same store and the same cursor.
        let restored: super::super::Session = serde_json::from_value(cursor).unwrap();
        let messages = Transcript::open(dir.path(), "rec").unwrap().messages(chat).unwrap();
        assert_eq!(
            super::pending_steers(&messages, 5, restored.steers_taken),
            [(2, "Ещё про бюджет.".to_string())],
            "the correction is still there after the restart"
        );
        // The next chunk takes it and saves the count with the cursor...
        let mut taken = restored;
        taken.steers_taken += 1;
        let saved = serde_json::to_value(&taken).unwrap();
        // ...so a second restart does not take it again.
        let again: super::super::Session = serde_json::from_value(saved).unwrap();
        assert!(super::pending_steers(&messages, 5, again.steers_taken).is_empty());
    }

    #[test]
    fn a_correction_after_the_last_step_becomes_the_next_question() {
        use crate::run::{open_store, Host, Streams};
        use crate::slots::Slots;
        use mediaa_core::agent::AgentSettings;
        use mediaa_core::chat::Author;
        use mediaa_core::transcript::Transcript;
        use mediaa_engine::testing::FakeEngine;
        use mediaa_engine::Engine;
        use mediaa_queue::{Resource, Task};
        use serde_json::json;
        use std::sync::Arc;

        let dir = tempfile::tempdir().unwrap();
        let chat = {
            let t = Transcript::open(dir.path(), "rec").unwrap();
            let chat = t.create_chat("Сроки").unwrap();
            t.add_message(chat, Author::User, "Когда отчёт?", &json!(null)).unwrap();
            t.add_message(chat, Author::Agent, "В пятницу.", &json!({ "task": 7 })).unwrap();
            chat
        };
        let store = open_store(dir.path()).unwrap();
        let input = json!({ "chat": chat, "store": "rec", "question": "Когда отчёт?", "file_id": "rec", "path": "rec.wav" });
        let id = store.enqueue(super::answer_task("rec", chat, "Когда отчёт?", input, Resource::Llm, None)).unwrap();
        let host = Host {
            ai_dir: dir.path().to_path_buf(),
            engine: Arc::new(FakeEngine::new()) as Arc<dyn Engine>,
            store,
            slots: Slots::default(),
            settings: Arc::new(AgentSettings::default),
            streams: Arc::new(Streams::default()),
        };
        let task = host.store.get(id).unwrap().unwrap();
        super::ask_pending(&host, &task, chat, "rec", 0).unwrap();
        assert_eq!(host.store.list().unwrap().len(), 1, "nothing is pending yet");

        // The user sent two corrections while the answer ran; it finished before taking them.
        let t = Transcript::open(dir.path(), "rec").unwrap();
        t.add_message(chat, Author::User, "Ещё про бюджет.", &json!({ "steer": { "task": id } })).unwrap();
        t.add_message(chat, Author::User, "И про сроки.", &json!({ "steer": { "task": id } })).unwrap();
        super::ask_pending(&host, &task, chat, "rec", 0).unwrap();

        // The first one is asked as the chat's next question...
        let tasks = host.store.list().unwrap();
        let next: Vec<&Task> = tasks.iter().filter(|t| t.id != id).collect();
        assert_eq!(next.len(), 1);
        assert_eq!(next[0].kind, super::ANSWER_KIND);
        assert_eq!(next[0].input["question"], "Ещё про бюджет.");
        assert_eq!(next[0].input["chat"], chat);
        assert_eq!(next[0].input["file_id"], "rec", "the recording stays with the chat");
        assert_eq!(next[0].scope.as_deref(), Some("rec"));
        // ...the one after it becomes a correction of that next answer.
        let messages = Transcript::open(dir.path(), "rec").unwrap().messages(chat).unwrap();
        assert!(messages[2].details["steer"].is_null(), "the first correction is a question of the chat now");
        assert_eq!(messages[3].details["steer"]["task"], next[0].id);
        assert_eq!(super::pending_steers(&messages, next[0].id, 0), [(3, "И про сроки.".to_string())]);
        // A correction is never a question of the history: it is taken by the answer it belongs to.
        assert_eq!(super::history_of(&messages).len(), 1);
    }
}
