use crate::engine::EngineError;
use crate::hub;
use crate::llm;
use mediaa_core::agent::ModelSource;
use mediaa_core::transcript::Turn;
use std::time::Duration;

pub const DEFAULT_KEEP_ALIVE: Duration = Duration::from_secs(5 * 60);

pub use mediaa_core::agent::Device;

pub const DEFAULT_CHAT_N_CTX: u32 = 8192;
pub const DEFAULT_EMBED_N_CTX: u32 = 2048;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EmbedKind {
    #[default]
    Query,
    Passage,
}

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

#[derive(Debug, Clone)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub json_schema: Option<serde_json::Value>,
    pub no_repeats: bool,
    pub n_ctx: Option<u32>,
    pub device: Option<Device>,
    pub priority: Option<Priority>,
    pub hf_token: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Thinking {
    pub budget: u32,
}

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

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EmbeddingsRequest {
    pub model: String,
    pub input: Vec<String>,
    pub kind: EmbedKind,
    pub n_ctx: Option<u32>,
    pub device: Option<Device>,
    pub hf_token: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RerankRequest {
    pub model: String,
    pub query: String,
    pub documents: Vec<String>,
    pub n_ctx: Option<u32>,
    pub device: Option<Device>,
    pub hf_token: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RerankResult {
    pub index: usize,
    pub relevance_score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeechSegment {
    pub start: f64,
    pub samples: Vec<f32>,
}

pub trait VadSession: Send {
    fn push(&mut self, samples: &[f32]) -> Vec<SpeechSegment>;
    fn finish(&mut self) -> Vec<SpeechSegment>;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiarizeRequest {
    pub segmentation: String,
    pub embedding: String,
    pub speakers: Option<u32>,
    pub clustering_threshold: Option<f32>,
    pub merge_threshold: Option<f32>,
    pub min_speaker_seconds: Option<f64>,
    pub hf_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    pub embedding: Vec<f32>,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diarization {
    pub turns: Vec<Turn>,
    pub voices: Vec<Voice>,
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedModel {
    pub model: String,
    pub pool: Pool,
    pub bytes: u64,
    pub users: u32,
    pub idle: Duration,
    pub unloads_in: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pool {
    Gpu,
    Cpu,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WaitingModel {
    pub id: u64,
    pub model: Option<String>,
    pub reason: String,
    pub progress: Option<Progress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    pub gpu_bytes: u64,
    pub ram_bytes: u64,
    pub keep_alive: Duration,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            gpu_bytes: 0,
            ram_bytes: 0,
            keep_alive: DEFAULT_KEEP_ALIVE,
        }
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub loaded: Vec<LoadedModel>,
    pub waiting: Vec<WaitingModel>,
    pub budget: Budget,
    pub used: Used,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelFile {
    pub name: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ModelFormat {
    Gguf,
    SherpaOnnx,
    Onnx,
    #[default]
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    pub spec: String,
    pub path: String,
    pub bytes: u64,
    pub format: ModelFormat,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInspect {
    pub spec: String,
    pub files: Vec<ModelFile>,
    pub format: ModelFormat,
    pub download_bytes: u64,
    pub roles: Vec<String>,
    pub problems: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelCheck {
    pub ok: bool,
    pub bytes: u64,
}

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
    pub(crate) fn into_call(self) -> Result<ChatCall, EngineError> {
        let source = parse_spec(&self.model)?;
        if self.messages.is_empty() {
            return Err(EngineError::bad_request("messages must not be empty"));
        }
        if self.max_tokens == 0 {
            return Err(EngineError::bad_request(
                "max_tokens must be greater than 0",
            ));
        }
        if !(0.0..=100.0).contains(&self.temperature) {
            return Err(EngineError::bad_request(
                "temperature must be between 0 and 100",
            ));
        }
        let json_schema = match &self.json_schema {
            None => None,
            Some(schema) => {
                let text = serde_json::to_string(schema).expect("a JSON value serialises");
                llm::check_schema(&text).map_err(|e| {
                    EngineError::bad_request(format!("json_schema is not supported: {e:#}"))
                })?;
                Some(text)
            }
        };
        let messages = self
            .messages
            .into_iter()
            .map(|message| llm::Message {
                role: message.role.into(),
                content: message.content,
            })
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

pub fn parse_spec(spec: &str) -> Result<ModelSource, EngineError> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(EngineError::bad_request("model is required"));
    }
    let path = std::path::Path::new(spec);
    if path.is_absolute() || spec.starts_with('/') || spec.starts_with('\\') {
        return Ok(ModelSource::Local {
            path: path.to_path_buf(),
        });
    }
    let (repo, _) = hub::split_spec(spec);
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() == 2 && parts.iter().all(|p| !p.is_empty()) {
        return Ok(ModelSource::HuggingFace {
            repo: spec.to_string(),
        });
    }
    Err(EngineError::bad_request(format!(
        "\"{spec}\" is neither an absolute path nor owner/repo"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_comes_from_the_settings_in_bytes() {
        let settings = mediaa_core::agent::Budget {
            gpu_memory_gb: 2.0,
            ram_memory_gb: 0.5,
            ..Default::default()
        };
        assert_eq!(
            Budget::from(settings.clone()),
            Budget {
                gpu_bytes: 2 << 30,
                ram_bytes: 1 << 29,
                keep_alive: DEFAULT_KEEP_ALIVE
            }
        );
        let custom = mediaa_core::agent::Budget {
            keep_alive_secs: 12,
            ..settings
        };
        assert_eq!(Budget::from(custom).keep_alive, Duration::from_secs(12));
        assert_eq!(Budget::from(settings).ram_bytes, 536_870_912);
        assert_eq!(Budget::default().keep_alive, DEFAULT_KEEP_ALIVE);
    }

    #[test]
    fn specs_are_paths_or_hugging_face_repositories() {
        assert!(
            matches!(parse_spec("a/b"), Ok(ModelSource::HuggingFace { repo }) if repo == "a/b")
        );
        assert!(matches!(
            parse_spec("a/b/dir/m.gguf"),
            Ok(ModelSource::HuggingFace { .. })
        ));
        assert!(matches!(
            parse_spec("/m/models/m.gguf"),
            Ok(ModelSource::Local { .. })
        ));
        assert!(matches!(
            parse_spec(r"\m\models\m.gguf"),
            Ok(ModelSource::Local { .. })
        ));
        #[cfg(windows)]
        assert!(matches!(
            parse_spec(r"C:\m\models\m.gguf"),
            Ok(ModelSource::Local { .. })
        ));
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
        assert_eq!(
            (call.generate.max_tokens, call.generate.temperature),
            (10, 0.5)
        );
        assert!(call.generate.no_repeats);
        assert_eq!(
            call.generate.json_schema.as_deref(),
            Some(r#"{"type":"object"}"#)
        );
        assert!(!call.generate.enable_thinking);
        assert_eq!(
            (call.n_ctx, call.device, call.interactive),
            (1000, Device::Cpu, false)
        );
        assert_eq!(call.hf_token.as_deref(), Some("token"));
    }

    #[test]
    fn a_plain_chat_request_has_no_dry_penalty() {
        let request = ChatRequest {
            model: "a/b".into(),
            messages: vec![ChatMessage::user("u")],
            no_repeats: false,
            ..ChatRequest::default()
        };
        let call = request.into_call().unwrap();
        assert!(!call.generate.no_repeats);
        assert_eq!(call.generate.max_tokens, 512);
        assert_eq!(call.n_ctx, DEFAULT_CHAT_N_CTX);
        assert_eq!(call.device, Device::Auto);
        assert!(
            call.interactive,
            "a request without a priority is interactive"
        );
        assert!(
            call.generate.enable_thinking,
            "thinking stays available unless the call turns it off"
        );
    }

    #[test]
    fn bad_chat_requests_are_refused() {
        let no_messages = ChatRequest {
            model: "a/b".into(),
            ..ChatRequest::default()
        };
        assert_eq!(no_messages.into_call().unwrap_err().kind, "bad_request");
        let no_tokens = ChatRequest {
            model: "a/b".into(),
            messages: vec![ChatMessage::user("u")],
            max_tokens: 0,
            ..ChatRequest::default()
        };
        assert_eq!(no_tokens.into_call().unwrap_err().kind, "bad_request");
        let hot = ChatRequest {
            model: "a/b".into(),
            messages: vec![ChatMessage::user("u")],
            temperature: 1000.0,
            ..ChatRequest::default()
        };
        assert_eq!(hot.into_call().unwrap_err().kind, "bad_request");
        assert_eq!(
            EmbeddingsRequest {
                model: "a/b".into(),
                input: Vec::new(),
                ..EmbeddingsRequest::default()
            }
            .into_call()
            .unwrap_err()
            .kind,
            "bad_request"
        );
    }
}
