use crate::engine::{
    apply_embed_prefix, phrases_from_words, shift_phrases, ChatAnswer, ChatRequest,
    EmbeddingsRequest, EngineError, RerankRequest, RerankResult, SpeechBody, ToolCall, ToolChoice,
    TranscribeRequest, Usage,
};
use crate::presets::{self, Reasoning};
use mediaa_core::agent::Connection;
use mediaa_core::transcript::Word;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

pub const CHAT_TIMEOUT: Duration = Duration::from_secs(300);
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);

pub fn agent(proxy: Option<&str>, timeout: Duration) -> Result<ureq::Agent, EngineError> {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .http_status_as_error(false);
    if let Some(url) = proxy {
        let parsed = ureq::Proxy::new(url)
            .map_err(|error| EngineError::bad_request(format!("proxy \"{url}\": {error}")))?;
        builder = builder.proxy(Some(parsed));
    }
    Ok(builder.build().new_agent())
}

fn bearer(connection: &Connection) -> Option<String> {
    let key = connection.api_key.trim();
    (!key.is_empty()).then(|| format!("Bearer {key}"))
}

fn status_error(connection: &Connection, url: &str, status: u16, body: &str) -> EngineError {
    let message = match status {
        401 | 403 => format!("the API key was rejected ({status})"),
        429 => format!(
            "{}: too many requests, try later ({status})",
            connection.name
        ),
        _ => {
            let text: String = body.chars().take(300).collect();
            format!("{url}: {status} {text}")
        }
    };
    EngineError::bad_request(message)
}

fn read_limited(response: &mut ureq::http::Response<ureq::Body>, limit: usize) -> String {
    let mut buf = String::new();
    let _ = response
        .body_mut()
        .as_reader()
        .take(limit as u64)
        .read_to_string(&mut buf);
    buf
}

fn send_json(
    agent: &ureq::Agent,
    connection: &Connection,
    url: &str,
    body: &Value,
) -> Result<Value, EngineError> {
    let mut request = agent.post(url);
    if let Some(token) = bearer(connection) {
        request = request.header("Authorization", &token);
    }
    let response = request
        .send_json(body)
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let mut response = response;
        let text = read_limited(&mut response, 4000);
        return Err(status_error(connection, url, status, &text));
    }
    response
        .into_body()
        .read_json()
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))
}

fn get_json(agent: &ureq::Agent, connection: &Connection, url: &str) -> Result<Value, EngineError> {
    let mut request = agent.get(url);
    if let Some(token) = bearer(connection) {
        request = request.header("Authorization", &token);
    }
    let response = request
        .call()
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let mut response = response;
        let text = read_limited(&mut response, 4000);
        return Err(status_error(connection, url, status, &text));
    }
    response
        .into_body()
        .read_json()
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))
}

pub fn list_model_ids(
    agent: &ureq::Agent,
    connection: &Connection,
) -> Result<Vec<String>, EngineError> {
    if connection.kind == mediaa_core::agent::ConnectionKind::OpenRouter
        && connection.api_key.trim().is_empty()
    {
        return Err(EngineError::bad_request("no API key"));
    }
    if connection.kind == mediaa_core::agent::ConnectionKind::OpenRouter {
        get_json(agent, connection, &format!("{}/key", connection.api_url()))?;
    }
    if connection.kind == mediaa_core::agent::ConnectionKind::HuggingFace
        && !connection.api_key.trim().is_empty()
    {
        get_json(agent, connection, "https://huggingface.co/api/whoami-v2")?;
    }
    let body = get_json(
        agent,
        connection,
        &format!("{}/models", connection.api_url()),
    )?;
    let ids = body["data"]
        .as_array()
        .ok_or_else(|| EngineError::bad_request("unexpected /models answer"))?
        .iter()
        .filter_map(|model| model["id"].as_str().map(str::to_string))
        .collect();
    Ok(ids)
}

