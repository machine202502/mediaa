//! The engine of MediaA: models are loaded on demand under a memory budget, downloaded from
//! Hugging Face when missing, and run in this process.
//!
//! [`Engine`] is what the rest of the app talks to; [`LocalEngine`] is the one implementation
//! that runs the models (a fake for tests is in [`crate::testing`]).

pub mod downloads;
pub mod error;
pub mod models;
pub mod types;
mod validate;

pub use downloads::{Download, Downloads};
pub use error::EngineError;
pub use models::{inspect_files, role_from_name, role_name};
pub use types::*;

use crate::embed::{Embedder, Reranker};
use crate::llm::{Generate, LocalModel};
use crate::runtime::{Runtime, Use};
use crate::scheduler::Wait;
use crate::speech::{self, Phrase, Turn};
use crate::{hub, local};
use mediaa_core::agent::{ModelRole, ModelSource, Provider, ProviderKind, WorkKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// Download progress in bytes, callable from the downloading threads.
pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

/// What the app needs from the engine. Every call is blocking; callers that run on the UI
/// thread put them on a worker thread.
pub trait Engine: Send + Sync + 'static {
    /// What the engine is and which GPUs it sees.
    fn health(&self) -> Health;
    /// Loaded models, waiting requests and the memory budget.
    fn status(&self) -> Status;
    /// Sets the memory budget models are kept loaded under.
    fn set_budget(&self, budget: Budget);
    /// Answers `request`; `on_text` gets the answer as it is produced (returning `false` stops),
    /// and a set `cancel` stops the model at the next token.
    fn chat(&self, request: &ChatRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError>;
    /// Embeddings of `request.input`, in the order of the inputs.
    fn embeddings(&self, request: &EmbeddingsRequest) -> Result<Vec<Vec<f32>>, EngineError>;
    /// The documents by relevance to the query, most relevant first.
    fn rerank(&self, request: &RerankRequest) -> Result<Vec<RerankResult>, EngineError>;
    /// Loads the voice activity model and opens a session at `offset` seconds.
    fn vad_open(&self, model: &str, offset: f64, hf_token: Option<&str>) -> Result<Box<dyn VadSession>, EngineError>;
    /// Recognises one piece of speech that starts `offset` seconds into the recording.
    fn recognize(&self, model: &str, offset: f64, samples: &[f32], hf_token: Option<&str>) -> Result<Phrase, EngineError>;
    /// Who speaks when in the whole recording.
    fn diarize(&self, request: &DiarizeRequest, samples: &[f32]) -> Result<Vec<Turn>, EngineError>;
    /// The models downloaded to the engine's models folder.
    fn models(&self) -> Result<Vec<ModelInfo>, EngineError>;
    /// What a model spec is, without downloading it: files, format, roles it fits and problems.
    fn inspect(&self, spec: &str, hf_token: Option<&str>) -> Result<ModelInspect, EngineError>;
    /// Downloads a model, reporting bytes done and total; returns the path of the model.
    fn pull(&self, spec: &str, hf_token: Option<&str>, on_progress: ProgressCallback) -> Result<PathBuf, EngineError>;
    /// Deletes a downloaded model.
    fn delete_model(&self, spec: &str) -> Result<(), EngineError>;
    /// Loads the model in `role`'s way and returns the bytes it takes.
    fn check_model(&self, spec: &str, role: ModelRole, hf_token: Option<&str>) -> Result<ModelCheck, EngineError>;
}

/// The engine itself: models are loaded on demand under the budget and downloaded when missing.
pub struct LocalEngine {
    runtime: Runtime,
    models_dir: PathBuf,
    /// The budget as the app set it, in bytes (the scheduler keeps GB).
    budget: Mutex<Budget>,
    downloads: Downloads,
}

impl LocalEngine {
    /// `models_dir`: where Hugging Face models are downloaded to. It is created when missing.
    pub fn new(models_dir: PathBuf, budget: Budget) -> LocalEngine {
        let _ = std::fs::create_dir_all(&models_dir);
        let runtime = Runtime::new(&models_dir, to_budget(budget));
        LocalEngine { runtime, models_dir, budget: Mutex::new(budget), downloads: Downloads::default() }
    }

    /// Where downloaded models live.
    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    /// Checks the spec and downloads a Hugging Face model that is not on disk yet; progress is
    /// shown in [`Engine::status`].
    fn ensure(&self, source: &ModelSource, hf_token: Option<&str>) -> Result<(), EngineError> {
        let ModelSource::HuggingFace { repo } = source else { return Ok(()) };
        if hub::is_downloaded(&self.models_dir, hub::split_spec(repo).0) {
            return Ok(());
        }
        self.download(repo, hf_token, None).map(drop)
    }

    /// Downloads a Hugging Face spec into the models folder, reporting bytes to the status and
    /// to `on_progress`.
    fn download(&self, repo: &str, hf_token: Option<&str>, on_progress: Option<ProgressCallback>) -> Result<PathBuf, EngineError> {
        let provider = hf_provider(hf_token);
        let id = self.downloads.start(repo);
        let downloads = self.downloads.clone();
        // `download` may report from several threads: the sink is shared and only sends.
        let result = hub::download(&self.models_dir, repo, provider.as_ref(), move |done, total| {
            downloads.progress(id, done, total);
            if let Some(on_progress) = &on_progress {
                on_progress(done, total);
            }
        });
        self.downloads.finish(id);
        result.map_err(EngineError::classify)
    }

    /// The file or folder of a model, downloading a Hugging Face one when missing.
    fn source_path(&self, source: &ModelSource, hf_token: Option<&str>) -> Result<PathBuf, EngineError> {
        match source {
            ModelSource::Local { path } => Ok(path.clone()),
            ModelSource::HuggingFace { repo } => {
                self.ensure(source, hf_token)?;
                // The download folder is named after the repository, not the file part.
                let (repo_id, file) = hub::split_spec(repo);
                let dir = hub::model_dir(&self.models_dir, repo_id);
                Ok(file.map_or_else(|| dir.clone(), |file| dir.join(file)))
            }
            ModelSource::Provider { .. } => Err(EngineError::bad_request("speech models run locally, not at a provider")),
        }
    }
}

impl Engine for LocalEngine {
    fn health(&self) -> Health {
        let gpus = crate::llm::gpus()
            .into_iter()
            .map(|d| Gpu { name: d.name, backend: d.backend, memory_total: d.memory_total, memory_free: d.memory_free })
            .collect();
        Health {
            name: "MediaA".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            gpus,
            models_dir: self.models_dir.display().to_string(),
        }
    }

    fn status(&self) -> Status {
        let status = self.runtime.scheduler().status();
        let mut waiting: Vec<WaitingModel> = status
            .waiting
            .iter()
            .map(|(id, _, wait)| match wait {
                Wait::Memory { key, .. } => WaitingModel { id: *id, model: Some(strip_context(key)), reason: "memory".into(), progress: None },
                Wait::Slot(_) => WaitingModel { id: *id, model: None, reason: "slot".into(), progress: None },
                Wait::Turn => WaitingModel { id: *id, model: None, reason: "turn".into(), progress: None },
            })
            .collect();
        waiting.extend(self.downloads.list().into_iter().map(|(id, download)| WaitingModel {
            id,
            model: Some(download.spec),
            reason: "downloading".into(),
            progress: Some(Progress { done: download.done, total: download.total }),
        }));
        Status {
            loaded: status
                .loaded
                .iter()
                .map(|m| LoadedModel {
                    model: strip_context(&m.key),
                    pool: match m.pool {
                        crate::scheduler::Pool::Gpu => Pool::Gpu,
                        crate::scheduler::Pool::Cpu => Pool::Cpu,
                    },
                    bytes: m.bytes,
                    users: m.leases,
                })
                .collect(),
            waiting,
            budget: *self.budget.lock().expect("budget lock"),
            used: Used { gpu_bytes: status.gpu_used, ram_bytes: status.ram_used },
        }
    }

    fn set_budget(&self, budget: Budget) {
        self.runtime.set_budget(to_budget(budget));
        *self.budget.lock().expect("budget lock") = budget;
    }

    fn chat(&self, request: &ChatRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError> {
        let call = request.clone().into_call()?;
        self.ensure(&call.source, call.hf_token.as_deref())?;
        let how = Use { kind: WorkKind::Agent, device: call.device, n_ctx: call.n_ctx, interactive: call.interactive };
        let generated = self
            .runtime
            .with_model::<LocalModel, _>(&call.source, how, cancel, |model, _lease| {
                model.chat(&call.messages, &call.generate, cancel, &mut *on_text)
            })
            .map_err(EngineError::classify)?;
        let usage = Usage { prompt_tokens: generated.prompt_tokens, completion_tokens: generated.completion_tokens };
        Ok(ChatAnswer { content: generated.text, usage: Some(usage) })
    }

    fn embeddings(&self, request: &EmbeddingsRequest) -> Result<Vec<Vec<f32>>, EngineError> {
        let call = request.clone().into_call()?;
        self.ensure(&call.source, call.hf_token.as_deref())?;
        let how = Use { kind: WorkKind::Indexing, device: call.device, n_ctx: call.n_ctx, interactive: false };
        let cancel = AtomicBool::new(false);
        let kind = call.kind;
        self.runtime
            .with_model::<Embedder, _>(&call.source, how, &cancel, |model, _lease| {
                call.texts
                    .iter()
                    .map(|text| match kind {
                        EmbedKind::Query => model.query(text),
                        EmbedKind::Passage => model.passage(text),
                    })
                    .collect()
            })
            .map_err(EngineError::classify)
    }

    fn rerank(&self, request: &RerankRequest) -> Result<Vec<RerankResult>, EngineError> {
        let call = request.clone().into_call()?;
        self.ensure(&call.source, call.hf_token.as_deref())?;
        let how = Use { kind: WorkKind::Indexing, device: call.device, n_ctx: call.n_ctx, interactive: false };
        let cancel = AtomicBool::new(false);
        let scores = self
            .runtime
            .with_model::<Reranker, Vec<f32>>(&call.source, how, &cancel, |model, _lease| {
                call.documents.iter().map(|document| model.score(&call.query, document)).collect::<anyhow::Result<Vec<f32>>>()
            })
            .map_err(EngineError::classify)?;
        let mut ranked: Vec<RerankResult> =
            scores.into_iter().enumerate().map(|(index, relevance_score)| RerankResult { index, relevance_score }).collect();
        ranked.sort_by(|a, b| b.relevance_score.partial_cmp(&a.relevance_score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(ranked)
    }

    fn vad_open(&self, model: &str, offset: f64, hf_token: Option<&str>) -> Result<Box<dyn VadSession>, EngineError> {
        let offset = validate::offset(offset)?;
        let source = types::parse_spec(model)?;
        let path = self.source_path(&source, hf_token)?;
        let vad = speech::Vad::load(&path, offset).map_err(EngineError::classify)?;
        Ok(Box::new(RealVad(vad)))
    }

    fn recognize(&self, model: &str, offset: f64, samples: &[f32], hf_token: Option<&str>) -> Result<Phrase, EngineError> {
        let offset = validate::offset(offset)?;
        let path = self.source_path(&types::parse_spec(model)?, hf_token)?;
        let recognizer = speech::Recognizer::load(&path).map_err(EngineError::classify)?;
        recognizer.recognize(samples, offset).map_err(EngineError::classify)
    }

    fn diarize(&self, request: &DiarizeRequest, samples: &[f32]) -> Result<Vec<Turn>, EngineError> {
        let speakers = validate::speakers(request.speakers)?;
        let segmentation = self.source_path(&types::parse_spec(&request.segmentation)?, request.hf_token.as_deref())?;
        let embedding = self.source_path(&types::parse_spec(&request.embedding)?, request.hf_token.as_deref())?;
        speech::diarize(&segmentation, &embedding, samples, speakers).map_err(EngineError::classify)
    }

    fn models(&self) -> Result<Vec<ModelInfo>, EngineError> {
        let entries =
            std::fs::read_dir(&self.models_dir).map_err(|e| EngineError::internal(format!("reading {}: {e}", self.models_dir.display())))?;
        let mut downloaded = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(spec) = hub::downloaded_repo(&path) else { continue };
            let files = walk_files(&path)?;
            downloaded.push(ModelInfo { bytes: total_bytes(&files), format: models::detect_format(&files), spec, path: path.display().to_string(), files });
        }
        downloaded.sort_by(|a, b| a.spec.cmp(&b.spec));
        Ok(downloaded)
    }

    fn inspect(&self, spec: &str, hf_token: Option<&str>) -> Result<ModelInspect, EngineError> {
        match types::parse_spec(spec)? {
            ModelSource::Local { path } => Ok(models::inspect_files(spec, walk_files(&path)?, false)),
            ModelSource::HuggingFace { repo } => {
                let (repo_id, file) = hub::split_spec(&repo);
                let files = models::fetch_remote_files(repo_id, hf_token)?;
                match file {
                    Some(file) => {
                        let files: Vec<ModelFile> = files.into_iter().filter(|remote| remote.name == file).collect();
                        if files.is_empty() {
                            return Err(EngineError::not_found(format!("{repo_id} has no file {file}")));
                        }
                        Ok(models::inspect_files(spec, files, false))
                    }
                    None => Ok(models::inspect_files(spec, files, true)),
                }
            }
            ModelSource::Provider { .. } => Err(EngineError::bad_request("a provider model is inspected at its provider")),
        }
    }

    fn pull(&self, spec: &str, hf_token: Option<&str>, on_progress: ProgressCallback) -> Result<PathBuf, EngineError> {
        match types::parse_spec(spec)? {
            ModelSource::HuggingFace { repo } => {
                let (repo_id, file) = hub::split_spec(&repo);
                if hub::is_downloaded(&self.models_dir, repo_id) {
                    let dir = hub::model_dir(&self.models_dir, repo_id);
                    let bytes = walk_files(&dir).map_or(0, |files| total_bytes(&files));
                    on_progress(bytes, bytes);
                    return Ok(file.map_or_else(|| dir.clone(), |file| dir.join(file)));
                }
                let dir = self.download(&repo, hf_token, Some(on_progress))?;
                Ok(file.map_or_else(|| dir.clone(), |file| dir.join(file)))
            }
            ModelSource::Local { path } => {
                let bytes = if path.is_dir() { walk_files(&path).map_or(0, |files| total_bytes(&files)) } else { path.metadata().map_or(0, |m| m.len()) };
                on_progress(bytes, bytes);
                Ok(path)
            }
            ModelSource::Provider { .. } => Err(EngineError::bad_request("provider models are not downloaded by the engine")),
        }
    }

    fn delete_model(&self, spec: &str) -> Result<(), EngineError> {
        let root = std::fs::canonicalize(&self.models_dir)
            .map_err(|e| EngineError::internal(format!("reading the models folder {}: {e}", self.models_dir.display())))?;
        let path = match types::parse_spec(spec)? {
            ModelSource::HuggingFace { repo } => hub::model_dir(&self.models_dir, hub::split_spec(&repo).0),
            ModelSource::Local { path } => path,
            ModelSource::Provider { .. } => return Err(EngineError::bad_request("provider models are not stored by the engine")),
        };
        let target = std::fs::canonicalize(&path).map_err(|_| EngineError::not_found(format!("{} not found", path.display())))?;
        if target == root || !target.starts_with(&root) {
            return Err(EngineError::bad_request(format!("refusing to delete {}: it is outside the models folder", path.display())));
        }
        let removed = if target.is_dir() { std::fs::remove_dir_all(&target) } else { std::fs::remove_file(&target) };
        removed.map_err(|e| EngineError::internal(format!("deleting {}: {e}", path.display())))
    }

    fn check_model(&self, spec: &str, role: ModelRole, hf_token: Option<&str>) -> Result<ModelCheck, EngineError> {
        let path = self.source_path(&types::parse_spec(spec)?, hf_token)?;
        let (files, bytes) = local::check(&path).map_err(|e| EngineError::model_unsupported(format!("{e:#}")))?;
        let gguf = files.iter().find(|file| file.extension().is_some_and(|e| e.eq_ignore_ascii_case("gguf"))).cloned();
        let no_gguf = || anyhow::anyhow!("{}: no GGUF model inside (llama.cpp needs one): choose a repository with GGUF files", path.display());
        let run: anyhow::Result<()> = match role {
            ModelRole::SpeechRecognition => speech::Recognizer::load(&path).map(drop),
            ModelRole::Diarization => speech::check_embedding(&path),
            ModelRole::Embedding => {
                let gguf = gguf.ok_or_else(no_gguf).map_err(|e| EngineError::model_unsupported(format!("{e:#}")))?;
                Embedder::load(&gguf, false).and_then(|embedder| embedder.passage("check").map(drop))
            }
            ModelRole::Reranker => {
                let gguf = gguf.ok_or_else(no_gguf).map_err(|e| EngineError::model_unsupported(format!("{e:#}")))?;
                Reranker::load(&gguf, false).and_then(|reranker| reranker.score("question", "answer").map(drop))
            }
            // Speech synthesis has no runtime yet: the files on disk are all there is to check.
            ModelRole::Speech => Ok(()),
            ModelRole::Agent | ModelRole::Subagent | ModelRole::Titles => {
                let gguf = gguf.ok_or_else(no_gguf).map_err(|e| EngineError::model_unsupported(format!("{e:#}")))?;
                LocalModel::load(&gguf, false, 256).and_then(|model| {
                    let cancel = AtomicBool::new(false);
                    model.complete("Hello", &Generate { max_tokens: 1, ..Default::default() }, &cancel, &mut |_| true).map(drop)
                })
            }
        };
        run.map_err(|e| EngineError::model_unsupported(format!("{e:#}")))?;
        Ok(ModelCheck { ok: true, bytes })
    }
}

/// The real voice activity session: sherpa-onnx's stateful detector.
struct RealVad(speech::Vad);

impl VadSession for RealVad {
    fn push(&mut self, samples: &[f32]) -> Vec<SpeechSegment> {
        segments(self.0.push(samples))
    }

    fn finish(&mut self) -> Vec<SpeechSegment> {
        segments(self.0.finish())
    }
}

fn segments(found: Vec<speech::Speech>) -> Vec<SpeechSegment> {
    found.into_iter().map(|speech| SpeechSegment { start: speech.start, samples: speech.samples }).collect()
}

fn hf_provider(token: Option<&str>) -> Option<Provider> {
    token.map(str::trim).filter(|token| !token.is_empty()).map(|token| {
        let mut provider = Provider::new(ProviderKind::HuggingFace, "Hugging Face");
        provider.api_key = token.to_string();
        provider
    })
}

/// The files of a model folder (or the one file of a model file), paths relative to it. The
/// completion marker of a download is internal and not listed.
fn walk_files(root: &Path) -> Result<Vec<ModelFile>, EngineError> {
    if root.is_file() {
        let name = root.file_name().and_then(|name| name.to_str()).unwrap_or_default().to_string();
        let bytes = root.metadata().map_err(|e| EngineError::internal(format!("{}: {e}", root.display())))?.len();
        return Ok(vec![ModelFile { name, bytes }]);
    }
    if !root.is_dir() {
        return Err(EngineError::not_found(format!("{} not found", root.display())));
    }
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir).map_err(|e| EngineError::internal(format!("{}: {e}", dir.display())))?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name: Vec<String> =
                path.strip_prefix(root).unwrap_or(&path).components().filter_map(|part| part.as_os_str().to_str().map(str::to_string)).collect();
            if name.last().is_some_and(|file| file == ".mediaa-complete") {
                continue;
            }
            let bytes = path.metadata().map_or(0, |metadata| metadata.len());
            files.push(ModelFile { name: name.join("/"), bytes });
        }
    }
    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

fn total_bytes(files: &[ModelFile]) -> u64 {
    files.iter().map(|file| file.bytes).sum()
}

fn to_budget(bytes: Budget) -> mediaa_core::agent::Budget {
    mediaa_core::agent::Budget {
        gpu_memory_gb: gigabytes(bytes.gpu_bytes),
        ram_memory_gb: gigabytes(bytes.ram_bytes),
        ..mediaa_core::agent::Budget::default()
    }
}

fn gigabytes(bytes: u64) -> f32 {
    (bytes as f64 / (1024.0 * 1024.0 * 1024.0)) as f32
}

/// The scheduler keys a model by `path#n_ctx`; the status shows the file.
fn strip_context(key: &str) -> String {
    key.rsplit_once('#').map_or_else(|| key.to_string(), |(path, _)| path.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_scheduler_keys() {
        let budget = to_budget(Budget { gpu_bytes: 2 << 30, ram_bytes: 512 << 20 });
        assert_eq!(budget.gpu_memory_gb, 2.0);
        assert_eq!(budget.ram_memory_gb, 0.5);
        assert_eq!(strip_context("/m/qwen.gguf#8192"), "/m/qwen.gguf");
        assert_eq!(strip_context("no-context"), "no-context");
    }

    #[test]
    fn a_local_engine_starts_without_loading_models() {
        let dir = tempfile::tempdir().unwrap();
        let engine = LocalEngine::new(dir.path().to_path_buf(), Budget { gpu_bytes: 1 << 30, ram_bytes: 1 << 30 });
        assert_eq!(engine.status().loaded.len(), 0);
        assert_eq!(engine.status().budget.gpu_bytes, 1 << 30);
        engine.set_budget(Budget { gpu_bytes: 2 << 30, ram_bytes: 3 << 30 });
        assert_eq!(engine.status().budget, Budget { gpu_bytes: 2 << 30, ram_bytes: 3 << 30 });
        assert!(engine.ensure(&ModelSource::Local { path: std::path::Path::new("/nope.gguf").into() }, None).is_ok());
        assert_eq!(engine.health().models_dir, engine.models_dir().display().to_string());
    }

    /// A downloaded repository on disk, as `hub::download` leaves it.
    fn downloaded(root: &Path, repo: &str, files: &[&str]) -> PathBuf {
        let dir = hub::model_dir(root, repo);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".mediaa-complete"), repo).unwrap();
        for file in files {
            std::fs::write(dir.join(file), b"x").unwrap();
        }
        dir
    }

    #[test]
    fn a_speech_spec_resolves_into_its_repository_folder() {
        let root = tempfile::tempdir().unwrap();
        let dir = downloaded(root.path(), "csukuangfj/vad", &["silero_vad.onnx"]);
        let engine = LocalEngine::new(root.path().to_path_buf(), Budget::default());
        // No network: the completion marker is already there.
        let spec = ModelSource::HuggingFace { repo: "csukuangfj/vad/silero_vad.onnx".into() };
        assert_eq!(engine.source_path(&spec, None).unwrap(), dir.join("silero_vad.onnx"));
        let folder = ModelSource::HuggingFace { repo: "csukuangfj/vad".into() };
        assert_eq!(engine.source_path(&folder, None).unwrap(), dir);
        assert!(engine.source_path(&ModelSource::Provider { provider: "p".into(), model: "m".into() }, None).is_err());
    }

    #[test]
    fn a_file_spec_deletes_its_repository_and_the_folder_itself_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let model = downloaded(root.path(), "a/b", &["model.onnx"]);
        let other = downloaded(root.path(), "c/d", &["m.gguf"]);
        let engine = LocalEngine::new(root.path().to_path_buf(), Budget::default());

        engine.delete_model("a/b/model.onnx").unwrap();
        assert!(!model.exists(), "the whole downloaded repository goes");
        assert!(other.exists(), "another model is untouched");

        let outside = tempfile::tempdir().unwrap();
        let outside_file = outside.path().join("outside.gguf");
        std::fs::write(&outside_file, b"x").unwrap();
        let error = engine.delete_model(outside_file.to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind, "bad_request");
        assert!(outside_file.exists());

        let error = engine.delete_model(root.path().to_str().unwrap()).unwrap_err();
        assert_eq!(error.kind, "bad_request");
        assert!(root.path().exists(), "the models folder itself is never deleted");
    }
}
