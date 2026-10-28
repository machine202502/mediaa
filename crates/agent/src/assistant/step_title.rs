use crate::prompts;
use crate::scope::Scope;
use mediaa_core::agent::{AgentSettings, ModelRole};
use mediaa_core::chat::{Timeline, TimelineEntry};
use mediaa_core::transcript::Transcript;
use mediaa_engine::llm::split_thinking;
use mediaa_engine::{ChatMessage, ChatRequest, ChatRole, Priority};
use mediaa_queue::{Resource, Status, Task};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use super::Session;

pub const KIND: &str = "step-title";
const PHRASE_TOKENS: u32 = 128;
const PHRASE_CONTEXT: u32 = 2048;

pub fn caption(tool: &str, topic: bool, english: bool) -> String {
    let key = if tool == "outline" && topic {
        "topic"
    } else {
        tool
    };
    let (ru, en) = match key {
        "outline" => ("Карта тем", "Topic map"),
        "topic" => ("Разбор темы", "Topic breakdown"),
        "search" => ("Поиск", "Search"),
        "read" => ("Чтение записи", "Reading the recording"),
        "mentions" => ("Упоминания", "Mentions"),
        "speakers" => ("Говорящие", "Speakers"),
        "calculator" => ("Калькулятор", "Calculator"),
        "time" => ("Время", "Time"),
        "scan" => ("Просмотр записи", "Looking through the recording"),
        "rename_speakers" => ("Имена говорящих", "Speaker names"),
        "remember" => ("Память", "Memory"),
        "plan" => ("План", "Plan"),
        "say" => ("Ответ", "Answer"),
        "web_search" => ("Поиск в интернете", "Web search"),
        "web_read" => ("Чтение страницы", "Reading a page"),
        "recordings" => ("Записи", "Recordings"),
        _ => ("Шаг", "Step"),
    };
    if english { en } else { ru }.to_string()
}

pub fn is_topic(args: &Value) -> bool {
    args.get("topic").and_then(Value::as_i64).is_some()
}

pub fn clean_phrase(raw: &str) -> Option<String> {
    let text = split_thinking(raw).1;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let quotes = ['"', '\'', '«', '»', '“', '”', '„', '`', '*'];
    let junk = |c: char| quotes.contains(&c) || c.is_whitespace();
    let line = line
        .trim_start_matches(junk)
        .trim_end_matches(|c: char| junk(c) || c == '.');
    if line.is_empty() {
        None
    } else {
        Some(line.to_string())
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "explicit parameters of one pipeline call"
)]
fn ask(
    scope: &Scope,
    settings: &AgentSettings,
    question: &str,
    tool: &str,
    purpose: &str,
    arguments: &str,
    language: &str,
    cancel: &AtomicBool,
) -> anyhow::Result<Option<String>> {
    let Some(source) = settings.models.get(ModelRole::Titles).as_ref() else {
        return Ok(None);
    };
    let model = crate::models::local_spec(source)?;
    let request = ChatRequest {
        model,
        messages: vec![
            ChatMessage::system(prompts::render("chat/step_title_system.j2", ())),
            ChatMessage::new(
                ChatRole::User,
                prompts::render(
                    "chat/step_title.j2",
                    minijinja::context! { question, tool, purpose, arguments, language },
                ),
            ),
        ],
        max_tokens: PHRASE_TOKENS,
        temperature: 0.0,
        n_ctx: Some(PHRASE_CONTEXT),
        device: Some(settings.devices.get(ModelRole::Titles)),
        priority: Some(Priority::Interactive),
        hf_token: super::hf_token(settings),
        enable_thinking: false,
        ..ChatRequest::default()
    };
    let answer = scope.engine().chat(&request, cancel, &mut |_| true)?;
    Ok(clean_phrase(&answer.content))
}

