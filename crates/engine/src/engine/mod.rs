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
use crate::runtime::{self, Runtime, Use};
use crate::scheduler::Wait;
use crate::speech::{self, Phrase};
use crate::{hub, local};
use mediaa_core::agent::{ModelRole, ModelSource, Provider, ProviderKind, WorkKind};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub type ProgressCallback = Arc<dyn Fn(u64, u64) + Send + Sync>;

pub trait Engine: Send + Sync + 'static {
    fn health(&self) -> Health;
    fn status(&self) -> Status;
    fn set_budget(&self, budget: Budget);
    fn set_wanted(&self, _specs: &[String]) {}
    fn forget_loaded(&self, _model: &str) {}
    fn chat(&self, request: &ChatRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError>;
    fn chat_thinking(&self, request: &ChatRequest, thinking: Thinking, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError>;
    fn embeddings(&self, request: &EmbeddingsRequest, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, EngineError>;
    fn rerank(&self, request: &RerankRequest, cancel: &AtomicBool) -> Result<Vec<RerankResult>, EngineError>;
    fn vad_open(&self, model: &str, offset: f64, hf_token: Option<&str>) -> Result<Box<dyn VadSession>, EngineError>;
    fn recognize(&self, model: &str, offset: f64, samples: &[f32], hf_token: Option<&str>) -> Result<Phrase, EngineError>;
    fn diarize(&self, request: &DiarizeRequest, samples: &[f32]) -> Result<Diarization, EngineError>;
    fn models(&self) -> Result<Vec<ModelInfo>, EngineError>;
    fn inspect(&self, spec: &str, hf_token: Option<&str>) -> Result<ModelInspect, EngineError>;
    fn pull(&self, spec: &str, hf_token: Option<&str>, on_progress: ProgressCallback) -> Result<PathBuf, EngineError>;
    fn delete_model(&self, spec: &str) -> Result<(), EngineError>;
    fn check_model(&self, spec: &str, role: ModelRole, hf_token: Option<&str>) -> Result<ModelCheck, EngineError>;
}

pub struct LocalEngine {
    runtime: Runtime,
    models_dir: PathBuf,
    budget: Mutex<Budget>,
    downloads: Downloads,
    stop: Arc<AtomicBool>,
}

impl LocalEngine {
    pub fn new(models_dir: PathBuf, budget: Budget) -> LocalEngine {
        let _ = std::fs::create_dir_all(&models_dir);
        let runtime = Runtime::new(&models_dir, to_budget(budget), budget.keep_alive);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let scheduler = runtime.scheduler().clone();
        let _ = std::thread::Builder::new().name("model-keepalive".into()).spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                scheduler.sweep_wait(&thread_stop);
            }
        });
        LocalEngine { runtime, models_dir, budget: Mutex::new(budget), downloads: Downloads::default(), stop }
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    fn ensure(&self, source: &ModelSource, hf_token: Option<&str>) -> Result<(), EngineError> {
        let ModelSource::HuggingFace { repo } = source else { return Ok(()) };
        if hub::is_downloaded(&self.models_dir, repo) {
            return Ok(());
        }
        self.download(repo, hf_token, None).map(drop)
    }

    fn download(&self, repo: &str, hf_token: Option<&str>, on_progress: Option<ProgressCallback>) -> Result<PathBuf, EngineError> {
        let provider = hf_provider(hf_token);
        let id = self.downloads.start(repo);
        let downloads = self.downloads.clone();
        let result = hub::download(&self.models_dir, repo, provider.as_ref(), move |done, total| {
            downloads.progress(id, done, total);
            if let Some(on_progress) = &on_progress {
                on_progress(done, total);
            }
        });
        self.downloads.finish(id);
        result.map_err(EngineError::classify)
    }

    fn source_path(&self, source: &ModelSource, hf_token: Option<&str>) -> Result<PathBuf, EngineError> {
        match source {
            ModelSource::Local { path } => Ok(path.clone()),
            ModelSource::HuggingFace { repo } => {
                self.ensure(source, hf_token)?;
                let (_, file) = hub::split_spec(repo);
                let dir = hub::model_dir(&self.models_dir, repo);
                Ok(file.map_or_else(|| dir.clone(), |file| dir.join(file)))
            }
            ModelSource::Provider { .. } => Err(EngineError::bad_request("speech models run locally, not at a provider")),
        }
    }
}

