use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAiCompatible,
    OpenRouter,
    HuggingFace,
}

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
pub const HUGGING_FACE_ROUTER_URL: &str = "https://router.huggingface.co/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    #[serde(skip)]
    pub api_key: String,
    #[serde(default)]
    pub proxy: String,
    #[serde(default = "default_provider_context")]
    pub context: u32,
}

fn default_provider_context() -> u32 {
    32_768
}

impl Provider {
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

    pub fn api_url(&self) -> String {
        match self.kind {
            ProviderKind::OpenRouter => OPENROUTER_URL.into(),
            ProviderKind::HuggingFace => HUGGING_FACE_ROUTER_URL.into(),
            ProviderKind::OpenAiCompatible => self.base_url.trim().trim_end_matches('/').to_string(),
        }
    }

    pub fn proxy(&self) -> Option<&str> {
        Some(self.proxy.trim()).filter(|p| !p.is_empty())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum ModelSource {
    HuggingFace { repo: String },
    Local { path: PathBuf },
    Provider { provider: String, model: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    Embedding,
    Reranker,
    Agent,
    Titles,
    SpeechRecognition,
    Diarization,
    Speech,
}

impl ModelRole {
    pub const ALL: [ModelRole; 7] = [
        ModelRole::Embedding,
        ModelRole::Reranker,
        ModelRole::Agent,
        ModelRole::Titles,
        ModelRole::SpeechRecognition,
        ModelRole::Diarization,
        ModelRole::Speech,
    ];

    pub fn required(self) -> bool {
        matches!(self, ModelRole::Embedding | ModelRole::Agent | ModelRole::Titles | ModelRole::SpeechRecognition)
    }

    pub fn default_source(self) -> Option<ModelSource> {
        let hf = |repo: &str| Some(ModelSource::HuggingFace { repo: repo.into() });
        match self {
            ModelRole::Embedding => hf("ggml-org/embeddinggemma-300M-GGUF"),
            ModelRole::Reranker => hf("gpustack/bge-reranker-v2-m3-GGUF"),
            ModelRole::Agent => hf("unsloth/Qwen3.5-4B-GGUF"),
            ModelRole::Titles => hf("unsloth/Qwen3-0.6B-GGUF"),
            ModelRole::SpeechRecognition => hf("csukuangfj/sherpa-onnx-nemo-ctc-punct-giga-am-v3-russian-2025-12-16"),
            ModelRole::Diarization => hf("csukuangfj/speaker-embedding-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx"),
            ModelRole::Speech => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelChoices {
    pub embedding: Option<ModelSource>,
    pub reranker: Option<ModelSource>,
    pub agent: Option<ModelSource>,
    pub titles: Option<ModelSource>,
    pub speech_recognition: Option<ModelSource>,
    pub diarization: Option<ModelSource>,
    pub speech: Option<ModelSource>,
}

impl Default for ModelChoices {
    fn default() -> Self {
        Self {
            embedding: ModelRole::Embedding.default_source(),
            reranker: ModelRole::Reranker.default_source(),
            agent: ModelRole::Agent.default_source(),
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
            ModelRole::Titles => &mut self.titles,
            ModelRole::SpeechRecognition => &mut self.speech_recognition,
            ModelRole::Diarization => &mut self.diarization,
            ModelRole::Speech => &mut self.speech,
        }
    }

}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    #[default]
    Auto,
    Gpu,
    Cpu,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelDevices {
    pub embedding: Device,
    pub reranker: Device,
    pub agent: Device,
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
            ModelRole::Titles => &mut self.titles,
            ModelRole::SpeechRecognition => &mut self.speech_recognition,
            ModelRole::Diarization => &mut self.diarization,
            ModelRole::Speech => &mut self.speech,
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
    pub const ALL: [WorkKind; 5] = [WorkKind::Agent, WorkKind::Transcription, WorkKind::Indexing, WorkKind::WebSearch, WorkKind::ProviderCalls];
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Budget {
    pub gpu_memory_gb: f32,
    pub ram_memory_gb: f32,
    pub agent: u32,
    pub transcription: u32,
    pub indexing: u32,
    pub web_search: u32,
    pub provider_calls: u32,
    #[serde(default = "default_keep_alive_secs")]
    pub keep_alive_secs: u64,
}

fn default_keep_alive_secs() -> u64 {
    300
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            gpu_memory_gb: 8.0,
            ram_memory_gb: 8.0,
            agent: 2,
            transcription: 1,
            indexing: 1,
            web_search: 2,
            provider_calls: 4,
            keep_alive_secs: default_keep_alive_secs(),
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
    pub diarize: bool,
    pub speakers: Option<u32>,
    pub index: bool,
    pub topics: bool,
    pub details: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        AnalysisOptions { diarize: true, speakers: None, index: true, topics: true, details: true }
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
    pub providers: Vec<Provider>,
    pub models: ModelChoices,
    pub devices: ModelDevices,
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
            ui_language: None,
        }
    }
}

impl ModelSource {
    pub fn supports_thinking(&self) -> bool {
        match self {
            ModelSource::Provider { .. } => true,
            ModelSource::HuggingFace { repo: name } => name.to_lowercase().contains("qwen3"),
            ModelSource::Local { path } => path.to_string_lossy().to_lowercase().contains("qwen3"),
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

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn hugging_face(&self) -> Option<&Provider> {
        let hf = || self.providers.iter().filter(|p| p.kind == ProviderKind::HuggingFace);
        hf().find(|p| !p.api_key.trim().is_empty()).or_else(|| hf().next())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_language_names_the_language_of_the_prompts() {
        let mut a = AgentSettings::default();
        assert_eq!(a.language_name(), "English", "a tool that has no interface writes English");
        a.ui_language = Some("ru".into());
        assert_eq!(a.language_name(), "Russian");
        a.ui_language = Some("xx".into());
        assert_eq!(a.language_name(), "English", "an unknown code falls back to English");
        a.ui_language = Some("ru".into());
        let saved = serde_json::to_string(&a).unwrap();
        assert!(!saved.contains("ui_language"), "{saved}");
    }

    #[test]
    fn defaults_cover_required_roles() {
        let m = ModelChoices::default();
        for role in ModelRole::ALL {
            assert!(m.get(role).is_some() || !role.required(), "{role:?}");
        }
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
        a.models.titles = Some(ModelSource::Provider { provider: p.id.clone(), model: "gpt-x".into() });
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
        assert_eq!(back.budget.keep_alive_secs, 300);
        let bare: Budget = toml::from_str("gpu_memory_gb = 1.0\n").unwrap();
        assert_eq!(bare.keep_alive_secs, 300);
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
