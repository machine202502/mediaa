use super::super::tools::tests::recording;
use super::super::{answer, ask, Brain, Message, Piece, Step, ToolSpec, Tools};
use super::*;
use crate::slots::Slots;
use mediaa_core::agent::{AgentSettings, Budget};
use mediaa_queue::Status as TaskStatus;
use mediaa_engine::testing::FakeEngine;
use std::time::Duration;

const STEP_SEARCH: &str = r#"{"thought":"","action":"search","title":"Ищу отчёт","args":{"query":"отчёт"}}"#;
const STEP_ANSWER: &str = r#"{"thought":"","action":"answer","answer":"В пятницу [0:10]."}"#;
const STEP_CHECK: &str = r#"{"changed":true,"answer":"В пятницу [0:10]."}"#;

fn nothing(_: &str, _: crate::search::Options, _: usize) -> anyhow::Result<Vec<mediaa_core::retrieval::Hit>> {
    Ok(vec![])
}

#[test]
fn an_answer_is_a_task_with_model_steps_and_tools_that_count_their_work() {
    let dir = tempfile::tempdir().unwrap();
    let ai = dir.path();
    let engine = FakeEngine::new();
    engine.chat_answers(&[STEP_SEARCH, STEP_ANSWER, STEP_CHECK]);
    let t = Transcript::open(ai, "rec").unwrap();
    let chat = t.create_chat("Сроки").unwrap();
    let input = json!({ "chat": chat, "question": "Когда отчёт?" });
    let scope = Scope::begin(ai, Arc::new(engine.clone()), "chat", "Когда отчёт?", &input, Some("rec")).unwrap();
    let run = Run::new(&scope);
    let (rec, settings, slots) = (recording(), AgentSettings::default(), Slots::new(Budget::default()));
    let mut memory = Memory::default();

    let outcome = work(&run, &scope, ai, "rec", chat, &mut memory, |memory, on_event| {
        ask(&scope, &slots, &settings, ai, "rec", &rec, &[], memory, "Когда отчёт?", &run.cancel, on_event)
    });
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(engine.chats().len(), 3, "the search, the draft and the check");

    let q = scope.store();
    let root = q.get(scope.task()).unwrap().unwrap();
    assert_eq!((root.kind.as_str(), root.status, root.scope.as_deref()), ("chat", TaskStatus::Done, Some("rec")));
    assert_eq!(root.title, "Когда отчёт?");
    assert_eq!(root.input, input);
    let children = q.children(root.id).unwrap();
    assert_eq!(
        children.iter().map(|c| (c.kind.as_str(), c.status)).collect::<Vec<_>>(),
        [("model-step", TaskStatus::Done), ("tool", TaskStatus::Done), ("model-step", TaskStatus::Done), ("model-step", TaskStatus::Done)]
    );
    assert!(children.iter().all(|c| c.inline && c.scope.as_deref() == Some("rec")));
    let agent_model = "unsloth/Qwen3.5-4B-GGUF";
    for step in [&children[0], &children[2]] {
        assert_eq!((step.stats.model_calls, step.stats.prompt_tokens, step.stats.completion_tokens), (1, 7, 3), "{}", step.title);
        assert_eq!(step.stats.models, [agent_model]);
    }
    assert_eq!((children[0].title.as_str(), children[2].title.as_str(), children[3].title.as_str()), ("1", "2", "Проверка ответа"));
    assert_eq!(children[3].stats.model_calls, 1);
    let check = &engine.chats()[2];
    assert!(check.json_schema.as_ref().is_some_and(|s| s.contains("changed")), "{:?}", check.json_schema);
    assert!(check.texts.iter().any(|t| t.contains("Отчёт отправляем в пятницу.")), "{:?}", check.texts);
    let tool = &children[1];
    assert_eq!(tool.title, "Поиск");
    assert_eq!(tool.detail["tool"], "Поиск");
    assert_eq!(tool.detail["args"], r#"{"query":"отчёт"}"#);
    assert!(tool.detail["result_chars"].as_u64().unwrap() > 0);
    assert_eq!(tool.stats.model_calls, 1);
    assert_eq!(tool.stats.models, ["ggml-org/embeddinggemma-300M-GGUF"]);
    assert_eq!(root.stats.model_calls, 0);

    let messages = t.messages(chat).unwrap();
    let answer_message = messages.last().unwrap();
    assert_eq!(answer_message.text, "В пятницу [0:10].");
    let live = run.live();
    assert_eq!(live.state, State::Done);
    assert_eq!(answer_message.timeline(), live.timeline, "the details keep the timeline");
    assert_eq!(answer_message.details["steps"][0]["tool"], "search");
    assert!(!answer_message.stopped());
    use mediaa_core::chat::TimelineEntry::{Answer, Step as StepEntry};
    assert_eq!(
        live.timeline.entries,
        [
            StepEntry { title: "Поиск".into(), tool: "search".into(), status: "nothing".into() },
            StepEntry { title: "Черновик ответа".into(), tool: "draft".into(), status: "found".into() },
            StepEntry { title: "Проверка ответа".into(), tool: "verify".into(), status: "nothing".into() },
            Answer { text: "В пятницу [0:10].".into(), section: false },
        ]
    );
}

struct Blocking {
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl Blocking {
    fn wait_for_the_test(&self) -> bool {
        self.release.lock().unwrap().recv_timeout(Duration::from_secs(5)).is_ok()
    }
}

impl Brain for Blocking {
    fn step(&self, _: &[Message], _: &[ToolSpec], _: bool, cancel: &AtomicBool, on_piece: &mut dyn FnMut(Piece)) -> anyhow::Result<Step> {
        on_piece(Piece::Thinking("Думаю про "));
        on_piece(Piece::Thinking("отчёт."));
        while !cancel.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(self.wait_for_the_test(), "the stop waited for the worker's late work");
        on_piece(Piece::Thinking(" Уже поздно."));
        on_piece(Piece::Answer("Поздний ответ."));
        Ok(Step::Answer { thought: String::new(), text: "Поздний ответ.".into() })
    }

    fn complete(&self, _: &[Message], _: &AtomicBool) -> anyhow::Result<String> {
        Ok(String::new())
    }

    fn context(&self) -> u32 {
        8192
    }
}

#[test]
fn stop_ends_the_answer_at_once_and_what_comes_later_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let ai = dir.path();
    let t = Transcript::open(ai, "rec").unwrap();
    let chat = t.create_chat("Сроки").unwrap();
    let scope = Scope::begin(ai, Arc::new(FakeEngine::new()), "chat", "Когда?", &json!({}), Some("rec")).unwrap();
    let run = Run::new(&scope);
    let (release, gate) = std::sync::mpsc::channel();
    let late = Blocking { release: std::sync::Mutex::new(gate) };
    std::thread::scope(|s| {
        let worker = s.spawn(|| {
            let (rec, mut memory) = (recording(), Memory::default());
            let tools = Tools::new(&rec, &nothing, 4000);
            work(&run, &scope, ai, "rec", chat, &mut memory, |memory, on_event| answer(&late, &tools, &scope, &[], memory, "Когда?", &run.cancel, None, false, on_event))
        });
        while run.live().timeline.is_empty() {
            std::thread::sleep(Duration::from_millis(5));
        }
        run.stop(&t, chat).unwrap();
        assert_eq!(run.live().state, State::Stopped);
        assert!(!worker.is_finished(), "the stop does not wait for the worker");
        release.send(()).unwrap();
        assert_eq!(worker.join().unwrap(), Outcome::Stopped);
    });

    let messages = t.messages(chat).unwrap();
    assert_eq!(messages.len(), 1, "no second message from the worker");
    let m = &messages[0];
    assert!(m.stopped());
    assert_eq!(m.text, "");
    assert_eq!(m.timeline().entries, [mediaa_core::chat::TimelineEntry::Thinking { text: "Думаю про отчёт.".into() }]);
    assert_eq!(run.live().timeline.entries, m.timeline().entries, "the pane did not take the later events either");
    assert_eq!(t.chat_memory(chat).unwrap(), None, "a stopped answer saves nothing of the worker's");

    let q = scope.store();
    assert_eq!(q.get(scope.task()).unwrap().unwrap().status, TaskStatus::Cancelled);
    let children = q.children(scope.task()).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].status, TaskStatus::Cancelled, "{:?}", children[0]);
    run.stop(&t, chat).unwrap();
    assert_eq!(t.messages(chat).unwrap().len(), 1);
}

