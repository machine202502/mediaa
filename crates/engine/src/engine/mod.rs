mod error;
mod types;

pub use error::EngineError;
pub use types::*;

use crate::command;
use crate::ollama;
use crate::openai::{self, CALL_TIMEOUT, CHAT_TIMEOUT};
use mediaa_core::agent::{Connection, ConnectionKind, ModelRole, ModelSource};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
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
    ) -> Result<Vec<mediaa_core::transcript::Phrase>, EngineError>;
    fn diarize(
        &self,
        request: &DiarizeRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<mediaa_core::transcript::Turn>, EngineError>;
    fn voice_embeddings(
        &self,
        request: &VoiceRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError>;
    fn models(&self, connection: &str) -> Result<Vec<ModelInfo>, EngineError>;
    fn check(&self, source: &ModelSource, role: ModelRole) -> Result<ModelCheck, EngineError>;
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
}

impl Connections {
    pub fn new(connections: Vec<Connection>) -> Connections {
        Connections {
            state: Mutex::new(State {
                connections,
                agents: HashMap::new(),
            }),
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

fn command_of(connection: &Connection) -> Result<&mediaa_core::agent::CommandSpec, EngineError> {
    connection
        .command
        .as_ref()
        .ok_or_else(|| EngineError::bad_request(format!("{} has no command", connection.name)))
}

#[allow(
    clippy::too_many_arguments,
    reason = "one external command: connection, file, model, language, range and cancel"
)]
fn run_command(
    connection: &Connection,
    input: &Path,
    model: &str,
    language: &str,
    from: f64,
    to: f64,
    inputs: &[std::path::PathBuf],
    cancel: &AtomicBool,
) -> Result<(command::Prepared, String), EngineError> {
    let spec = command_of(connection)?.clone();
    let work = std::env::temp_dir().join(format!("mediaa-cmd-{}", connection.id));
    let _ = std::fs::create_dir_all(&work);
    let prepared = command::prepare(&spec, input, model, language, from, to, inputs, &work);
    let text = command::run(&spec, &prepared, cancel)?;
    Ok((prepared, text))
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
        if connection.kind == ConnectionKind::Command {
            return Err(EngineError::unavailable("a command cannot chat"));
        }
        if connection.kind == ConnectionKind::Ollama {
            let http = self.http(&connection, CHAT_TIMEOUT)?;
            return openai::chat(&http, &connection, request, cancel, on_text);
        }
        let http = self.http(&connection, CHAT_TIMEOUT)?;
        openai::chat(&http, &connection, request, cancel, on_text)
    }

    fn embeddings(
        &self,
        request: &EmbeddingsRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        let http = self.http(&connection, CALL_TIMEOUT)?;
        if connection.kind == ConnectionKind::Ollama {
            ollama::embed(&http, &connection, request)
        } else {
            openai::embeddings(&http, &connection, request)
        }
    }

    fn rerank(
        &self,
        request: &RerankRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<RerankResult>, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        let http = self.http(&connection, CALL_TIMEOUT)?;
        openai::rerank(&http, &connection, request)
    }

    fn transcribe(
        &self,
        request: &TranscribeRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<mediaa_core::transcript::Phrase>, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        match connection.kind {
            ConnectionKind::OpenAi | ConnectionKind::OpenAiCompatible => {
                let bytes = std::fs::read(&request.path).map_err(|error| {
                    EngineError::not_found(format!("{}: {error}", request.path.display()))
                })?;
                let http = self.http(&connection, CHAT_TIMEOUT)?;
                openai::transcribe(&http, &connection, request, &bytes)
            }
            ConnectionKind::Command => {
                let language = request.language.clone().unwrap_or_default();
                let spec = command_of(&connection)?.clone();
                let (_prepared, text) = run_command(
                    &connection,
                    &request.path,
                    &request.source.model,
                    &language,
                    request.from,
                    request.to,
                    &[],
                    cancel,
                )?;
                command::parse_transcript(spec.output, &text, request.from)
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
    ) -> Result<Vec<mediaa_core::transcript::Turn>, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        let (_prepared, text) = run_command(
            &connection,
            &request.path,
            &request.source.model,
            "",
            0.0,
            0.0,
            &[],
            cancel,
        )?;
        command::parse_rttm(&text)
    }

    fn voice_embeddings(
        &self,
        request: &VoiceRequest,
        cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        cancelled(cancel)?;
        let connection = self.connection(&request.source.connection)?;
        let input = request.clips.first().cloned().unwrap_or_default();
        let (_prepared, text) = run_command(
            &connection,
            &input,
            &request.source.model,
            "",
            0.0,
            0.0,
            &request.clips,
            cancel,
        )?;
        command::parse_vectors(&text, request.clips.len())
    }

    fn models(&self, connection: &str) -> Result<Vec<ModelInfo>, EngineError> {
        let connection = self.connection(connection)?;
        if connection.kind == ConnectionKind::Command {
            return Ok(Vec::new());
        }
        let http = self.http(&connection, CALL_TIMEOUT)?;
        if connection.kind == ConnectionKind::Ollama {
            return ollama::tags(&http, &connection);
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
        if connection.kind == ConnectionKind::Command {
            let spec = command_of(&connection)?;
            let program = command::expand_home(&spec.program.display().to_string());
            if !executable(Path::new(&program)) {
                return Ok(ModelCheck {
                    ok: false,
                    detail: format!("{program} was not found"),
                });
            }
            return Ok(ModelCheck {
                ok: true,
                detail: program,
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
}

fn executable(program: &Path) -> bool {
    if program.components().count() > 1 {
        return program.exists();
    }
    let name = program.as_os_str();
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths)
            .any(|dir| dir.join(name).exists() || dir.join(program).with_extension("exe").exists())
    })
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

pub fn pull(
    connection: &Connection,
    model: &str,
    on_progress: &mut dyn FnMut(u64, u64),
    cancel: &AtomicBool,
) -> Result<(), EngineError> {
    ollama::pull(connection, model, on_progress, cancel)
}
