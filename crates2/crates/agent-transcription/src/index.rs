use crate::queue::Task;
use crate::retrieval;
use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use crate::transcript_db::Transcript;
use mediaa2_core::agent::AgentSettings;
use mediaa2_model_eval::EmbedKind;
use mediaa2_model_eval::EmbeddingsRequest;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const KIND: &str = "index";

pub const EMBED_CTX: u32 = 2048;
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
        Index {
            ai_dir: ai_dir.to_path_buf(),
            settings,
        }
    }
}

impl Stage for Index {
    fn handles(&self, kind: &str) -> bool {
        kind == KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = crate::transcript_db::file_id(&input.path)?;
        anyhow::ensure!(
            Transcript::exists(&self.ai_dir, &id),
            "{} has no transcript",
            input.path.display()
        );
        let mut t = Transcript::open(&self.ai_dir, &id)?;
        if task.cursor.is_null() {
            t.set_chunks(&retrieval::chunks(&t.dialog()?))?;
            return Ok(Work::More {
                cursor: json!({ "phase": "chunked", "n": 0 }),
                partial: Value::Null,
                progress: 0.0,
            });
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let settings = (self.settings)();
        let Some(source) = settings.models.embedding.clone() else {
            let chunks = t.chunks()?;
            return Ok(Work::Done(json!({ "chunks": chunks.len(), "embedded": 0 })));
        };
        let key = source.key();
        // a text change invalidates this model's vectors; other models keep
        // theirs, so switching back restores their index without work
        if !t.embed_index_ok(&key)? {
            t.clear_model_embeddings(&key)?;
        }
        let chunks = t.chunks()?;
        let todo: Vec<(i64, String)> = t.chunks_todo(&key)?;
        log::info!(
            "index: {} chunks, {} without an embedding",
            chunks.len(),
            todo.len()
        );
        if todo.is_empty() {
            return Ok(Work::Done(
                json!({ "chunks": chunks.len(), "embedded": chunks.len() }),
            ));
        }
        let total = chunks.len().max(1) as f32;
        let before = chunks.len() - todo.len();
        let scope = Scope::attach(
            host.store.clone(),
            task.id,
            host.engine.clone(),
            task.scope.clone(),
        );
        let engine = scope.engine();
        let batch = todo.len().min(BATCH);
        for (chunk, text) in todo.iter().take(batch) {
            if cancel.load(Ordering::Relaxed) {
                return Ok(Work::Stopped);
            }
            let request = EmbeddingsRequest {
                source: source.clone(),
                input: vec![text.clone()],
                kind: EmbedKind::Document,
            };
            log::debug!(
                "index: embedding chunk {chunk} ({} chars)",
                text.chars().count()
            );
            let at = std::time::Instant::now();
            let vector = engine
                .embeddings(&request, cancel)?
                .into_iter()
                .next()
                .filter(|v| !v.is_empty());
            log::debug!(
                "index: chunk {chunk} embedded in {:.2} s",
                at.elapsed().as_secs_f32()
            );
            t.set_embedding(
                *chunk,
                &key,
                &vector.ok_or_else(|| anyhow::anyhow!("the model returned no embedding"))?,
            )?;
        }
        let done = before + batch;
        if done >= chunks.len() {
            t.set_embed_rev(&key)?;
            return Ok(Work::Done(
                json!({ "chunks": chunks.len(), "embedded": chunks.len() }),
            ));
        }
        Ok(Work::More {
            cursor: json!({ "phase": "embed", "n": done }),
            partial: Value::Null,
            progress: done as f32 / total,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::Chunk;
    use crate::run::Host;
    use mediaa2_core::agent::{ModelChoices, ModelSource};
    use mediaa2_model_eval::testing::FakeEngine;

    #[test]
    fn a_different_embedding_model_keeps_the_old_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let media = dir.path().join("talk.wav");
        std::fs::write(&media, b"wav").unwrap();
        let id = crate::transcript_db::file_id(&media).unwrap();
        let mut transcript = Transcript::open(ai, &id).unwrap();
        transcript
            .set_chunks(&[Chunk {
                start: 0.0,
                end: 1.0,
                speakers: vec![],
                text: "hello".into(),
            }])
            .unwrap();
        let chunk_id = transcript.chunks().unwrap()[0].0;
        // an index built by another model, with its revision recorded
        transcript
            .set_embedding(chunk_id, "old/model", &[1.0])
            .unwrap();
        transcript.set_embed_rev("old/model").unwrap();
        let settings = AgentSettings {
            models: ModelChoices {
                embedding: Some(ModelSource::new("new", "model")),
                ..ModelChoices::default()
            },
            ..AgentSettings::default()
        };
        let index = Index::new(ai, Arc::new(move || settings.clone()));
        let store = crate::run::open_store(ai).unwrap();
        let task_id = store
            .begin_inline(KIND, "index", &json!({ "path": &media }), None, None)
            .unwrap();
        let mut task = store.get(task_id).unwrap().unwrap();
        task.cursor = json!({ "phase": "embed" });
        let fake = FakeEngine::new();
        let host = Host {
            ai_dir: ai.to_path_buf(),
            engine: Arc::new(fake.clone()),
            store,
            slots: mediaa2_model_eval::slots::Slots::new(),
            settings: Arc::new(AgentSettings::default),
            streams: Arc::new(crate::run::Streams::default()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        index.chunk(&task, &cancel, &host).unwrap();
        assert_eq!(fake.embeddings_count(), 1);
        let transcript = Transcript::open(ai, &id).unwrap();
        assert!(
            transcript.embed_index_ok("old/model").unwrap(),
            "the old model keeps its revision"
        );
        assert!(
            transcript.chunks_with_embeddings("old/model").unwrap()[0]
                .1
                .is_some(),
            "the old model keeps its vectors"
        );
        assert!(transcript.chunks_todo("old/model").unwrap().is_empty());
        assert!(transcript.embed_index_ok("new/model").unwrap());
        assert!(transcript.chunks_todo("new/model").unwrap().is_empty());

        // switching back to the old model: no embedding calls, index is ready
        let settings = AgentSettings {
            models: ModelChoices {
                embedding: Some(ModelSource::new("old", "model")),
                ..ModelChoices::default()
            },
            ..AgentSettings::default()
        };
        let index = Index::new(ai, Arc::new(move || settings.clone()));
        let before = fake.embeddings_count();
        index.chunk(&task, &cancel, &host).unwrap();
        assert_eq!(fake.embeddings_count(), before, "nothing to embed again");
    }
}
