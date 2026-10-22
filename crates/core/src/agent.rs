//! Settings of the Agent section: LLM providers and the models used for each role.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How a provider is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// Any server with the OpenAI API (`/v1/models`, `/v1/chat/completions`…) at `base_url`.
    OpenAiCompatible,
    /// OpenRouter (`https://openrouter.ai/api/v1`).
    OpenRouter,
    /// Hugging Face: its token (optional) and proxy are also used to download models; its
    /// Inference Providers router serves models over the OpenAI API.
    HuggingFace,
}

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
pub const HUGGING_FACE_ROUTER_URL: &str = "https://router.huggingface.co/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// Stable id that model choices refer to (names can be edited).
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    /// API root, e.g. `http://localhost:11434/v1` (OpenAI-compatible servers only).
    pub base_url: String,
    /// API key (Hugging Face: access token, optional). Kept in the OS keychain ([`crate::secrets`]).
    #[serde(skip)]
    pub api_key: String,
    /// Proxy for this provider's requests (`http://`, `https://` or `socks5://`); empty: direct.
    #[serde(default)]
    pub proxy: String,
    /// Context of its models, tokens (providers do not tell; sizes their reads and history).
    #[serde(default = "default_provider_context")]
    pub context: u32,
}

fn default_provider_context() -> u32 {
    32_768
}

impl Provider {
    /// A new provider with a fresh id.
    pub fn new(kind: ProviderKind, name: &str) -> Provider {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        Provider {
            id: format!("p{nanos:x}"),
            name: name.to_string(),
            kind,
            base_url: match kind {
                ProviderKind::OpenAiCompatible => "http://localhost:11434/v1".into(),
                ProviderKind::OpenRouter | ProviderKind::HuggingFace => String::new(),
            },
            api_key: String::new(),
            proxy: String::new(),
            context: default_provider_context(),
        }
    }

    /// API root without a trailing slash.
    pub fn api_url(&self) -> String {
        match self.kind {
            ProviderKind::OpenRouter => OPENROUTER_URL.into(),
            ProviderKind::HuggingFace => HUGGING_FACE_ROUTER_URL.into(),
            ProviderKind::OpenAiCompatible => self.base_url.trim().trim_end_matches('/').to_string(),
        }
    }

    /// The proxy, if one is set.
    pub fn proxy(&self) -> Option<&str> {
        Some(self.proxy.trim()).filter(|p| !p.is_empty())
    }
}

/// Where a model comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ModelSource {
    /// Downloaded from Hugging Face by the inference engine on first use.
    HuggingFace { repo: String },
    /// A model file or folder on disk (the engine reads it where it is).
    Local { path: PathBuf },
    /// Served by one of the providers (by its id).
    Provider { provider: String, model: String },
}

/// What each model is used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    Embedding,
    /// Re-orders search hits by how well they answer the question (a cross-encoder).
    Reranker,
    Agent,
    Subagent,
    /// Short titles for recordings, notes and chats.
    Titles,
    SpeechRecognition,
    Diarization,
    Speech,
}

impl ModelRole {
    pub const ALL: [ModelRole; 8] = [
        ModelRole::Embedding,
        ModelRole::Reranker,
        ModelRole::Agent,
        ModelRole::Subagent,
        ModelRole::Titles,
        ModelRole::SpeechRecognition,
        ModelRole::Diarization,
        ModelRole::Speech,
    ];

    /// Roles that must have a model; the others may stay unset.
    pub fn required(self) -> bool {
        matches!(self, ModelRole::Embedding | ModelRole::Agent | ModelRole::Titles | ModelRole::SpeechRecognition)
    }

