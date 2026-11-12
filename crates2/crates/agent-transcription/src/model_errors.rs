//! Raw provider failures mapped to stable kinds the UI can explain.

/// Classifies a model-call error by the stable phrases our engine puts into
/// its messages. The UI translates the kind into a human sentence and shows
/// the raw text as detail.
pub fn model_error_kind(error: &str) -> &'static str {
    let lower = error.to_lowercase();
    if lower.contains("api key was rejected") {
        "unauthorized"
    } else if lower.contains("too many requests") {
        "rate_limited"
    } else if lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("timed out")
        || lower.contains("timeout")
        || lower.contains("dns error")
        || lower.contains("temporarily unavailable")
        || lower.contains("proxy")
    {
        "network"
    } else if lower.contains("not found") || lower.contains("does not serve") {
        "not_found"
    } else {
        "other"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_phrases_map_to_their_kinds() {
        assert_eq!(
            model_error_kind("http://x/v1: the API key was rejected (401)"),
            "unauthorized"
        );
        assert_eq!(
            model_error_kind("OpenRouter: too many requests, try later (429)"),
            "rate_limited"
        );
        assert_eq!(
            model_error_kind("http://localhost:11434/v1: connection refused"),
            "network"
        );
        assert_eq!(
            model_error_kind("http://x/v1: 500Internal: timed out"),
            "network"
        );
        assert_eq!(
            model_error_kind("connection q1: model gpt-x was not found"),
            "not_found"
        );
        assert_eq!(model_error_kind("a broken event: <html>"), "other");
    }
}
