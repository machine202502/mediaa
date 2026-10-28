use crate::prompts;
use crate::scope::Scope;
use mediaa_core::agent::{AgentSettings, ModelRole};
use mediaa_core::chat::ChatId;
use mediaa_core::transcript::Transcript;
use mediaa_engine::llm::split_thinking;
use mediaa_engine::{ChatMessage, ChatRequest, ChatRole, Priority};
use serde_json::json;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

const FALLBACK_CHARS: usize = 48;
const TITLE_TOKENS: u32 = 128;
const TITLE_CONTEXT: u32 = 2048;

pub fn task_title(question: &str) -> String {
    let q = question.split_whitespace().collect::<Vec<_>>().join(" ");
    let head: String = q.chars().take(40).collect();
    format!("{head}{}", if q.chars().count() > 40 { "…" } else { "" })
}

pub fn fallback_title(question: &str) -> String {
    let q = question.split_whitespace().collect::<Vec<_>>().join(" ");
    let cut: String = q.chars().take(FALLBACK_CHARS).collect();
    let q = if q.chars().count() <= FALLBACK_CHARS {
        q
    } else {
        format!(
            "{}…",
            cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head)
        )
    };
    capitalised(&q)
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

pub fn clean_title(raw: &str) -> Option<String> {
    let text = split_thinking(raw).1;
    let line = text.lines().map(str::trim).find(|l| !l.is_empty())?;
    let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
    let quotes = ['"', '\'', '«', '»', '“', '”', '„', '`', '*', '#'];
    let junk = |c: char| quotes.contains(&c) || c.is_whitespace();
    let line = line
        .trim_start_matches(junk)
        .trim_end_matches(|c: char| junk(c) || c == '.');
    if line.is_empty() {
        return None;
    }
    Some(capitalised(line))
}

fn ask_model(
    scope: &Scope,
    settings: &AgentSettings,
    question: &str,
    cancel: &AtomicBool,
) -> anyhow::Result<(Option<String>, String)> {
    let source = settings
        .models
        .get(ModelRole::Titles)
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("no model for titles chosen (Agent → Models)"))?;
    let model = crate::models::local_spec(source)?;
    let language = settings.language_name();
    let request = ChatRequest {
        model,
        messages: vec![
            ChatMessage::system(prompts::render("chat/title_system.j2", ())),
            ChatMessage::new(
                ChatRole::User,
                prompts::render("chat/title.j2", minijinja::context! { language, question }),
            ),
        ],
        max_tokens: TITLE_TOKENS,
        temperature: 0.0,
        n_ctx: Some(TITLE_CONTEXT),
        device: Some(settings.devices.get(ModelRole::Titles)),
        priority: Some(Priority::Interactive),
        hf_token: super::hf_token(settings),
        enable_thinking: false,
        ..ChatRequest::default()
    };
    let answer = scope.engine().chat(&request, cancel, &mut |_| true)?;
    Ok((clean_title(&answer.content), answer.content))
}