    /// The Hugging Face model a role starts with.
    pub fn default_source(self) -> Option<ModelSource> {
        let hf = |repo: &str| Some(ModelSource::HuggingFace { repo: repo.into() });
        match self {
            // EmbeddingGemma 300M (Google, 100+ languages), GGUF for llama.cpp.
            ModelRole::Embedding => hf("ggml-org/embeddinggemma-300M-GGUF"),
            // BGE reranker v2 M3 (multilingual cross-encoder), GGUF (Q4_K_M, ~440 MB).
            ModelRole::Reranker => hf("gpustack/bge-reranker-v2-m3-GGUF"),
            // GGUF for llama.cpp (one Q4_K_M file is downloaded).
            ModelRole::Agent => hf("unsloth/Qwen3.5-4B-GGUF"),
            // A light model is enough for a few words.
            ModelRole::Titles => hf("unsloth/Qwen3-0.6B-GGUF"),
            // GigaAM v3 (Sber) with punctuation, sherpa-onnx layout (CTC, ~225 MB).
            ModelRole::SpeechRecognition => hf("csukuangfj/sherpa-onnx-nemo-ctc-punct-giga-am-v3-russian-2025-12-16"),
            // Speaker embeddings (WeSpeaker ResNet34, VoxCeleb: many languages), one file of a
            // repository with many; the segmentation model comes with it.
            ModelRole::Diarization => hf("csukuangfj/speaker-embedding-models/wespeaker_en_voxceleb_resnet34_LM.onnx"),
            // Sub-agent: the main agent's model unless set; speech synthesis: off.
            ModelRole::Subagent | ModelRole::Speech => None,
        }
    }
}

/// Model per role (`None`: unset — for the sub-agent, the main agent's model is used).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelChoices {
    pub embedding: Option<ModelSource>,
    pub reranker: Option<ModelSource>,
    pub agent: Option<ModelSource>,
    pub subagent: Option<ModelSource>,
    pub titles: Option<ModelSource>,
    pub speech_recognition: Option<ModelSource>,
    pub diarization: Option<ModelSource>,
    /// Speech synthesis.
    pub speech: Option<ModelSource>,
}

impl Default for ModelChoices {
    fn default() -> Self {
        Self {
            embedding: ModelRole::Embedding.default_source(),
            reranker: ModelRole::Reranker.default_source(),
            agent: ModelRole::Agent.default_source(),
            subagent: None,
            titles: ModelRole::Titles.default_source(),
            speech_recognition: ModelRole::SpeechRecognition.default_source(),
            diarization: ModelRole::Diarization.default_source(),
            speech: None,
        }
    }
}

impl ModelChoices {
    pub fn get(&self, role: ModelRole) -> &Option<ModelSource> {
        match role {
            ModelRole::Embedding => &self.embedding,
            ModelRole::Reranker => &self.reranker,
            ModelRole::Agent => &self.agent,
            ModelRole::Subagent => &self.subagent,
            ModelRole::Titles => &self.titles,
            ModelRole::SpeechRecognition => &self.speech_recognition,
            ModelRole::Diarization => &self.diarization,
            ModelRole::Speech => &self.speech,
        }
    }

    pub fn get_mut(&mut self, role: ModelRole) -> &mut Option<ModelSource> {
        match role {
            ModelRole::Embedding => &mut self.embedding,
            ModelRole::Reranker => &mut self.reranker,
            ModelRole::Agent => &mut self.agent,
            ModelRole::Subagent => &mut self.subagent,
            ModelRole::Titles => &mut self.titles,
            ModelRole::SpeechRecognition => &mut self.speech_recognition,
            ModelRole::Diarization => &mut self.diarization,
            ModelRole::Speech => &mut self.speech,
        }
    }

    /// The model actually used for `role` (the sub-agent falls back to the main agent's).
    pub fn effective(&self, role: ModelRole) -> Option<&ModelSource> {
        match (role, self.get(role)) {
            (ModelRole::Subagent, None) => self.agent.as_ref(),
            (_, m) => m.as_ref(),
        }
    }
}

/// Where a local model should run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    /// The GPU when it fits the budget, else the CPU.
    #[default]
    Auto,
    /// Preferably the GPU; the CPU when the GPU cannot take it.
    Gpu,
    /// Never the GPU.
    Cpu,
}

/// Device per role (local models only).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelDevices {
    pub embedding: Device,
    pub reranker: Device,
    pub agent: Device,
    pub subagent: Device,
    pub titles: Device,
    pub speech_recognition: Device,
    pub diarization: Device,
    pub speech: Device,
}

