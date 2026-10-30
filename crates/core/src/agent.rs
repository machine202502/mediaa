use serde::{Deserialize, Serialize};

pub const GROUP_NAME_MAX: usize = 16;
pub const GROUP_LIMIT_MAX: u32 = 16;
pub const WEB_LIMIT_DEFAULT: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub limit: u32,
}

impl Group {
    pub fn new(name: impl Into<String>, limit: u32) -> Group {
        Group {
            name: name.into(),
            limit: limit.clamp(1, GROUP_LIMIT_MAX),
        }
    }

    pub fn valid_name(name: &str) -> bool {
        let count = name.chars().count();
        let mut chars = name.chars();
        match chars.next() {
            Some(first) if first.is_ascii_alphabetic() => {
                (1..=GROUP_NAME_MAX).contains(&count) && chars.all(|c| c.is_ascii_alphanumeric())
            }
            _ => false,
        }
    }
}

pub const OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
pub const DEEPSEEK_URL: &str = "https://api.deepseek.com";
pub const HUGGING_FACE_ROUTER_URL: &str = "https://router.huggingface.co/v1";
pub const OLLAMA_URL: &str = "http://localhost:11434";
pub const TRANSCRIPTION_URL: &str = "http://localhost:8293";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionKind {
    OpenRouter,
    DeepSeek,
    HuggingFace,
    Ollama,
    OpenAiCompatible,
    Transcription,
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

fn default_context() -> u32 {
    32_768
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ReasoningDepth {
    Low,
    #[default]
    Medium,
    High,
    XHigh,
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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub group: String,
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
                ConnectionKind::Transcription => TRANSCRIPTION_URL.into(),
                _ => String::new(),
            },
            api_key: String::new(),
            proxy: String::new(),
            group: String::new(),
        }
    }

    pub fn api_url(&self) -> String {
        match self.kind {
            ConnectionKind::OpenRouter => OPENROUTER_URL.into(),
            ConnectionKind::DeepSeek => DEEPSEEK_URL.into(),
            ConnectionKind::HuggingFace => HUGGING_FACE_ROUTER_URL.into(),
            ConnectionKind::Ollama => format!("{}/v1", trim_url(&self.base_url, OLLAMA_URL)),
            ConnectionKind::OpenAiCompatible => {
                trim_url(&self.base_url, "http://localhost:1234/v1")
            }
            ConnectionKind::Transcription => trim_url(&self.base_url, TRANSCRIPTION_URL),
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
            ConnectionKind::DeepSeek | ConnectionKind::HuggingFace => vec![Capability::Chat],
            ConnectionKind::Ollama => vec![Capability::Chat, Capability::Embeddings],
            ConnectionKind::OpenAiCompatible => vec![
                Capability::Chat,
                Capability::Embeddings,
                Capability::Rerank,
                Capability::Transcription,
            ],
            ConnectionKind::Transcription => vec![Capability::Transcription],
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
    #[serde(default = "default_context")]
    pub context: u32,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub thinking_depth: ReasoningDepth,
    #[serde(default)]
    pub diarize: bool,
    #[serde(default)]
    pub word_times: bool,
    #[serde(default)]
    pub voiceprint: bool,
}

impl ModelSource {
    pub fn new(connection: impl Into<String>, model: impl Into<String>) -> ModelSource {
        ModelSource {
            connection: connection.into(),
            model: model.into(),
            context: default_context(),
            thinking: false,
            thinking_depth: ReasoningDepth::Medium,
            diarize: false,
            word_times: false,
            voiceprint: false,
        }
    }

    pub fn key(&self) -> String {
        format!("{}/{}", self.connection, self.model)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModelRole {
    Embedding,
    Reranker,
    Agent,
    Titles,
    SpeechRecognition,
}

impl ModelRole {
    pub const ALL: [ModelRole; 5] = [
        ModelRole::Embedding,
        ModelRole::Reranker,
        ModelRole::Agent,
        ModelRole::Titles,
        ModelRole::SpeechRecognition,
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
}

impl ModelChoices {
    pub fn get(&self, role: ModelRole) -> &Option<ModelSource> {
        match role {
            ModelRole::Embedding => &self.embedding,
            ModelRole::Reranker => &self.reranker,
            ModelRole::Agent => &self.agent,
            ModelRole::Titles => &self.titles,
            ModelRole::SpeechRecognition => &self.speech_recognition,
        }
    }

    pub fn get_mut(&mut self, role: ModelRole) -> &mut Option<ModelSource> {
        match role {
            ModelRole::Embedding => &mut self.embedding,
            ModelRole::Reranker => &mut self.reranker,
            ModelRole::Agent => &mut self.agent,
            ModelRole::Titles => &mut self.titles,
            ModelRole::SpeechRecognition => &mut self.speech_recognition,
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
            diarize: false,
            speakers: None,
            index: true,
            topics: true,
            details: true,
        }
    }
}

impl AnalysisOptions {
    pub fn resolved(mut self, models: &ModelChoices) -> AnalysisOptions {
        let allowed = models
            .speech_recognition
            .as_ref()
            .is_some_and(|source| source.diarize);
        if !allowed {
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
    #[serde(default)]
    pub groups: Vec<Group>,
    #[serde(default = "web_limit")]
    pub web_limit: u32,
    pub web: WebSettings,
    pub analysis: AnalysisOptions,
    #[serde(default = "answers_checked")]
    pub verify_answers: bool,
    #[serde(skip)]
    pub ui_language: Option<String>,
}

fn web_limit() -> u32 {
    WEB_LIMIT_DEFAULT
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
            groups: Vec::new(),
            web_limit: WEB_LIMIT_DEFAULT,
            web: WebSettings::default(),
            analysis: AnalysisOptions::default(),
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

    pub fn connection(&self, id: &str) -> Option<&Connection> {
        self.connections
            .iter()
            .find(|connection| connection.id == id)
    }

    pub fn group(&self, name: &str) -> Option<&Group> {
        self.groups.iter().find(|group| group.name == name)
    }

    pub fn add_group(&mut self, name: &str, limit: u32) -> bool {
        if !Group::valid_name(name)
            || !(1..=GROUP_LIMIT_MAX).contains(&limit)
            || self.group(name).is_some()
        {
            return false;
        }
        self.groups.push(Group::new(name, limit));
        true
    }

    pub fn remove_group(&mut self, name: &str) {
        self.groups.retain(|group| group.name != name);
        for connection in &mut self.connections {
            if connection.group == name {
                connection.group.clear();
            }
        }
    }

    pub fn place(&self, connection: &str) -> Option<&str> {
        self.connection(connection)
            .map(|connection| connection.group.trim())
            .filter(|group| !group.is_empty())
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
    fn thinking_is_stored_on_the_model() {
        let mut source = ModelSource::new("c1", "gemma");
        assert!(!source.thinking);
        assert_eq!(source.thinking_depth, ReasoningDepth::Medium);
        source.thinking = true;
        source.thinking_depth = ReasoningDepth::High;
        let settings = AgentSettings {
            models: ModelChoices {
                agent: Some(source.clone()),
                ..ModelChoices::default()
            },
            ..AgentSettings::default()
        };
        let back: AgentSettings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(back.models.agent, Some(source));
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
    fn empty_settings_have_no_groups_and_no_connections() {
        let bare: AgentSettings = toml::from_str("").unwrap();
        assert!(bare.groups.is_empty());
        assert_eq!(bare.web_limit, WEB_LIMIT_DEFAULT);
        assert!(bare.connections.is_empty());
        assert!(AgentSettings::default().groups.is_empty());
    }

    #[test]
    fn an_old_budget_table_is_ignored() {
        let text = "[budget]\nagent = 3\nweb_search = 5\n\n[web]\nenabled = true\n";
        let settings: AgentSettings = toml::from_str(text).unwrap();
        assert!(settings.groups.is_empty());
        assert_eq!(settings.web_limit, WEB_LIMIT_DEFAULT);
    }

    #[test]
    fn a_connection_without_a_group_reads_as_empty() {
        let mut connection = Connection::new(ConnectionKind::OpenAiCompatible, "Local");
        assert!(connection.group.is_empty());
        connection.group = "Single".into();
        let text = toml::to_string(&connection).unwrap();
        let back: Connection = toml::from_str(&text).unwrap();
        assert_eq!(back.group, "Single");
        let old: Connection =
            toml::from_str("id = \"c1\"\nname = \"a\"\nkind = \"ollama\"\n").unwrap();
        assert!(old.group.is_empty());
    }

    #[test]
    fn group_names_are_short_latin_words() {
        for name in ["A", "Single", "local", "g2", "abcdefghijklmnop"] {
            assert!(Group::valid_name(name), "{name}");
        }
        for name in [
            "",
            "1group",
            "with space",
            "underscore_group",
            "abcdefghijklmnopq",
            "группа",
            "a-b",
            "a.b",
        ] {
            assert!(!Group::valid_name(name), "{name}");
        }
    }

    #[test]
    fn groups_are_added_and_removed_by_name() {
        let mut settings = AgentSettings::default();
        assert!(settings.add_group("Local", 4));
        assert!(!settings.add_group("Local", 2), "a duplicate is refused");
        assert!(!settings.add_group("single", 0), "a bad name is refused");
        assert_eq!(settings.group("Local"), Some(&Group::new("Local", 4)));
        let mut connection = Connection::new(ConnectionKind::OpenAiCompatible, "Local");
        connection.group = "Local".into();
        settings.connections.push(connection);
        settings.remove_group("Local");
        assert!(settings.group("Local").is_none());
        assert!(
            settings.connections[0].group.is_empty(),
            "the providers become unattached"
        );
    }

    #[test]
    fn an_unattached_provider_has_no_place() {
        let mut settings = AgentSettings::default();
        let connection = Connection::new(ConnectionKind::OpenAiCompatible, "Local");
        settings.connections.push(connection);
        assert_eq!(settings.place(&settings.connections[0].id), None);
        settings.connections[0].group = "Single".into();
        assert_eq!(settings.place(&settings.connections[0].id), Some("Single"));
    }

    #[test]
    fn speech_is_a_separate_connection() {
        assert!(Connection::new(ConnectionKind::OpenAiCompatible, "Custom")
            .supports(Capability::Transcription));
        assert!(Connection::new(ConnectionKind::Transcription, "Speech")
            .supports(Capability::Transcription));
        assert!(
            !Connection::new(ConnectionKind::Transcription, "Speech").supports(Capability::Chat)
        );
        assert!(
            !Connection::new(ConnectionKind::DeepSeek, "DeepSeek").supports(Capability::Embeddings)
        );
    }
}
