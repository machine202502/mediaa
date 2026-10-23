//! The `index` queue task: a file's dialog cut into chunks with a keyword index
//! (`mediaa_core::retrieval`), then an embedding per chunk with the Embedding role's model.
//! Resumable chunk by chunk. The pipeline queues it after the transcription (or diarization)
//! and the topics after it.

use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use mediaa_core::agent::{AgentSettings, ModelRole};
use mediaa_queue::Task;
use std::sync::atomic::{AtomicBool, Ordering};
use mediaa_core::retrieval;
use mediaa_core::transcript::{self, Transcript};
use mediaa_engine::EmbedKind;
use mediaa_engine::EmbeddingsRequest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const KIND: &str = "index";

/// Context of the embedding model (chunks are ~200 words).
pub const EMBED_CTX: u32 = 2048;
/// Embeddings written before the slot is given to someone else.
const BATCH: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
}

pub struct Index {
    ai_dir: PathBuf,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Index {
    pub fn new(ai_dir: &Path, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Index {
        Index { ai_dir: ai_dir.to_path_buf(), settings }
    }
}

impl Stage for Index {
    fn handles(&self, kind: &str) -> bool {
        kind == KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(Transcript::exists(&self.ai_dir, &id), "{} has no transcript", input.path.display());
        let mut t = Transcript::open(&self.ai_dir, &id)?;
        if task.cursor.is_null() {
            t.set_chunks(&retrieval::chunks(&t.dialog()?))?;
            return Ok(Work::More { cursor: json!({ "phase": "chunked", "n": 0 }), partial: Value::Null, progress: 0.0 });
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let chunks = t.chunks()?;
        let settings = (self.settings)();
        let model = match settings.models.embedding.as_ref() {
            Some(source) => match crate::models::local_spec(source) {
                Ok(model) => model,
                Err(_) => return Ok(Work::Done(json!({ "chunks": chunks.len(), "embedded": 0 }))),
            },
            None => return Ok(Work::Done(json!({ "chunks": chunks.len(), "embedded": 0 }))),
        };
        let todo: Vec<(i64, String)> = chunks.iter().filter(|(_, _, done)| !done).map(|(id, c, _)| (*id, c.text.clone())).collect();
        log::info!("index: {} chunks, {} without an embedding", chunks.len(), todo.len());
        if todo.is_empty() {
            return Ok(Work::Done(json!({ "chunks": chunks.len(), "embedded": chunks.len() })));
        }
        let total = chunks.len().max(1) as f32;
        let before = chunks.len() - todo.len();
        let device = settings.devices.get(ModelRole::Embedding);
        let hf_token = crate::assistant::hf_token(&settings);
        let scope = Scope::attach(host.store.clone(), task.id, host.engine.clone(), task.scope.clone());
        let engine = scope.engine();
        let batch = todo.len().min(BATCH);
        for (chunk, text) in todo.iter().take(batch) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(Work::Stopped);
            }
            let request = EmbeddingsRequest {
                model: model.clone(),
                input: vec![text.clone()],
                kind: EmbedKind::Passage,
                n_ctx: Some(EMBED_CTX),
                device: Some(device),
                hf_token: hf_token.clone(),
            };
            log::debug!("index: embedding chunk {chunk} ({} chars)", text.chars().count());
            let at = std::time::Instant::now();
            let vector = engine.embeddings(&request, cancel)?.into_iter().next().filter(|v| !v.is_empty());
            log::debug!("index: chunk {chunk} embedded in {:.2} s", at.elapsed().as_secs_f32());
            t.set_embedding(*chunk, &vector.ok_or_else(|| anyhow::anyhow!("the model returned no embedding"))?)?;
        }
        let done = before + batch;
        if done >= chunks.len() {
            return Ok(Work::Done(json!({ "chunks": chunks.len(), "embedded": chunks.len() })));
        }
        Ok(Work::More { cursor: json!({ "phase": "embed", "n": done }), partial: Value::Null, progress: done as f32 / total })
    }
}
