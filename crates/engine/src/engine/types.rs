use mediaa_core::agent::ModelSource;
use mediaa_core::agent::ReasoningDepth;
use mediaa_core::transcript::{Phrase, Turn};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbedKind {
    #[default]
    Query,
    Document,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: ChatRole, content: impl Into<String>) -> ChatMessage {
        ChatMessage {
            role,
            content: content.into(),
        }
    }

    pub fn system(content: impl Into<String>) -> ChatMessage {
        ChatMessage::new(ChatRole::System, content)
    }

    pub fn user(content: impl Into<String>) -> ChatMessage {
        ChatMessage::new(ChatRole::User, content)
    }

    pub fn assistant(content: impl Into<String>) -> ChatMessage {
        ChatMessage::new(ChatRole::Assistant, content)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

impl ChatRole {
    pub fn as_str(self) -> &'static str {
        match self {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolChoice {
    #[default]
    Auto,
    Required,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thinking {
    pub depth: ReasoningDepth,
}

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub source: ModelSource,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub json_schema: Option<serde_json::Value>,
    pub thinking: Option<Thinking>,
    pub tools: Vec<ToolSpec>,
    pub tool_choice: ToolChoice,
}

impl Default for ChatRequest {
    fn default() -> Self {
        ChatRequest {
            source: ModelSource::new("", ""),
            messages: Vec::new(),
            max_tokens: 512,
            temperature: 0.0,
            json_schema: None,
            thinking: None,
            tools: Vec::new(),
            tool_choice: ToolChoice::Auto,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAnswer {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCall>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

impl Usage {
    pub fn total_tokens(self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EmbeddingsRequest {
    pub source: ModelSource,
    pub input: Vec<String>,
    pub kind: EmbedKind,
}

impl Default for EmbeddingsRequest {
    fn default() -> Self {
        Self {
            source: ModelSource::new("", ""),
            input: Vec::new(),
            kind: EmbedKind::Query,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RerankRequest {
    pub source: ModelSource,
    pub query: String,
    pub documents: Vec<String>,
}

impl Default for RerankRequest {
    fn default() -> Self {
        Self {
            source: ModelSource::new("", ""),
            query: String::new(),
            documents: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerankResult {
    pub index: usize,
    pub relevance_score: f32,
}

#[derive(Debug, Clone)]
pub struct TranscribeRequest {
    pub source: ModelSource,
    pub path: PathBuf,
    pub from: f64,
    pub to: f64,
    pub language: Option<String>,
    pub word_times: bool,
    pub speakers: bool,
    pub voiceprint: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SpeechBody {
    pub phrases: Vec<Phrase>,
    pub turns: Vec<Turn>,
    pub voices: Vec<Vec<f32>>,
}

#[derive(Debug, Clone)]
pub struct DiarizeRequest {
    pub source: ModelSource,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct VoiceRequest {
    pub source: ModelSource,
    pub clips: Vec<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCheck {
    pub ok: bool,
    pub detail: String,
}

pub fn embed_prefix(model: &str, kind: EmbedKind) -> &'static str {
    let name = model.to_ascii_lowercase();
    if name.contains("gemma") && name.contains("embedding") {
        match kind {
            EmbedKind::Query => "task: search result | query: ",
            EmbedKind::Document => "title: none | text: ",
        }
    } else if name.contains("qwen3") && name.contains("embedding") {
        match kind {
            EmbedKind::Query => {
                "Instruct: Given a question, find the parts of a conversation that answer it\nQuery: "
            }
            EmbedKind::Document => "",
        }
    } else if name.contains("e5") {
        match kind {
            EmbedKind::Query => "query: ",
            EmbedKind::Document => "passage: ",
        }
    } else {
        ""
    }
}

pub fn apply_embed_prefix(model: &str, kind: EmbedKind, input: &[String]) -> Vec<String> {
    let prefix = embed_prefix(model, kind);
    input.iter().map(|text| format!("{prefix}{text}")).collect()
}

pub(crate) fn shift_phrases(phrases: Vec<Phrase>, offset: f64) -> Vec<Phrase> {
    phrases
        .into_iter()
        .map(|mut phrase| {
            phrase.start += offset;
            phrase.end += offset;
            for word in &mut phrase.words {
                word.start += offset;
                word.end += offset;
            }
            phrase
        })
        .collect()
}

pub(crate) fn phrases_from_words(words: Vec<mediaa_core::transcript::Word>) -> Vec<Phrase> {
    if words.is_empty() {
        return Vec::new();
    }
    vec![Phrase {
        start: words[0].start,
        end: words.last().map(|word| word.end).unwrap_or(0.0),
        words,
    }]
}
