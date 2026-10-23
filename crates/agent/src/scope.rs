use mediaa_core::agent::ModelRole;
use mediaa_engine::speech::Phrase;
use mediaa_engine::{
    Budget as EngineBudget, ChatAnswer, ChatRequest, Diarization, DiarizeRequest, EmbeddingsRequest, Engine, EngineError, Health, ModelCheck, ModelInfo,
    ModelInspect, ProgressCallback, RerankRequest, RerankResult, Status, Thinking, VadSession,
};
use mediaa_queue::{Stats, Status as TaskStatus, Store, TaskId};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRef {
    pub kind: String,
    pub title: String,
}

#[derive(Clone)]
pub struct Scope {
    store: Arc<dyn Store>,
    task: TaskId,
    engine: Arc<dyn Engine>,
    key: Option<String>,
    focus: Arc<Mutex<HashMap<ThreadId, TaskId>>>,
}

impl Scope {
    pub fn attach(store: Arc<dyn Store>, task: TaskId, engine: Arc<dyn Engine>, key: Option<String>) -> Scope {
        Scope { store, task, engine, key, focus: Arc::new(Mutex::new(HashMap::new())) }
    }

    pub fn begin(dir: &Path, engine: Arc<dyn Engine>, kind: &str, title: &str, input: &Value, key: Option<&str>) -> anyhow::Result<Scope> {
        let store = crate::run::open_store(dir)?;
        let task = store.begin_inline(kind, title, input, None, key)?;
        Ok(Scope::attach(store, task, engine, key.map(str::to_string)))
    }

    pub fn task(&self) -> TaskId {
        self.task
    }

    pub fn store(&self) -> &Arc<dyn Store> {
        &self.store
    }

    pub fn focus(&self, task: Option<TaskId>) {
        let mut focus = self.focus.lock().expect("scope focus lock");
        match task {
            Some(task) => {
                focus.insert(std::thread::current().id(), task);
            }
            None => {
                focus.remove(&std::thread::current().id());
            }
        }
    }

    fn accounted(&self) -> TaskId {
        self.focus.lock().expect("scope focus lock").get(&std::thread::current().id()).copied().unwrap_or(self.task)
    }

    pub fn record_call(&self, model: &str, started: Instant, prompt_tokens: u64, completion_tokens: u64) {
        self.add_stats(&[model], started, prompt_tokens, completion_tokens);
    }

    pub fn set_detail(&self, detail: &Value) {
        if let Err(e) = self.store.set_detail(self.task, detail) {
            log::warn!("task {}: its detail was not saved: {e:#}", self.task);
        }
    }

    pub fn finish(&self, result: &Value) {
        self.end("done", |store, id| Ok(store.complete(id, result).map(|_| ())?));
    }

    pub fn fail(&self, error: &str) {
        self.end("failed", |store, id| Ok(store.fail(id, error).map(|_| ())?));
    }

    pub fn cancel(&self) {
        self.end("cancelled", |store, id| Ok(store.cancel(id).map(|_| ())?));
    }

    pub fn busy_with(&self) -> Vec<TaskRef> {
        let tasks = self.store.list().unwrap_or_default();
        let parents: std::collections::HashSet<TaskId> = tasks.iter().filter_map(|t| t.parent).collect();
        tasks
            .into_iter()
            .filter(|t| t.status == TaskStatus::Running && !t.inline && !parents.contains(&t.id))
            .map(|t| TaskRef { kind: t.kind, title: t.title })
            .take(3)
            .collect()
    }

    fn end(&self, what: &str, f: impl FnOnce(&dyn Store, TaskId) -> anyhow::Result<()>) {
        let result = (|| {
            match self.store.get(self.task)? {
                Some(t) if t.status == TaskStatus::Running => f(self.store.as_ref(), self.task),
                _ => Ok(()),
            }
        })();
        if let Err(e) = result {
            log::warn!("task {}: it was not marked {what}: {e:#}", self.task);
        }
    }

    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    pub fn engine(&self) -> Arc<dyn Engine> {
        Arc::new(ScopedEngine { scope: self.clone() })
    }

    pub fn inline_child(&self, kind: &str, title: &str, input: &Value) -> anyhow::Result<Scope> {
        let id = self.store.begin_inline(kind, title, input, Some(self.task), self.key.as_deref())?;
        Ok(Scope {
            store: self.store.clone(),
            task: id,
            engine: self.engine.clone(),
            key: self.key.clone(),
            focus: Arc::new(Mutex::new(HashMap::new())),
        })
    }

    fn add_stats(&self, models: &[&str], started: Instant, prompt_tokens: u64, completion_tokens: u64) {
        let delta = Stats {
            model_calls: 1,
            prompt_tokens,
            completion_tokens,
            model_ms: started.elapsed().as_millis() as u64,
            models: models.iter().map(|m| (*m).to_string()).filter(|m| !m.is_empty()).collect(),
        };
        if let Err(e) = self.store.add_stats(self.accounted(), &delta) {
            log::warn!("task {}: its stats were not saved: {e:#}", self.task);
        }
    }
}

