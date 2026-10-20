//! The model behind the agent. Each turn the model returns one step: a tool call or the
//! answer. Local models write it as JSON constrained by a grammar built from the tools' schemas
//! (small models never emit a broken call); providers use native tool calls
//! (`super::provider`).

use mediaa_engine::{ChatMessage, ChatRequest, ChatRole, Device, Engine, Priority};
use serde_json::{json, Map, Value};
use std::sync::atomic::AtomicBool;

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// A message of a chat with the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

impl Message {
    pub fn new(role: Role, content: impl Into<String>) -> Message {
        Message { role, content: content.into() }
    }
}

/// A tool the model may call.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: String,
    /// JSON Schema of the arguments (an object).
    pub params: Value,
    /// Example arguments (shown to the model).
    pub example: Value,
}

/// One step of the model.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Tool { thought: String, title: String, tool: String, args: Value },
    Answer { thought: String, text: String },
}

/// What thinks for the agent.
pub trait Brain {
    /// The next step for `messages`: a call of one of `tools` or (when `may_answer`) the answer.
    /// With no tools it must answer. `on_answer` gets the answer's text as it is written.
    fn step(&self, messages: &[Message], tools: &[ToolSpec], may_answer: bool, cancel: &AtomicBool, on_answer: &mut dyn FnMut(&str)) -> anyhow::Result<Step>;
    /// A plain text answer to `messages` (summaries, sub-tasks).
    fn complete(&self, messages: &[Message], cancel: &AtomicBool) -> anyhow::Result<String>;
    /// Context window in tokens.
    fn context(&self) -> u32;
}

/// The action of a step that answers (next to the tool names: the model picks a word, not a
/// JSON key).
pub const ANSWER: &str = "answer";

/// Longest private reasoning before a step (characters).
const THOUGHT_CHARS: u32 = 600;

/// JSON Schema of a step: one of the tool calls, or (when `may_answer` or without tools) the
/// answer.
pub fn step_schema(tools: &[ToolSpec], may_answer: bool) -> Value {
    let thought = json!({"type": "string", "maxLength": THOUGHT_CHARS});
    let mut options: Vec<Value> = tools
        .iter()
        .map(|t| {
            json!({
                "type": "object",
                "properties": {
                    "thought": thought,
                    "action": {"const": t.name},
                    "title": {"type": "string", "maxLength": 80},
                    "args": t.params,
                },
                "required": ["thought", "action", "title", "args"],
            })
        })
        .collect();
    if may_answer || tools.is_empty() {
        // First: models lean towards the first option once they have what they need.
        options.insert(0, json!({
            "type": "object",
            "properties": {"thought": thought, "action": {"const": ANSWER}, "answer": {"type": "string"}},
            "required": ["thought", "action", "answer"],
        }));
    }
    if options.len() == 1 {
        return options.remove(0);
    }
    json!({ "oneOf": options })
}

/// Reads a step the model wrote (JSON).
pub fn parse_step(text: &str) -> anyhow::Result<Step> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| anyhow::anyhow!("the step is not JSON ({e}): {text}"))?;
    let o = v.as_object().ok_or_else(|| anyhow::anyhow!("the step is not an object: {text}"))?;
    let s = |k: &str| o.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    let action = s("action");
    if action == ANSWER {
        return Ok(Step::Answer { thought: s("thought"), text: s("answer") });
    }
    anyhow::ensure!(!action.is_empty(), "the step has no action: {text}");
    let tool = action;
    Ok(Step::Tool { thought: s("thought"), title: s("title"), tool, args: o.get("args").cloned().unwrap_or(Value::Object(Map::new())) })
}

/// Streams the text of the `answer` field out of a step's JSON while it is being written:
/// feed it the raw pieces, it returns the newly decoded text of the answer (escapes resolved).
#[derive(Debug, Default)]
pub struct AnswerStream {
    raw: String,
    /// Byte offset in `raw` where the answer's text starts, once found.
    start: Option<usize>,
    /// Bytes of `raw` already decoded.
    done: usize,
    finished: bool,
}