fn chat_body(connection: &Connection, request: &ChatRequest) -> Value {
    let messages: Vec<Value> = request
        .messages
        .iter()
        .map(|message| json!({"role": message.role.as_str(), "content": message.content}))
        .collect();
    let mut body = json!({
        "model": request.source.model,
        "messages": messages,
        "max_tokens": request.max_tokens,
        "temperature": request.temperature,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if request.thinking.is_some() {
        let word = match request.source.thinking_depth {
            mediaa_core::agent::ReasoningDepth::Low => "low",
            mediaa_core::agent::ReasoningDepth::Medium => "medium",
            mediaa_core::agent::ReasoningDepth::High => "high",
            mediaa_core::agent::ReasoningDepth::XHigh => "xhigh",
        };
        match presets::reasoning(connection.kind) {
            Reasoning::OpenRouter => body["reasoning"] = json!({"effort": word}),
            Reasoning::Effort => body["reasoning_effort"] = json!(word),
            Reasoning::None => {}
        }
    }
    if let Some(schema) = &request.json_schema {
        body["response_format"] = json!({
            "type": "json_schema",
            "json_schema": {"name": "answer", "strict": true, "schema": schema}
        });
    }
    if !request.tools.is_empty() && request.tool_choice != ToolChoice::None {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name,
                        "description": tool.description,
                        "parameters": tool.parameters,
                    }
                })
            })
            .collect();
        body["tools"] = Value::Array(tools);
        body["tool_choice"] = json!(match request.tool_choice {
            ToolChoice::Required => "required",
            ToolChoice::Auto | ToolChoice::None => "auto",
        });
        body["parallel_tool_calls"] = json!(true);
    }
    body
}

pub fn chat(
    agent: &ureq::Agent,
    connection: &Connection,
    request: &ChatRequest,
    cancel: &AtomicBool,
    on_text: &mut dyn FnMut(&str) -> bool,
) -> Result<ChatAnswer, EngineError> {
    let url = format!("{}/chat/completions", connection.api_url());
    let mut call = agent.post(&url);
    if let Some(token) = bearer(connection) {
        call = call.header("Authorization", &token);
    }
    let response = call
        .send_json(chat_body(connection, request))
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    mediaa_core::pulse::beat();
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let mut response = response;
        let text = read_limited(&mut response, 4000);
        return Err(status_error(connection, &url, status, &text));
    }
    let mut answer = ChatAnswer::default();
    let mut calls: std::collections::BTreeMap<u64, (String, String)> =
        std::collections::BTreeMap::new();
    let mut response = response;
    let reader = response.body_mut().as_reader();
    for line in BufReader::new(reader).lines() {
        mediaa_core::pulse::beat();
        if cancel.load(Ordering::Relaxed) {
            return Err(EngineError::cancelled("cancelled"));
        }
        let line = line.map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
        let Some(data) = line.strip_prefix("data:").map(str::trim) else {
            continue;
        };
        if data == "[DONE]" {
            break;
        }
        let chunk: Value = serde_json::from_str(data).map_err(|error| {
            EngineError::internal(format!("{url}: a broken event ({error}): {data}"))
        })?;
        if let Some(error) = chunk.get("error") {
            return Err(EngineError::bad_request(format!(
                "{}: {error}",
                connection.name
            )));
        }
        if let Some(usage) = chunk.get("usage").filter(|usage| usage.is_object()) {
            answer.usage = Some(Usage {
                prompt_tokens: usage["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                completion_tokens: usage["completion_tokens"].as_u64().unwrap_or(0) as u32,
            });
        }
        let delta = &chunk["choices"][0]["delta"];
        if let Some(thought) = delta["reasoning_content"]
            .as_str()
            .or_else(|| delta["reasoning"].as_str())
            .filter(|text| !text.is_empty())
        {
            answer.reasoning.push_str(thought);
        }
        if let Some(tool_calls) = delta["tool_calls"].as_array() {
            for call in tool_calls {
                let slot = calls
                    .entry(call["index"].as_u64().unwrap_or(0))
                    .or_default();
                slot.0
                    .push_str(call["function"]["name"].as_str().unwrap_or_default());
                slot.1
                    .push_str(call["function"]["arguments"].as_str().unwrap_or_default());
            }
        }
        if let Some(piece) = delta["content"].as_str().filter(|piece| !piece.is_empty()) {
            answer.content.push_str(piece);
            if !on_text(piece) {
                break;
            }
        }
    }
    answer.tool_calls = calls
        .into_values()
        .map(|(name, arguments)| ToolCall { name, arguments })
        .collect();
    Ok(answer)
}

