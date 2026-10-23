//! The engine's requests and answers: plain in-memory types, no transport in them.

use crate::engine::EngineError;
use crate::hub;
use crate::llm;
use mediaa_core::agent::ModelSource;
use mediaa_core::transcript::Turn;
use std::time::Duration;

/// How long an idle model stays loaded when a budget does not say. Zero unloads it at once.
pub const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(5 * 60);

pub use mediaa_core::agent::Device;

/// Context of a chat request when the caller does not say.
pub const DEFAULT_CHAT_N_CTX: u32 = 8192;
/// Embedding models see up to 2048 tokens (`embed::MAX_TOKENS`), no chat context is involved.
pub const DEFAULT_EMBED_N_CTX: u32 = 2048;

/// Whether an embedding is of a search query or of a passage of text (the engine adds the
/// model's prefixes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EmbedKind {
    #[default]
    Query,
    Passage,
}

/// How urgent a request is (the engine lets interactive work go before background work).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    Interactive,
    Background,
}

impl Priority {
    pub fn interactive(self) -> bool {
        matches!(self, Priority::Interactive)
    }
}

/// A message of a chat: a role and its text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

impl ChatMessage {
    pub fn new(role: ChatRole, content: impl Into<String>) -> ChatMessage {
        ChatMessage { role, content: content.into() }
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

/// A chat completion request. `model` is a model spec: `owner/repo`, `owner/repo/path/file` or
/// an absolute path. A Hugging Face model the engine does not have yet is downloaded with
/// `hf_token`.
#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    /// 0: greedy (deterministic).
    pub temperature: f32,
    /// The answer must be JSON following this JSON Schema.
    pub json_schema: Option<serde_json::Value>,
    /// Penalise repeating what was already written (DRY): small models fall into loops.
    pub no_repeats: bool,
    /// Context window to load the model with (`None`: the engine's default).
    pub n_ctx: Option<u32>,
    pub device: Option<Device>,
    pub priority: Option<Priority>,
    /// The Hugging Face token for downloading a model that is not on disk yet.
    pub hf_token: Option<String>,
    /// When false, the model must not reason first (`enable_thinking=false`). Short service
    /// calls (a chat title) set this; a request that does not say leaves thinking to the model.
    pub enable_thinking: bool,
}

impl Default for ChatRequest {
    fn default() -> Self {
        ChatRequest {
            model: String::new(),
            messages: Vec::new(),
            max_tokens: 512,
            temperature: 0.0,
            json_schema: None,
            no_repeats: true,
            n_ctx: None,
            device: None,
            priority: None,
            hf_token: None,
            enable_thinking: true,
        }
    }
}

/// A chat answer that starts with reasoning (local models): the model thinks inside
/// `<think>…</think>` for at most `budget` tokens and only then writes the answer, which the
/// request's `json_schema` constrains. The reasoning stays in the streamed text and in
/// [`ChatAnswer::content`] (split it off with `llm::ThinkSplitter` / `llm::split_thinking`);
/// `max_tokens` counts what follows the reasoning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thinking {
    pub budget: u32,
}

/// A finished answer: its text and the token usage.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatAnswer {
    pub content: String,
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

/// An embedding request: the model spec, the texts and the load options.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbeddingsRequest {
    pub model: String,
    pub input: Vec<String>,
    pub kind: EmbedKind,
    pub n_ctx: Option<u32>,
    pub device: Option<Device>,
    pub hf_token: Option<String>,
}

/// A rerank request: the model spec, the query and the documents.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RerankRequest {
    pub model: String,
    pub query: String,
    pub documents: Vec<String>,
    pub n_ctx: Option<u32>,
    pub device: Option<Device>,
    pub hf_token: Option<String>,
}

/// How relevant one of the documents is and its position in the request.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerankResult {
    pub index: usize,
    pub relevance_score: f32,
}

/// A piece of speech found by the voice activity detector: where it starts (seconds into the
/// recording) and its samples (16 kHz mono).
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSegment {
    pub start: f64,
    pub samples: Vec<f32>,
}

/// An open voice activity session: it keeps the detector's state between pushed chunks.
pub trait VadSession: Send {
    fn push(&mut self, samples: &[f32]) -> Vec<SpeechSegment>;
    fn finish(&mut self) -> Vec<SpeechSegment>;
}

