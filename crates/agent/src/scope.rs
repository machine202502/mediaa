//! Work in the context of one queue task: its id, the queue and the engine. Every model call of
//! agent code goes through [`Scope::engine`], which adds the call (tokens, time, model) to the
//! task's stats; there is no other way for agent code to reach the engine. The service calls of
//! the engine (status, downloading and checking models) are not task work and go around it.

use mediaa_core::agent::ModelRole;
use mediaa_core::queue::{Queue, Stats, TaskId};
use mediaa_engine::speech::{Phrase, Turn};
use mediaa_engine::{
    Budget as EngineBudget, ChatAnswer, ChatRequest, DiarizeRequest, EmbeddingsRequest, Engine, EngineError, Health, ModelCheck, ModelInfo, ModelInspect,
    ProgressCallback, RerankRequest, RerankResult, Status, Thinking, VadSession,
};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// A task as the queue page names it: its kind and its title (the data part, see `Task::title`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRef {
    pub kind: String,
    pub title: String,
}

/// A task's work: which task, in which queue, with which engine. Clones share one queue
/// connection (opened on the first use, not one per call).
#[derive(Clone)]
pub struct Scope {
    dir: PathBuf,
    task: TaskId,
    engine: Arc<dyn Engine>,
    key: Option<String>,
    queue: Arc<Mutex<Option<Queue>>>,
    /// The task that model calls are counted in while it is set (a step or a tool of an answer
    /// that is run by this scope); otherwise the scope's own. Clones share it.
    focus: Arc<Mutex<Option<TaskId>>>,
}

impl Scope {
    pub fn new(dir: &Path, task: TaskId, engine: Arc<dyn Engine>, key: Option<String>) -> Scope {
        Scope { dir: dir.to_path_buf(), task, engine, key, queue: Arc::new(Mutex::new(None)), focus: Arc::new(Mutex::new(None)) }
    }

    /// Starts a task that the caller runs itself (a chat answer) and returns its scope.
    pub fn begin(dir: &Path, engine: Arc<dyn Engine>, kind: &str, title: &str, input: &Value, key: Option<&str>) -> anyhow::Result<Scope> {
        let queue = Queue::open(dir)?;
        let task = queue.begin_inline(kind, title, input, None, key)?;
        Ok(Scope { dir: dir.to_path_buf(), task, engine, key: key.map(str::to_string), queue: Arc::new(Mutex::new(Some(queue))), focus: Arc::new(Mutex::new(None)) })
    }

    /// The task this work belongs to.
    pub fn task(&self) -> TaskId {
        self.task
    }

    /// The folder of the queue.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// From now on the model calls of this scope (and of its clones) are counted in `task`, a
    /// child that this caller runs (`None`: in the scope's own task again).
    pub fn focus(&self, task: Option<TaskId>) {
        *self.focus.lock().expect("scope focus lock") = task;
    }

    fn accounted(&self) -> TaskId {
        self.focus.lock().expect("scope focus lock").unwrap_or(self.task)
    }

    /// Counts one model call that did not go through [`Scope::engine`] (a provider's).
    pub fn record_call(&self, model: &str, started: Instant, prompt_tokens: u64, completion_tokens: u64) {
        self.add_stats(&[model], started, prompt_tokens, completion_tokens);
    }

    /// What the task is (for a tool: its name, short arguments and the size of its result).
    pub fn set_detail(&self, detail: &Value) {
        if let Err(e) = self.with_queue(|q| q.set_detail(self.task, detail)) {
            log::warn!("task {}: its detail was not saved: {e:#}", self.task);
        }
    }

    /// The task is done. A task that was stopped meanwhile stays stopped.
    pub fn finish(&self, result: &Value) {
        self.end("done", |q, id| q.finish_inline(id, result));
    }

    /// The task failed.
    pub fn fail(&self, error: &str) {
        self.end("failed", |q, id| q.fail_inline(id, error));
    }

    /// The task was stopped (also its running children).
    pub fn cancel(&self) {
        self.end("cancelled", |q, id| q.cancel_inline(id));
    }

    /// The background tasks running now (not the callers' own work, and not composite
    /// tasks whose children run): what holds the models when this work has to wait.
    pub fn busy_with(&self) -> Vec<TaskRef> {
        let tasks = self.with_queue(|q| q.list()).unwrap_or_default();
        let parents: std::collections::HashSet<TaskId> = tasks.iter().filter_map(|t| t.parent).collect();
        tasks
            .into_iter()
            .filter(|t| t.status == mediaa_core::queue::TaskStatus::Running && !t.inline && !parents.contains(&t.id))
            .map(|t| TaskRef { kind: t.kind, title: t.title })
            .take(3)
            .collect()
    }

    fn end(&self, what: &str, f: impl FnOnce(&Queue, TaskId) -> anyhow::Result<()>) {
        let result = self.with_queue(|q| match q.get(self.task)? {
            // Only a running task ends: one that was cancelled from elsewhere (the stop button)
            // is not brought back to life by the work that was still finishing.
            Some(t) if t.status == mediaa_core::queue::TaskStatus::Running => f(q, self.task),
            _ => Ok(()),
        });
        if let Err(e) = result {
            log::warn!("task {}: it was not marked {what}: {e:#}", self.task);
        }
    }

    /// The recording (or folder) of the task.
    pub fn key(&self) -> Option<&str> {
        self.key.as_deref()
    }

    /// The engine for this task: every model call is counted in its stats.
    pub fn engine(&self) -> Arc<dyn Engine> {
        Arc::new(ScopedEngine { scope: self.clone() })
    }