pub fn embeddings(
    agent: &ureq::Agent,
    connection: &Connection,
    request: &EmbeddingsRequest,
) -> Result<Vec<Vec<f32>>, EngineError> {
    let input = apply_embed_prefix(&request.source.model, request.kind, &request.input);
    let url = format!("{}/embeddings", connection.api_url());
    let body = send_json(
        agent,
        connection,
        &url,
        &json!({"model": request.source.model, "input": input}),
    )?;
    let rows = body["data"]
        .as_array()
        .ok_or_else(|| EngineError::bad_request("unexpected /embeddings answer"))?;
    rows.iter()
        .map(|row| read_vector(&row["embedding"]))
        .collect()
}

pub fn rerank(
    agent: &ureq::Agent,
    connection: &Connection,
    request: &RerankRequest,
) -> Result<Vec<RerankResult>, EngineError> {
    let url = format!("{}/rerank", connection.api_url());
    let body = send_json(
        agent,
        connection,
        &url,
        &json!({
            "model": request.source.model,
            "query": request.query,
            "documents": request.documents,
            "top_n": request.documents.len(),
        }),
    )?;
    let rows = body["results"]
        .as_array()
        .ok_or_else(|| EngineError::bad_request("unexpected /rerank answer"))?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(RerankResult {
                index: row["index"].as_u64()? as usize,
                relevance_score: row["relevance_score"].as_f64()? as f32,
            })
        })
        .collect())
}

pub fn multipart_body(fields: &[(&str, &str)], file_name: &str, wav: &[u8]) -> (String, Vec<u8>) {
    let boundary = "mediaa-audio-boundary";
    let mut body = Vec::new();
    for (name, value) in fields {
        body.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n").as_bytes());
    }
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{file_name}\"\r\nContent-Type: audio/wav\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(wav);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn post_audio(
    agent: &ureq::Agent,
    connection: &Connection,
    request: &TranscribeRequest,
    wav: &[u8],
    url: &str,
    format: Option<&str>,
    word_times: bool,
) -> Result<Value, EngineError> {
    let mut fields = vec![("model", request.source.model.as_str())];
    if let Some(format) = format {
        fields.push(("response_format", format));
    }
    if word_times {
        fields.push(("timestamp_granularities[]", "word"));
    }
    let language = request.language.clone().unwrap_or_default();
    if !language.is_empty() {
        fields.push(("language", language.as_str()));
    }
    let (content_type, body) = multipart_body(&fields, "window.wav", wav);
    let mut call = agent.post(url).content_type(&content_type);
    if let Some(token) = bearer(connection) {
        call = call.header("Authorization", &token);
    }
    let response = call
        .send(&body)
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        let mut response = response;
        let text = read_limited(&mut response, 4000);
        return Err(status_error(connection, url, status, &text));
    }
    response
        .into_body()
        .read_json()
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))
}

fn text_phrase(body: &Value, from: f64, to: f64) -> Vec<mediaa_core::transcript::Phrase> {
    let text = body["text"].as_str().unwrap_or("").trim().to_string();
    if text.is_empty() {
        return Vec::new();
    }
    let end = from + (to - from).max(0.0);
    phrases_from_words(vec![Word {
        text,
        start: from,
        end,
    }])
}