impl ModelDevices {
    pub fn get(&self, role: ModelRole) -> Device {
        match role {
            ModelRole::Embedding => self.embedding,
            ModelRole::Reranker => self.reranker,
            ModelRole::Agent => self.agent,
            ModelRole::Subagent => self.subagent,
            ModelRole::Titles => self.titles,
            ModelRole::SpeechRecognition => self.speech_recognition,
            ModelRole::Diarization => self.diarization,
            ModelRole::Speech => self.speech,
        }
    }

    pub fn get_mut(&mut self, role: ModelRole) -> &mut Device {
        match role {
            ModelRole::Embedding => &mut self.embedding,
            ModelRole::Reranker => &mut self.reranker,
            ModelRole::Agent => &mut self.agent,
            ModelRole::Subagent => &mut self.subagent,
            ModelRole::Titles => &mut self.titles,
            ModelRole::SpeechRecognition => &mut self.speech_recognition,
            ModelRole::Diarization => &mut self.diarization,
            ModelRole::Speech => &mut self.speech,
        }
    }
}

/// Kinds of work with their own limit of how many run at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WorkKind {
    /// Answers in chats.
    Agent,
    Transcription,
    Indexing,
    WebSearch,
    /// Requests to model providers (their rate limits, not memory, are the limit).
    ProviderCalls,
}

impl WorkKind {
    pub const ALL: [WorkKind; 5] = [WorkKind::Agent, WorkKind::Transcription, WorkKind::Indexing, WorkKind::WebSearch, WorkKind::ProviderCalls];
}

/// Limits for local models and for work running at once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    /// Video memory local models may take, GB.
    pub gpu_memory_gb: f32,
    /// RAM models running on the CPU may take, GB.
    pub ram_memory_gb: f32,
    pub agent: u32,
    pub transcription: u32,
    pub indexing: u32,
    pub web_search: u32,
    pub provider_calls: u32,
}

impl Default for Budget {
    fn default() -> Self {
        Self { gpu_memory_gb: 8.0, ram_memory_gb: 8.0, agent: 2, transcription: 1, indexing: 1, web_search: 2, provider_calls: 4 }
    }
}

impl Budget {
    /// How many jobs of `kind` may run at once (at least one).
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

    pub fn gpu_bytes(&self) -> u64 {
        gb(self.gpu_memory_gb)
    }

    pub fn ram_bytes(&self) -> u64 {
        gb(self.ram_memory_gb)
    }
}

fn gb(v: f32) -> u64 {
    (f64::from(v.max(0.0)) * 1024.0 * 1024.0 * 1024.0) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AnalysisOptions {
    /// Speakers by diarization (needs a diarization model).
    pub diarize: bool,
    /// How many people speak; `None`: let diarization decide.
    pub speakers: Option<u32>,
    /// Chunks, full-text search and embeddings.
    pub index: bool,
    /// Topics (needs the index).
    pub topics: bool,
    /// Details of each topic (needs topics).
    pub details: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        AnalysisOptions { diarize: true, speakers: None, index: true, topics: true, details: true }
    }
}

impl AnalysisOptions {
    /// Turns off what cannot run: details without topics, topics without the index, and
    /// diarization without a speaker count model.
    pub fn resolved(mut self, models: &ModelChoices) -> AnalysisOptions {
        if models.diarization.is_none() {
            self.diarize = false;
        }
        self.enforced()
    }

    /// Stages that depend on a disabled one are off. Earlier stages are not required here: a
    /// restart from a later stage keeps their data.
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
    pub providers: Vec<Provider>,
    pub models: ModelChoices,
    pub devices: ModelDevices,
    pub budget: Budget,
    /// Context of a local agent model, tokens: more lets it read more at once and takes more
    /// memory (the KV cache).
    pub agent_context: u32,
    pub web: WebSettings,
    /// Stages of the analysis of recordings, the default for the next one.
    pub analysis: AnalysisOptions,
    /// Models that reason before they act (the «Thinking» switch); off for every model until
    /// the user turns it on.
    pub thinking: Vec<ModelSource>,
    /// Every answer is checked against the moments and pages it cites. On until turned off
    /// (a settings file that has no key keeps the check on).
    #[serde(default = "answers_checked")]
    pub verify_answers: bool,
}

