//! The `index` queue task: a file's dialog cut into chunks with a keyword index
//! (`mediaa_core::retrieval`), then an embedding per chunk with the Embedding role's model.
//! Queued after every transcription (and queues the outline); resumable chunk by chunk.

use crate::embed::Embedder;
use crate::hub;
use crate::runtime::{Runtime, Use};
use crate::worker::{Checkpoints, TaskHandler};
use mediaa_core::agent::{AgentSettings, ModelRole, ModelSource, WorkKind};
use mediaa_core::queue::{Queue, Task};
use mediaa_core::retrieval;
use mediaa_core::transcript::{self, Transcript};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const KIND: &str = "index";

/// Context of the embedding model (chunks are ~200 words).
pub const EMBED_CTX: u32 = 2048;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
}

/// Queues indexing of `path` (after its transcription).
pub fn enqueue(ai_dir: &Path, path: &Path) -> anyhow::Result<()> {
    let title = path.file_name().unwrap_or_default().to_string_lossy();
    Queue::open(ai_dir)?.enqueue_once(KIND, &title, &serde_json::to_value(Input { path: path.to_path_buf() })?, 0)?;
    Ok(())
}

pub struct Index {
    ai_dir: PathBuf,
    runtime: Runtime,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Index {
    pub fn new(ai_dir: &Path, runtime: Runtime, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Index {
        Index { ai_dir: ai_dir.to_path_buf(), runtime, settings }
    }
}

impl TaskHandler for Index {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(Transcript::exists(&self.ai_dir, &id), "{} has no transcript", input.path.display());
        let mut t = Transcript::open(&self.ai_dir, &id)?;
        if task.cursor.is_null() {
            t.set_chunks(&retrieval::chunks(&t.dialog()?))?;
            // The outline next: its own task (the language model, slower), not needing embeddings.
            crate::outline::enqueue(&self.ai_dir, &input.path)?;
            cp.save(&json!("chunked"), &Value::Null, 0.0)?;
        }
        let chunks = t.chunks()?;
        let settings = (self.settings)();
        // Keywords only when the embedding model is a provider's (not supported yet) or unset.
        let source = match settings.models.embedding.clone() {
            Some(s @ (ModelSource::HuggingFace { .. } | ModelSource::Local { .. })) => s,
            _ => return Ok(json!({ "chunks": chunks.len(), "embedded": 0 })),
        };
        if let ModelSource::HuggingFace { repo } = &source {
            hub::download(&self.ai_dir, repo, settings.hugging_face(), |_, _| {})?;
        }
        let todo: Vec<(i64, String)> = chunks.iter().filter(|(_, _, done)| !done).map(|(id, c, _)| (*id, c.text.clone())).collect();
        let total = chunks.len().max(1) as f32;
        let before = chunks.len() - todo.len();
        let how = Use { kind: WorkKind::Indexing, device: settings.devices.get(ModelRole::Embedding), n_ctx: EMBED_CTX, interactive: false };
        let cancel = cp.stopping();
        self.runtime.with_model(&source, how, cancel, |e: &Embedder, _lease| {
            for (n, (chunk, text)) in todo.iter().enumerate() {
                t.set_embedding(*chunk, &e.passage(text)?)?;
                cp.save(&json!("embedding"), &Value::Null, (before + n + 1) as f32 / total)?;
            }
            Ok(())
        })?;
        Ok(json!({ "chunks": chunks.len(), "embedded": chunks.len() }))
    }
}
