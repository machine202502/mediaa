use crate::features::agent::host::AgentServices;
use mediaa_core::agent::{AgentSettings, Connection};
use mediaa_core::jobs::Job;
use mediaa_engine::Connections;
use mediaa_protocol::backend::{AgentRequestWire, AgentResponseStateWire, AgentResponseWire};
use serde_json::{json, Value};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

struct Pending {
    request: u64,
    job: Job<Result<Value, String>>,
    progress: Option<Arc<Mutex<(u64, u64)>>>,
    sent: (u64, u64),
}

#[derive(Default)]
pub struct AgentRequests {
    jobs: Vec<Pending>,
}

impl AgentRequests {
    pub fn start(
        &mut self,
        request: &AgentRequestWire,
        services: &AgentServices,
    ) -> Result<(), String> {
        let settings = services.settings.read().expect("agent settings").clone();
        let kind = request.kind.as_str();
        let payload = request.payload.clone();
        let engine = services.engine.clone();
        let ai_dir = services.ai_dir.clone();
        let mut progress = None;
        let worker: Box<dyn FnOnce() -> Result<Value, String> + Send> = match kind {
            "search" => {
                let store = string(&payload, "store")?;
                let query = string(&payload, "query")?;
                let limit = payload.get("limit").and_then(Value::as_u64).unwrap_or(6) as usize;
                Box::new(move || {
                    mediaa_agent::search::search(
                        engine.as_ref(),
                        &settings,
                        &ai_dir,
                        &store,
                        &query,
                        limit,
                    )
                    .map(|(hits, trace)| search_json(&hits, &trace))
                    .map_err(|error| format!("{error:#}"))
                })
            }
            "search_files" => {
                let query = string(&payload, "query")?;
                let limit = payload.get("limit").and_then(Value::as_u64).unwrap_or(6) as usize;
                let sources: Vec<mediaa_agent::search::Source> = payload
                    .get("files")
                    .and_then(Value::as_array)
                    .map(|files| {
                        files
                            .iter()
                            .filter_map(|file| {
                                Some(mediaa_agent::search::Source {
                                    id: file.get("id")?.as_str()?.to_string(),
                                    path: std::path::PathBuf::from(file.get("path")?.as_str()?),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                Box::new(move || {
                    mediaa_agent::search::search_files(
                        engine.as_ref(),
                        &settings,
                        &ai_dir,
                        &sources,
                        &query,
                        limit,
                    )
                    .map(|(hits, trace)| search_files_json(&hits, &trace))
                    .map_err(|error| format!("{error:#}"))
                })
            }
            "prioritize_detail" => {
                let store = string(&payload, "store")?;
                let topic = payload.get("topic").and_then(Value::as_u64).unwrap_or(0) as usize;
                let sched = services.sched.clone();
                Box::new(move || {
                    mediaa_agent::pipeline::prioritize_detail(&sched, &store, topic)
                        .map(|raised| json!({ "raised": raised }))
                        .map_err(|error| format!("{error:#}"))
                })
            }
            "media_probe" => {
                let path = string(&payload, "path")?;
                Box::new(move || {
                    let path = std::path::Path::new(&path);
                    let kind = mediaa_core::MediaKind::detect(path)
                        .ok_or_else(|| "unsupported media".to_string())?;
                    let info = mediaa_media::preview::details(path, kind)
                        .map_err(|error| format!("{error:#}"))?;
                    let size = std::fs::metadata(path).map(|meta| meta.len()).ok();
                    Ok(json!({
                        "size": size,
                        "width": info.width,
                        "height": info.height,
                        "fps": info.fps,
                        "duration_us": info.duration_us,
                        "audio_streams": info.audio_streams,
                        "video_codec": info.video_codec,
                        "audio_codec": info.audio_codec,
                    }))
                })
            }
            "models" => {
                let connection = string(&payload, "connection")?;
                Box::new(move || {
                    engine
                        .models(&connection)
                        .map(|models| {
                            json!(models.into_iter().map(|model| model.id).collect::<Vec<_>>())
                        })
                        .map_err(|error| format!("{error:#}"))
                })
            }
            "pull" => {
                let connection = string(&payload, "connection")?;
                let model = string(&payload, "model")?;
                let shared = Arc::new(Mutex::new((0, 0)));
                let report = shared.clone();
                progress = Some(shared);
                Box::new(move || {
                    let cancel = AtomicBool::new(false);
                    engine
                        .pull(
                            &connection,
                            &model,
                            &mut |done, total| {
                                if let Ok(mut report) = report.lock() {
                                    *report = (done, total);
                                }
                            },
                            &cancel,
                        )
                        .map(|()| Value::Null)
                        .map_err(|error| format!("{error:#}"))
                })
            }
            "check_models" => Box::new(move || {
                let mut rows = Vec::new();
                mediaa_agent::check::check_all(&settings, engine.as_ref(), |role, status| {
                    rows.push(json!({
                        "role": format!("{role:?}"),
                        "status": check_json(&status),
                    }));
                });
                Ok(json!({ "rows": rows }))
            }),
            "check_connection" => {
                let id = string(&payload, "connection")?;
                let connection = find(&settings, &id)?;
                Box::new(move || {
                    let connections = Connections::new(vec![connection]);
                    connections
                        .ollama_version(&id)
                        .map(|version| json!({ "version": version }))
                        .map_err(|error| format!("{error:#}"))
                })
            }
            "ollama_serve" => {
                let id = string(&payload, "connection")?;
                let connection = find(&settings, &id)?;
                Box::new(move || {
                    let connections = Connections::new(vec![connection]);
                    connections
                        .ollama_serve(&id)
                        .map(|()| Value::Null)
                        .map_err(|error| format!("{error:#}"))
                })
            }
            "web_check" => Box::new(move || {
                mediaa_agent::web::search(&settings.web, "MediaA", 3)
                    .map(|hits| json!({ "count": hits.len() }))
                    .map_err(|error| format!("{error:#}"))
            }),
            other => return Err(format!("unknown agent request: {other}")),
        };
        let job = Job::spawn("agent-request", Arc::default(), worker)
            .map_err(|error| format!("{error:#}"))?;
        self.jobs.push(Pending {
            request: request.request,
            job,
            progress,
            sent: (0, 0),
        });
        Ok(())
    }

    pub fn poll(&mut self) -> Vec<AgentResponseWire> {
        let mut out = Vec::new();
        let mut index = 0;
        while index < self.jobs.len() {
            if let Some(progress) = self.jobs[index].progress.as_ref() {
                let now = progress.lock().map(|value| *value).unwrap_or((0, 0));
                if now != self.jobs[index].sent {
                    self.jobs[index].sent = now;
                    out.push(AgentResponseWire {
                        request: self.jobs[index].request,
                        state: AgentResponseStateWire::Progress {
                            done: now.0,
                            total: now.1,
                        },
                    });
                }
            }
            let finished = self.jobs[index].job.poll();
            if let Some(result) = finished {
                let pending = self.jobs.remove(index);
                out.push(AgentResponseWire {
                    request: pending.request,
                    state: match result {
                        Ok(Ok(payload)) => AgentResponseStateWire::Done { payload },
                        Ok(Err(error)) => AgentResponseStateWire::Failed { error },
                        Err(error) => AgentResponseStateWire::Failed {
                            error: format!("{error:#}"),
                        },
                    },
                });
                continue;
            }
            index += 1;
        }
        out
    }
}

fn string(payload: &Value, key: &str) -> Result<String, String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| format!("agent request has no {key}"))
}

fn find(settings: &AgentSettings, id: &str) -> Result<Connection, String> {
    settings
        .connection(id)
        .cloned()
        .ok_or_else(|| format!("connection {id} is missing"))
}

fn check_json(status: &mediaa_agent::check::Status) -> Value {
    match status {
        mediaa_agent::check::Status::Checking => json!({ "state": "checking" }),
        mediaa_agent::check::Status::Ready(mediaa_agent::check::Ready::Checked) => {
            json!({ "state": "ready" })
        }
        mediaa_agent::check::Status::Ready(mediaa_agent::check::Ready::NotUsed) => {
            json!({ "state": "not_used" })
        }
        mediaa_agent::check::Status::Failed(error) => json!({ "state": "failed", "error": error }),
    }
}

fn search_files_json(
    hits: &[mediaa_agent::search::FileHit],
    trace: &mediaa_agent::search::Trace,
) -> Value {
    json!({
        "hits": hits.iter().map(|hit| json!({
            "path": hit.path,
            "speaker": hit.speaker,
            "hit": {
                "id": hit.hit.id,
                "score": hit.hit.score,
                "chunk": {
                    "start": hit.hit.chunk.start,
                    "end": hit.hit.chunk.end,
                    "speakers": hit.hit.chunk.speakers,
                    "text": hit.hit.chunk.text,
                },
            },
        })).collect::<Vec<_>>(),
        "trace": {
            "by_meaning": trace.by_meaning,
            "reranked": trace.reranked,
            "files": trace.files,
            "not_indexed": trace.not_indexed,
            "embedded": trace.embedded,
        },
    })
}

fn search_json(hits: &[mediaa_core::retrieval::Hit], trace: &mediaa_agent::search::Trace) -> Value {
    json!({
        "hits": hits.iter().map(|hit| json!({
            "id": hit.id,
            "score": hit.score,
            "chunk": {
                "start": hit.chunk.start,
                "end": hit.chunk.end,
                "speakers": hit.chunk.speakers,
                "text": hit.chunk.text,
            },
        })).collect::<Vec<_>>(),
        "trace": {
            "by_meaning": trace.by_meaning,
            "reranked": trace.reranked,
            "files": trace.files,
            "not_indexed": trace.not_indexed,
            "embedded": trace.embedded,
        },
    })
}
