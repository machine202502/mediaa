//! The model runtime: local GGUF models loaded on demand under the budget. A job asks for a
//! model; the scheduler decides the device (or makes it wait), the model is loaded there once
//! and shared by everyone using it, and unloaded as soon as no one needs it.

use crate::embed::{Embedder, Reranker};
use crate::hub;
use crate::llm::{estimate_bytes, LocalModel};
use crate::scheduler::{Lease, ModelNeed, Pool, Scheduler};
use mediaa_core::agent::{Budget, Device, ModelSource, WorkKind};
use std::any::Any;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long a lease may keep memory others wait for before it is asked to yield.
pub const QUANTUM: Duration = Duration::from_secs(20);

/// A kind of local model the runtime loads (language models, embedding models).
pub trait Engine: Sized + Send + 'static {
    fn load(path: &Path, gpu: bool, n_ctx: u32) -> anyhow::Result<Self>;
    fn on_gpu(&self) -> bool;
}

impl Engine for LocalModel {
    fn load(path: &Path, gpu: bool, n_ctx: u32) -> anyhow::Result<Self> {
        LocalModel::load(path, gpu, n_ctx)
    }

    fn on_gpu(&self) -> bool {
        LocalModel::on_gpu(self)
    }
}

impl Engine for Embedder {
    fn load(path: &Path, gpu: bool, _n_ctx: u32) -> anyhow::Result<Self> {
        Embedder::load(path, gpu)
    }

    fn on_gpu(&self) -> bool {
        Embedder::on_gpu(self)
    }
}

impl Engine for Reranker {
    fn load(path: &Path, gpu: bool, _n_ctx: u32) -> anyhow::Result<Self> {
        Reranker::load(path, gpu)
    }

    fn on_gpu(&self) -> bool {
        Reranker::on_gpu(self)
    }
}

/// Loaded models by key: `Arc<Mutex<E>>` of their engine `E`.
type Loaded = Arc<Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>>;

/// The GGUF file of a local model source: a file, or the GGUF inside a folder (a downloaded
/// Hugging Face model or a folder the user picked).
pub fn gguf_path(models_root: &Path, source: &ModelSource) -> anyhow::Result<PathBuf> {
    let place = match source {
        ModelSource::HuggingFace { repo } => {
            anyhow::ensure!(hub::is_downloaded(models_root, repo), "{repo} is not downloaded yet (Models → Check models)");
            hub::model_dir(models_root, repo)
        }
        ModelSource::Local { path } => path.clone(),
        ModelSource::Provider { .. } => anyhow::bail!("a provider model is not a local file"),
    };
    if place.is_file() {
        return Ok(place);
    }
    let mut found: Vec<PathBuf> = std::fs::read_dir(&place)
        .map_err(|e| anyhow::anyhow!("{}: {e}", place.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("gguf")))
        .filter(|p| !p.file_name().unwrap_or_default().to_string_lossy().to_ascii_lowercase().contains("mmproj"))
        .collect();
    found.sort();
    let names: Vec<String> = found.iter().map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned()).collect();
    let pick = hub::pick_gguf(&names).ok_or_else(|| anyhow::anyhow!("{}: no GGUF model inside", place.display()))?;
    Ok(place.join(pick))
}

/// How a job wants a model run.
#[derive(Debug, Clone, Copy)]
pub struct Use {
    pub kind: WorkKind,
    pub device: Device,
    /// Context length in tokens.
    pub n_ctx: u32,
    /// A user is waiting for the answer (goes before background work).
    pub interactive: bool,
}

#[derive(Clone)]
pub struct Runtime {
    models_root: PathBuf,
    scheduler: Scheduler,
    loaded: Loaded,
}