fn answers_checked() -> bool {
    true
}

/// Where the agent searches the web.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WebEngine {
    /// DuckDuckGo's HTML results: no key, but fragile (the page may change, or ask for a captcha).
    #[default]
    DuckDuckGo,
    /// A SearXNG instance (its JSON API).
    SearXng,
    /// Brave Search API (a key).
    Brave,
    /// Tavily API (a key).
    Tavily,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSettings {
    /// The agent may search the web and read pages.
    pub enabled: bool,
    pub engine: WebEngine,
    /// The SearXNG instance, e.g. `https://searx.example.org`.
    pub searxng_url: String,
    /// Brave / Tavily key; kept in the OS keychain ([`crate::secrets`]).
    #[serde(skip)]
    pub api_key: String,
    /// Proxy for web requests; empty: direct.
    pub proxy: String,
}

impl Default for WebSettings {
    fn default() -> Self {
        Self { enabled: true, engine: WebEngine::DuckDuckGo, searxng_url: String::new(), api_key: String::new(), proxy: String::new() }
    }
}

impl Default for AgentSettings {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            models: ModelChoices::default(),
            devices: ModelDevices::default(),
            budget: Budget::default(),
            agent_context: 8192,
            web: WebSettings::default(),
            analysis: AnalysisOptions::default(),
            thinking: Vec::new(),
            verify_answers: true,
        }
    }
}

impl ModelSource {
    /// Whether the model can reason before it acts, so the «Thinking» switch is shown for it:
    /// local models of the Qwen3 family (their chat template has a thinking mode), and every
    /// provider model (a provider that does not know the reasoning parameter ignores it).
    pub fn supports_thinking(&self) -> bool {
        match self {
            ModelSource::Provider { .. } => true,
            ModelSource::HuggingFace { repo: name } => name.to_lowercase().contains("qwen3"),
            ModelSource::Local { path } => path.to_string_lossy().to_lowercase().contains("qwen3"),
        }
    }
}

impl AgentSettings {
    /// Whether `model` reasons before it acts (and can).
    pub fn thinking_on(&self, model: &ModelSource) -> bool {
        model.supports_thinking() && self.thinking.contains(model)
    }

