mod error;
pub mod retry;
mod types;

pub use error::EngineError;
pub use retry::RetryPolicy;
pub use types::*;

use crate::genai_chat;
use crate::ollama;
use crate::openai::{self, CALL_TIMEOUT, CHAT_TIMEOUT};
use mediaa2_core::agent::{Connection, ConnectionKind, ModelRole, ModelSource};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

pub trait Engine: Send + Sync + 'static {
    fn chat(
        &self,
        request: &ChatRequest,
        cancel: &AtomicBool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<ChatAnswer, EngineError>;
    fn embeddings(
        &self,
        request: &EmbeddingsRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError>;
    fn rerank(
        &self,
        request: &RerankRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<RerankResult>, EngineError>;
    fn transcribe(
        &self,
        request: &TranscribeRequest,
        cancel: &AtomicBool,
    ) -> Result<SpeechBody, EngineError>;
    fn diarize(
        &self,
        request: &DiarizeRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<mediaa2_core::transcript::Turn>, EngineError>;
    fn voice_embeddings(
        &self,
        request: &VoiceRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError>;
    fn models(&self, connection: &str) -> Result<Vec<ModelInfo>, EngineError>;
    fn check(&self, source: &ModelSource, role: ModelRole) -> Result<ModelCheck, EngineError>;
    fn pull(
        &self,
        connection: &str,
        model: &str,
        on_progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<(), EngineError> {
        let _ = (connection, model, on_progress, cancel);
        Err(EngineError::unavailable("pulling a model is not supported"))
    }
}

struct CachedAgent {
    proxy: String,
    timeout_ms: u128,
    agent: ureq::Agent,
}

struct State {
    connections: Vec<Connection>,
    agents: HashMap<String, CachedAgent>,
}

pub struct Connections {
    state: Mutex<State>,
    retries: AtomicU32,
}

/// First repeat after ~0.8 s, then doubling; the cap and the jitter live in
/// [`retry`]. The number of tries comes from the settings.
const RETRY_BASE_MS: u64 = 800;

impl Connections {
    pub fn new(connections: Vec<Connection>) -> Connections {
        Connections {
            state: Mutex::new(State {
                connections,
                agents: HashMap::new(),
            }),
            retries: AtomicU32::new(2),
        }
    }

    /// How many tries (including the first) a model call gets.
    pub fn set_retries(&self, attempts: u32) {
        self.retries.store(attempts.max(1), Ordering::Relaxed);
    }

    fn policy(&self) -> RetryPolicy {
        RetryPolicy {
            attempts: self.retries.load(Ordering::Relaxed).max(1),
            base_ms: RETRY_BASE_MS,
        }
    }

    pub fn set_connections(&self, connections: Vec<Connection>) {
        let mut state = self.state.lock().expect("connections lock");
        state.connections = connections;
        state.agents.clear();
    }

    fn connection(&self, id: &str) -> Result<Connection, EngineError> {
        self.state
            .lock()
            .expect("connections lock")
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .cloned()
            .ok_or_else(|| EngineError::not_found(format!("connection {id} was removed")))
    }

    fn http(&self, connection: &Connection, timeout: Duration) -> Result<ureq::Agent, EngineError> {
        let mut state = self.state.lock().expect("connections lock");
        let proxy = connection.proxy().unwrap_or("").to_string();
        let timeout_ms = timeout.as_millis();
        if let Some(cached) = state.agents.get(&connection.id) {
            if cached.proxy == proxy && cached.timeout_ms == timeout_ms {
                return Ok(cached.agent.clone());
            }
        }
        let agent = openai::agent(connection.proxy(), timeout)?;
        state.agents.insert(
            connection.id.clone(),
            CachedAgent {
                proxy,
                timeout_ms,
                agent: agent.clone(),
            },
        );
        Ok(agent)
    }
}

fn cancelled(cancel: &AtomicBool) -> Result<(), EngineError> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(EngineError::cancelled("cancelled"))
    } else {
        Ok(())
    }
}

impl Engine for Connections {
    fn chat(
        &self,
        request: &ChatRequest,
        cancel: &AtomicBool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<ChatAnswer, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        if connection.kind == ConnectionKind::SpeechRecognition {
            return Err(EngineError::unavailable(
                "a transcription server cannot chat",
            ));
        }
        // a repeat may only restart the request while nothing reached the UI
        let policy = self.policy();
        let mut emitted = false;
        retry::run_if(&policy, cancel, &|_| true, || {
            if emitted {
                return Err(EngineError::cancelled(
                    "the answer already started, repeating is impossible",
                ));
            }
            genai_chat::chat(&connection, request, cancel, &mut |text| {
                emitted = true;
                on_text(text)
            })
        })
    }

    fn embeddings(
        &self,
        request: &EmbeddingsRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        let policy = self.policy();
        if connection.kind == ConnectionKind::Ollama {
            return retry::run(&policy, cancel, || {
                genai_chat::embeddings(&connection, request)
            });
        }
        let http = self.http(&connection, CALL_TIMEOUT)?;
        retry::run(&policy, cancel, || {
            openai::embeddings(&http, &connection, request)
        })
    }

    fn rerank(
        &self,
        request: &RerankRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<RerankResult>, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        let http = self.http(&connection, CALL_TIMEOUT)?;
        let policy = self.policy();
        retry::run(&policy, cancel, || {
            openai::rerank(&http, &connection, request)
        })
    }

    fn transcribe(
        &self,
        request: &TranscribeRequest,
        cancel: &AtomicBool,
    ) -> Result<SpeechBody, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        match connection.kind {
            ConnectionKind::OpenAiCompatible | ConnectionKind::SpeechRecognition => {
                let bytes = std::fs::read(&request.path).map_err(|error| {
                    EngineError::not_found(format!("{}: {error}", request.path.display()))
                })?;
                let http = self.http(&connection, CHAT_TIMEOUT)?;
                let policy = self.policy();
                retry::run(&policy, cancel, || {
                    openai::transcribe(&http, &connection, request, &bytes)
                })
            }
            _ => Err(EngineError::unavailable(format!(
                "{} cannot transcribe",
                connection.name
            ))),
        }
    }

    fn diarize(
        &self,
        request: &DiarizeRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<mediaa2_core::transcript::Turn>, EngineError> {
        let _ = (request, cancel);
        Err(EngineError::unavailable(
            "speakers come back inside transcription",
        ))
    }

    fn voice_embeddings(
        &self,
        request: &VoiceRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        let _ = (request, cancel);
        Err(EngineError::unavailable(
            "voiceprints come back inside transcription",
        ))
    }

    fn models(&self, connection: &str) -> Result<Vec<ModelInfo>, EngineError> {
        let connection = self.connection(connection)?;
        let http = self.http(&connection, CALL_TIMEOUT)?;
        if connection.kind == ConnectionKind::Ollama {
            return ollama::tags(&http, &connection);
        }
        if connection.kind == ConnectionKind::SpeechRecognition {
            let url = format!("{}/health", connection.api_url());
            let response = http
                .get(&url)
                .call()
                .map_err(|error| EngineError::internal(format!("{url}: {error}")))?;
            if !(200..300).contains(&response.status().as_u16()) {
                return Err(EngineError::bad_request(format!(
                    "{} did not answer",
                    connection.name
                )));
            }
            return Ok(Vec::new());
        }
        Ok(openai::list_model_ids(&http, &connection)?
            .into_iter()
            .map(|id| ModelInfo {
                id: id.clone(),
                name: id,
            })
            .collect())
    }

    fn check(&self, source: &ModelSource, role: ModelRole) -> Result<ModelCheck, EngineError> {
        let connection = self.connection(&source.connection)?;
        if !connection.supports(role.capability()) {
            return Ok(ModelCheck {
                ok: false,
                detail: format!("{} cannot do this", connection.name),
            });
        }
        if connection.kind == ConnectionKind::SpeechRecognition {
            self.models(&connection.id)?;
            return Ok(ModelCheck {
                ok: true,
                detail: connection.name.clone(),
            });
        }
        let ids = self.models(&connection.id)?;
        let wanted = source.model.trim();
        if ids.iter().any(|model| model.id == wanted) {
            Ok(ModelCheck {
                ok: true,
                detail: connection.name.clone(),
            })
        } else {
            Ok(ModelCheck {
                ok: false,
                detail: format!("{} does not serve \"{wanted}\"", connection.name),
            })
        }
    }

    fn pull(
        &self,
        connection: &str,
        model: &str,
        on_progress: &mut dyn FnMut(u64, u64),
        cancel: &AtomicBool,
    ) -> Result<(), EngineError> {
        let connection = self.connection(connection)?;
        ollama::pull(&connection, model, on_progress, cancel)
    }
}

impl Connections {
    pub fn ollama_version(&self, id: &str) -> Result<String, EngineError> {
        let connection = self.connection(id)?;
        ollama::version(&ollama::program_path(&connection))
    }

    pub fn ollama_serve(&self, id: &str) -> Result<(), EngineError> {
        let connection = self.connection(id)?;
        ollama::serve(&ollama::program_path(&connection))
    }
}
