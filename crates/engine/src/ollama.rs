use crate::engine::{apply_embed_prefix, EmbeddingsRequest, EngineError, ModelInfo};
use crate::openai::{self, CALL_TIMEOUT};
use mediaa_core::agent::Connection;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

pub fn program_path(connection: &Connection) -> PathBuf {
    if connection.program.trim().is_empty() {
        PathBuf::from("ollama")
    } else {
        PathBuf::from(connection.program.trim())
    }
}

pub fn version(program: &Path) -> Result<String, EngineError> {
    let output = Command::new(program)
        .arg("--version")
        .output()
        .map_err(|error| EngineError::not_found(format!("{}: {error}", program.display())))?;
    if !output.status.success() {
        return Err(EngineError::not_found(format!(
            "{}: {}",
            program.display(),
            String::from_utf8_lossy(&output.stderr)
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn serve(program: &Path) -> Result<(), EngineError> {
    Command::new(program)
        .arg("serve")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| EngineError::internal(format!("{} serve: {error}", program.display())))?;
    Ok(())
}

pub fn tags(agent: &ureq::Agent, connection: &Connection) -> Result<Vec<ModelInfo>, EngineError> {
    let url = format!("{}/api/tags", connection.root_url());
    let response = agent
        .get(&url)
        .call()
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(EngineError::bad_request(format!("{url}: {status}")));
    }
    let body: Value = response
        .into_body()
        .read_json()
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let models = body["models"]
        .as_array()
        .ok_or_else(|| EngineError::bad_request("unexpected /api/tags answer"))?;
    Ok(models
        .iter()
        .filter_map(|model| {
            let name = model["name"].as_str()?.to_string();
            Some(ModelInfo {
                id: name.clone(),
                name,
            })
        })
        .collect())
}

pub fn embed(
    agent: &ureq::Agent,
    connection: &Connection,
    request: &EmbeddingsRequest,
) -> Result<Vec<Vec<f32>>, EngineError> {
    let url = format!("{}/api/embed", connection.root_url());
    let input = apply_embed_prefix(&request.source.model, request.kind, &request.input);
    let response = agent
        .post(&url)
        .send_json(json!({"model": request.source.model, "input": input}))
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(EngineError::bad_request(format!("{url}: {status}")));
    }
    let body: Value = response
        .into_body()
        .read_json()
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let rows = body["embeddings"]
        .as_array()
        .ok_or_else(|| EngineError::bad_request("unexpected /api/embed answer"))?;
    Ok(rows
        .iter()
        .map(|row| {
            row.as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value.as_f64().map(|number| number as f32))
                .collect()
        })
        .collect())
}

pub fn pull(
    connection: &Connection,
    model: &str,
    on_progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), EngineError> {
    let http = openai::agent(connection.proxy(), CALL_TIMEOUT)?;
    let url = format!("{}/api/pull", connection.root_url());
    let mut response = http
        .post(&url)
        .send_json(json!({"model": model, "stream": true}))
        .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(EngineError::bad_request(format!("{url}: {status}")));
    }
    let reader = response.body_mut().as_reader();
    for line in BufReader::new(reader).lines() {
        if cancel.load(Ordering::Relaxed) {
            return Err(EngineError::cancelled("cancelled"));
        }
        let line = line.map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
        if line.trim().is_empty() {
            continue;
        }
        let event: Value = serde_json::from_str(&line)
            .map_err(|error| EngineError::internal(format!("{url}: {error}: {line}")))?;
        if let Some(error) = event["error"].as_str() {
            return Err(EngineError::internal(error));
        }
        let done = event["completed"].as_u64().unwrap_or(0);
        let total = event["total"].as_u64().unwrap_or(0);
        if total > 0 {
            on_progress(done, total);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{Connection, ConnectionKind};
    use std::sync::atomic::AtomicBool;

    #[test]
    fn tags_and_embed_come_from_the_daemon() {
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = srv.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for request in srv.incoming_requests() {
                let body = if request.url().contains("/api/tags") {
                    r#"{"models":[{"name":"qwen3:4b"}]}"#
                } else {
                    r#"{"embeddings":[[0.25,0.75]]}"#
                };
                let _ = request.respond(tiny_http::Response::from_string(body));
            }
        });
        let mut connection = Connection::new(ConnectionKind::Ollama, "Ollama");
        connection.base_url = format!("http://{addr}");
        let http = openai::agent(None, CALL_TIMEOUT).unwrap();
        assert_eq!(tags(&http, &connection).unwrap()[0].id, "qwen3:4b");
        let vectors = embed(
            &http,
            &connection,
            &EmbeddingsRequest {
                source: mediaa_core::agent::ModelSource::new(&connection.id, "qwen3:4b"),
                input: vec!["hi".into()],
                kind: crate::engine::EmbedKind::Document,
            },
        )
        .unwrap();
        assert_eq!(vectors, vec![vec![0.25, 0.75]]);
        assert!(version(Path::new("ollama-missing-binary")).is_err());
    }

    #[test]
    fn pull_reports_progress_and_stops_when_cancelled() {
        let srv = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let addr = srv.server_addr().to_ip().unwrap();
        std::thread::spawn(move || {
            for request in srv.incoming_requests() {
                let body = "{\"completed\":1,\"total\":2}\n{\"completed\":2,\"total\":2}\n";
                let _ = request.respond(tiny_http::Response::from_string(body));
            }
        });
        let mut connection = Connection::new(ConnectionKind::Ollama, "Ollama");
        connection.base_url = format!("http://{addr}");
        let mut seen = Vec::new();
        pull(
            &connection,
            "qwen3",
            &mut |done, total| seen.push((done, total)),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(seen, vec![(1, 2), (2, 2)]);
    }
}
