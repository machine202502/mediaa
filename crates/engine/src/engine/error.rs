use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub kind: &'static str,
    pub message: String,
}

impl EngineError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> EngineError {
        EngineError { kind, message: message.into() }
    }

    pub fn bad_request(message: impl Into<String>) -> EngineError {
        EngineError::new("bad_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> EngineError {
        EngineError::new("not_found", message)
    }

    pub fn model_unsupported(message: impl Into<String>) -> EngineError {
        EngineError::new("model_unsupported", message)
    }

    pub fn out_of_memory(message: impl Into<String>) -> EngineError {
        EngineError::new("out_of_memory", message)
    }

    pub fn cancelled(message: impl Into<String>) -> EngineError {
        EngineError::new("cancelled", message)
    }

    pub fn internal(message: impl Into<String>) -> EngineError {
        EngineError::new("internal", message)
    }

    pub fn classify(error: anyhow::Error) -> EngineError {
        let message = format!("{error:#}");
        let lower = message.to_lowercase();
        if lower.contains("more than the budget") {
            EngineError::out_of_memory(message)
        } else if lower.contains("cancelled") {
            EngineError::cancelled(message)
        } else if lower.contains("not an embedding model") || lower.contains("not a reranker") || lower.contains("does not load") {
            EngineError::model_unsupported(message)
        } else if lower.contains("not downloaded")
            || lower.contains("not found")
            || lower.contains("no such file")
            || lower.contains("no gguf model inside")
            || lower.contains("has no file")
        {
            EngineError::not_found(message)
        } else if lower.contains("grammar")
            || lower.contains("schema")
            || lower.contains("empty prompt")
            || lower.contains("context only")
            || lower.contains("nothing to embed")
            || lower.contains("not owner/name")
        {
            EngineError::bad_request(message)
        } else {
            EngineError::internal(message)
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for EngineError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_errors_get_their_kinds() {
        let kinds = |m: &str| EngineError::classify(anyhow::anyhow!("{m}")).kind;
        assert_eq!(kinds("model needs 9000 MB, more than the budget allows"), "out_of_memory");
        assert_eq!(kinds("cancelled"), "cancelled");
        assert_eq!(kinds("not an embedding model: no embeddings"), "model_unsupported");
        assert_eq!(kinds("/m/x.gguf not found"), "not_found");
        assert_eq!(kinds("a/b: no GGUF model inside"), "not_found");
        assert_eq!(kinds("the prompt has 9000 tokens, the context only 8192"), "bad_request");
        assert_eq!(kinds("something else broke"), "internal");
        assert_eq!(EngineError::bad_request("no model").to_string(), "no model");
    }
}