impl AnswerStream {
    /// Where the `answer` key's string value begins (the key, not the `"answer"` action value).
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
                return None; // the key is there, its value not yet
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
                    // An escape needs its following characters; wait for them.
                    let Some(&e) = bytes.get(i + 1) else { break };
                    let (text, len) = match e {
                        b'n' => ("\n".to_string(), 2),
                        b't' => ("\t".to_string(), 2),
                        b'r' => (String::new(), 2),
                        b'u' => {
                            let Some(hex) = self.raw.get(i + 2..i + 6) else { break };
                            (u32::from_str_radix(hex, 16).ok().and_then(char::from_u32).map(String::from).unwrap_or_default(), 6)
                        }
                        other => ((other as char).to_string(), 2),
                    };
                    out.push_str(&text);
                    i += len;
                }
                _ => {
                    // A whole UTF-8 character (the raw text is valid UTF-8).
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

/// Characters at the end of a text that, seen before, mean the model is going round in a loop.
const LOOP_WINDOW: usize = 120;

/// Where `text` starts repeating itself (a byte index), when its last [`LOOP_WINDOW`] characters
/// already occurred earlier: the repeat is followed back to where the copy began, so cutting
/// there keeps one whole copy.
pub fn loop_start(text: &str) -> Option<usize> {
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    if n < LOOP_WINDOW * 2 {
        return None;
    }
    let window: String = chars[n - LOOP_WINDOW..].iter().collect();
    // The earliest occurrence; it may overlap the end when the repeated part is short.
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

/// Longest dangling end (bytes) dropped from a cut-off text.
const DANGLING: usize = 400;

/// A cut-off text up to its last finished sentence or paragraph (a dangling start is dropped).
pub fn finished(text: &str) -> String {
    let mut t = text.trim_end();
    // A list item that never got its text («11.», «-»).
    let marker = |l: &str| {
        let l = l.trim();
        matches!(l, "-" | "*" | "•") || (l.len() > 1 && l.ends_with(['.', ')']) && l[..l.len() - 1].chars().all(|c| c.is_ascii_digit()))
    };
    if let Some(last) = t.rsplit('\n').next().filter(|l| marker(l)) {
        t = t[..t.len() - last.len()].trim_end();
    }
    if t.ends_with(['.', '!', '?', '…', ')', ']']) {
        return t.to_string();
    }
    let paragraph = t.rfind("\n\n").map(|i| i + 2);
    let sentence = t.rfind(['.', '!', '?', '…']).map(|i| i + t[i..].chars().next().map_or(1, char::len_utf8));
    match paragraph.max(sentence) {
        // A short dangling end goes; a long one is kept rather than lose much.
        Some(end) if t.len() - end <= DANGLING || end > t.len() / 2 => t[..end].trim_end().to_string(),
        _ => t.to_string(),
    }
}

/// A model of the engine as the brain: the same step grammar, streamed as it is written.
pub struct EngineBrain<'a> {
    pub engine: &'a dyn Engine,
    /// The engine's model spec (`owner/repo` or an absolute path).
    pub model: String,
    pub n_ctx: u32,
    pub device: Device,
    pub priority: Priority,
    /// Longest step (tokens).
    pub max_tokens: u32,
    /// The Hugging Face token for downloading the model when it is not on disk yet.
    pub hf_token: Option<String>,
}

impl EngineBrain<'_> {
    fn request(&self, messages: &[Message], max_tokens: u32) -> ChatRequest {
        ChatRequest {
            model: self.model.clone(),
            messages: messages.iter().map(|m| ChatMessage::new(m.role.into(), m.content.clone())).collect(),
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

impl Brain for EngineBrain<'_> {
    fn step(&self, messages: &[Message], tools: &[ToolSpec], may_answer: bool, cancel: &AtomicBool, on_answer: &mut dyn FnMut(&str)) -> anyhow::Result<Step> {
        let request = ChatRequest { json_schema: Some(step_schema(tools, may_answer)), no_repeats: true, ..self.request(messages, self.max_tokens) };
        let mut stream = AnswerStream::default();
        let mut answer = String::new();
        let mut looped = None;
        let generated = self.engine.chat(&request, cancel, &mut |piece| {
            let new = stream.push(piece);
            if new.is_empty() {
                return true;
            }
            answer.push_str(&new);
            if let Some(cut) = loop_start(&answer) {
                looped = Some(cut);
                return false;
            }
            on_answer(&new);
            true
        })?;
        match (parse_step(&generated.content), looped) {
            (Ok(step), None) => Ok(step),
            // Stopped in a loop, or cut off at the token limit: the answer written so far.
            (_, cut) if !answer.trim().is_empty() => {
                if cut.is_some() {
                    log::info!("the model went round in a loop; the answer is cut where it began");
                }
                Ok(Step::Answer { thought: String::new(), text: finished(&answer[..cut.unwrap_or(answer.len())]) })
            }
            (result, _) => result,
        }
    }

    fn complete(&self, messages: &[Message], cancel: &AtomicBool) -> anyhow::Result<String> {
        anyhow::ensure!(!cancel.load(std::sync::atomic::Ordering::Relaxed), "cancelled");
        let request = self.request(messages, self.max_tokens);
        Ok(self.engine.chat(&request, cancel, &mut |_| true)?.content.trim().to_string())
    }

    fn context(&self) -> u32 {
        self.n_ctx
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &'static str) -> ToolSpec {
        ToolSpec { name, description: String::new(), params: json!({"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}), example: json!({"q": "x"}) }
    }

    #[test]
    fn schema_is_a_choice_of_calls_and_the_answer() {
        let s = step_schema(&[spec("search"), spec("read")], true);
        let options = s["oneOf"].as_array().unwrap();
        assert_eq!(options.len(), 3);
        assert_eq!(options[1]["properties"]["action"]["const"], "search");
        assert!(options[0]["properties"]["answer"].is_object(), "the answer first");
        // Without tools: the answer only.
        assert_eq!(step_schema(&[], false)["required"], json!(["thought", "action", "answer"]));
        // Before anything was looked up: calls only.
        assert_eq!(step_schema(&[spec("search"), spec("read")], false)["oneOf"].as_array().unwrap().len(), 2);
        assert_eq!(step_schema(&[spec("search")], false)["properties"]["action"]["const"], "search");
    }

    /// Feeds `raw` in pieces of `n` characters; returns what was streamed.
    fn streamed(raw: &str, n: usize) -> String {
        let chars: Vec<char> = raw.chars().collect();
        let mut s = AnswerStream::default();
        chars.chunks(n).map(|c| s.push(&c.iter().collect::<String>())).collect()
    }

    #[test]
    fn the_answer_streams_out_of_the_json() {
        let raw = r#"{"thought":"скажу \"да\"","action":"answer","answer":"Да, \"дуб\" [0:10].\nВторая строка é и \\ всё."}"#;
        let want = "Да, \"дуб\" [0:10].\nВторая строка é и \\ всё.";
        for n in [1, 2, 3, 7, 1000] {
            assert_eq!(streamed(raw, n), want, "pieces of {n}");
        }
        // With spaces the grammar allows; a tool step streams nothing.
        assert_eq!(streamed(r#"{ "thought" : "", "action" : "answer" , "answer" : "Нет." }"#, 4), "Нет.");
        assert_eq!(streamed(r#"{"thought":"","action":"search","title":"answer","args":{"query":"answer"}}"#, 3), "");
    }

    #[test]
    fn loops_are_noticed_and_cut_where_they_began() {
        let intro = "Документы для подтверждения опыта: договор, счета и акты выполненных работ. ";
        let block = "**Примечание:** видео не содержит прямых ссылок на конкретные документы, кроме общих категорий [1:40].\n\n";
        assert_eq!(loop_start(&format!("{intro}{block}")), None, "no repeat");
        assert_eq!(loop_start("коротко"), None);
        // One copy and part of a second: cut at the second copy.
        let text = format!("{intro}{block}{block}{}", &block[..block.char_indices().nth(60).unwrap().0]);
        let cut = loop_start(&text).unwrap();
        assert_eq!(&text[..cut], format!("{intro}{block}"));
        // A loop with a little drift before it: still one copy kept.
        let drift = format!("{intro}{block}Также:{block}{block}{block}");
        let cut = loop_start(&drift).unwrap();
        assert!(drift[..cut].matches("**Примечание:**").count() <= 2, "{}", &drift[..cut]);
    }

    #[test]
    fn cut_answers_end_at_a_finished_sentence() {
        assert_eq!(finished("Первое. Второе [1:40].\n\n**Примечание:** Видео не"), "Первое. Второе [1:40].");
        assert_eq!(finished("Один абзац.\n\nВторой абзац без точки"), "Один абзац.");
        assert_eq!(finished("Всё сказано. "), "Всё сказано.");
        assert_eq!(finished("1. Первое.\n2. Второе.\n11."), "1. Первое.\n2. Второе.", "a list item without text");
        assert_eq!(finished("Пункты:\n- один.\n-"), "Пункты:\n- один.");
        let long_tail = format!("Начало. {}", "слово ".repeat(100));
        assert_eq!(finished(&long_tail), long_tail.trim_end(), "too much would be lost");
    }

    #[test]
    fn steps_parse() {
        let t = parse_step(r#"{"thought":"ищу","action":"search","title":"Ищу бюджет","args":{"q":"бюджет"}}"#).unwrap();
        assert_eq!(t, Step::Tool { thought: "ищу".into(), title: "Ищу бюджет".into(), tool: "search".into(), args: json!({"q": "бюджет"}) });
        assert_eq!(parse_step(r#" {"thought":"","action":"answer","answer":" Да. "} "#).unwrap(), Step::Answer { thought: String::new(), text: "Да.".into() });
        assert!(parse_step("не json").is_err());
        assert!(parse_step(r#"{"thought":"x"}"#).is_err());
    }
}
