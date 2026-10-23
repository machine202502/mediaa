use crate::embed::{Embedder, Reranker};
use crate::hub;
use crate::llm::{estimate_bytes, LocalModel};
use crate::scheduler::{Lease, ModelNeed, Pool, Scheduler};
use anyhow::Context;
use mediaa_core::agent::{Budget, Device, ModelSource, WorkKind};
use mediaa_core::pulse;
use std::any::Any;
use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const QUANTUM: Duration = Duration::from_secs(20);

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

type Loaded = Arc<Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>>;

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

pub fn source_name(source: &ModelSource) -> anyhow::Result<String> {
    Ok(match source {
        ModelSource::HuggingFace { repo } => repo.trim().to_string(),
        ModelSource::Local { path } => std::path::absolute(path)
            .with_context(|| format!("the path of a model: {}", path.display()))?
            .display()
            .to_string(),
        ModelSource::Provider { .. } => anyhow::bail!("a provider's model does not run locally"),
    })
}

#[derive(Debug, Clone, Copy)]
pub struct Use {
    pub kind: WorkKind,
    pub device: Device,
    pub n_ctx: u32,
    pub interactive: bool,
}

#[derive(Clone)]
pub struct Runtime {
    models_root: PathBuf,
    scheduler: Scheduler,
    loaded: Loaded,
}

impl Runtime {
    pub fn new(models_root: &Path, budget: Budget, keep_alive: Duration) -> Runtime {
        let loaded: Loaded = Arc::default();
        let l = loaded.clone();
        let scheduler = Scheduler::new(budget, QUANTUM, keep_alive, move |key, _| {
            l.lock().expect("models lock").remove(key);
            log::info!("model unloaded: {key}");
        });
        Runtime { models_root: models_root.to_path_buf(), scheduler, loaded }
    }

    pub fn scheduler(&self) -> &Scheduler {
        &self.scheduler
    }

    pub fn set_budget(&self, budget: Budget, keep_alive: Duration) {
        self.scheduler.set_budget(budget);
        self.scheduler.set_keep_alive(keep_alive);
    }

    pub fn set_wanted(&self, keys: HashSet<String>) {
        self.scheduler.set_wanted(keys);
    }

    pub fn with_model<E: Engine, R>(
        &self,
        source: &ModelSource,
        how: Use,
        cancel: &AtomicBool,
        mut f: impl FnMut(&E, &Lease) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        let Use { kind, device, n_ctx, interactive } = how;
        let path = gguf_path(&self.models_root, source)?;
        let key = path.display().to_string();
        let need = ModelNeed { key: key.clone(), name: source_name(source)?, bytes: estimate_bytes(&path, n_ctx)?, device, n_ctx };
        let lease = self.scheduler.request(kind, Some(need), interactive).wait(cancel)?;
        pulse::using(&key);
        let outcome = self.run_with_retry::<E, R>(&path, &key, device, &lease, &mut f);
        drop(lease);
        pulse::release();
        outcome
    }

    fn run_with_retry<E: Engine, R>(
        &self,
        path: &Path,
        key: &str,
        device: Device,
        lease: &Lease,
        f: &mut impl FnMut(&E, &Lease) -> anyhow::Result<R>,
    ) -> anyhow::Result<R> {
        match self.attempt::<E, R>(path, key, device, lease, f) {
            Ok(value) => Ok(value),
            Err(Fail::GiveUp(error)) => Err(error),
            Err(Fail::Again(error)) => {
                log::warn!("model call failed ({error:#}); reloading {key} and retrying once");
                self.loaded.lock().expect("models lock").remove(key);
                match self.attempt::<E, R>(path, key, device, lease, f) {
                    Ok(value) => Ok(value),
                    Err(Fail::GiveUp(error)) => Err(error),
                    Err(Fail::Again(error)) => {
                        self.loaded.lock().expect("models lock").remove(key);
                        self.scheduler.forget_failed(key);
                        Err(model_failed(&error))
                    }
                }
            }
        }
    }