    /// Starts a task run by this caller (a model step, a tool) under this task, in the same
    /// recording and with the same queue connection.
    pub fn inline_child(&self, kind: &str, title: &str, input: &Value) -> anyhow::Result<Scope> {
        let id = self.with_queue(|q| q.begin_inline(kind, title, input, Some(self.task), self.key.as_deref()))?;
        Ok(Scope { dir: self.dir.clone(), task: id, engine: self.engine.clone(), key: self.key.clone(), queue: self.queue.clone(), focus: Arc::new(Mutex::new(None)) })
    }

    fn with_queue<T>(&self, f: impl FnOnce(&Queue) -> anyhow::Result<T>) -> anyhow::Result<T> {
        let mut queue = self.queue.lock().expect("scope queue lock");
        if queue.is_none() {
            *queue = Some(Queue::open(&self.dir)?);
        }
        f(queue.as_ref().expect("opened above"))
    }

    fn add_stats(&self, models: &[&str], started: Instant, prompt_tokens: u64, completion_tokens: u64) {
        let delta = Stats {
            model_calls: 1,
            prompt_tokens,
            completion_tokens,
            model_ms: started.elapsed().as_millis() as u64,
            models: models.iter().map(|m| (*m).to_string()).filter(|m| !m.is_empty()).collect(),
        };
        if let Err(e) = self.with_queue(|q| q.add_stats(self.accounted(), &delta)) {
            log::warn!("task {}: its stats were not saved: {e:#}", self.task);
        }
    }
}

/// The engine of one scope: model calls are forwarded and counted, service calls are forwarded.
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

    fn embeddings(&self, request: &EmbeddingsRequest) -> Result<Vec<Vec<f32>>, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.embeddings(request);
        self.scope.add_stats(&[&request.model], started, 0, 0);
        result
    }

    fn rerank(&self, request: &RerankRequest) -> Result<Vec<RerankResult>, EngineError> {
        let started = Instant::now();
        let result = self.scope.engine.rerank(request);
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

    fn diarize(&self, request: &DiarizeRequest, samples: &[f32]) -> Result<Vec<Turn>, EngineError> {
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

    fn pull(&self, spec: &str, hf_token: Option<&str>, on_progress: ProgressCallback) -> Result<PathBuf, EngineError> {
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

    fn task(q: &Queue, kind: &str, scope: Option<&str>) -> TaskId {
        q.begin_inline(kind, kind, &json!({}), None, scope).unwrap()
    }

    #[test]
    fn model_calls_are_counted_and_service_calls_are_not() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = task(&q, "model-step", Some("rec-1"));
        let fake = FakeEngine::new();
        fake.chat_usage(Some(Usage { prompt_tokens: 11, completion_tokens: 4 }));
        let scope = Scope::new(dir.path(), id, Arc::new(fake.clone()), Some("rec-1".into()));
        let engine = scope.engine();

        let request = ChatRequest { model: "owner/model".into(), messages: vec![ChatMessage::user("hi")], ..ChatRequest::default() };
        let cancel = AtomicBool::new(false);
        engine.chat(&request, &cancel, &mut |_| true).unwrap();
        engine.embeddings(&EmbeddingsRequest { model: "owner/embed".into(), input: vec!["a".into(), "b".into()], ..EmbeddingsRequest::default() }).unwrap();
        engine.rerank(&RerankRequest { model: "owner/rerank".into(), query: "q".into(), documents: vec!["d".into()], ..RerankRequest::default() }).unwrap();
        engine.vad_open("owner/vad.onnx", 0.0, None).unwrap();
        engine.recognize("owner/asr", 0.0, &[0.0; 16], None).unwrap();
        engine
            .diarize(
                &DiarizeRequest { segmentation: "owner/seg.onnx".into(), embedding: "owner/spk.onnx".into(), speakers: None, hf_token: None },
                &[0.0; 16],
            )
            .unwrap();

        let stats = q.get(id).unwrap().unwrap().stats;
        assert_eq!(stats.model_calls, 6);
        assert_eq!((stats.prompt_tokens, stats.completion_tokens), (11, 4), "tokens come from the answer");
        assert_eq!(stats.models, ["owner/model", "owner/embed", "owner/rerank", "owner/vad.onnx", "owner/asr", "owner/seg.onnx", "owner/spk.onnx"]);

        // Service calls work and change nothing.
        engine.health();
        engine.status();
        engine.models().unwrap();
        engine.set_budget(EngineBudget::default());
        let after = q.get(id).unwrap().unwrap().stats;
        assert_eq!(after, stats);
    }

    #[test]
    fn no_usage_means_no_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = task(&q, "model-step", None);
        let fake = FakeEngine::new();
        fake.chat_usage(None);
        let scope = Scope::new(dir.path(), id, Arc::new(fake), None);
        let cancel = AtomicBool::new(false);
        scope.engine().chat(&ChatRequest { model: "owner/model".into(), messages: vec![ChatMessage::user("hi")], ..ChatRequest::default() }, &cancel, &mut |_| true).unwrap();
        let stats = q.get(id).unwrap().unwrap().stats;
        assert_eq!((stats.model_calls, stats.prompt_tokens, stats.completion_tokens), (1, 0, 0));
    }

    #[test]
    fn an_inline_child_belongs_to_the_task_and_the_recording() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let parent = task(&q, "answer", Some("rec-7"));
        let scope = Scope::new(dir.path(), parent, Arc::new(FakeEngine::new()), Some("rec-7".into()));
        let child = scope.inline_child("tool", "search", &json!({"query": "x"})).unwrap();
        let t = q.get(child.task()).unwrap().unwrap();
        assert!(t.inline);
        assert_eq!(t.parent, Some(parent));
        assert_eq!(t.scope.as_deref(), Some("rec-7"));
        assert_eq!(t.kind, "tool");
        assert_eq!(t.status, mediaa_core::queue::TaskStatus::Running);
    }
}