/// The models diarization needs; `speakers` is how many speakers there are when known.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiarizeRequest {
    pub segmentation: String,
    pub embedding: String,
    pub speakers: Option<u32>,
    /// Cut height of the clustering; `None` uses [`crate::speech::DEFAULT_CLUSTERING_THRESHOLD`].
    /// A larger value gives fewer speakers.
    pub clustering_threshold: Option<f32>,
    /// Cosine similarity above which two speakers are merged by their mean voice; `None` keeps
    /// the clusters as found.
    pub merge_threshold: Option<f32>,
    /// A speaker whose speech is shorter than this is moved to the nearest voice by similarity;
    /// `None` uses [`crate::speech::MIN_SPEAKER_SECONDS`].
    pub min_speaker_seconds: Option<f64>,
    pub hf_token: Option<String>,
}

/// The mean voice of one found speaker: the normalised speaker embeddings of their speech,
/// averaged, and how long they talk.
#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    /// Mean embedding; empty when the extractor could not make one (too little speech).
    pub embedding: Vec<f32>,
    /// Seconds of speech.
    pub seconds: f64,
}

/// Who speaks when, and the voice of each speaker: `voices` is indexed by `Turn::speaker`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diarization {
    pub turns: Vec<Turn>,
    pub voices: Vec<Voice>,
}

/// What the engine is and which GPUs it sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Health {
    pub name: String,
    pub version: String,
    pub gpus: Vec<Gpu>,
    pub models_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Gpu {
    pub name: String,
    pub backend: String,
    pub memory_total: u64,
    pub memory_free: u64,
}

/// A model the engine keeps in memory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    /// The model as the caller asked for it: a Hugging Face `owner/repo` or an absolute path.
    pub model: String,
    pub pool: Pool,
    /// Memory the loaded model takes (weights and context).
    pub bytes: u64,
    pub users: u32,
    /// How long the model has been unused. Zero while a job uses it.
    pub idle: Duration,
    /// How soon an idle model is unloaded. `None` while a job uses it.
    pub unloads_in: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Gpu,
    Cpu,
}

/// How far a download or a diarization is: `done` of `total` (bytes for downloads, pieces for
/// diarization).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

/// A request waiting for memory, a slot or its model's download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingModel {
    pub id: u64,
    pub model: Option<String>,
    pub reason: String,
    /// The download's progress, when the reason is `downloading`.
    pub progress: Option<Progress>,
}

/// Memory the engine may use for models, and how long an idle model stays loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub gpu_bytes: u64,
    pub ram_bytes: u64,
    /// How long an idle model stays loaded. Zero unloads it as soon as its last job finishes.
    pub keep_alive: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self { gpu_bytes: 0, ram_bytes: 0, keep_alive: DEFAULT_KEEP_ALIVE }
    }
}

impl From<mediaa_core::agent::Budget> for Budget {
    fn from(budget: mediaa_core::agent::Budget) -> Budget {
        Budget {
            gpu_bytes: budget.gpu_bytes(),
            ram_bytes: budget.ram_bytes(),
            keep_alive: Duration::from_secs(budget.keep_alive_secs),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Used {
    pub gpu_bytes: u64,
    pub ram_bytes: u64,
}

/// Loaded models, waiting requests and the memory budget.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub loaded: Vec<LoadedModel>,
    pub waiting: Vec<WaitingModel>,
    pub budget: Budget,
    pub used: Used,
}

/// One file of a model: its path inside the model and its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub name: String,
    pub bytes: u64,
}

/// What a model is: weights llama.cpp runs, a sherpa-onnx folder, a single ONNX model, or
/// something the engine cannot run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelFormat {
    Gguf,
    SherpaOnnx,
    Onnx,
    #[default]
    Other,
}

/// A model downloaded to the engine's models folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub spec: String,
    pub path: String,
    pub bytes: u64,
    pub format: ModelFormat,
    pub files: Vec<ModelFile>,
}

/// A model spec seen without downloading: its files, the format, the roles it fits and what is
/// wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInspect {
    pub spec: String,
    pub files: Vec<ModelFile>,
    pub format: ModelFormat,
    pub download_bytes: u64,
    /// Roles of `mediaa_core::agent::ModelRole`, in snake_case.
    pub roles: Vec<String>,
    pub problems: Vec<String>,
}

/// The result of a trial run of a downloaded model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCheck {
    pub ok: bool,
    pub bytes: u64,
}

