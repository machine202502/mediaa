use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
pub const OPENAI_URL: &str = "https://api.openai.com/v1";
pub const DEEPSEEK_URL: &str = "https://api.deepseek.com";
pub const HUGGING_FACE_ROUTER_URL: &str = "https://router.huggingface.co/v1";
pub const OLLAMA_URL: &str = "http://localhost:11434";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    OpenRouter,
    OpenAi,
    DeepSeek,
    HuggingFace,
    Ollama,
    OpenAiCompatible,
    Command,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Chat,
    Embeddings,
    Rerank,
    Transcription,
    Diarization,
    VoiceEmbedding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    WhisperJson,
    OpenAiJson,
    Srt,
    Vtt,
    Rttm,
    Vectors,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub output: OutputFormat,
    #[serde(default)]
    pub whole_file: bool,
}

fn default_context() -> u32 {
    32_768
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: String,
    pub name: String,
    pub kind: ConnectionKind,
    #[serde(default)]
    pub base_url: String,
    #[serde(skip)]
    pub api_key: String,
    #[serde(default)]
    pub proxy: String,
    #[serde(default = "default_context")]
    pub context: u32,
    #[serde(default)]
    pub command: Option<CommandSpec>,
    #[serde(default)]
    pub program: String,
}

impl Connection {
    pub fn new(kind: ConnectionKind, name: &str) -> Connection {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        Connection {
            id: format!("c{nanos:x}"),
            name: name.to_string(),
            kind,
            base_url: match kind {
                ConnectionKind::Ollama => OLLAMA_URL.into(),
                ConnectionKind::OpenAiCompatible => "http://localhost:1234/v1".into(),
                _ => String::new(),
            },
            api_key: String::new(),
            proxy: String::new(),
            context: default_context(),
            command: None,
            program: String::new(),
        }
    }

    pub fn api_url(&self) -> String {
        match self.kind {
            ConnectionKind::OpenRouter => OPENROUTER_URL.into(),
            ConnectionKind::OpenAi => OPENAI_URL.into(),
            ConnectionKind::DeepSeek => DEEPSEEK_URL.into(),
            ConnectionKind::HuggingFace => HUGGING_FACE_ROUTER_URL.into(),
            ConnectionKind::Ollama => format!("{}/v1", trim_url(&self.base_url, OLLAMA_URL)),
            ConnectionKind::OpenAiCompatible => {
                trim_url(&self.base_url, "http://localhost:1234/v1")
            }
            ConnectionKind::Command => String::new(),
        }
    }

    pub fn root_url(&self) -> String {
        match self.kind {
            ConnectionKind::Ollama => trim_url(&self.base_url, OLLAMA_URL),
            _ => self.api_url(),
        }
    }

    pub fn proxy(&self) -> Option<&str> {
        Some(self.proxy.trim()).filter(|p| !p.is_empty())
    }

    pub fn capabilities(&self) -> Vec<Capability> {
        match self.kind {
            ConnectionKind::OpenRouter => vec![Capability::Chat, Capability::Embeddings],
            ConnectionKind::OpenAi => {
                vec![
                    Capability::Chat,
                    Capability::Embeddings,
                    Capability::Transcription,
                ]
            }
            ConnectionKind::DeepSeek | ConnectionKind::HuggingFace => vec![Capability::Chat],
            ConnectionKind::Ollama => vec![Capability::Chat, Capability::Embeddings],
            ConnectionKind::OpenAiCompatible => vec![
                Capability::Chat,
                Capability::Embeddings,
                Capability::Rerank,
                Capability::Transcription,
            ],
            ConnectionKind::Command => match self.command.as_ref().map(|spec| spec.output) {
                Some(OutputFormat::Rttm) => vec![Capability::Diarization],
                Some(OutputFormat::Vectors) => vec![Capability::VoiceEmbedding],
                Some(
                    OutputFormat::WhisperJson
                    | OutputFormat::OpenAiJson
                    | OutputFormat::Srt
                    | OutputFormat::Vtt,
                ) => vec![Capability::Transcription],
                None => Vec::new(),
            },
        }
    }