    fn attempt<E: Engine, R>(
        &self,
        path: &Path,
        key: &str,
        device: Device,
        lease: &Lease,
        f: &mut impl FnMut(&E, &Lease) -> anyhow::Result<R>,
    ) -> Result<R, Fail> {
        let model = match self.prepare::<E>(path, key, device, lease) {
            Ok(model) => model,
            Err(error) => return Err(Fail::Again(error)),
        };
        pulse::beat();
        let called = catch_unwind(AssertUnwindSafe(|| {
            let guard = model.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            f(&guard, lease)
        }));
        match called {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) if recoverable(&error) => Err(Fail::Again(error)),
            Ok(Err(error)) => Err(Fail::GiveUp(error)),
            Err(payload) => Err(Fail::Again(anyhow::anyhow!("{}", panic_text(&payload)))),
        }
    }

    fn prepare<E: Engine>(&self, path: &Path, key: &str, device: Device, lease: &Lease) -> anyhow::Result<Arc<Mutex<E>>> {
        if !lease.fresh() {
            if let Some(model) = self.resident::<E>(key) {
                return Ok(model);
            }
        }
        self.loaded.lock().expect("models lock").remove(key);
        match catch_unwind(AssertUnwindSafe(|| self.load_model::<E>(path, key, device, lease))) {
            Ok(result) => result,
            Err(payload) => Err(anyhow::anyhow!("{}", panic_text(&payload))),
        }
    }

    fn resident<E: Engine>(&self, key: &str) -> Option<Arc<Mutex<E>>> {
        self.loaded.lock().expect("models lock").get(key).cloned().and_then(|model| model.downcast::<Mutex<E>>().ok())
    }

    fn load_model<E: Engine>(&self, path: &Path, key: &str, device: Device, lease: &Lease) -> anyhow::Result<Arc<Mutex<E>>> {
        let gpu = lease.pool() == Some(Pool::Gpu);
        let n_ctx = lease.n_ctx();
        let model = match E::load(path, gpu, n_ctx) {
            Ok(model) => model,
            Err(error) if gpu && device != Device::Cpu => {
                log::warn!("loading {} on the GPU failed ({error:#}); using the CPU", path.display());
                E::load(path, false, n_ctx)?
            }
            Err(error) => return Err(error),
        };
        log::info!("model loaded: {key} ({})", if model.on_gpu() { "GPU" } else { "CPU" });
        let model = Arc::new(Mutex::new(model));
        self.loaded.lock().expect("models lock").insert(key.to_string(), model.clone() as Arc<dyn Any + Send + Sync>);
        Ok(model)
    }

    pub fn loaded(&self) -> Vec<String> {
        self.loaded.lock().expect("models lock").keys().cloned().collect()
    }

    pub fn forget(&self, key: &str) {
        self.loaded.lock().expect("models lock").remove(key);
        self.scheduler.reclaim(key);
    }
}

enum Fail {
    Again(anyhow::Error),
    GiveUp(anyhow::Error),
}

fn model_failed(error: &anyhow::Error) -> anyhow::Error {
    anyhow::anyhow!("model failed to load: {error:#}")
}

fn recoverable(error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_lowercase();
    message.contains("out of memory") || message.contains("outofmemory") || message.contains("failed to allocate") || message.contains("cannot allocate")
}