/// The library call of a checked chat request.
#[derive(Debug)]
pub(crate) struct ChatCall {
    pub source: ModelSource,
    pub messages: Vec<llm::Message>,
    pub generate: llm::Generate,
    pub n_ctx: u32,
    pub device: Device,
    pub interactive: bool,
    pub hf_token: Option<String>,
}

impl ChatRequest {
    /// Checks the request and turns it into the library call.
    pub(crate) fn into_call(self) -> Result<ChatCall, EngineError> {
        let source = parse_spec(&self.model)?;
        if self.messages.is_empty() {
            return Err(EngineError::bad_request("messages must not be empty"));
        }
        if self.max_tokens == 0 {
            return Err(EngineError::bad_request("max_tokens must be greater than 0"));
        }
        if !(0.0..=100.0).contains(&self.temperature) {
            return Err(EngineError::bad_request("temperature must be between 0 and 100"));
        }
        let json_schema = match &self.json_schema {
            None => None,
            Some(schema) => {
                let text = serde_json::to_string(schema).expect("a JSON value serialises");
                llm::check_schema(&text).map_err(|e| EngineError::bad_request(format!("json_schema is not supported: {e:#}")))?;
                Some(text)
            }
        };
        let messages = self
            .messages
            .into_iter()
            .map(|message| llm::Message { role: message.role.into(), content: message.content })
            .collect();
        Ok(ChatCall {
            source,
            messages,
            generate: llm::Generate {
                max_tokens: self.max_tokens,
                temperature: self.temperature,
                json_schema,
                no_repeats: self.no_repeats,
                think_tokens: None,
                enable_thinking: self.enable_thinking,
            },
            n_ctx: self.n_ctx.unwrap_or(DEFAULT_CHAT_N_CTX),
            device: self.device.unwrap_or_default(),
            interactive: self.priority.is_none_or(Priority::interactive),
            hf_token: self.hf_token,
        })
    }
}

impl From<ChatRole> for llm::Role {
    fn from(role: ChatRole) -> llm::Role {
        match role {
            ChatRole::System => llm::Role::System,
            ChatRole::User => llm::Role::User,
            ChatRole::Assistant => llm::Role::Assistant,
        }
    }
}

/// The library call of a checked embedding request.
#[derive(Debug)]
pub(crate) struct EmbedCall {
    pub source: ModelSource,
    pub texts: Vec<String>,
    pub kind: EmbedKind,
    pub n_ctx: u32,
    pub device: Device,
    pub hf_token: Option<String>,
}

impl EmbeddingsRequest {
    pub(crate) fn into_call(self) -> Result<EmbedCall, EngineError> {
        if self.input.is_empty() {
            return Err(EngineError::bad_request("input must not be empty"));
        }
        Ok(EmbedCall {
            source: parse_spec(&self.model)?,
            texts: self.input,
            kind: self.kind,
            n_ctx: self.n_ctx.unwrap_or(DEFAULT_EMBED_N_CTX),
            device: self.device.unwrap_or_default(),
            hf_token: self.hf_token,
        })
    }
}

/// The library call of a checked rerank request.
pub(crate) struct RerankCall {
    pub source: ModelSource,
    pub query: String,
    pub documents: Vec<String>,
    pub n_ctx: u32,
    pub device: Device,
    pub hf_token: Option<String>,
}

impl RerankRequest {
    pub(crate) fn into_call(self) -> Result<RerankCall, EngineError> {
        Ok(RerankCall {
            source: parse_spec(&self.model)?,
            query: self.query,
            documents: self.documents,
            n_ctx: self.n_ctx.unwrap_or(DEFAULT_EMBED_N_CTX),
            device: self.device.unwrap_or_default(),
            hf_token: self.hf_token,
        })
    }
}