    pub fn supports(&self, capability: Capability) -> bool {
        self.capabilities().contains(&capability)
    }
}

fn trim_url(value: &str, fallback: &str) -> String {
    let trimmed = value.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        fallback.trim_end_matches('/').to_string()
    } else {
        trimmed.to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSource {
    pub connection: String,
    pub model: String,
}

impl ModelSource {
    pub fn new(connection: impl Into<String>, model: impl Into<String>) -> ModelSource {
        ModelSource {
            connection: connection.into(),
            model: model.into(),
        }
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.connection, self.model)
    }

    pub fn supports_thinking(&self) -> bool {
        let name = self.model.to_lowercase();
        name.contains("qwen3") || name.contains("deepseek") || name.contains("reason")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    Embedding,
    Reranker,
    Agent,
    Titles,
    SpeechRecognition,
    Diarization,
    VoiceEmbedding,
}

impl ModelRole {
    pub const ALL: [ModelRole; 7] = [
        ModelRole::Embedding,
        ModelRole::Reranker,
        ModelRole::Agent,
        ModelRole::Titles,
        ModelRole::SpeechRecognition,
        ModelRole::Diarization,
        ModelRole::VoiceEmbedding,
    ];

    pub fn required(self) -> bool {
        matches!(
            self,
            ModelRole::Embedding
                | ModelRole::Agent
                | ModelRole::Titles
                | ModelRole::SpeechRecognition
        )
    }

    pub fn capability(self) -> Capability {
        match self {
            ModelRole::Embedding => Capability::Embeddings,
            ModelRole::Reranker => Capability::Rerank,
            ModelRole::Agent | ModelRole::Titles => Capability::Chat,
            ModelRole::SpeechRecognition => Capability::Transcription,
            ModelRole::Diarization => Capability::Diarization,
            ModelRole::VoiceEmbedding => Capability::VoiceEmbedding,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelChoices {
    pub embedding: Option<ModelSource>,
    pub reranker: Option<ModelSource>,
    pub agent: Option<ModelSource>,
    pub titles: Option<ModelSource>,
    pub speech_recognition: Option<ModelSource>,
    pub diarization: Option<ModelSource>,
    pub voice_embedding: Option<ModelSource>,
}

impl ModelChoices {
    pub fn get(&self, role: ModelRole) -> &Option<ModelSource> {
        match role {
            ModelRole::Embedding => &self.embedding,
            ModelRole::Reranker => &self.reranker,
            ModelRole::Agent => &self.agent,
            ModelRole::Titles => &self.titles,
            ModelRole::SpeechRecognition => &self.speech_recognition,
            ModelRole::Diarization => &self.diarization,
            ModelRole::VoiceEmbedding => &self.voice_embedding,
        }
    }

    pub fn get_mut(&mut self, role: ModelRole) -> &mut Option<ModelSource> {
        match role {
            ModelRole::Embedding => &mut self.embedding,
            ModelRole::Reranker => &mut self.reranker,
            ModelRole::Agent => &mut self.agent,
            ModelRole::Titles => &mut self.titles,
            ModelRole::SpeechRecognition => &mut self.speech_recognition,
            ModelRole::Diarization => &mut self.diarization,
            ModelRole::VoiceEmbedding => &mut self.voice_embedding,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkKind {
    Agent,
    Transcription,
    Indexing,
    WebSearch,
    ProviderCalls,
}

impl WorkKind {
    pub const ALL: [WorkKind; 5] = [
        WorkKind::Agent,
        WorkKind::Transcription,
        WorkKind::Indexing,
        WorkKind::WebSearch,
        WorkKind::ProviderCalls,
    ];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    pub agent: u32,
    pub transcription: u32,
    pub indexing: u32,
    pub web_search: u32,
    pub provider_calls: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            agent: 2,
            transcription: 1,
            indexing: 1,
            web_search: 2,
            provider_calls: 4,
        }
    }
}

impl Budget {
    pub fn limit(&self, kind: WorkKind) -> u32 {
        match kind {
            WorkKind::Agent => self.agent,
            WorkKind::Transcription => self.transcription,
            WorkKind::Indexing => self.indexing,
            WorkKind::WebSearch => self.web_search,
            WorkKind::ProviderCalls => self.provider_calls,
        }
        .max(1)
    }

    pub fn limit_mut(&mut self, kind: WorkKind) -> &mut u32 {
        match kind {
            WorkKind::Agent => &mut self.agent,
            WorkKind::Transcription => &mut self.transcription,
            WorkKind::Indexing => &mut self.indexing,
            WorkKind::WebSearch => &mut self.web_search,
            WorkKind::ProviderCalls => &mut self.provider_calls,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisOptions {
    pub diarize: bool,
    pub speakers: Option<u32>,
    pub index: bool,
    pub topics: bool,
    pub details: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        AnalysisOptions {
            diarize: true,
            speakers: None,
            index: true,
            topics: true,
            details: true,
        }
    }
}

impl AnalysisOptions {
    pub fn resolved(mut self, models: &ModelChoices) -> AnalysisOptions {
        if models.diarization.is_none() {
            self.diarize = false;
        }
        self.enforced()
    }

    pub fn enforced(mut self) -> AnalysisOptions {
        if !self.index {
            self.topics = false;
        }
        if !self.topics {
            self.details = false;
        }
        if !self.diarize {
            self.speakers = None;
        }
        self
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentSettings {
    pub connections: Vec<Connection>,
    pub models: ModelChoices,
    pub budget: Budget,
    pub agent_context: u32,
    pub web: WebSettings,
    pub analysis: AnalysisOptions,
    pub thinking: Vec<ModelSource>,
    #[serde(default = "answers_checked")]
    pub verify_answers: bool,
    #[serde(skip)]
    pub ui_language: Option<String>,
}

fn answers_checked() -> bool {
    true
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebEngine {
    #[default]
    DuckDuckGo,
    SearXng,
    Brave,
    Tavily,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSettings {
    pub enabled: bool,
    pub engine: WebEngine,
    pub searxng_url: String,
    #[serde(skip)]
    pub api_key: String,
    pub proxy: String,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            engine: WebEngine::DuckDuckGo,
            searxng_url: String::new(),
            api_key: String::new(),
            proxy: String::new(),
        }
    }
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            connections: Vec::new(),
            models: ModelChoices::default(),
            budget: Budget::default(),
            agent_context: 8192,
            web: WebSettings::default(),
            analysis: AnalysisOptions::default(),
            thinking: Vec::new(),
            verify_answers: true,
            ui_language: None,
        }
    }
}

impl AgentSettings {
    pub fn language_name(&self) -> &'static str {
        match self.ui_language.as_deref() {
            Some("ru") => "Russian",
            _ => "English",
        }
    }

    pub fn thinking_on(&self, model: &ModelSource) -> bool {
        model.supports_thinking() && self.thinking.contains(model)
    }

    pub fn set_thinking(&mut self, model: &ModelSource, on: bool) {
        self.thinking.retain(|m| m != model);
        if on {
            self.thinking.push(model.clone());
        }
    }

    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.connections
            .iter()
            .find(|connection| connection.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_language_names_the_language_of_the_prompts() {
        let mut a = AgentSettings::default();
        assert_eq!(a.language_name(), "English");
        a.ui_language = Some("ru".into());
        assert_eq!(a.language_name(), "Russian");
        a.ui_language = Some("xx".into());
        assert_eq!(a.language_name(), "English");
        a.ui_language = Some("ru".into());
        let saved = serde_json::to_string(&a).unwrap();
        assert!(!saved.contains("ui_language"), "{saved}");
    }

    #[test]
    fn defaults_leave_every_role_empty() {
        let models = ModelChoices::default();
        for role in ModelRole::ALL {
            assert!(models.get(role).is_none(), "{role:?}");
        }
    }

    #[test]
    fn roundtrips_through_toml() {
        let mut settings = AgentSettings::default();
        let mut local = Connection::new(ConnectionKind::OpenAiCompatible, "Local");
        local.base_url = "http://host:8000/v1/".into();
        local.proxy = " socks5://127.0.0.1:1080 ".into();
        assert_eq!(local.api_url(), "http://host:8000/v1");
        assert_eq!(local.proxy(), Some("socks5://127.0.0.1:1080"));
        settings.models.titles = Some(ModelSource::new(local.id.clone(), "gpt-x"));
        settings.connections.push(local);
        settings
            .connections
            .push(Connection::new(ConnectionKind::OpenRouter, "OR"));
        assert_eq!(settings.connections[1].api_url(), OPENROUTER_URL);
        assert_eq!(settings.connections[1].proxy(), None);
        let text = toml::to_string(&settings).unwrap();
        assert!(!text.contains("api_key"), "{text}");
        assert_eq!(toml::from_str::<AgentSettings>(&text).unwrap(), settings);
    }

    #[test]
    fn thinking_is_off_until_turned_on_per_model() {
        let mut settings = AgentSettings::default();
        let qwen = ModelSource::new("c1", "qwen3");
        let other = ModelSource::new("c1", "gemma-3");
        let reasoner = ModelSource::new("c2", "deepseek-reasoner");
        assert!(qwen.supports_thinking() && reasoner.supports_thinking());
        assert!(!other.supports_thinking());
        settings.set_thinking(&qwen, true);
        settings.set_thinking(&other, true);
        assert!(settings.thinking_on(&qwen));
        assert!(!settings.thinking_on(&other));
        settings.set_thinking(&qwen, true);
        assert_eq!(settings.thinking.iter().filter(|m| **m == qwen).count(), 1);
        let back: AgentSettings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back, settings);
        settings.set_thinking(&qwen, false);
        assert!(!settings.thinking_on(&qwen));
    }

    #[test]
    fn answers_are_checked_until_turned_off() {
        assert!(AgentSettings::default().verify_answers);
        let off = AgentSettings {
            verify_answers: false,
            ..AgentSettings::default()
        };
        let back: AgentSettings = toml::from_str(&toml::to_string(&off).unwrap()).unwrap();
        assert!(!back.verify_answers);
        let text = toml::to_string(&AgentSettings::default()).unwrap();
        let stripped = text
            .lines()
            .filter(|line| !line.starts_with("verify_answers"))
            .collect::<Vec<_>>()
            .join("\n");
        let missing: AgentSettings = toml::from_str(&stripped).unwrap();
        assert!(missing.verify_answers);
    }

    #[test]
    fn budget_limits_never_drop_to_zero() {
        let mut budget = Budget::default();
        *budget.limit_mut(WorkKind::WebSearch) = 0;
        assert_eq!(budget.limit(WorkKind::WebSearch), 1);
        let bare: Budget = toml::from_str("").unwrap();
        assert_eq!(bare.provider_calls, 4);
    }

    #[test]
    fn command_capabilities_follow_the_output_format() {
        let mut connection = Connection::new(ConnectionKind::Command, "Whisper");
        connection.command = Some(CommandSpec {
            program: "whisper-cli".into(),
            args: vec!["-f".into(), "{input}".into()],
            output: OutputFormat::WhisperJson,
            whole_file: false,
        });
        assert!(connection.supports(Capability::Transcription));
        connection.command.as_mut().unwrap().output = OutputFormat::Rttm;
        assert!(connection.supports(Capability::Diarization));
        connection.command.as_mut().unwrap().output = OutputFormat::Vectors;
        assert!(connection.supports(Capability::VoiceEmbedding));
        assert!(
            Connection::new(ConnectionKind::OpenAi, "OpenAI").supports(Capability::Transcription)
        );
        assert!(
            !Connection::new(ConnectionKind::DeepSeek, "DeepSeek").supports(Capability::Embeddings)
        );
    }
}