fn panic_text(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_string();
    }
    payload.downcast_ref::<String>().cloned().unwrap_or_else(|| "panicked".to_string())
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
        let direct = dir.path().join("direct.gguf");
        std::fs::write(&direct, b"x").unwrap();
        assert_eq!(gguf_path(dir.path(), &local(&direct)).unwrap(), direct);
        let repo = dir.path().join("a--b");
        std::fs::create_dir(&repo).unwrap();
        std::fs::write(repo.join("m-Q4_K_M.gguf"), b"x").unwrap();
        std::fs::write(repo.join(".mediaa-complete"), b"a/b").unwrap();
        assert_eq!(gguf_path(dir.path(), &hf).unwrap(), repo.join("m-Q4_K_M.gguf"));
    }

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
        let rt = Runtime::new(dir.path(), Budget::default(), Duration::ZERO);
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
        let tiny = Runtime::new(dir.path(), Budget { gpu_memory_gb: 0.0, ram_memory_gb: 0.0, ..Budget::default() }, Duration::ZERO);
        assert!(tiny.with_model(&source, how, &cancel, |_: &LocalModel, _| Ok(())).is_err());
    }

    use std::sync::atomic::{AtomicU32, Ordering};

    struct Script {
        loads: AtomicU32,
        calls: AtomicU32,
        fail_loads: u32,
        panic_calls: u32,
        oom_calls: u32,
    }

    impl Default for Script {
        fn default() -> Self {
            Self { loads: AtomicU32::new(0), calls: AtomicU32::new(0), fail_loads: 0, panic_calls: 0, oom_calls: 0 }
        }
    }

    fn scripts() -> &'static Mutex<HashMap<String, Arc<Script>>> {
        use std::sync::OnceLock;
        static SCRIPTS: OnceLock<Mutex<HashMap<String, Arc<Script>>>> = OnceLock::new();
        SCRIPTS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    fn script(path: &Path) -> Arc<Script> {
        scripts().lock().unwrap().entry(path.display().to_string()).or_insert_with(|| Arc::new(Script::default())).clone()
    }

    struct FakeModel {
        n_ctx: u32,
        path: PathBuf,
    }

    impl Engine for FakeModel {
        fn load(path: &Path, _gpu: bool, n_ctx: u32) -> anyhow::Result<Self> {
            let script = script(path);
            let n = script.loads.fetch_add(1, Ordering::SeqCst);
            if n < script.fail_loads {
                anyhow::bail!("weights are corrupt");
            }
            Ok(FakeModel { n_ctx, path: path.to_path_buf() })
        }

        fn on_gpu(&self) -> bool {
            false
        }
    }

    fn runtime(dir: &Path) -> Runtime {
        Runtime::new(dir, Budget { gpu_memory_gb: 0.0, ram_memory_gb: 8.0, ..Budget::default() }, Duration::from_secs(300))
    }

    fn file(dir: &Path, name: &str) -> (PathBuf, ModelSource) {
        let path = dir.join(name);
        std::fs::write(&path, b"x").unwrap();
        let source = ModelSource::Local { path: path.clone() };
        (path, source)
    }

    fn use_model(rt: &Runtime, source: &ModelSource, n_ctx: u32, f: impl FnMut(&FakeModel, &Lease) -> anyhow::Result<u32>) -> anyhow::Result<u32> {
        let cancel = AtomicBool::new(false);
        rt.with_model(source, Use { kind: WorkKind::Agent, device: Device::Cpu, n_ctx, interactive: false }, &cancel, f)
    }

    #[test]
    fn different_contexts_of_one_file_are_one_load() {
        let dir = tempfile::tempdir().unwrap();
        let (path, source) = file(dir.path(), "m.gguf");
        let rt = runtime(dir.path());
        let ctx = |n_ctx| use_model(&rt, &source, n_ctx, |model, lease| Ok(assert_same(model, lease))).unwrap();
        assert_eq!(ctx(32), 32);
        assert_eq!(script(&path).loads.load(Ordering::SeqCst), 1);
        assert_eq!(ctx(16), 32, "a smaller context reuses the load");
        assert_eq!(script(&path).loads.load(Ordering::SeqCst), 1);
        assert_eq!(ctx(64), 64, "a larger context reloads once");
        assert_eq!(script(&path).loads.load(Ordering::SeqCst), 2);
        assert_eq!(ctx(32), 64, "it does not shrink back");
        assert_eq!(script(&path).loads.load(Ordering::SeqCst), 2);
        assert_eq!(rt.loaded().len(), 1, "still the one resident");
    }

    fn assert_same(model: &FakeModel, lease: &Lease) -> u32 {
        assert_eq!(model.n_ctx, lease.n_ctx());
        model.n_ctx
    }

    #[test]
    fn a_resident_is_reported_by_the_name_the_caller_asked_for() {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("owner--repo");
        std::fs::create_dir(&repo).unwrap();
        let file = repo.join("m-Q4_K_M.gguf");
        std::fs::write(&file, b"x").unwrap();
        std::fs::write(repo.join(".mediaa-complete"), b"owner/repo").unwrap();
        let source = ModelSource::HuggingFace { repo: "owner/repo".into() };
        let rt = runtime(dir.path());
        let ctx = |n_ctx| use_model(&rt, &source, n_ctx, |model, lease| Ok(assert_same(model, lease))).unwrap();
        assert_eq!(ctx(32), 32);
        assert_eq!(ctx(32), 32);
        assert_eq!(script(&file).loads.load(Ordering::SeqCst), 1, "one load for two calls");
        let status = rt.scheduler().status();
        assert_eq!(status.loaded.len(), 1);
        assert_eq!(status.loaded[0].name, "owner/repo");
        assert_eq!(status.loaded[0].key, file.display().to_string(), "the key stays the file");
    }

    #[test]
    fn a_failing_call_reloads_once_and_then_reports_the_model() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime(dir.path());

        let (panicked, source) = file(dir.path(), "panic.gguf");
        set_script(&panicked, Script { panic_calls: 1, ..Script::default() });
        let value = use_model(&rt, &source, 32, |model, _| {
            let seen = script(&model.path).calls.fetch_add(1, Ordering::SeqCst);
            if seen < script(&model.path).panic_calls {
                panic!("weights blew up");
            }
            Ok(7)
        })
        .unwrap();
        assert_eq!(value, 7, "the caller sees the recovered call");
        assert_eq!(script(&panicked).loads.load(Ordering::SeqCst), 2, "unloaded and loaded again");

        let (oom, source) = file(dir.path(), "oom.gguf");
        set_script(&oom, Script { oom_calls: 1, ..Script::default() });
        let value = use_model(&rt, &source, 32, |model, _| {
            let seen = script(&model.path).calls.fetch_add(1, Ordering::SeqCst);
            if seen < script(&model.path).oom_calls {
                anyhow::bail!("out of memory");
            }
            Ok(8)
        })
        .unwrap();
        assert_eq!(value, 8);
        assert_eq!(script(&oom).loads.load(Ordering::SeqCst), 2);

        let (broken, source) = file(dir.path(), "load.gguf");
        set_script(&broken, Script { fail_loads: 1, ..Script::default() });
        let value = use_model(&rt, &source, 32, |model, _| {
            script(&model.path).calls.fetch_add(1, Ordering::SeqCst);
            Ok(9)
        })
        .unwrap();
        assert_eq!(value, 9);
        assert_eq!(script(&broken).loads.load(Ordering::SeqCst), 2, "the failed load is retried once");
        assert_eq!(script(&broken).calls.load(Ordering::SeqCst), 1, "the call runs on the load that worked");

        let (dead, source) = file(dir.path(), "dead.gguf");
        set_script(&dead, Script { fail_loads: 2, ..Script::default() });
        let error = use_model(&rt, &source, 32, |_, _| Ok(0)).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("model failed to load"), "{message}");
        assert!(!message.to_lowercase().contains("engine"), "{message}");
        assert_eq!(script(&dead).loads.load(Ordering::SeqCst), 2);
        let dead_key = dead.display().to_string();
        assert!(rt.scheduler().status().loaded.iter().all(|m| m.key != dead_key), "a model that failed to load is not kept");
        assert!(!rt.loaded().iter().any(|k| k == &dead_key));

        let (plain, source) = file(dir.path(), "plain.gguf");
        let error = use_model(&rt, &source, 32, |_, _| anyhow::bail!("the prompt is empty")).unwrap_err();
        assert!(format!("{error:#}").contains("the prompt is empty"), "a prompt error is not a reload");
        assert_eq!(script(&plain).loads.load(Ordering::SeqCst), 1);
    }

    fn set_script(path: &Path, script: Script) {
        scripts().lock().unwrap().insert(path.display().to_string(), Arc::new(script));
    }

    #[test]
    fn a_stuck_call_is_forgotten_and_the_retry_loads_a_fresh_model() {
        use std::sync::Barrier;

        let dir = tempfile::tempdir().unwrap();
        let (path, source) = file(dir.path(), "stuck.gguf");
        let rt = runtime(dir.path());
        let entered = Arc::new(Barrier::new(2));
        let release = Arc::new(AtomicBool::new(false));
        let stuck = {
            let (rt, source, entered, release) = (rt.clone(), source.clone(), entered.clone(), release.clone());
            std::thread::spawn(move || {
                let cancel = AtomicBool::new(false);
                rt.with_model(&source, Use { kind: WorkKind::Indexing, device: Device::Cpu, n_ctx: 32, interactive: false }, &cancel, |model: &FakeModel, _| {
                    entered.wait();
                    while !release.load(Ordering::SeqCst) {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Ok(model.n_ctx)
                })
            })
        };
        entered.wait();
        let key = path.display().to_string();
        rt.forget(&key);
        assert!(rt.loaded().is_empty(), "the stuck model is dropped, nothing resident is left to reuse");

        let (tx, rx) = std::sync::mpsc::channel();
        let (retry_rt, retry_source) = (rt.clone(), source.clone());
        std::thread::spawn(move || {
            let value = use_model(&retry_rt, &retry_source, 32, |model, _| Ok(model.n_ctx));
            let _ = tx.send(value.map_err(|e| format!("{e:#}")));
        });
        let value = rx.recv_timeout(Duration::from_secs(5)).expect("the retry is not blocked by the stuck call").unwrap();
        assert_eq!(value, 32);
        assert_eq!(script(&path).loads.load(Ordering::SeqCst), 2, "a fresh model was loaded");

        release.store(true, Ordering::SeqCst);
        assert_eq!(stuck.join().unwrap().unwrap(), 32, "the stuck call still finishes on the old model");
    }
}