/// A model spec: an absolute path, or `owner/repo` / `owner/repo/path/file`.
pub fn parse_spec(spec: &str) -> Result<ModelSource, EngineError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(EngineError::bad_request("model is required"));
    }
    let path = std::path::Path::new(spec);
    // `/m/...` is absolute on Unix only; accept it everywhere (a model spec is written by hand).
    if path.is_absolute() || spec.starts_with('/') || spec.starts_with('\\') {
        return Ok(ModelSource::Local { path: path.to_path_buf() });
    }
    let (repo, _) = hub::split_spec(spec);
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() == 2 && parts.iter().all(|p| !p.is_empty()) {
        return Ok(ModelSource::HuggingFace { repo: spec.to_string() });
    }
    Err(EngineError::bad_request(format!("\"{spec}\" is neither an absolute path nor owner/repo")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_comes_from_the_settings_in_bytes() {
        let settings = mediaa_core::agent::Budget { gpu_memory_gb: 2.0, ram_memory_gb: 0.5, ..Default::default() };
        assert_eq!(Budget::from(settings.clone()), Budget { gpu_bytes: 2 << 30, ram_bytes: 1 << 29, keep_alive: DEFAULT_KEEP_ALIVE });
        let custom = mediaa_core::agent::Budget { keep_alive_secs: 12, ..settings };
        assert_eq!(Budget::from(custom).keep_alive, Duration::from_secs(12));
        assert_eq!(Budget::from(settings).ram_bytes, 536_870_912);
        assert_eq!(Budget::default().keep_alive, DEFAULT_KEEP_ALIVE);
    }

    #[test]
    fn specs_are_paths_or_hugging_face_repositories() {
        assert!(matches!(parse_spec("a/b"), Ok(ModelSource::HuggingFace { repo }) if repo == "a/b"));
        assert!(matches!(parse_spec("a/b/dir/m.gguf"), Ok(ModelSource::HuggingFace { .. })));
        assert!(matches!(parse_spec("/m/models/m.gguf"), Ok(ModelSource::Local { .. })));
        assert!(matches!(parse_spec(r"\m\models\m.gguf"), Ok(ModelSource::Local { .. })));
        #[cfg(windows)]
        assert!(matches!(parse_spec(r"C:\m\models\m.gguf"), Ok(ModelSource::Local { .. })));
        assert_eq!(parse_spec("oneword").unwrap_err().kind, "bad_request");
        assert_eq!(parse_spec("").unwrap_err().kind, "bad_request");
    }

    #[test]
    fn a_chat_request_maps_to_the_library_call() {
        let request = ChatRequest {
            model: "a/b".into(),
            messages: vec![ChatMessage::system("s"), ChatMessage::user("u")],
            max_tokens: 10,
            temperature: 0.5,
            json_schema: Some(serde_json::json!({"type": "object"})),
            no_repeats: true,
            n_ctx: Some(1000),
            device: Some(Device::Cpu),
            priority: Some(Priority::Background),
            hf_token: Some("token".into()),
            enable_thinking: false,
        };
        let call = request.into_call().unwrap();
        assert_eq!(call.messages.len(), 2);
        assert_eq!(call.messages[0].role, llm::Role::System);
        assert_eq!((call.generate.max_tokens, call.generate.temperature), (10, 0.5));
        assert!(call.generate.no_repeats);
        assert_eq!(call.generate.json_schema.as_deref(), Some(r#"{"type":"object"}"#));
        assert!(!call.generate.enable_thinking);
        assert_eq!((call.n_ctx, call.device, call.interactive), (1000, Device::Cpu, false));
        assert_eq!(call.hf_token.as_deref(), Some("token"));
    }

    #[test]
    fn a_plain_chat_request_has_no_dry_penalty() {
        let request = ChatRequest { model: "a/b".into(), messages: vec![ChatMessage::user("u")], no_repeats: false, ..ChatRequest::default() };
        let call = request.into_call().unwrap();
        assert!(!call.generate.no_repeats);
        assert_eq!(call.generate.max_tokens, 512);
        assert_eq!(call.n_ctx, DEFAULT_CHAT_N_CTX);
        assert_eq!(call.device, Device::Auto);
        assert!(call.interactive, "a request without a priority is interactive");
        assert!(call.generate.enable_thinking, "thinking stays available unless the call turns it off");
    }

    #[test]
    fn bad_chat_requests_are_refused() {
        let no_messages = ChatRequest { model: "a/b".into(), ..ChatRequest::default() };
        assert_eq!(no_messages.into_call().unwrap_err().kind, "bad_request");
        let no_tokens = ChatRequest { model: "a/b".into(), messages: vec![ChatMessage::user("u")], max_tokens: 0, ..ChatRequest::default() };
        assert_eq!(no_tokens.into_call().unwrap_err().kind, "bad_request");
        let hot = ChatRequest { model: "a/b".into(), messages: vec![ChatMessage::user("u")], temperature: 1000.0, ..ChatRequest::default() };
        assert_eq!(hot.into_call().unwrap_err().kind, "bad_request");
        assert_eq!(
            EmbeddingsRequest { model: "a/b".into(), input: Vec::new(), ..EmbeddingsRequest::default() }.into_call().unwrap_err().kind,
            "bad_request"
        );
    }
}