pub fn work(
    scope: &Scope,
    settings: &AgentSettings,
    ai_dir: &Path,
    store: &str,
    chat: ChatId,
    question: &str,
    cancel: &AtomicBool,
) -> Option<String> {
    let written = ask_model(scope, settings, question, cancel);
    if cancel.load(Ordering::Relaxed) {
        scope.cancel();
        return None;
    }
    let (title, error) = match written {
        Ok((Some(title), _)) => (title, None),
        Ok((None, raw)) => (
            fallback_title(question),
            Some(format!("the model wrote no title: {raw:?}")),
        ),
        Err(e) => (fallback_title(question), Some(format!("{e:#}"))),
    };
    let stored = Transcript::open(ai_dir, store).and_then(|t| t.set_chat_title(chat, &title));
    match (stored, error) {
        (Err(e), _) => scope.fail(&format!("{e:#}")),
        (Ok(_), None) => scope.finish(&json!({ "title": title })),
        (Ok(_), Some(error)) => scope.fail(&format!("{error}; the title is the first message")),
    }
    Some(title)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_engine::testing::FakeEngine;
    use mediaa_queue::Status as TaskStatus;
    use std::sync::Arc;

    #[test]
    fn titles_are_cleaned_and_the_fallback_is_the_first_message() {
        assert_eq!(
            clean_title("  «Сроки отчёта».\nлишнее").as_deref(),
            Some("Сроки отчёта")
        );
        assert_eq!(
            clean_title("<think>\nподумаю\n</think>\n\"приветствие\"").as_deref(),
            Some("Приветствие")
        );
        assert_eq!(clean_title("**Бюджет**").as_deref(), Some("Бюджет"));
        assert_eq!(clean_title(" \n "), None);
        assert_eq!(clean_title("«»"), None);
        let long = clean_title(&"слово ".repeat(30)).unwrap();
        assert_eq!(
            long.split_whitespace().count(),
            30,
            "a long title is not cut: {long}"
        );
        assert!(long.starts_with("Слово"), "{long}");

        assert_eq!(fallback_title("  когда   отчёт? "), "Когда отчёт?");
        let long = fallback_title(
            "расскажи подробно, что решили про бюджет на следующий квартал и кто отвечает",
        );
        assert!(
            long.starts_with("Расскажи")
                && long.ends_with('…')
                && long.chars().count() <= FALLBACK_CHARS + 1,
            "{long}"
        );
        assert_eq!(task_title("Привет"), "Привет");
    }

    fn start(ai: &Path, engine: &FakeEngine, question: &str) -> (Scope, ChatId) {
        let t = Transcript::open(ai, "rec").unwrap();
        let chat = t.create_chat("").unwrap();
        let input = json!({ "chat": chat, "question": question });
        let scope = Scope::begin(
            ai,
            Arc::new(engine.clone()),
            "chat-title",
            &task_title(question),
            &input,
            Some("rec"),
        )
        .unwrap();
        (scope, chat)
    }

    #[test]
    fn the_title_comes_from_the_titles_model_once_and_the_task_counts_the_call() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let engine = FakeEngine::new();
        engine.chat_answers(&["«Приветствие»."]);
        let (scope, chat) = start(ai, &engine, "првиет");
        let title = work(
            &scope,
            &AgentSettings::default(),
            ai,
            "rec",
            chat,
            "првиет",
            &AtomicBool::new(false),
        );
        assert_eq!(title.as_deref(), Some("Приветствие"));
        let t = Transcript::open(ai, "rec").unwrap();
        assert_eq!(t.chat_title(chat).unwrap().as_deref(), Some("Приветствие"));
        let chats = engine.chats();
        assert_eq!(chats.len(), 1);
        assert_eq!(chats[0].model, "unsloth/Qwen3-0.6B-GGUF");
        assert!(
            !chats[0].enable_thinking,
            "a title is a short service call: the model does not think"
        );
        assert_eq!(chats[0].max_tokens, TITLE_TOKENS);
        assert!(
            chats[0].texts[1].contains("Write its title in English")
                && chats[0].texts[1].contains("првиет"),
            "{:?}",
            chats[0].texts
        );
        let task = scope.store().get(scope.task()).unwrap().unwrap();
        assert_eq!(
            (task.kind.as_str(), task.status, task.inline),
            ("chat-title", TaskStatus::Done, true)
        );
        assert_eq!(
            (task.stats.model_calls, task.stats.models.clone()),
            (1, vec!["unsloth/Qwen3-0.6B-GGUF".to_string()])
        );
        assert_eq!(task.result.as_ref().unwrap()["title"], "Приветствие");
        assert!(!t.set_chat_title(chat, "Другое").unwrap());
    }

    #[test]
    fn the_title_follows_the_interface_language() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        for (code, language) in [
            (None, "English"),
            (Some("en"), "English"),
            (Some("ru"), "Russian"),
        ] {
            let engine = FakeEngine::new();
            engine.chat_answers(&["«Приветствие»."]);
            let settings = AgentSettings {
                ui_language: code.map(str::to_string),
                ..AgentSettings::default()
            };
            let (scope, chat) = start(ai, &engine, "првиет");
            let title = work(
                &scope,
                &settings,
                ai,
                "rec",
                chat,
                "првиет",
                &AtomicBool::new(false),
            );
            assert_eq!(title.as_deref(), Some("Приветствие"));
            let chats = engine.chats();
            assert_eq!(chats.len(), 1);
            let asked = format!("Write its title in {language}");
            assert!(
                chats[0].texts[1].contains(&asked),
                "{code:?}: {:?}",
                chats[0].texts
            );
        }
    }

    #[test]
    fn a_model_that_fails_leaves_the_first_message_as_the_title() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let engine = FakeEngine::new();
        engine.chat_answers(&["  "]);
        let (scope, chat) = start(ai, &engine, "когда отчёт?");
        work(
            &scope,
            &AgentSettings::default(),
            ai,
            "rec",
            chat,
            "когда отчёт?",
            &AtomicBool::new(false),
        );
        let t = Transcript::open(ai, "rec").unwrap();
        assert_eq!(t.chat_title(chat).unwrap().as_deref(), Some("Когда отчёт?"));
        let task = scope.store().get(scope.task()).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
        assert_eq!(
            task.error.as_deref(),
            Some("the model wrote no title: \"  \"; the title is the first message")
        );

        let mut settings = AgentSettings::default();
        settings.models.titles = None;
        let (scope, chat) = start(ai, &engine, "как дела");
        work(
            &scope,
            &settings,
            ai,
            "rec",
            chat,
            "как дела",
            &AtomicBool::new(false),
        );
        assert_eq!(t.chat_title(chat).unwrap().as_deref(), Some("Как дела"));
        assert_eq!(engine.chats().len(), 1, "the model was not asked");
    }

    #[test]
    fn a_stopped_title_task_sets_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let engine = FakeEngine::new();
        let (scope, chat) = start(ai, &engine, "привет");
        let cancel = AtomicBool::new(true);
        assert_eq!(
            work(
                &scope,
                &AgentSettings::default(),
                ai,
                "rec",
                chat,
                "привет",
                &cancel
            ),
            None
        );
        assert_eq!(
            Transcript::open(ai, "rec")
                .unwrap()
                .chat_title(chat)
                .unwrap()
                .as_deref(),
            Some("")
        );
        assert_eq!(
            scope.store().get(scope.task()).unwrap().unwrap().status,
            TaskStatus::Cancelled
        );
    }

    #[test]
    #[ignore]
    fn titles_model_names_three_questions() {
        let engine = crate::live::engine();
        let settings = AgentSettings::default();
        let dir = tempfile::tempdir().unwrap();
        let questions = [
            "привет",
            "о чём ролик?",
            "Расскажи подробно, как подтвердить опыт без трудовой книжки: какие документы собрать, что говорить работодателю и чем подтвердить проекты, если официального стажа нет",
        ];
        for question in questions {
            let scope = Scope::begin(
                dir.path(),
                engine.clone(),
                "chat-title",
                &task_title(question),
                &json!({}),
                None,
            )
            .unwrap();
            let (title, raw) =
                ask_model(&scope, &settings, question, &AtomicBool::new(false)).unwrap();
            eprintln!("QUESTION: {question}\nRAW: {raw:?}\nTITLE: {title:?}");
            assert!(
                title.as_ref().is_some_and(|t| !t.is_empty()),
                "no title for {question:?}, the model returned {raw:?}"
            );
        }
    }
}