    /// Turns the reasoning of `model` on or off.
    pub fn set_thinking(&mut self, model: &ModelSource, on: bool) {
        self.thinking.retain(|m| m != model);
        if on {
            self.thinking.push(model.clone());
        }
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// The Hugging Face provider whose token and proxy model downloads use: the first one with
    /// a token, else the first one; `None`: anonymous, direct.
    pub fn hugging_face(&self) -> Option<&Provider> {
        let hf = || self.providers.iter().filter(|p| p.kind == ProviderKind::HuggingFace);
        hf().find(|p| !p.api_key.trim().is_empty()).or_else(|| hf().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_required_roles() {
        let m = ModelChoices::default();
        for role in ModelRole::ALL {
            assert!(m.get(role).is_some() || !role.required(), "{role:?}");
        }
        assert_eq!(m.effective(ModelRole::Subagent), m.agent.as_ref(), "sub-agent follows the main agent");
        assert!(m.diarization.is_some(), "diarization is on out of the box");
        assert!(m.speech.is_none());
    }

    #[test]
    fn roundtrips_through_toml() {
        let mut a = AgentSettings::default();
        let mut p = Provider::new(ProviderKind::OpenAiCompatible, "Local");
        p.base_url = "http://host:8000/v1/".into();
        p.proxy = " socks5://127.0.0.1:1080 ".into();
        assert_eq!(p.api_url(), "http://host:8000/v1");
        assert_eq!(p.proxy(), Some("socks5://127.0.0.1:1080"));
        a.models.subagent = Some(ModelSource::Provider { provider: p.id.clone(), model: "gpt-x".into() });
        a.models.speech = Some(ModelSource::Local { path: "/m/tts.onnx".into() });
        a.providers.push(p);
        a.providers.push(Provider::new(ProviderKind::OpenRouter, "OR"));
        assert_eq!(a.providers[1].api_url(), OPENROUTER_URL);
        assert_eq!(a.providers[1].proxy(), None);
        let text = toml::to_string(&a).unwrap();
        assert!(text.contains("source = \"hugging_face\""), "{text}");
        assert_eq!(toml::from_str::<AgentSettings>(&text).unwrap(), a);
    }

    #[test]
    fn thinking_is_off_until_turned_on_per_model() {
        let mut a = AgentSettings::default();
        let qwen = ModelSource::HuggingFace { repo: "unsloth/Qwen3.5-4B-GGUF".into() };
        let other = ModelSource::HuggingFace { repo: "unsloth/gemma-3-4b-it-GGUF".into() };
        let provider = ModelSource::Provider { provider: "p".into(), model: "deepseek-reasoner".into() };
        assert!(a.models.agent.as_ref().is_some_and(|m| !a.thinking_on(m)), "off by default");
        assert!(qwen.supports_thinking() && provider.supports_thinking() && !other.supports_thinking());
        assert!(ModelSource::Local { path: "C:/m/Qwen3-8B-Q4.gguf".into() }.supports_thinking());
        a.set_thinking(&qwen, true);
        a.set_thinking(&other, true);
        assert!(a.thinking_on(&qwen));
        assert!(!a.thinking_on(&other), "a model without the mode never thinks");
        a.set_thinking(&qwen, true);
        assert_eq!(a.thinking.iter().filter(|m| **m == qwen).count(), 1, "no duplicates");
        let back: AgentSettings = toml::from_str(&toml::to_string(&a).unwrap()).unwrap();
        assert_eq!(back, a);
        a.set_thinking(&qwen, false);
        assert!(!a.thinking_on(&qwen));
    }

    #[test]
    fn answers_are_checked_until_turned_off() {
        assert!(AgentSettings::default().verify_answers);
        let off = AgentSettings { verify_answers: false, ..AgentSettings::default() };
        let back: AgentSettings = toml::from_str(&toml::to_string(&off).unwrap()).unwrap();
        assert!(!back.verify_answers);
        let text = toml::to_string(&AgentSettings::default()).unwrap();
        let stripped = text.lines().filter(|line| !line.starts_with("verify_answers")).collect::<Vec<_>>().join("\n");
        let missing: AgentSettings = toml::from_str(&stripped).unwrap();
        assert!(missing.verify_answers, "a settings file from before the switch keeps the check on");
    }

    #[test]
    fn budget_and_devices() {
        let mut b = Budget::default();
        assert_eq!(b.gpu_bytes(), 8 << 30);
        *b.limit_mut(WorkKind::WebSearch) = 0;
        assert_eq!(b.limit(WorkKind::WebSearch), 1, "never zero: work would never run");
        b.ram_memory_gb = -1.0;
        assert_eq!(b.ram_bytes(), 0);
        let mut a = AgentSettings::default();
        *a.devices.get_mut(ModelRole::SpeechRecognition) = Device::Cpu;
        a.budget.gpu_memory_gb = 5.5;
        let back: AgentSettings = toml::from_str(&toml::to_string(&a).unwrap()).unwrap();
        assert_eq!(back.devices.get(ModelRole::SpeechRecognition), Device::Cpu);
        assert_eq!(back.devices.get(ModelRole::Agent), Device::Auto);
        assert_eq!(back.budget.gpu_memory_gb, 5.5);
    }

    #[test]
    fn downloads_use_the_hugging_face_provider_with_a_token() {
        let mut a = AgentSettings::default();
        assert!(a.hugging_face().is_none());
        let anon = Provider::new(ProviderKind::HuggingFace, "HF");
        a.providers.push(anon.clone());
        assert_eq!(a.hugging_face().map(|p| &p.id), Some(&anon.id));
        let mut with_token = Provider::new(ProviderKind::HuggingFace, "HF 2");
        with_token.id = "other".into();
        with_token.api_key = "hf_x".into();
        a.providers.push(with_token);
        assert_eq!(a.hugging_face().map(|p| p.id.as_str()), Some("other"));
        assert_eq!(a.providers[0].api_url(), HUGGING_FACE_ROUTER_URL);
    }
}