fn speaker_ids(body: &Value) -> std::collections::BTreeMap<String, u32> {
    let mut ids = std::collections::BTreeMap::new();
    let mut next = 0u32;
    let remember =
        |ids: &mut std::collections::BTreeMap<String, u32>, next: &mut u32, name: &str| {
            ids.entry(name.to_string()).or_insert_with(|| {
                let id = *next;
                *next += 1;
                id
            });
        };
    if let Some(words) = body["words"].as_array() {
        for word in words {
            if let Some(name) = word["speaker"].as_str() {
                remember(&mut ids, &mut next, name);
            }
        }
    }
    if let Some(segments) = body["segments"].as_array() {
        for segment in segments {
            if let Some(name) = segment["speaker"].as_str() {
                remember(&mut ids, &mut next, name);
            }
        }
    }
    ids
}

fn turns_of(body: &Value, from: f64) -> Vec<mediaa_core::transcript::Turn> {
    let ids = speaker_ids(body);
    body["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|segment| {
            let name = segment["speaker"].as_str()?;
            Some(mediaa_core::transcript::Turn {
                start: segment["start"].as_f64()? + from,
                end: segment["end"].as_f64()? + from,
                speaker: *ids.get(name)?,
            })
        })
        .collect()
}

fn phrases_from_segments(body: &Value, from: f64) -> Vec<mediaa_core::transcript::Phrase> {
    let words = body["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|segment| {
            let text = segment["text"].as_str()?.trim().to_string();
            if text.is_empty() {
                return None;
            }
            Some(Word {
                text,
                start: segment["start"].as_f64()? + from,
                end: segment["end"].as_f64()? + from,
            })
        })
        .collect();
    phrases_from_words(words)
}

fn voices_of(body: &Value) -> Vec<Vec<f32>> {
    let ids = speaker_ids(body);
    let mut voices = vec![Vec::new(); ids.len()];
    let Some(speakers) = body["speakers"].as_array() else {
        return voices;
    };
    for speaker in speakers {
        let Some(name) = speaker["id"].as_str() else {
            continue;
        };
        let Some(index) = ids.get(name).copied() else {
            continue;
        };
        let vector = speaker["embedding"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| value.as_f64().map(|number| number as f32))
            .collect();
        if let Some(slot) = voices.get_mut(index as usize) {
            *slot = vector;
        }
    }
    voices
}

pub fn transcribe(
    agent: &ureq::Agent,
    connection: &Connection,
    request: &TranscribeRequest,
    wav: &[u8],
) -> Result<SpeechBody, EngineError> {
    if connection.kind == mediaa_core::agent::ConnectionKind::Transcription {
        let url = format!("{}/v1/audio/transcriptions", connection.api_url());
        let parsed = post_audio(agent, connection, request, wav, &url, None, false)?;
        let words = words_of(&parsed);
        let phrases = if words.is_empty() {
            if request.word_times {
                return Err(EngineError::bad_request("the transcript has no word times"));
            }
            if parsed["segments"]
                .as_array()
                .is_some_and(|rows| !rows.is_empty())
            {
                phrases_from_segments(&parsed, request.from)
            } else {
                text_phrase(&parsed, request.from, request.to)
            }
        } else {
            shift_phrases(phrases_from_words(words), request.from)
        };
        let turns = if request.speakers {
            turns_of(&parsed, request.from)
        } else {
            Vec::new()
        };
        let voices = if request.speakers && request.voiceprint {
            voices_of(&parsed)
        } else {
            Vec::new()
        };
        return Ok(SpeechBody {
            phrases,
            turns,
            voices,
        });
    }
    let url = format!("{}/audio/transcriptions", connection.api_url());
    let mut body = SpeechBody::default();
    if request.word_times {
        let parsed = post_audio(
            agent,
            connection,
            request,
            wav,
            &url,
            Some("verbose_json"),
            true,
        )?;
        let words = words_of(&parsed);
        if words.is_empty() {
            return Err(EngineError::bad_request("the transcript has no word times"));
        }
        body.phrases = shift_phrases(phrases_from_words(words), request.from);
    }
    if request.speakers {
        let parsed = post_audio(
            agent,
            connection,
            request,
            wav,
            &url,
            Some("diarized_json"),
            false,
        )?;
        body.turns = turns_of(&parsed, request.from);
        if body.phrases.is_empty() {
            body.phrases = phrases_from_segments(&parsed, request.from);
        }
    }
    if !request.word_times && !request.speakers {
        let parsed = post_audio(agent, connection, request, wav, &url, Some("json"), false)?;
        body.phrases = text_phrase(&parsed, request.from, request.to);
    }
    Ok(body)
}