struct ScopedEngine {
    scope: Scope,
}

impl Engine for ScopedEngine {
    fn health(&self) -> Health {
        self.scope.engine.health()
    }

    fn status(&self) -> Status {
        self.scope.engine.status()
    }

    fn set_budget(&self, budget: EngineBudget) {
        self.scope.engine.set_budget(budget);
    }

    fn set_wanted(&self, specs: &[String]) {
        self.scope.engine.set_wanted(specs);
    }

    fn forget_loaded(&self, model: &str) {
        self.scope.engine.forget_loaded(model);
    }

    fn chat(&self, request: &ChatRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.chat(request, cancel, on_text);
        let usage = result.as_ref().ok().and_then(|answer| answer.usage);
        self.scope.add_stats(
            &[&request.model],
            started,
            usage.map_or(0, |u| u64::from(u.prompt_tokens)),
            usage.map_or(0, |u| u64::from(u.completion_tokens)),
        );
        result
    }

    fn chat_thinking(&self, request: &ChatRequest, thinking: Thinking, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ChatAnswer, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.chat_thinking(request, thinking, cancel, on_text);
        let usage = result.as_ref().ok().and_then(|answer| answer.usage);
        self.scope.add_stats(
            &[&request.model],
            started,
            usage.map_or(0, |u| u64::from(u.prompt_tokens)),
            usage.map_or(0, |u| u64::from(u.completion_tokens)),
        );
        result
    }

    fn embeddings(&self, request: &EmbeddingsRequest, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.embeddings(request, cancel);
        self.scope.add_stats(&[&request.model], started, 0, 0);
        result
    }