#[test]
fn a_stop_keeps_the_partial_answer_text_and_marks_a_step_that_was_running() {
    let dir = tempfile::tempdir().unwrap();
    let ai = dir.path();
    let t = Transcript::open(ai, "rec").unwrap();
    let chat = t.create_chat("Сроки").unwrap();
    let scope = Scope::begin(ai, Arc::new(FakeEngine::new()), "chat", "Когда?", &json!({}), Some("rec")).unwrap();
    let run = Run::new(&scope);
    {
        let mut live = run.live();
        live.apply(&Event::Thinking("Сначала ищу.".into()));
        live.apply(&Event::StepStarted { title: "Читаю всё".into(), tool: "scan".into() });
        live.apply(&Event::Scanning { done: 2, total: 9 });
        live.apply(&Event::AnswerText("Пока что извест".into()));
        assert_eq!(live.scanning, Some((2, 9)));
    }
    run.stop(&t, chat).unwrap();
    let m = &t.messages(chat).unwrap()[0];
    assert_eq!(m.text, "Пока что извест", "what was written so far is the text");
    use mediaa_core::chat::TimelineEntry::{Answer, Step as StepEntry, Thinking};
    assert_eq!(
        m.timeline().entries,
        [
            Thinking { text: "Сначала ищу.".into() },
            StepEntry { title: "Читаю всё".into(), tool: "scan".into(), status: "stopped".into() },
            Answer { text: "Пока что извест".into(), section: false },
        ]
    );
}