impl LocalEngine {
    fn chat_with(&self, request: &ChatRequest, thinking: Option<Thinking>, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError> {
        let mut call = request.clone().into_call()?;
        call.generate.think_tokens = thinking.map(|t| t.budget);
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
                Wait::Memory { name, .. } => WaitingModel { id: *id, model: Some(name.clone()), reason: "memory".into(), progress: None },
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
                    model: m.name.clone(),
                    pool: match m.pool {
                        crate::scheduler::Pool::Gpu => Pool::Gpu,
                        crate::scheduler::Pool::Cpu => Pool::Cpu,
                    },
                    bytes: m.bytes,
                    users: m.leases,
                    idle: m.idle,
                    unloads_in: m.unloads_in,
                })
                .collect(),
            waiting,
            budget: *self.budget.lock().expect("budget lock"),
            used: Used { gpu_bytes: status.gpu_used, ram_bytes: status.ram_used },
        }
    }

    fn set_budget(&self, budget: Budget) {
        self.runtime.set_budget(to_budget(budget), budget.keep_alive);
        *self.budget.lock().expect("budget lock") = budget;
    }

    fn set_wanted(&self, specs: &[String]) {
        let mut keys = HashSet::new();
        for spec in specs {
            let Ok(source) = types::parse_spec(spec) else { continue };
            if let Ok(path) = runtime::gguf_path(&self.models_dir, &source) {
                keys.insert(path.display().to_string());
            }
        }
        self.runtime.set_wanted(keys);
    }

    fn forget_loaded(&self, model: &str) {
        self.runtime.forget(model);
    }

    fn chat(&self, request: &ChatRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError> {
        self.chat_with(request, None, cancel, on_text)
    }

    fn chat_thinking(&self, request: &ChatRequest, thinking: Thinking, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError> {
        self.chat_with(request, Some(thinking), cancel, on_text)
    }

    fn embeddings(&self, request: &EmbeddingsRequest, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, EngineError> {
        let call = request.clone().into_call()?;
        self.ensure(&call.source, call.hf_token.as_deref())?;
        let how = Use { kind: WorkKind::Indexing, device: call.device, n_ctx: call.n_ctx, interactive: false };
        let kind = call.kind;
        self.runtime
            .with_model::<Embedder, _>(&call.source, how, cancel, |model, _lease| {
                call.texts
                    .iter()
                    .map(|text| {
                        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
                        match kind {
                            EmbedKind::Query => model.query(text),
                            EmbedKind::Passage => model.passage(text),
                        }
                    })
                    .collect()
            })
            .map_err(EngineError::classify)
    }

    fn rerank(&self, request: &RerankRequest, cancel: &AtomicBool) -> Result<Vec<RerankResult>, EngineError> {
        let call = request.clone().into_call()?;
        self.ensure(&call.source, call.hf_token.as_deref())?;
        let how = Use { kind: WorkKind::Indexing, device: call.device, n_ctx: call.n_ctx, interactive: false };
        let scores = self
            .runtime
            .with_model::<Reranker, Vec<f32>>(&call.source, how, cancel, |model, _lease| {
                call.documents
                    .iter()
                    .map(|document| {
                        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
                        model.score(&call.query, document)
                    })
                    .collect::<anyhow::Result<Vec<f32>>>()
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

    fn diarize(&self, request: &DiarizeRequest, samples: &[f32]) -> Result<Diarization, EngineError> {
        let speakers = validate::speakers(request.speakers)?;
        let segmentation = self.source_path(&types::parse_spec(&request.segmentation)?, request.hf_token.as_deref())?;
        let embedding = self.source_path(&types::parse_spec(&request.embedding)?, request.hf_token.as_deref())?;
        let found = speech::diarize(&segmentation, &embedding, samples, speakers, request.clustering_threshold)
            .map_err(EngineError::classify)?;
        let Some(threshold) = request.merge_threshold else { return Ok(found) };
        let min_seconds = request.min_speaker_seconds.unwrap_or(speech::MIN_SPEAKER_SECONDS);
        let (turns, voices) = speech::merge_voices(&found.turns, &found.voices, threshold, min_seconds);
        Ok(Diarization { turns, voices })
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
                let (_, file) = hub::split_spec(&repo);
                if hub::is_downloaded(&self.models_dir, &repo) {
                    let dir = hub::model_dir(&self.models_dir, &repo);
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
            ModelSource::Provider { .. } => Err(EngineError::bad_request("a provider's model is not downloaded: it runs at the provider")),
        }
    }

    fn delete_model(&self, spec: &str) -> Result<(), EngineError> {
        let root = std::fs::canonicalize(&self.models_dir)
            .map_err(|e| EngineError::internal(format!("reading the models folder {}: {e}", self.models_dir.display())))?;
        let path = match types::parse_spec(spec)? {
            ModelSource::HuggingFace { repo } => hub::model_dir(&self.models_dir, &repo),
            ModelSource::Local { path } => path,
            ModelSource::Provider { .. } => return Err(EngineError::bad_request("a provider's model is not stored locally: it lives at the provider")),
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
            ModelRole::Speech => Ok(()),
            ModelRole::Agent | ModelRole::Titles => {
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

struct RealVad(speech::Vad);

impl Drop for LocalEngine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.runtime.scheduler().wake();
    }
}

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
        keep_alive_secs: bytes.keep_alive.as_secs(),
        ..mediaa_core::agent::Budget::default()
    }
}

fn gigabytes(bytes: u64) -> f32 {
    (bytes as f64 / (1024.0 * 1024.0 * 1024.0)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_and_scheduler_keys() {
        let budget = to_budget(Budget { gpu_bytes: 2 << 30, ram_bytes: 512 << 20, keep_alive: DEFAULT_KEEP_ALIVE });
        assert_eq!(budget.gpu_memory_gb, 2.0);
        assert_eq!(budget.ram_memory_gb, 0.5);
    }

    #[test]
    fn a_local_engine_starts_without_loading_models() {
        let dir = tempfile::tempdir().unwrap();
        let engine = LocalEngine::new(dir.path().to_path_buf(), Budget { gpu_bytes: 1 << 30, ram_bytes: 1 << 30, keep_alive: DEFAULT_KEEP_ALIVE });
        assert_eq!(engine.status().loaded.len(), 0);
        assert_eq!(engine.status().budget.gpu_bytes, 1 << 30);
        assert_eq!(engine.status().budget.keep_alive, DEFAULT_KEEP_ALIVE);
        engine.set_budget(Budget { gpu_bytes: 2 << 30, ram_bytes: 3 << 30, keep_alive: std::time::Duration::from_secs(12) });
        assert_eq!(engine.status().budget, Budget { gpu_bytes: 2 << 30, ram_bytes: 3 << 30, keep_alive: std::time::Duration::from_secs(12) });
        assert!(engine.ensure(&ModelSource::Local { path: std::path::Path::new("/nope.gguf").into() }, None).is_ok());
        assert_eq!(engine.health().models_dir, engine.models_dir().display().to_string());
    }

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
    fn a_file_spec_resolves_into_the_folder_its_download_made() {
        let root = tempfile::tempdir().unwrap();
        let dir = downloaded(root.path(), "csukuangfj/vad/silero_vad.onnx", &["silero_vad.onnx"]);
        assert_eq!(dir, hub::model_dir(root.path(), "csukuangfj/vad/silero_vad.onnx"));
        let engine = LocalEngine::new(root.path().to_path_buf(), Budget::default());
        let spec = ModelSource::HuggingFace { repo: "csukuangfj/vad/silero_vad.onnx".into() };
        assert_eq!(engine.source_path(&spec, None).unwrap(), dir.join("silero_vad.onnx"));
        let repo = downloaded(root.path(), "csukuangfj/other", &["model.onnx"]);
        let folder = ModelSource::HuggingFace { repo: "csukuangfj/other".into() };
        assert_eq!(engine.source_path(&folder, None).unwrap(), repo);
        assert!(engine.source_path(&ModelSource::Provider { provider: "p".into(), model: "m".into() }, None).is_err());
    }

    #[test]
    fn a_file_spec_deletes_its_folder_and_the_models_folder_itself_is_kept() {
        let root = tempfile::tempdir().unwrap();
        let model = downloaded(root.path(), "a/b/model.onnx", &["model.onnx"]);
        let other = downloaded(root.path(), "c/d", &["m.gguf"]);
        let engine = LocalEngine::new(root.path().to_path_buf(), Budget::default());

        engine.delete_model("a/b/model.onnx").unwrap();
        assert!(!model.exists(), "the folder of the downloaded file goes");
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
