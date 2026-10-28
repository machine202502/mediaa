use mediaa_engine::llm::{split_thinking, Part, ThinkSplitter};
use mediaa_engine::{ChatMessage, ChatRequest, ChatRole, Device, Engine, Priority, Thinking};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
        }
    }
}

impl From<Role> for ChatRole {
    fn from(role: Role) -> ChatRole {
        match role {
            Role::System => ChatRole::System,
            Role::User => ChatRole::User,
            Role::Assistant => ChatRole::Assistant,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Message {
        Message {
            role,
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    pub params: Value,
    pub example: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub tool: String,
    pub args: Value,
}

pub const MAX_CALLS: usize = 3;

#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Tool { thought: String, calls: Vec<Call> },
    Answer { thought: String, text: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Piece<'a> {
    Thinking(&'a str),
    Answer(&'a str),
}

pub trait Brain {
    fn step(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        may_answer: bool,
        cancel: &AtomicBool,
        on_piece: &mut dyn FnMut(Piece),
    ) -> anyhow::Result<Step>;
    fn complete(&self, messages: &[Message], cancel: &AtomicBool) -> anyhow::Result<String>;
    fn complete_json(
        &self,
        messages: &[Message],
        schema: &Value,
        cancel: &AtomicBool,
    ) -> anyhow::Result<String> {
        let _ = schema;
        self.complete(messages, cancel)
    }
    fn context(&self) -> u32;
    fn speed(&self) -> Option<f64> {
        None
    }
    fn thinking(&self) -> bool {
        false
    }
}

pub const ANSWER: &str = "answer";

pub const THINK_TOKENS: u32 = 2048;

const THOUGHT_CHARS: u32 = 600;

pub fn step_schema(tools: &[ToolSpec], may_answer: bool, thinking: bool) -> Value {
    let thought = json!({"type": "string", "maxLength": THOUGHT_CHARS});
    let object = |mut properties: Value, mut required: Vec<&str>| {
        if !thinking {
            properties["thought"] = thought.clone();
            required.insert(0, "thought");
        }
        json!({"type": "object", "properties": properties, "required": required})
    };
    let mut options: Vec<Value> = Vec::new();
    if may_answer || tools.is_empty() {
        options.push(object(
            json!({"action": {"const": ANSWER}, "answer": {"type": "string"}}),
            vec!["action", "answer"],
        ));
    }
    if !tools.is_empty() {
        let mut calls: Vec<Value> = tools
            .iter()
            .map(|t| json!({"type": "object", "properties": {"action": {"const": t.name}, "args": t.params}, "required": ["action", "args"]}))
            .collect();
        let call = if calls.len() == 1 {
            calls.remove(0)
        } else {
            json!({ "oneOf": calls })
        };
        options.push(object(json!({"calls": {"type": "array", "minItems": 1, "maxItems": MAX_CALLS, "items": call}}), vec!["calls"]));
    }
    if options.len() == 1 {
        return options.remove(0);
    }
    json!({ "oneOf": options })
}

pub fn parse_step(text: &str) -> anyhow::Result<Step> {
    let v: Value = serde_json::from_str(text.trim())
        .map_err(|e| anyhow::anyhow!("the step is not JSON ({e}): {text}"))?;
    let o = v
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("the step is not an object: {text}"))?;
    let s = |k: &str| {
        o.get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string()
    };
    let action = s("action");
    if action == ANSWER {
        return Ok(Step::Answer {
            thought: s("thought"),
            text: s("answer"),
        });
    }
    if let Some(items) = o.get("calls").and_then(Value::as_array) {
        let mut calls: Vec<Call> = items
            .iter()
            .filter_map(|item| {
                let tool = item
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                (!tool.is_empty()).then(|| Call {
                    tool,
                    args: item
                        .get("args")
                        .cloned()
                        .unwrap_or(Value::Object(Map::new())),
                })
            })
            .collect();
        anyhow::ensure!(!calls.is_empty(), "the step calls no tool: {text}");
        if calls.len() > MAX_CALLS {
            log::warn!(
                "the model wrote {} calls in one step; only the first {MAX_CALLS} are kept",
                calls.len()
            );
            calls.truncate(MAX_CALLS);
        }
        return Ok(Step::Tool {
            thought: s("thought"),
            calls,
        });
    }
    anyhow::ensure!(!action.is_empty(), "the step has no action: {text}");
    let tool = action;
    Ok(Step::Tool {
        thought: s("thought"),
        calls: vec![Call {
            tool,
            args: o.get("args").cloned().unwrap_or(Value::Object(Map::new())),
        }],
    })
}

#[derive(Debug, Default)]
pub struct AnswerStream {
    raw: String,
    start: Option<usize>,
    done: usize,
    finished: bool,
}

impl AnswerStream {
    fn find_start(raw: &str) -> Option<usize> {
        let mut from = 0;
        while let Some(at) = raw[from..].find("\"answer\"") {
            let after = from + at + "\"answer\"".len();
            let rest = raw[after..].trim_start();
            if let Some(value) = rest.strip_prefix(':') {
                let value = value.trim_start();
                if value.starts_with('"') {
                    return Some(raw.len() - value.len() + 1);
                }
                return None;
            }
            from = after;
        }
        None
    }

    pub fn push(&mut self, piece: &str) -> String {
        self.raw.push_str(piece);
        if self.finished {
            return String::new();
        }
        if self.start.is_none() {
            self.start = Self::find_start(&self.raw);
            self.done = self.start.unwrap_or(0);
        }
        if self.start.is_none() {
            return String::new();
        }
        let mut out = String::new();
        let bytes = self.raw.as_bytes();
        let mut i = self.done;
        while i < bytes.len() {
            match bytes[i] {
                b'"' => {
                    self.finished = true;
                    i += 1;
                    break;
                }
                b'\\' => {
                    let Some(&e) = bytes.get(i + 1) else { break };
                    let (text, len) = match e {
                        b'n' => ("\n".to_string(), 2),
                        b't' => ("\t".to_string(), 2),
                        b'r' => (String::new(), 2),
                        b'u' => {
                            let Some(hex) = self.raw.get(i + 2..i + 6) else {
                                break;
                            };
                            (
                                u32::from_str_radix(hex, 16)
                                    .ok()
                                    .and_then(char::from_u32)
                                    .map(String::from)
                                    .unwrap_or_default(),
                                6,
                            )
                        }
                        other => ((other as char).to_string(), 2),
                    };
                    out.push_str(&text);
                    i += len;
                }
                _ => {
                    let ch = self.raw[i..].chars().next().expect("in bounds");
                    out.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        self.done = i;
        out
    }
}

const LOOP_WINDOW: usize = 120;

pub fn loop_start(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n < LOOP_WINDOW * 2 {
        return None;
    }
    let window: String = chars[n - LOOP_WINDOW..].iter().collect();
    let first = text[..text.find(&window)?].chars().count();
    if first >= n - LOOP_WINDOW {
        return None;
    }
    let period = n - LOOP_WINDOW - first;
    let mut start = n - LOOP_WINDOW;
    while start > period && chars[start - 1] == chars[start - 1 - period] {
        start -= 1;
    }
    Some(chars[..start].iter().map(|c| c.len_utf8()).sum())
}

const DANGLING: usize = 400;

pub fn finished(text: &str) -> String {
    let mut t = text.trim_end();
    let marker = |l: &str| {
        let l = l.trim();
        matches!(l, "-" | "*" | "•")
            || (l.len() > 1
                && l.ends_with(['.', ')'])
                && l[..l.len() - 1].chars().all(|c| c.is_ascii_digit()))
    };
    if let Some(last) = t.rsplit('\n').next().filter(|l| marker(l)) {
        t = t[..t.len() - last.len()].trim_end();
    }
    if t.ends_with(['.', '!', '?', '…', ')', ']']) {
        return t.to_string();
    }
    let paragraph = t.rfind("\n\n").map(|i| i + 2);
    let sentence = t
        .rfind(['.', '!', '?', '…'])
        .map(|i| i + t[i..].chars().next().map_or(1, char::len_utf8));
    match paragraph.max(sentence) {
        Some(end) if t.len() - end <= DANGLING || end > t.len() / 2 => {
            t[..end].trim_end().to_string()
        }
        _ => t.to_string(),
    }
}

pub struct EngineBrain<'a> {
    pub engine: &'a dyn Engine,
    pub model: String,
    pub n_ctx: u32,
    pub device: Device,
    pub priority: Priority,
    pub max_tokens: u32,
    pub hf_token: Option<String>,
    pub thinking: bool,
    pub(crate) speed: std::sync::Mutex<Option<f64>>,
}

impl EngineBrain<'_> {
    fn request(&self, messages: &[Message], max_tokens: u32) -> ChatRequest {
        ChatRequest {
            model: self.model.clone(),
            messages: messages
                .iter()
                .map(|m| ChatMessage::new(m.role.into(), m.content.clone()))
                .collect(),
            max_tokens,
            temperature: 0.0,
            n_ctx: Some(self.n_ctx),
            device: Some(self.device),
            priority: Some(self.priority),
            hf_token: self.hf_token.clone(),
            ..ChatRequest::default()
        }
    }
}

impl EngineBrain<'_> {
    fn measure_speed(&self, tokens: u32, since: Option<std::time::Instant>) {
        let Some(since) = since else { return };
        let secs = since.elapsed().as_secs_f64();
        if secs >= 0.15 && tokens > 1 {
            *self.speed.lock().expect("speed lock") = Some(f64::from(tokens) / secs);
        }
    }
}

impl Brain for EngineBrain<'_> {
    fn step(
        &self,
        messages: &[Message],
        tools: &[ToolSpec],
        may_answer: bool,
        cancel: &AtomicBool,
        on_piece: &mut dyn FnMut(Piece),
    ) -> anyhow::Result<Step> {
        let request = ChatRequest {
            json_schema: Some(step_schema(tools, may_answer, self.thinking)),
            no_repeats: true,
            ..self.request(messages, self.max_tokens)
        };
        let mut stream = AnswerStream::default();
        let mut answer = String::new();
        let mut looped = None;
        let mut tokens = 0u32;
        let mut first: Option<std::time::Instant> = None;
        let mut handle = |part: Part| -> bool {
            tokens += 1;
            let at = *first.get_or_insert_with(std::time::Instant::now);
            self.measure_speed(tokens, Some(at));
            match part {
                Part::Thinking(text) => {
                    on_piece(Piece::Thinking(&text));
                    true
                }
                Part::Text(text) => {
                    let new = stream.push(&text);
                    if new.is_empty() {
                        return true;
                    }
                    answer.push_str(&new);
                    if let Some(cut) = loop_start(&answer) {
                        looped = Some(cut);
                        return false;
                    }
                    on_piece(Piece::Answer(&new));
                    true
                }
            }
        };
        let (content, usage) = if self.thinking {
            let mut splitter = ThinkSplitter::default();
            let generated = self.engine.chat_thinking(
                &request,
                Thinking {
                    budget: THINK_TOKENS,
                },
                cancel,
                &mut |piece| splitter.push(piece).into_iter().all(&mut handle),
            )?;
            splitter.finish().into_iter().for_each(|part| {
                handle(part);
            });
            (split_thinking(&generated.content).1, generated.usage)
        } else {
            let generated = self.engine.chat(&request, cancel, &mut |piece| {
                handle(Part::Text(piece.to_string()))
            })?;
            (generated.content, generated.usage)
        };
        if let Some(usage) = usage {
            self.measure_speed(usage.completion_tokens, first);
        }
        match (parse_step(&content), looped) {
            (Ok(step), None) => Ok(step),
            (_, cut) if !answer.trim().is_empty() => {
                if cut.is_some() {
                    log::info!("the model went round in a loop; the answer is cut where it began");
                }
                Ok(Step::Answer {
                    thought: String::new(),
                    text: finished(&answer[..cut.unwrap_or(answer.len())]),
                })
            }
            (result, _) => result,
        }
    }

    fn complete(&self, messages: &[Message], cancel: &AtomicBool) -> anyhow::Result<String> {
        anyhow::ensure!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "cancelled"
        );
        let request = self.request(messages, self.max_tokens);
        Ok(self
            .engine
            .chat(&request, cancel, &mut |_| true)?
            .content
            .trim()
            .to_string())
    }

    fn complete_json(
        &self,
        messages: &[Message],
        schema: &Value,
        cancel: &AtomicBool,
    ) -> anyhow::Result<String> {
        anyhow::ensure!(
            !cancel.load(std::sync::atomic::Ordering::Relaxed),
            "cancelled"
        );
        let request = ChatRequest {
            json_schema: Some(schema.clone()),
            ..self.request(messages, self.max_tokens)
        };
        let content = if self.thinking {
            let generated = self.engine.chat_thinking(
                &request,
                Thinking {
                    budget: THINK_TOKENS,
                },
                cancel,
                &mut |_| true,
            )?;
            split_thinking(&generated.content).1
        } else {
            self.engine.chat(&request, cancel, &mut |_| true)?.content
        };
        Ok(content.trim().to_string())
    }

    fn context(&self) -> u32 {
        self.n_ctx
    }

    fn speed(&self) -> Option<f64> {
        *self.speed.lock().expect("speed lock")
    }

    fn thinking(&self) -> bool {
        self.thinking
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &'static str) -> ToolSpec {
        ToolSpec {
            name,
            description: String::new(),
            params: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}),
            example: json!({"q": "x"}),
        }
    }

    #[test]
    fn schema_is_a_choice_of_calls_and_the_answer() {
        let s = step_schema(&[spec("search"), spec("read")], true, false);
        let options = s["oneOf"].as_array().unwrap();
        assert_eq!(options.len(), 2);
        assert!(
            options[0]["properties"]["answer"].is_object(),
            "the answer first"
        );
        let calls = &options[1]["properties"]["calls"];
        assert_eq!(calls["minItems"], 1);
        assert_eq!(calls["maxItems"], 3);
        let items = calls["items"]["oneOf"].as_array().unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0]["properties"]["action"]["const"], "search");
        assert!(
            items[0]["properties"].get("title").is_none(),
            "a call has no title field"
        );
        assert_eq!(items[0]["required"], json!(["action", "args"]));
        assert_eq!(options[1]["required"], json!(["thought", "calls"]));
        assert_eq!(
            step_schema(&[], false, false)["required"],
            json!(["thought", "action", "answer"])
        );
        let calls =
            &step_schema(&[spec("search"), spec("read")], false, false)["properties"]["calls"];
        assert_eq!(calls["items"]["oneOf"].as_array().unwrap().len(), 2);
        assert_eq!(
            step_schema(&[spec("search")], false, false)["properties"]["calls"]["items"]
                ["properties"]["action"]["const"],
            "search"
        );
    }

    #[test]
    fn the_engine_turns_the_schema_of_a_step_with_calls_into_a_grammar() {
        for tools in [
            vec![],
            vec![spec("search")],
            vec![spec("search"), spec("read"), spec("say")],
        ] {
            for may_answer in [true, false] {
                for thinking in [true, false] {
                    let schema = step_schema(&tools, may_answer, thinking);
                    mediaa_engine::llm::check_schema(&schema.to_string())
                        .unwrap_or_else(|e| panic!("{schema}\n{e:#}"));
                }
            }
        }
    }

    #[test]
    fn a_thinking_model_writes_no_thought_field() {
        let s = step_schema(&[spec("search")], true, true);
        for option in s["oneOf"].as_array().unwrap() {
            assert!(option["properties"].get("thought").is_none(), "{option}");
            assert!(
                !option["required"]
                    .as_array()
                    .unwrap()
                    .contains(&json!("thought")),
                "{option}"
            );
        }
        assert_eq!(
            step_schema(&[], false, true)["required"],
            json!(["action", "answer"])
        );
        assert_eq!(
            parse_step(r#"{"action":"answer","answer":"Да."}"#).unwrap(),
            Step::Answer {
                thought: String::new(),
                text: "Да.".into()
            }
        );
        for thinking in [true, false] {
            let fake = mediaa_engine::testing::FakeEngine::new();
            fake.chat_deltas(&[r#"{"action":"answer","answer":"Да."}"#]);
            let brain = engine_brain(&fake, thinking);
            brain
                .step(
                    &[Message::new(Role::User, "q")],
                    &[],
                    true,
                    &AtomicBool::new(false),
                    &mut |_| {},
                )
                .unwrap();
            let schema = fake.chats()[0].json_schema.clone().unwrap();
            assert_eq!(schema.contains("thought"), !thinking, "{schema}");
        }
    }

    fn streamed(raw: &str, n: usize) -> String {
        let chars: Vec<char> = raw.chars().collect();
        let mut s = AnswerStream::default();
        chars
            .chunks(n)
            .map(|c| s.push(&c.iter().collect::<String>()))
            .collect()
    }

    #[test]
    fn the_answer_streams_out_of_the_json() {
        let raw = r#"{"thought":"скажу \"да\"","action":"answer","answer":"Да, \"дуб\" [0:10].\nВторая строка é и \\ всё."}"#;
        let want = "Да, \"дуб\" [0:10].\nВторая строка é и \\ всё.";
        for n in [1, 2, 3, 7, 1000] {
            assert_eq!(streamed(raw, n), want, "pieces of {n}");
        }
        assert_eq!(
            streamed(
                r#"{ "thought" : "", "action" : "answer" , "answer" : "Нет." }"#,
                4
            ),
            "Нет."
        );
        assert_eq!(
            streamed(
                r#"{"thought":"","action":"search","title":"answer","args":{"query":"answer"}}"#,
                3
            ),
            ""
        );
    }

    #[test]
    fn loops_are_noticed_and_cut_where_they_began() {
        let intro = "Документы для подтверждения опыта: договор, счета и акты выполненных работ. ";
        let block = "**Примечание:** видео не содержит прямых ссылок на конкретные документы, кроме общих категорий [1:40].\n\n";
        assert_eq!(loop_start(&format!("{intro}{block}")), None, "no repeat");
        assert_eq!(loop_start("коротко"), None);
        let text = format!(
            "{intro}{block}{block}{}",
            &block[..block.char_indices().nth(60).unwrap().0]
        );
        let cut = loop_start(&text).unwrap();
        assert_eq!(&text[..cut], format!("{intro}{block}"));
        let drift = format!("{intro}{block}Также:{block}{block}{block}");
        let cut = loop_start(&drift).unwrap();
        assert!(
            drift[..cut].matches("**Примечание:**").count() <= 2,
            "{}",
            &drift[..cut]
        );
    }

    #[test]
    fn cut_answers_end_at_a_finished_sentence() {
        assert_eq!(
            finished("Первое. Второе [1:40].\n\n**Примечание:** Видео не"),
            "Первое. Второе [1:40]."
        );
        assert_eq!(
            finished("Один абзац.\n\nВторой абзац без точки"),
            "Один абзац."
        );
        assert_eq!(finished("Всё сказано. "), "Всё сказано.");
        assert_eq!(
            finished("1. Первое.\n2. Второе.\n11."),
            "1. Первое.\n2. Второе.",
            "a list item without text"
        );
        assert_eq!(finished("Пункты:\n- один.\n-"), "Пункты:\n- один.");
        let long_tail = format!("Начало. {}", "слово ".repeat(100));
        assert_eq!(
            finished(&long_tail),
            long_tail.trim_end(),
            "too much would be lost"
        );
    }

    fn engine_brain(
        engine: &mediaa_engine::testing::FakeEngine,
        thinking: bool,
    ) -> EngineBrain<'_> {
        EngineBrain {
            engine,
            model: "owner/model".into(),
            n_ctx: 4096,
            device: Device::Auto,
            priority: Priority::Interactive,
            max_tokens: 256,
            hf_token: None,
            thinking,
            speed: std::sync::Mutex::new(None),
        }
    }

    fn pieces_of(
        brain: &EngineBrain,
        tools: &[ToolSpec],
    ) -> (Vec<(bool, String)>, anyhow::Result<Step>) {
        let mut pieces = Vec::new();
        let step = brain.step(
            &[Message::new(Role::User, "q")],
            tools,
            true,
            &AtomicBool::new(false),
            &mut |p| match p {
                Piece::Thinking(t) => pieces.push((true, t.to_string())),
                Piece::Answer(t) => pieces.push((false, t.to_string())),
            },
        );
        (pieces, step)
    }

    fn deltas_of(text: &str, size: usize) -> Vec<String> {
        text.chars()
            .collect::<Vec<_>>()
            .chunks(size)
            .map(|c| c.iter().collect())
            .collect()
    }

    #[test]
    fn the_json_of_a_tool_call_is_never_streamed_as_text() {
        let fake = mediaa_engine::testing::FakeEngine::new();
        let call = r#"{"thought":"надо искать","action":"search","title":"answer","args":{"query":"\"answer\": \"x\""}}"#;
        for size in [1, 3, 500] {
            fake.chat_deltas(
                &deltas_of(call, size)
                    .iter()
                    .map(String::as_str)
                    .collect::<Vec<_>>(),
            );
            let (pieces, step) = pieces_of(&engine_brain(&fake, false), &[spec("search")]);
            assert!(pieces.is_empty(), "pieces of {size}: {pieces:?}");
            assert!(matches!(step.unwrap(), Step::Tool { calls, .. } if calls[0].tool == "search"));
        }
        let thinking = format!("<think>\nнадо искать\n</think>\n\n{call}");
        fake.chat_deltas(
            &deltas_of(&thinking, 4)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let (pieces, step) = pieces_of(&engine_brain(&fake, true), &[spec("search")]);
        assert!(pieces.iter().all(|(reasoning, _)| *reasoning), "{pieces:?}");
        assert_eq!(
            pieces.iter().map(|(_, t)| t.as_str()).collect::<String>(),
            "\nнадо искать\n"
        );
        assert!(matches!(step.unwrap(), Step::Tool { calls, .. } if calls[0].tool == "search"));
    }

    #[test]
    fn reasoning_streams_apart_from_the_answer() {
        let fake = mediaa_engine::testing::FakeEngine::new();
        let text = "<think>\nнадо посмотреть время\n</think>\n\n{\"thought\":\"\",\"action\":\"answer\",\"answer\":\"Да, [0:10] \\\"ок\\\".\"}";
        fake.chat_deltas(
            &deltas_of(text, 5)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
        );
        let (pieces, step) = pieces_of(&engine_brain(&fake, true), &[]);
        let reasoning: String = pieces
            .iter()
            .filter(|(r, _)| *r)
            .map(|(_, t)| t.as_str())
            .collect();
        let answer: String = pieces
            .iter()
            .filter(|(r, _)| !*r)
            .map(|(_, t)| t.as_str())
            .collect();
        assert_eq!(reasoning, "\nнадо посмотреть время\n");
        assert_eq!(
            answer, "Да, [0:10] \"ок\".",
            "the answer alone, no reasoning in it"
        );
        assert_eq!(
            step.unwrap(),
            Step::Answer {
                thought: String::new(),
                text: "Да, [0:10] \"ок\".".into()
            }
        );
        let first_answer = pieces.iter().position(|(r, _)| !*r).unwrap();
        assert!(pieces[..first_answer].iter().all(|(r, _)| *r));
        let chats = fake.chats();
        assert_eq!(
            chats[0].thinking,
            Some(THINK_TOKENS),
            "the engine is asked to think first"
        );
        assert!(
            chats[0].json_schema.is_some(),
            "the step grammar still applies after the reasoning"
        );

        fake.chat_deltas(&["{\"thought\":\"\",\"action\":\"answer\",\"answer\":\"Да.\"}"]);
        let (pieces, step) = pieces_of(&engine_brain(&fake, false), &[]);
        assert_eq!(pieces, [(false, "Да.".to_string())]);
        assert!(step.is_ok());
        assert_eq!(fake.chats()[1].thinking, None);
    }

    #[test]
    fn steps_parse() {
        let t = parse_step(
            r#"{"thought":"ищу","action":"search","title":"Ищу бюджет","args":{"q":"бюджет"}}"#,
        )
        .unwrap();
        assert_eq!(
            t,
            Step::Tool {
                thought: "ищу".into(),
                calls: vec![Call {
                    tool: "search".into(),
                    args: json!({"q": "бюджет"})
                }]
            },
            "a leftover title is ignored"
        );
        assert_eq!(
            parse_step(r#" {"thought":"","action":"answer","answer":" Да. "} "#).unwrap(),
            Step::Answer {
                thought: String::new(),
                text: "Да.".into()
            }
        );
        assert!(parse_step("не json").is_err());
        assert!(parse_step(r#"{"thought":"x"}"#).is_err());
        let t = parse_step(r#"{"thought":"","calls":[{"action":"search","args":{"q":"a"}},{"action":"read","args":{"from":"0:00","to":"1:00"}}]}"#).unwrap();
        let Step::Tool { calls, .. } = t else {
            panic!("not a step")
        };
        assert_eq!(
            calls,
            [
                Call {
                    tool: "search".into(),
                    args: json!({"q": "a"})
                },
                Call {
                    tool: "read".into(),
                    args: json!({"from": "0:00", "to": "1:00"})
                }
            ]
        );
        let many = r#"{"calls":[{"action":"a","args":{}},{"action":"b","args":{}},{"action":"c","args":{}},{"action":"d","args":{}}]}"#;
        assert_eq!(
            parse_step(many).unwrap(),
            Step::Tool {
                thought: String::new(),
                calls: ["a", "b", "c"]
                    .map(|t| Call {
                        tool: t.into(),
                        args: json!({})
                    })
                    .to_vec()
            }
        );
        assert!(parse_step(r#"{"calls":[]}"#).is_err());
        assert!(parse_step(r#"{"calls":[{"args":{}}]}"#).is_err());
        assert_eq!(
            parse_step(r#"{"action":"speakers"}"#).unwrap(),
            Step::Tool {
                thought: String::new(),
                calls: vec![Call {
                    tool: "speakers".into(),
                    args: json!({})
                }]
            }
        );
    }

    #[test]
    fn a_check_asks_for_json() {
        let fake = mediaa_engine::testing::FakeEngine::new();
        let schema = json!({"type":"object","properties":{"changed":{"type":"boolean"},"answer":{"type":"string"}},"required":["changed","answer"],"additionalProperties":false});
        fake.chat_answers(&[r#"{"changed":false,"answer":"Да."}"#]);
        let out = engine_brain(&fake, false)
            .complete_json(
                &[Message::new(Role::User, "q")],
                &schema,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(out, r#"{"changed":false,"answer":"Да."}"#);
        assert!(fake.chats()[0]
            .json_schema
            .as_ref()
            .unwrap()
            .contains("\"changed\""));

        fake.chat_answers(&["<think>\nсмотрю\n</think>\n\n{\"changed\":true,\"answer\":\"Нет.\"}"]);
        let out = engine_brain(&fake, true)
            .complete_json(
                &[Message::new(Role::User, "q")],
                &schema,
                &AtomicBool::new(false),
            )
            .unwrap();
        assert_eq!(out, r#"{"changed":true,"answer":"Нет."}"#);
        assert_eq!(fake.chats()[1].thinking, Some(THINK_TOKENS));
    }
}
