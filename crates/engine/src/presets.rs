use mediaa_core::agent::ConnectionKind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reasoning {
    None,
    OpenRouter,
    Effort,
}

pub fn reasoning(kind: ConnectionKind) -> Reasoning {
    match kind {
        ConnectionKind::OpenRouter => Reasoning::OpenRouter,
        ConnectionKind::DeepSeek
        | ConnectionKind::HuggingFace
        | ConnectionKind::OpenAiCompatible => Reasoning::Effort,
        ConnectionKind::Ollama | ConnectionKind::Transcription => Reasoning::None,
    }
}

pub fn embeds_via_ollama(kind: ConnectionKind) -> bool {
    kind == ConnectionKind::Ollama
}