impl Runtime {
    /// A runtime over a models folder (models are directly in `models_root`, as the engine
    /// stores them).
    pub fn new(models_root: &Path, budget: Budget) -> Runtime {
        let loaded: Loaded = Arc::default();
        let l = loaded.clone();
        let scheduler = Scheduler::new(budget, QUANTUM, move |key, _| {
            l.lock().expect("models lock").remove(key);
            log::info!("model unloaded: {key}");
        });
        Runtime { models_root: models_root.to_path_buf(), scheduler, loaded }
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn set_budget(&self, budget: Budget) {
        self.scheduler.set_budget(budget);
    }

    /// Runs `f` with the model of `source` loaded (waiting for the budget as needed). The model
    /// stays loaded while `f` runs; `f` gets the lease to check [`Lease::should_yield`].
    pub fn with_model<E: Engine, R>(
        &self,
        source: &ModelSource,
        how: Use,
        cancel: &AtomicBool,
        f: impl FnOnce(&E, &Lease) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        let Use { kind, device, n_ctx, interactive } = how;
        let path = gguf_path(&self.models_root, source)?;
        let key = format!("{}#{n_ctx}", path.display());
        let need = ModelNeed { key: key.clone(), bytes: estimate_bytes(&path, n_ctx)?, device };
        let lease = self.scheduler.request(kind, Some(need), interactive).wait(cancel)?;
        let existing = self.loaded.lock().expect("models lock").get(&key).cloned().and_then(|m| m.downcast::<Mutex<E>>().ok());
        let model = match existing {
            Some(m) => m,
            None => {
                let gpu = lease.pool() == Some(Pool::Gpu);
                let model = match E::load(&path, gpu, n_ctx) {
                    Ok(m) => m,
                    // The GPU could not take it after all: the CPU can.
                    Err(e) if gpu && device != Device::Cpu => {
                        log::warn!("loading {} on the GPU failed ({e:#}); using the CPU", path.display());
                        E::load(&path, false, n_ctx)?
                    }
                    Err(e) => return Err(e),
                };
                log::info!("model loaded: {key} ({})", if model.on_gpu() { "GPU" } else { "CPU" });
                let model = Arc::new(Mutex::new(model));
                self.loaded.lock().expect("models lock").insert(key.clone(), model.clone() as Arc<dyn Any + Send + Sync>);
                model
            }
        };
        let guard = model.lock().expect("model lock");
        let r = f(&guard, &lease);
        drop(guard);
        drop(lease);
        r
    }

    /// Models in memory now.
    pub fn loaded(&self) -> Vec<String> {
        self.loaded.lock().expect("models lock").keys().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gguf_files_of_sources() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path().join("m");
        std::fs::create_dir(&folder).unwrap();
        for f in ["mmproj-F16.gguf", "m-Q8_0.gguf", "m-Q4_K_M.gguf", "README.md"] {
            std::fs::write(folder.join(f), b"x").unwrap();
        }
        let local = |p: &Path| ModelSource::Local { path: p.to_path_buf() };
        assert_eq!(gguf_path(dir.path(), &local(&folder)).unwrap(), folder.join("m-Q4_K_M.gguf"), "the preferred quantisation");
        assert_eq!(gguf_path(dir.path(), &local(&folder.join("m-Q8_0.gguf"))).unwrap(), folder.join("m-Q8_0.gguf"));
        let empty = dir.path().join("empty");
        std::fs::create_dir(&empty).unwrap();
        assert!(gguf_path(dir.path(), &local(&empty)).is_err());
        let hf = ModelSource::HuggingFace { repo: "a/b".into() };
        assert!(gguf_path(dir.path(), &hf).unwrap_err().to_string().contains("not downloaded"));
        assert!(gguf_path(dir.path(), &ModelSource::Provider { provider: "p".into(), model: "m".into() }).is_err());
        // A model file directly in the models folder, and a downloaded repository.
        let direct = dir.path().join("direct.gguf");
        std::fs::write(&direct, b"x").unwrap();
        assert_eq!(gguf_path(dir.path(), &local(&direct)).unwrap(), direct);
        let repo = dir.path().join("a--b");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(repo.join("m-Q4_K_M.gguf"), b"x").unwrap();
        std::fs::write(repo.join(".mediaa-complete"), b"a/b").unwrap();
        assert_eq!(gguf_path(dir.path(), &hf).unwrap(), repo.join("m-Q4_K_M.gguf"));
    }

    /// Loads the 19 MB story model once for two jobs and unloads it after them.
    /// Network: `cargo test -p mediaa-engine -- --ignored`.
    #[test]
    #[ignore]
    fn loads_shares_and_unloads() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stories15M-q4_0.gguf");
        let bytes = reqwest::blocking::get("https://huggingface.co/ggml-org/models/resolve/main/tinyllamas/stories15M-q4_0.gguf")
            .unwrap()
            .bytes()
            .unwrap();
        std::fs::write(&path, &bytes).unwrap();
        let rt = Runtime::new(dir.path(), Budget::default());
        let source = ModelSource::Local { path: path.clone() };
        let cancel = AtomicBool::new(false);
        let how = Use { kind: WorkKind::Agent, device: Device::Auto, n_ctx: 256, interactive: true };
        let opts = crate::llm::Generate { max_tokens: 8, ..Default::default() };
        let text = rt
            .with_model(&source, how, &cancel, |m: &LocalModel, lease| {
                assert!(lease.fresh(), "first use loads it");
                assert_eq!(rt.loaded().len(), 1);
                m.complete("Once upon a time", &opts, &cancel, &mut |_| true).map(|g| g.text)
            })
            .unwrap();
        assert!(!text.is_empty());
        assert!(rt.loaded().is_empty(), "unloaded when no one needs it");
        // Too big for the budget: fails at once instead of waiting forever.
        let tiny = Runtime::new(dir.path(), Budget { gpu_memory_gb: 0.0, ram_memory_gb: 0.0, ..Budget::default() });
        assert!(tiny.with_model(&source, how, &cancel, |_: &LocalModel, _| Ok(())).is_err());
    }
}