#[test]
fn a_failed_answer_is_a_failed_task_and_the_chat_keeps_what_it_learned() {
    let dir = tempfile::tempdir().unwrap();
    let ai = dir.path();
    let t = Transcript::open(ai, "rec").unwrap();
    let chat = t.create_chat("Сроки").unwrap();
    let scope = Scope::begin(ai, Arc::new(FakeEngine::new()), "chat", "Когда?", &json!({}), Some("rec")).unwrap();
    let run = Run::new(&scope);
    let mut memory = Memory::default();
    let outcome = work(&run, &scope, ai, "rec", chat, &mut memory, |memory, _| {
        memory.remember("Пользователь любит кратко");
        anyhow::bail!("the provider is down")
    });
    assert_eq!(outcome, Outcome::Failed("the provider is down".into()));
    assert_eq!(run.live().state, State::Failed("the provider is down".into()));
    let task = scope.store().get(scope.task()).unwrap().unwrap();
    assert_eq!((task.status, task.error.as_deref()), (TaskStatus::Failed, Some("the provider is down")));
    assert!(t.messages(chat).unwrap().is_empty(), "no answer was stored");
    assert!(t.chat_memory(chat).unwrap().unwrap().contains("кратко"));
}

#[test]
fn sections_are_stored_as_they_come_and_the_closing_text_after_them() {
    let dir = tempfile::tempdir().unwrap();
    let ai = dir.path();
    let t = Transcript::open(ai, "rec").unwrap();
    let chat = t.create_chat("Сроки").unwrap();
    let scope = Scope::begin(ai, Arc::new(FakeEngine::new()), "chat", "Когда?", &json!({}), Some("rec")).unwrap();
    let run = Run::new(&scope);
    let mut memory = Memory::default();
    let seen = std::sync::Mutex::new(Vec::new());
    let outcome = work(&run, &scope, ai, "rec", chat, &mut memory, |_, on_event| {
        on_event(&Event::AnswerText("Часть 1 [0:10].".into()));
        on_event(&Event::Say("Часть 1 [0:10].".into()));
        seen.lock().unwrap().push((run.live().stored, t.messages(chat).unwrap().len()));
        on_event(&Event::AnswerText("Итог.".into()));
        Ok(Answer { text: "Итог.".into(), steps: vec![], forced: true })
    });
    assert_eq!(outcome, Outcome::Done);
    assert_eq!(*seen.lock().unwrap(), [(1, 1)]);
    let messages = t.messages(chat).unwrap();
    assert_eq!(messages.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["Часть 1 [0:10].", "Итог."]);
    assert_eq!(messages[0].details, json!({"section": true}));
    assert_eq!(messages[1].details["forced"], true);
    use mediaa_core::chat::TimelineEntry::Answer as AnswerEntry;
    assert_eq!(
        messages[1].timeline().entries,
        [AnswerEntry { text: "Часть 1 [0:10].".into(), section: true }, AnswerEntry { text: "Итог.".into(), section: false }]
    );
}

#[test]
fn the_pane_state_follows_events() {
    let mut live = Live::new();
    live.apply(&Event::Waiting { reason: WaitReason::Loading { model: "m".into() } });
    assert!(live.waiting.is_some());
    let metrics = Metrics { context_tokens: 6100, context: 8000, speed: Some(12.4) };
    live.apply(&Event::Metrics(metrics));
    assert_eq!(live.metrics, Some(metrics), "the numbers of the running answer");
    assert!(live.waiting.is_some(), "the numbers do not end the waiting");
    live.apply(&Event::Thinking("Думаю".into()));
    assert_eq!(live.waiting, None, "any other event ends the waiting");
    live.apply(&Event::Scanning { done: 1, total: 3 });
    live.apply(&Event::StepStarted { title: "Ищу".into(), tool: "search".into() });
    assert_eq!(live.scanning, None, "a step ends the scan shown");
    live.apply(&Event::AnswerText("Черн".into()));
    live.apply(&Event::AnswerDiscarded);
    assert_eq!(live.timeline.draft(), "");
    live.apply(&Event::Failed("плохо".into()));
    assert_eq!(live.state, State::Failed("плохо".into()));
    assert!(
        live.timeline.entries.iter().any(|e| matches!(e, mediaa_core::chat::TimelineEntry::Step { status, .. } if status == "stopped")),
        "a step that never ended is marked"
    );
    assert_eq!(live.details(false, false)["metrics"], json!(metrics), "the numbers stay in the saved answer");
}