    fn rerank(&self, request: &RerankRequest, cancel: &AtomicBool) -> Result<Vec<RerankResult>, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.rerank(request, cancel);
        self.scope.add_stats(&[&request.model], started, 0, 0);
        result
    }

    fn vad_open(&self, model: &str, offset: f64, hf_token: Option<&str>) -> Result<Box<dyn VadSession>, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.vad_open(model, offset, hf_token);
        self.scope.add_stats(&[model], started, 0, 0);
        result
    }

    fn recognize(&self, model: &str, offset: f64, samples: &[f32], hf_token: Option<&str>) -> Result<Phrase, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.recognize(model, offset, samples, hf_token);
        self.scope.add_stats(&[model], started, 0, 0);
        result
    }

    fn diarize(&self, request: &DiarizeRequest, samples: &[f32]) -> Result<Diarization, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.diarize(request, samples);
        self.scope.add_stats(&[&request.segmentation, &request.embedding], started, 0, 0);
        result
    }

    fn models(&self) -> Result<Vec<ModelInfo>, EngineError> {
        self.scope.engine.models()
    }

    fn inspect(&self, spec: &str, hf_token: Option<&str>) -> Result<ModelInspect, EngineError> {
        self.scope.engine.inspect(spec, hf_token)
    }

    fn pull(&self, spec: &str, hf_token: Option<&str>, on_progress: ProgressCallback) -> Result<std::path::PathBuf, EngineError> {
        self.scope.engine.pull(spec, hf_token, on_progress)
    }

    fn delete_model(&self, spec: &str) -> Result<(), EngineError> {
        self.scope.engine.delete_model(spec)
    }

    fn check_model(&self, spec: &str, role: ModelRole, hf_token: Option<&str>) -> Result<ModelCheck, EngineError> {
        self.scope.engine.check_model(spec, role, hf_token)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_engine::testing::FakeEngine;
    use mediaa_engine::{ChatMessage, Usage};
    use serde_json::json;

    fn opened(dir: &Path) -> Arc<dyn Store> {
        crate::run::open_store(dir).unwrap()
    }

    fn task(store: &dyn Store, kind: &str, scope: Option<&str>) -> TaskId {
        store.begin_inline(kind, kind, &json!({}), None, scope).unwrap()
    }

    #[test]
    fn model_calls_are_counted_and_service_calls_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let store = opened(dir.path());
        let id = task(store.as_ref(), "model-step", Some("rec-1"));
        let fake = FakeEngine::new();
        fake.chat_usage(Some(Usage { prompt_tokens: 11, completion_tokens: 4 }));
        let scope = Scope::attach(store.clone(), id, Arc::new(fake.clone()), Some("rec-1".into()));
        let engine = scope.engine();

        let request = ChatRequest { model: "owner/model".into(), messages: vec![ChatMessage::user("hi")], ..ChatRequest::default() };
        let cancel = AtomicBool::new(false);
        engine.chat(&request, &cancel, &mut |_| true).unwrap();
        engine.embeddings(&EmbeddingsRequest { model: "owner/embed".into(), input: vec!["a".into(), "b".into()], ..EmbeddingsRequest::default() }, &cancel).unwrap();
        engine.rerank(&RerankRequest { model: "owner/rerank".into(), query: "q".into(), documents: vec!["d".into()], ..RerankRequest::default() }, &cancel).unwrap();
        engine.vad_open("owner/vad.onnx", 0.0, None).unwrap();
        engine.recognize("owner/asr", 0.0, &[0.0; 16], None).unwrap();
        engine
            .diarize(
                &DiarizeRequest {
                    segmentation: "owner/seg.onnx".into(),
                    embedding: "owner/spk.onnx".into(),
                    speakers: None,
                    hf_token: None,
                    ..DiarizeRequest::default()
                },
                &[0.0; 16],
            )
            .unwrap();

        let stats = store.get(id).unwrap().unwrap().stats;
        assert_eq!(stats.model_calls, 6);
        assert_eq!((stats.prompt_tokens, stats.completion_tokens), (11, 4), "tokens come from the answer");
        assert_eq!(stats.models, ["owner/model", "owner/embed", "owner/rerank", "owner/vad.onnx", "owner/asr", "owner/seg.onnx", "owner/spk.onnx"]);

        engine.health();
        engine.status();
        engine.models().unwrap();
        engine.set_budget(EngineBudget::default());
        engine.set_wanted(&["owner/model".into()]);
        let after = store.get(id).unwrap().unwrap().stats;
        assert_eq!(after, stats);
    }

    #[test]
    fn no_usage_means_no_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let store = opened(dir.path());
        let id = task(store.as_ref(), "model-step", None);
        let fake = FakeEngine::new();
        fake.chat_usage(None);
        let scope = Scope::attach(store.clone(), id, Arc::new(fake), None);
        let cancel = AtomicBool::new(false);
        scope.engine().chat(&ChatRequest { model: "owner/model".into(), messages: vec![ChatMessage::user("hi")], ..ChatRequest::default() }, &cancel, &mut |_| true).unwrap();
        let stats = store.get(id).unwrap().unwrap().stats;
        assert_eq!((stats.model_calls, stats.prompt_tokens, stats.completion_tokens), (1, 0, 0));
    }

    #[test]
    fn an_inline_child_belongs_to_the_task_and_the_recording() {
        let dir = tempfile::tempdir().unwrap();
        let store = opened(dir.path());
        let parent = task(store.as_ref(), "answer", Some("rec-7"));
        let scope = Scope::attach(store.clone(), parent, Arc::new(FakeEngine::new()), Some("rec-7".into()));
        let child = scope.inline_child("tool", "search", &json!({"query": "x"})).unwrap();
        let t = store.get(child.task()).unwrap().unwrap();
        assert!(t.inline);
        assert_eq!(t.parent, Some(parent));
        assert_eq!(t.scope.as_deref(), Some("rec-7"));
        assert_eq!(t.kind, "tool");
        assert_eq!(t.status, TaskStatus::Running);
    }

    #[test]
    fn threads_of_one_scope_focus_their_own_tasks() {
        let dir = tempfile::tempdir().unwrap();
        let store = opened(dir.path());
        let parent = task(store.as_ref(), "answer", None);
        let scope = Scope::attach(store.clone(), parent, Arc::new(FakeEngine::new()), None);
        let children: Vec<TaskId> = (0..2).map(|_| scope.inline_child("tool", "search", &json!({})).unwrap().task()).collect();
        let engine = scope.engine();
        let cancel = AtomicBool::new(false);
        let cancel = &cancel;
        std::thread::scope(|s| {
            for &child in &children {
                let (engine, scope) = (engine.clone(), scope.clone());
                s.spawn(move || {
                    scope.focus(Some(child));
                    let request = ChatRequest { model: "owner/model".into(), messages: vec![ChatMessage::user("hi")], ..ChatRequest::default() };
                    engine.chat(&request, cancel, &mut |_| true).unwrap();
                    scope.focus(None);
                });
            }
        });
        for child in children {
            assert_eq!(store.get(child).unwrap().unwrap().stats.model_calls, 1);
        }
        assert_eq!(store.get(parent).unwrap().unwrap().stats.model_calls, 0, "the parent runs nothing itself");
    }

    #[test]
    fn set_wanted_reaches_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let store = opened(dir.path());
        let id = task(store.as_ref(), "chat", None);
        let fake = FakeEngine::new();
        let scope = Scope::attach(store, id, Arc::new(fake.clone()), None);
        scope.engine().set_wanted(&["owner/keep".into()]);
        assert_eq!(fake.wanted(), vec!["owner/keep".to_string()]);
    }
}
