//! Providers over the OpenAI-style API: checks (the key is accepted, the model exists) and chat
//! completions. Each provider's requests go through its own proxy, or directly (never through a
//! system proxy).

use mediaa_core::agent::{Provider, ProviderKind};
use std::time::Duration;

/// Hugging Face Hub API (account checks).
const HF_HUB: &str = "https://huggingface.co";

/// Parses a proxy address (`http://`, `https://`, `socks5://`).
pub fn proxy(url: &str) -> anyhow::Result<reqwest::Proxy> {
    reqwest::Proxy::all(url).map_err(|e| anyhow::anyhow!("proxy \"{url}\": {e}"))
}

fn client(p: &Provider, timeout: Duration) -> anyhow::Result<reqwest::blocking::Client> {
    let b = reqwest::blocking::Client::builder().timeout(timeout);
    let b = match p.proxy() {
        Some(url) => b.proxy(proxy(url)?),
        None => b.no_proxy(),
    };
    Ok(b.build()?)
}

fn get(p: &Provider, url: &str) -> anyhow::Result<serde_json::Value> {
    let mut req = client(p, Duration::from_secs(20))?.get(url);
    if !p.api_key.trim().is_empty() {
        req = req.bearer_auth(p.api_key.trim());
    }
    let resp = req.send().map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
    match resp.status().as_u16() {
        200..=299 => Ok(resp.json()?),
        401 | 403 => anyhow::bail!("the API key was rejected ({})", resp.status()),
        _ => anyhow::bail!("{url}: {}", resp.status()),
    }
}

/// Answers longer than this are given up on (a long answer of a slow model).
const CHAT_TIMEOUT: Duration = Duration::from_secs(300);

/// A streamed chat completion (`"stream": true` is set): `on_chunk` gets each chunk as it arrives
/// (server-sent events): its `choices[0].delta`, and at the end its `usage` when the server sends it.
pub fn chat_completion_stream(p: &Provider, body: &serde_json::Value, on_chunk: &mut dyn FnMut(&serde_json::Value)) -> anyhow::Result<()> {
    use std::io::BufRead;
    let url = format!("{}/chat/completions", p.api_url());
    let mut body = body.clone();
    body["stream"] = serde_json::Value::Bool(true);
    // The last chunk then carries the tokens used.
    body["stream_options"] = serde_json::json!({"include_usage": true});
    let mut req = client(p, CHAT_TIMEOUT)?.post(&url).json(&body);
    if !p.api_key.trim().is_empty() {
        req = req.bearer_auth(p.api_key.trim());
    }
    let resp = req.send().map_err(|e| anyhow::anyhow!("{url}: {e}"))?;
    let status = resp.status();
    match status.as_u16() {
        200..=299 => {}
        401 | 403 => anyhow::bail!("the API key was rejected ({status})"),
        429 => anyhow::bail!("{}: too many requests, try later ({status})", p.name),
        _ => anyhow::bail!("{url}: {status} {}", resp.text().unwrap_or_default().chars().take(300).collect::<String>()),
    }
    for line in std::io::BufReader::new(resp).lines() {
        let line = line?;
        let Some(data) = line.strip_prefix("data:").map(str::trim) else { continue };
        if data == "[DONE]" {
            break;
        }
        let chunk: serde_json::Value = serde_json::from_str(data).map_err(|e| anyhow::anyhow!("{url}: a broken event ({e}): {data}"))?;
        if let Some(e) = chunk.get("error") {
            anyhow::bail!("{}: {e}", p.name);
        }
        on_chunk(&chunk);
    }
    Ok(())
}

/// Model ids the provider serves; fails on a wrong key or an unreachable server. OpenRouter
/// and Hugging Face list models without a key, so their keys are checked separately (a Hugging
/// Face token is optional).
pub fn list_models(p: &Provider) -> anyhow::Result<Vec<String>> {
    match p.kind {
        ProviderKind::OpenRouter => {
            anyhow::ensure!(!p.api_key.trim().is_empty(), "no API key");
            get(p, &format!("{}/key", p.api_url()))?;
        }
        ProviderKind::HuggingFace if !p.api_key.trim().is_empty() => {
            get(p, &format!("{HF_HUB}/api/whoami-v2"))?;
        }
        _ => {}
    }
    let body = get(p, &format!("{}/models", p.api_url()))?;
    let ids: Vec<String> = body["data"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("unexpected /models answer"))?
        .iter()
        .filter_map(|m| m["id"].as_str().map(String::from))
        .collect();
    Ok(ids)
}

/// The provider accepts the key and serves `model`.
pub fn check_model(p: &Provider, model: &str) -> anyhow::Result<()> {
    let ids = list_models(p)?;
    anyhow::ensure!(ids.iter().any(|id| id == model.trim()), "{} does not serve \"{}\"", p.name, model.trim());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An OpenAI-style server: `/models` lists `gpt-x`, the key must be "good". Also answers
    /// proxied requests (absolute URLs), counting them.
    fn server() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = srv.server_addr().to_ip().unwrap();
        let proxied = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let count = proxied.clone();
        std::thread::spawn(move || {
            for req in srv.incoming_requests() {
                if req.url().starts_with("http://") {
                    count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                let auth_ok = req.headers().iter().any(|h| h.field.equiv("Authorization") && h.value.as_str() == "Bearer good");
                let resp = match (req.url().ends_with("/v1/models"), auth_ok) {
                    (_, false) => tiny_http::Response::from_string("{}").with_status_code(401),
                    (true, true) => tiny_http::Response::from_string(r#"{"data":[{"id":"gpt-x"},{"id":"emb-1"}]}"#),
                    (false, true) => tiny_http::Response::from_string("{}").with_status_code(404),
                };
                let _ = req.respond(resp);
            }
        });
        (format!("http://{addr}"), proxied)
    }

    #[test]
    fn keys_and_models_are_checked() {
        let (url, _) = server();
        let mut p = Provider::new(ProviderKind::OpenAiCompatible, "Local");
        p.base_url = format!("{url}/v1/");
        p.api_key = "good".into();
        assert_eq!(list_models(&p).unwrap(), ["gpt-x", "emb-1"]);
        assert!(check_model(&p, " gpt-x ").is_ok());
        let err = check_model(&p, "other").unwrap_err().to_string();
        assert!(err.contains("other"), "{err}");
        p.api_key = "bad".into();
        assert!(list_models(&p).unwrap_err().to_string().contains("rejected"));
        p.base_url = "http://127.0.0.1:9/v1".into();
        assert!(list_models(&p).is_err(), "unreachable server");
    }

    #[test]
    fn requests_go_through_the_provider_proxy() {
        let (url, proxied) = server();
        let mut p = Provider::new(ProviderKind::OpenAiCompatible, "Behind proxy");
        // Not reachable directly: only the proxy knows how to answer.
        p.base_url = "http://models.invalid/v1".into();
        p.api_key = "good".into();
        assert!(list_models(&p).is_err());
        p.proxy = url;
        assert_eq!(list_models(&p).unwrap().len(), 2);
        assert_eq!(proxied.load(std::sync::atomic::Ordering::SeqCst), 1);
        p.proxy = "::bad::".into();
        assert!(list_models(&p).unwrap_err().to_string().contains("proxy"));
    }

    #[test]
    fn openrouter_needs_a_key() {
        let p = Provider::new(ProviderKind::OpenRouter, "OR");
        assert!(list_models(&p).unwrap_err().to_string().contains("no API key"));
    }
}