fn words_of(body: &Value) -> Vec<Word> {
    body["words"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|word| {
            Some(Word {
                text: word["word"].as_str()?.trim().to_string(),
                start: word["start"].as_f64()?,
                end: word["end"].as_f64()?,
            })
        })
        .filter(|word| !word.text.is_empty())
        .collect()
}

fn read_vector(value: &Value) -> Result<Vec<f32>, EngineError> {
    let rows = value
        .as_array()
        .ok_or_else(|| EngineError::bad_request("embedding is not an array"))?;
    Ok(rows
        .iter()
        .filter_map(|item| item.as_f64().map(|number| number as f32))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{Connection, ConnectionKind};
    use std::sync::atomic::AtomicUsize;
    use std::sync::Arc;

    fn server(
        script: impl Fn(&str, &[u8], bool) -> tiny_http::Response<std::io::Cursor<Vec<u8>>>
            + Send
            + 'static,
    ) -> (String, Arc<AtomicUsize>) {
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = srv.server_addr().to_ip().unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let count = hits.clone();
        std::thread::spawn(move || {
            for mut request in srv.incoming_requests() {
                let url = request.url().to_string();
                let auth_ok = request.headers().iter().any(|header| {
                    header.field.equiv("Authorization") && header.value.as_str() == "Bearer good"
                });
                if url.starts_with("http://") {
                    count.fetch_add(1, Ordering::SeqCst);
                }
                let mut body = Vec::new();
                let _ = request.as_reader().read_to_end(&mut body);
                let _ = request.respond(script(&url, &body, auth_ok));
            }
        });
        (format!("http://{addr}"), hits)
    }

    fn text(body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
        tiny_http::Response::from_string(body)
    }

    fn connection(url: &str) -> Connection {
        let mut connection = Connection::new(ConnectionKind::OpenAiCompatible, "Local");
        connection.base_url = format!("{url}/v1");
        connection.api_key = "good".into();
        connection
    }

    #[test]
    fn models_reject_a_bad_key_and_follow_the_proxy() {
        let (url, _proxied) = server(|url, _body, ok| {
            if url.contains("/models") && ok {
                text(r#"{"data":[{"id":"gpt-x"},{"id":"emb-1"}]}"#)
            } else if !ok {
                text("{}").with_status_code(401)
            } else {
                text("{}").with_status_code(404)
            }
        });
        let http = agent(None, CALL_TIMEOUT).unwrap();
        let mut local = connection(&url);
        let ids = list_model_ids(&http, &local).unwrap();
        assert_eq!(ids, ["gpt-x", "emb-1"]);
        local.api_key = "bad".into();
        assert!(list_model_ids(&http, &local)
            .unwrap_err()
            .to_string()
            .contains("rejected"));
        local.api_key = "good".into();
        local.proxy = "::bad::".into();
        let error = agent(local.proxy(), CALL_TIMEOUT).unwrap_err().to_string();
        assert!(error.contains("proxy"), "{error}");
    }

    #[test]
    fn a_stream_collects_text_tools_reasoning_and_usage() {
        let (url, _) = server(|_url, body, _ok| {
            let body = String::from_utf8_lossy(body);
            assert!(body.contains("\"stream\""), "{body}");
            let payload = concat!(
                "data: {\"choices\":[{\"delta\":{\"reasoning_content\":\"think\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"content\":\"Hi\"}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"name\":\"look\",\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\n",
                "data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"x\\\"}\"}}]}}]}\n\n",
                "data: {\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2}}\n\n",
                "data: [DONE]\n\n",
            );
            text(payload).with_header(
                tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/event-stream"[..])
                    .unwrap(),
            )
        });
        let local = connection(&url);
        let http = agent(None, CHAT_TIMEOUT).unwrap();
        let mut seen = String::new();
        let answer = chat(
            &http,
            &local,
            &ChatRequest {
                source: mediaa_core::agent::ModelSource::new(&local.id, "gpt-x"),
                messages: vec![crate::engine::ChatMessage::user("hi")],
                max_tokens: 16,
                temperature: 0.0,
                tools: vec![crate::engine::ToolSpec {
                    name: "look".into(),
                    description: "look".into(),
                    parameters: json!({"type": "object"}),
                }],
                tool_choice: ToolChoice::Auto,
                ..ChatRequest::default()
            },
            &AtomicBool::new(false),
            &mut |text| {
                seen.push_str(text);
                true
            },
        )
        .unwrap();
        assert_eq!(seen, "Hi");
        assert_eq!(answer.reasoning, "think");
        assert_eq!(answer.tool_calls.len(), 1);
        assert_eq!(answer.tool_calls[0].name, "look");
        assert_eq!(answer.tool_calls[0].arguments, "{\"q\":\"x\"}");
        assert_eq!(answer.usage.unwrap().prompt_tokens, 3);
    }

    #[test]
    fn embeddings_rerank_and_transcription_parse() {
        let (url, _) = server(|url, body, _ok| {
            if url.contains("/embeddings") {
                text(r#"{"data":[{"embedding":[0.5,1.5]}]}"#)
            } else if url.contains("/rerank") {
                text(r#"{"results":[{"index":1,"relevance_score":0.25}]}"#)
            } else if url.contains("/audio/transcriptions") {
                let text_body = String::from_utf8_lossy(body);
                assert!(text_body.contains("name=\"model\""), "{text_body}");
                assert!(text_body.contains("verbose_json"), "{text_body}");
                assert!(
                    text_body.contains("timestamp_granularities[]"),
                    "{text_body}"
                );
                assert!(text_body.contains("audio/wav"), "{text_body}");
                text(r#"{"text":"hi","words":[{"word":"hi","start":0.1,"end":0.4}]}"#)
            } else {
                text("{}").with_status_code(404)
            }
        });
        let local = connection(&url);
        let http = agent(None, CALL_TIMEOUT).unwrap();
        let source = mediaa_core::agent::ModelSource::new(&local.id, "gpt-x");
        let vectors = embeddings(
            &http,
            &local,
            &EmbeddingsRequest {
                source: source.clone(),
                input: vec!["hello".into()],
                kind: crate::engine::EmbedKind::Query,
            },
        )
        .unwrap();
        assert_eq!(vectors, vec![vec![0.5, 1.5]]);
        let ranked = rerank(
            &http,
            &local,
            &RerankRequest {
                source: source.clone(),
                query: "q".into(),
                documents: vec!["a".into(), "b".into()],
            },
        )
        .unwrap();
        assert_eq!(ranked[0].index, 1);
        let phrases = transcribe(
            &http,
            &local,
            &TranscribeRequest {
                source,
                path: "x.wav".into(),
                from: 10.0,
                to: 12.0,
                language: Some("en".into()),
                word_times: true,
                speakers: false,
                voiceprint: false,
            },
            b"RIFFxxxx",
        )
        .unwrap();
        assert!((phrases.phrases[0].words[0].start - 10.1).abs() < 1e-6);
    }

    #[test]
    fn too_many_requests_keep_their_text() {
        let (url, _) = server(|_, _, _| text("slow").with_status_code(429));
        let local = connection(&url);
        let http = agent(None, CALL_TIMEOUT).unwrap();
        let error = list_model_ids(&http, &local).unwrap_err().to_string();
        assert!(error.contains("too many requests"), "{error}");
    }
}
