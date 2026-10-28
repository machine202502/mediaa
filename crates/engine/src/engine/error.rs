use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub kind: &'static str,
    pub message: String,
}

impl EngineError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> EngineError {
        EngineError {
            kind,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> EngineError {
        EngineError::new("bad_request", message)
    }

    pub fn not_found(message: impl Into<String>) -> EngineError {
        EngineError::new("not_found", message)
    }

    pub fn unavailable(message: impl Into<String>) -> EngineError {
        EngineError::new("unavailable", message)
    }

    pub fn cancelled(message: impl Into<String>) -> EngineError {
        EngineError::new("cancelled", message)
    }

    pub fn internal(message: impl Into<String>) -> EngineError {
        EngineError::new("internal", message)
    }

    pub fn from_anyhow(error: anyhow::Error) -> EngineError {
        let message = format!("{error:#}");
        let lower = message.to_lowercase();
        if lower.contains("cancel") {
            EngineError::cancelled(message)
        } else if lower.contains("not found") || lower.contains("does not serve") {
            EngineError::not_found(message)
        } else if lower.contains("rejected")
            || lower.contains("proxy ")
            || lower.contains("too many")
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
