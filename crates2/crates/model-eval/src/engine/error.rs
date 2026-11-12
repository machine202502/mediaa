use std::fmt;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineError {
    pub kind: &'static str,
    pub message: String,
    /// From the `Retry-After` header of a 429/503 answer, if the server sent one.
    pub retry_after: Option<Duration>,
}

impl EngineError {
    pub fn new(kind: &'static str, message: impl Into<String>) -> EngineError {
        EngineError {
            kind,
            message: message.into(),
            retry_after: None,
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

    /// The provider rejected the credentials (401/403) — retrying cannot help.
    pub fn unauthorized(message: impl Into<String>) -> EngineError {
        EngineError::new("unauthorized", message)
    }

    /// The provider is limiting the request rate (429).
    pub fn rate_limited(message: impl Into<String>, retry_after: Option<Duration>) -> EngineError {
        let mut error = EngineError::new("rate_limited", message);
        error.retry_after = retry_after;
        error
    }

    /// The server or the network is unreachable (transport error, 5xx).
    pub fn network(message: impl Into<String>) -> EngineError {
        EngineError::new("network", message)
    }

    pub fn from_anyhow(error: anyhow::Error) -> EngineError {
        let message = format!("{error:#}");
        let lower = message.to_lowercase();
        if lower.contains("cancel") {
            EngineError::cancelled(message)
        } else if lower.contains("not found") || lower.contains("does not serve") {
            EngineError::not_found(message)
        } else if lower.contains("rejected") || lower.contains("proxy ") {
            EngineError::unauthorized(message)
        } else if lower.contains("too many") {
            EngineError::rate_limited(message, None)
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
    fn status_kinds_carry_their_retry_after() {
        let limited = EngineError::rate_limited("too many", Some(Duration::from_secs(3)));
        assert_eq!(limited.kind, "rate_limited");
        assert_eq!(limited.retry_after, Some(Duration::from_secs(3)));

        let rejected = EngineError::unauthorized("the API key was rejected (401)");
        assert_eq!(rejected.kind, "unauthorized");
        assert_eq!(rejected.retry_after, None);
    }

    #[test]
    fn anyhow_classification_separates_auth_from_limits() {
        assert_eq!(
            EngineError::from_anyhow(anyhow::anyhow!("the API key was rejected")).kind,
            "unauthorized"
        );
        assert_eq!(
            EngineError::from_anyhow(anyhow::anyhow!("provider: too many requests")).kind,
            "rate_limited"
        );
    }
}