pub fn work(
    scope: &Scope,
    settings: &AgentSettings,
    task: &Task,
    cancel: &AtomicBool,
) -> Option<String> {
    let question = task
        .input
        .get("question")
        .and_then(Value::as_str)
        .unwrap_or("");
    let tool = task.input.get("tool").and_then(Value::as_str).unwrap_or("");
    let purpose = task
        .input
        .get("purpose")
        .and_then(Value::as_str)
        .unwrap_or("");
    let arguments = task
        .input
        .get("arguments")
        .and_then(Value::as_str)
        .unwrap_or("{}");
    let language = task
        .input
        .get("language")
        .and_then(Value::as_str)
        .unwrap_or("Russian");
    if cancel.load(Ordering::Relaxed) {
        scope.cancel();
        return None;
    }
    match ask(
        scope, settings, question, tool, purpose, arguments, language, cancel,
    ) {
        Ok(Some(phrase)) => {
            scope.finish(&json!({ "phrase": phrase }));
            Some(phrase)
        }
        Ok(None) => {
            scope.fail("the model wrote no phrase");
            None
        }
        Err(e) => {
            scope.fail(&format!("{e:#}"));
            None
        }
    }
}

pub fn overlay(tasks: &[Task], answer: i64, session: &mut Session) {
    let mut timeline = Timeline::from_details(&json!({ "timeline": session.timeline }));
    let mut changed = false;
    for task in tasks {
        if task.kind != KIND || task.status != Status::Done {
            continue;
        }
        if task.input.get("answer").and_then(Value::as_i64) != Some(answer) {
            continue;
        }
        let Some(phrase) = task
            .result
            .as_ref()
            .and_then(|r| r.get("phrase"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(index) = task.input.get("index").and_then(Value::as_u64) else {
            continue;
        };
        let index = index as usize;
        if let Some(step) = session.steps.get_mut(index) {
            if step.title != phrase {
                step.title = phrase.to_string();
                changed = true;
            }
        }
        if let Some(title) = nth_step(&mut timeline.entries, index) {
            if title != phrase {
                *title = phrase.to_string();
                changed = true;
            }
        }
    }
    if changed {
        session.timeline = timeline.to_value();
    }
}

pub fn save_phrase(ai: &Path, task: &Task, phrase: &str) {
    let Some(store) = task
        .input
        .get("store")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
    else {
        return;
    };
    let Some(chat) = task.input.get("chat").and_then(Value::as_i64) else {
        return;
    };
    let Some(answer) = task.input.get("answer").and_then(Value::as_i64) else {
        return;
    };
    let Some(index) = task.input.get("index").and_then(Value::as_u64) else {
        return;
    };
    let saved = Transcript::open(ai, store)
        .and_then(|t| t.set_step_phrase(chat, answer, index as usize, phrase));
    if let Err(e) = saved {
        log::warn!("step phrase was not saved: {e:#}");
    }
}

pub fn phrases(tasks: &[Task], answer: i64) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for task in tasks {
        if task.kind != KIND || task.status != Status::Done {
            continue;
        }
        if task.input.get("answer").and_then(Value::as_i64) != Some(answer) {
            continue;
        }
        let Some(phrase) = task
            .result
            .as_ref()
            .and_then(|r| r.get("phrase"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(index) = task.input.get("index").and_then(Value::as_u64) else {
            continue;
        };
        out.push((index as usize, phrase.to_string()));
    }
    out
}

fn nth_step(entries: &mut [TimelineEntry], index: usize) -> Option<&mut String> {
    entries
        .iter_mut()
        .filter_map(|e| {
            if let TimelineEntry::Step { title, .. } = e {
                Some(title)
            } else {
                None
            }
        })
        .nth(index)
}

pub fn enqueue(
    scope: &Scope,
    session: &Session,
    index: usize,
    question: &str,
    tool: &str,
    purpose: &str,
    args: &Value,
) {
    if !session.titles {
        return;
    }
    let language = if session.ui_english {
        "English"
    } else {
        "Russian"
    };
    let arguments = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_string());
    let title = caption(tool, is_topic(args), session.ui_english);
    let input = json!({
        "answer": scope.task(),
        "index": index,
        "question": question,
        "tool": tool,
        "purpose": purpose,
        "arguments": arguments,
        "language": language,
        "store": session.store,
        "chat": session.chat,
    });
    let mut task = mediaa_queue::NewTask::new(KIND, &title, &input)
        .resource(Resource::Llm)
        .priority(1);
    if !session.store.is_empty() {
        task = task
            .scope(&session.store)
            .chat(&super::queued::chat_key(&session.store, session.chat));
    }
    if let Err(e) = scope.store().enqueue(task) {
        log::warn!("step title was not queued: {e:#}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_engine::testing::FakeEngine;
    use std::path::Path;
    use std::sync::Arc;

    #[test]
    fn a_phrase_is_cleaned_without_being_rewritten() {
        assert_eq!(
            clean_phrase("  «Ищу бюджет».\nлишнее").as_deref(),
            Some("Ищу бюджет")
        );
        assert_eq!(clean_phrase("смотрю @s1.").as_deref(), Some("смотрю @s1"));
        assert_eq!(clean_phrase("<think>\nхм\n</think>\n\n  "), None);
        assert_eq!(clean_phrase("\"\""), None);
    }

    #[test]
    fn captions_are_words_and_never_the_tool_name() {
        for (tool, topic) in [
            ("search", false),
            ("outline", false),
            ("outline", true),
            ("read", false),
            ("say", false),
            ("time", false),
        ] {
            for english in [false, true] {
                let text = caption(tool, topic, english);
                assert_ne!(text, tool, "{tool} {english}");
                assert!(!text.split_whitespace().any(|w| w == tool), "{text}");
            }
        }
        assert_eq!(caption("outline", true, false), "Разбор темы");
        assert_eq!(caption("outline", false, false), "Карта тем");
        assert_eq!(caption("search", false, false), "Поиск");
        assert_eq!(caption("read", false, false), "Чтение записи");
    }

    fn open_task(ai: &Path, engine: &FakeEngine) -> (Scope, Task) {
        let input = json!({
            "answer": 1,
            "index": 0,
            "question": "Когда отчёт?",
            "tool": "search",
            "purpose": "Finds places.",
            "arguments": "{\"query\":\"отчёт\"}",
            "language": "Russian",
        });
        let scope = Scope::begin(
            ai,
            Arc::new(engine.clone()),
            KIND,
            "Поиск",
            &input,
            Some("rec"),
        )
        .unwrap();
        let task = scope.store().get(scope.task()).unwrap().unwrap();
        (scope, task)
    }

    #[test]
    fn the_titles_model_writes_the_phrase_without_thinking() {
        let dir = tempfile::tempdir().unwrap();
        let engine = FakeEngine::new();
        engine.chat_answers(&["«Ищу срок отчёта»."]);
        let (scope, task) = open_task(dir.path(), &engine);
        let phrase = work(
            &scope,
            &AgentSettings::default(),
            &task,
            &AtomicBool::new(false),
        );
        assert_eq!(phrase.as_deref(), Some("Ищу срок отчёта"));
        let stored = scope.store().get(scope.task()).unwrap().unwrap();
        assert_eq!(stored.status, Status::Done);
        assert_eq!(stored.result.as_ref().unwrap()["phrase"], "Ищу срок отчёта");
        let chats = engine.chats();
        assert_eq!(chats.len(), 1);
        assert!(!chats[0].enable_thinking, "the titles model does not think");
        assert_eq!(chats[0].temperature, 0.0);
        assert!(
            chats[0].texts[1].contains("Когда отчёт?") && chats[0].texts[1].contains("Russian"),
            "{:?}",
            chats[0].texts
        );
    }

    #[test]
    fn a_titles_model_that_fails_leaves_no_phrase() {
        let dir = tempfile::tempdir().unwrap();
        let engine = FakeEngine::new();
        engine.chat_answers(&["   "]);
        let (scope, task) = open_task(dir.path(), &engine);
        assert_eq!(
            work(
                &scope,
                &AgentSettings::default(),
                &task,
                &AtomicBool::new(false)
            ),
            None
        );
        let stored = scope.store().get(scope.task()).unwrap().unwrap();
        assert_eq!(stored.status, Status::Failed);
        assert!(stored.result.is_none(), "the label is not replaced");

        let mut settings = AgentSettings::default();
        settings.models.titles = None;
        let engine = FakeEngine::new();
        let (scope, task) = open_task(dir.path(), &engine);
        assert_eq!(
            work(&scope, &settings, &task, &AtomicBool::new(false)),
            None
        );
        assert!(engine.chats().is_empty(), "no model was asked");
        assert_eq!(
            scope.store().get(scope.task()).unwrap().unwrap().status,
            Status::Failed
        );
    }
}
