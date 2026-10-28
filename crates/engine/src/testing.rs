use crate::engine::{
    inspect_files, role_name, Budget, ChatAnswer, ChatRequest, Diarization, DiarizeRequest,
    EmbedKind, EmbeddingsRequest, Engine, EngineError, Gpu, Health, LoadedModel, ModelCheck,
    ModelFile, ModelFormat, ModelInfo, ModelInspect, Pool, ProgressCallback, RerankRequest,
    RerankResult, SpeechSegment, Status, Thinking, Usage, Used, VadSession, Voice, WaitingModel,
    DEFAULT_CHAT_N_CTX,
};
use crate::speech::{Phrase, Turn, Word};
use mediaa_core::agent::ModelRole;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedChat {
    pub model: String,
    pub messages: usize,
    pub texts: Vec<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub no_repeats: bool,
    pub json_schema: Option<String>,
    pub n_ctx: u32,
    pub device: String,
    pub interactive: bool,
    pub hf_token: Option<String>,
    pub thinking: Option<u32>,
    pub enable_thinking: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedVad {
    pub model: String,
    pub offset: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedRecognize {
    pub model: String,
    pub offset: f64,
    pub samples: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedDiarize {
    pub segmentation: String,
    pub embedding: String,
    pub samples: usize,
    pub speakers: Option<u32>,
    pub merge_threshold: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedInspect {
    pub spec: String,
    pub hf_token: Option<String>,
}

#[derive(Clone, Default)]
pub struct FakeEngine {
    inner: Arc<Inner>,
}

#[derive(Default)]
struct Inner {
    chats: Mutex<Vec<RecordedChat>>,
    deltas: Mutex<Option<Vec<String>>>,
    answers: Mutex<Vec<String>>,
    usage: Mutex<Option<Option<Usage>>>,
    kinds: Mutex<Vec<EmbedKind>>,
    vectors: Mutex<Option<Vec<Vec<f32>>>>,
    scores: Mutex<Option<Vec<f32>>>,
    budget: Mutex<Option<Budget>>,
    cancel_observed: AtomicBool,
    embeddings: AtomicU64,
    vads: Mutex<Vec<RecordedVad>>,
    recognizes: Mutex<Vec<RecordedRecognize>>,
    diarizes: Mutex<Vec<RecordedDiarize>>,
    inspects: Mutex<Vec<RecordedInspect>>,
    pulls: Mutex<Vec<String>>,
    pull_progress: Mutex<Vec<(u64, u64)>>,
    deletes: Mutex<Vec<String>>,
    checks: Mutex<Vec<(String, String)>>,
    wanted: Mutex<Vec<String>>,
    forgotten: Mutex<Vec<String>>,
    hangups: AtomicU32,
    release_hang: AtomicBool,
}

struct Mark;

impl Drop for Mark {
    fn drop(&mut self) {
        mediaa_core::pulse::release();
    }
}

impl FakeEngine {
    fn chat_inner(
        &self,
        request: &ChatRequest,
        thinking: Option<u32>,
        cancel: &AtomicBool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<ChatAnswer, EngineError> {
        request.clone().into_call()?;
        mediaa_core::pulse::using(request.model.trim());
        let _mark = Mark;
        self.inner.chats.lock().unwrap().push(RecordedChat {
            model: request.model.trim().to_string(),
            messages: request.messages.len(),
            texts: request.messages.iter().map(|m| m.content.clone()).collect(),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            no_repeats: request.no_repeats,
            json_schema: request
                .json_schema
                .as_ref()
                .map(|schema| schema.to_string()),
            n_ctx: request.n_ctx.unwrap_or(DEFAULT_CHAT_N_CTX),
            device: device_name(request.device),
            interactive: request
                .priority
                .is_none_or(|priority| priority.interactive()),
            hf_token: request.hf_token.clone(),
            thinking,
            enable_thinking: request.enable_thinking,
        });
        if request.model.trim() == "unsupported/model" {
            return Err(EngineError::model_unsupported("not a chat model"));
        }
        if request.model.trim() == "fails/midstream" {
            on_text("partial");
            return Err(EngineError::internal("the model crashed"));
        }
        if request.model.trim() == "hangs/model"
            && self.inner.hangups.fetch_add(1, Ordering::SeqCst) == 0
        {
            while !self.inner.release_hang.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        if request.model.trim() == "slow/model" {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    self.inner.cancel_observed.store(true, Ordering::SeqCst);
                    return Ok(ChatAnswer::default());
                }
                if !on_text("x") {
                    if cancel.load(Ordering::Relaxed) {
                        self.inner.cancel_observed.store(true, Ordering::SeqCst);
                    }
                    return Ok(ChatAnswer::default());
                }
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        let next = {
            let mut answers = self.inner.answers.lock().unwrap();
            if answers.is_empty() {
                None
            } else {
                Some(answers.remove(0))
            }
        };
        let deltas = next.map(|answer| vec![answer]).unwrap_or_else(|| {
            self.inner
                .deltas
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| vec!["Привет".into(), ", ".into(), "мир".into()])
        });
        let mut content = String::new();
        for delta in &deltas {
            if !on_text(delta) {
                break;
            }
            content.push_str(delta);
        }
        let usage = (*self.inner.usage.lock().unwrap()).unwrap_or(Some(Usage {
            prompt_tokens: 7,
            completion_tokens: 3,
        }));
        Ok(ChatAnswer { content, usage })
    }

    pub fn new() -> FakeEngine {
        FakeEngine::default()
    }

    pub fn chat_deltas(&self, deltas: &[&str]) {
        *self.inner.deltas.lock().unwrap() =
            Some(deltas.iter().map(|d| (*d).to_string()).collect());
    }

    pub fn chat_answers(&self, answers: &[&str]) {
        *self.inner.answers.lock().unwrap() = answers.iter().map(|a| (*a).to_string()).collect();
    }

    pub fn chat_usage(&self, usage: Option<Usage>) {
        *self.inner.usage.lock().unwrap() = Some(usage);
    }

    pub fn embed_vectors(&self, vectors: Vec<Vec<f32>>) {
        *self.inner.vectors.lock().unwrap() = Some(vectors);
    }

    pub fn rerank_scores(&self, scores: Vec<f32>) {
        *self.inner.scores.lock().unwrap() = Some(scores);
    }

    pub fn chats(&self) -> Vec<RecordedChat> {
        self.inner.chats.lock().unwrap().clone()
    }

    pub fn kinds(&self) -> Vec<EmbedKind> {
        self.inner.kinds.lock().unwrap().clone()
    }

    pub fn embeddings_count(&self) -> u64 {
        self.inner.embeddings.load(Ordering::Relaxed)
    }

    pub fn budget(&self) -> Option<Budget> {
        *self.inner.budget.lock().unwrap()
    }

    pub fn cancel_observed(&self) -> bool {
        self.inner.cancel_observed.load(Ordering::SeqCst)
    }

    pub fn vads(&self) -> Vec<RecordedVad> {
        self.inner.vads.lock().unwrap().clone()
    }

    pub fn recognizes(&self) -> Vec<RecordedRecognize> {
        self.inner.recognizes.lock().unwrap().clone()
    }

    pub fn diarizes(&self) -> Vec<RecordedDiarize> {
        self.inner.diarizes.lock().unwrap().clone()
    }

    pub fn inspects(&self) -> Vec<RecordedInspect> {
        self.inner.inspects.lock().unwrap().clone()
    }

    pub fn pulls(&self) -> Vec<String> {
        self.inner.pulls.lock().unwrap().clone()
    }

    pub fn pull_progress(&self) -> Vec<(u64, u64)> {
        self.inner.pull_progress.lock().unwrap().clone()
    }

    pub fn deletes(&self) -> Vec<String> {
        self.inner.deletes.lock().unwrap().clone()
    }

    pub fn checks(&self) -> Vec<(String, String)> {
        self.inner.checks.lock().unwrap().clone()
    }

    pub fn wanted(&self) -> Vec<String> {
        self.inner.wanted.lock().unwrap().clone()
    }

    pub fn forgotten(&self) -> Vec<String> {
        self.inner.forgotten.lock().unwrap().clone()
    }

    pub fn release_hang(&self) {
        self.inner.release_hang.store(true, Ordering::SeqCst);
    }
}

struct FakeVad;

impl VadSession for FakeVad {
    fn push(&mut self, samples: &[f32]) -> Vec<SpeechSegment> {
        if samples.is_empty() {
            return Vec::new();
        }
        vec![SpeechSegment {
            start: 2.0,
            samples: samples.to_vec(),
        }]
    }

    fn finish(&mut self) -> Vec<SpeechSegment> {
        vec![SpeechSegment {
            start: 5.0,
            samples: vec![0.5],
        }]
    }
}

fn device_name(device: Option<mediaa_core::agent::Device>) -> String {
    match device.unwrap_or_default() {
        mediaa_core::agent::Device::Auto => "auto",
        mediaa_core::agent::Device::Gpu => "gpu",
        mediaa_core::agent::Device::Cpu => "cpu",
    }
    .into()
}

impl Engine for FakeEngine {
    fn health(&self) -> Health {
        Health {
            name: "MediaA".into(),
            version: "test".into(),
            gpus: vec![Gpu {
                name: "Test GPU".into(),
                backend: "Vulkan".into(),
                memory_total: 8 << 30,
                memory_free: 4 << 30,
            }],
            models_dir: "C:/models".into(),
        }
    }

    fn status(&self) -> Status {
        Status {
            loaded: vec![LoadedModel {
                model: "C:/models/m.gguf".into(),
                pool: Pool::Gpu,
                bytes: 1 << 30,
                users: 1,
                idle: Duration::ZERO,
                unloads_in: None,
            }],
            waiting: Vec::<WaitingModel>::new(),
            budget: Budget {
                gpu_bytes: 8 << 30,
                ram_bytes: 8 << 30,
                keep_alive: Duration::from_secs(5 * 60),
            },
            used: Used {
                gpu_bytes: 1 << 30,
                ram_bytes: 0,
            },
        }
    }

    fn set_budget(&self, budget: Budget) {
        *self.inner.budget.lock().unwrap() = Some(budget);
    }

    fn set_wanted(&self, specs: &[String]) {
        *self.inner.wanted.lock().unwrap() = specs.to_vec();
    }

    fn forget_loaded(&self, model: &str) {
        self.inner.forgotten.lock().unwrap().push(model.to_string());
    }

    fn chat(
        &self,
        request: &ChatRequest,
        cancel: &AtomicBool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<ChatAnswer, EngineError> {
        self.chat_inner(request, None, cancel, on_text)
    }

    fn chat_thinking(
        &self,
        request: &ChatRequest,
        thinking: Thinking,
        cancel: &AtomicBool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<ChatAnswer, EngineError> {
        self.chat_inner(request, Some(thinking.budget), cancel, on_text)
    }

    fn embeddings(
        &self,
        request: &EmbeddingsRequest,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        request.clone().into_call()?;
        mediaa_core::pulse::using(request.model.trim());
        let _mark = Mark;
        self.inner.kinds.lock().unwrap().push(request.kind);
        self.inner
            .embeddings
            .fetch_add(request.input.len() as u64, Ordering::Relaxed);
        if let Some(vectors) = self.inner.vectors.lock().unwrap().clone() {
            return Ok(vectors);
        }
        Ok(request
            .input
            .iter()
            .enumerate()
            .map(|(i, _)| vec![i as f32, i as f32 + 0.5])
            .collect())
    }

    fn rerank(
        &self,
        request: &RerankRequest,
        _cancel: &AtomicBool,
    ) -> Result<Vec<RerankResult>, EngineError> {
        request.clone().into_call()?;
        let scores = self
            .inner
            .scores
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(|| {
                request
                    .documents
                    .iter()
                    .enumerate()
                    .map(|(i, _)| [0.2, 0.9, 0.5][i % 3])
                    .collect()
            });
        let mut ranked: Vec<RerankResult> = scores
            .into_iter()
            .enumerate()
            .map(|(index, relevance_score)| RerankResult {
                index,
                relevance_score,
            })
            .collect();
        ranked.sort_by(|a, b| {
            b.relevance_score
                .partial_cmp(&a.relevance_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(ranked)
    }

    fn vad_open(
        &self,
        model: &str,
        offset: f64,
        _hf_token: Option<&str>,
    ) -> Result<Box<dyn VadSession>, EngineError> {
        if model.trim() == "unsupported/model" {
            return Err(EngineError::model_unsupported(
                "the voice activity model does not load",
            ));
        }
        self.inner.vads.lock().unwrap().push(RecordedVad {
            model: model.trim().to_string(),
            offset,
        });
        Ok(Box::new(FakeVad))
    }

    fn recognize(
        &self,
        model: &str,
        offset: f64,
        samples: &[f32],
        _hf_token: Option<&str>,
    ) -> Result<Phrase, EngineError> {
        if model.trim() == "unsupported/model" {
            return Err(EngineError::model_unsupported(
                "not a speech recognition model",
            ));
        }
        self.inner
            .recognizes
            .lock()
            .unwrap()
            .push(RecordedRecognize {
                model: model.trim().to_string(),
                offset,
                samples: samples.len(),
            });
        let end = offset + samples.len() as f64 / 16_000.0;
        Ok(Phrase {
            start: offset,
            end,
            words: vec![Word {
                text: "привет".into(),
                start: offset,
                end: offset + 0.5,
            }],
        })
    }

    fn diarize(
        &self,
        request: &DiarizeRequest,
        samples: &[f32],
    ) -> Result<Diarization, EngineError> {
        self.inner.diarizes.lock().unwrap().push(RecordedDiarize {
            segmentation: request.segmentation.clone(),
            embedding: request.embedding.clone(),
            samples: samples.len(),
            speakers: request.speakers,
            merge_threshold: request.merge_threshold,
        });
        if request.embedding.trim() == "fails/model" {
            return Err(EngineError::internal("diarization failed"));
        }
        let turns = vec![
            Turn {
                start: 0.0,
                end: 1.5,
                speaker: 0,
            },
            Turn {
                start: 1.5,
                end: 3.0,
                speaker: 1,
            },
        ];
        let voices = vec![
            Voice {
                embedding: vec![1.0, 0.0],
                seconds: 1.5,
            },
            Voice {
                embedding: vec![0.0, 1.0],
                seconds: 1.5,
            },
        ];
        Ok(Diarization { turns, voices })
    }

    fn models(&self) -> Result<Vec<ModelInfo>, EngineError> {
        Ok(vec![ModelInfo {
            spec: "a/b".into(),
            path: "C:/models/a--b".into(),
            bytes: 100,
            format: ModelFormat::Gguf,
            files: vec![ModelFile {
                name: "m-Q4_K_M.gguf".into(),
                bytes: 100,
            }],
        }])
    }

    fn inspect(&self, spec: &str, hf_token: Option<&str>) -> Result<ModelInspect, EngineError> {
        self.inner.inspects.lock().unwrap().push(RecordedInspect {
            spec: spec.into(),
            hf_token: hf_token.map(str::to_string),
        });
        match spec {
            "missing/model" => Err(EngineError::not_found(
                "missing/model not found on Hugging Face",
            )),
            "plain/model" => Ok(inspect_files(
                spec,
                vec![ModelFile {
                    name: "model.safetensors".into(),
                    bytes: 900,
                }],
                false,
            )),
            _ if spec.to_ascii_lowercase().ends_with(".onnx") => {
                let name = spec.rsplit(['/', '\\']).next().unwrap_or(spec).to_string();
                Ok(inspect_files(
                    spec,
                    vec![ModelFile { name, bytes: 900 }],
                    false,
                ))
            }
            _ => Ok(inspect_files(
                spec,
                vec![
                    ModelFile {
                        name: "README.md".into(),
                        bytes: 10,
                    },
                    ModelFile {
                        name: "m-Q4_K_M.gguf".into(),
                        bytes: 500,
                    },
                    ModelFile {
                        name: "m-Q8_0.gguf".into(),
                        bytes: 900,
                    },
                ],
                true,
            )),
        }
    }

    fn pull(
        &self,
        spec: &str,
        _hf_token: Option<&str>,
        on_progress: ProgressCallback,
    ) -> Result<PathBuf, EngineError> {
        self.inner.pulls.lock().unwrap().push(spec.into());
        for (done, total) in [(0, 100), (50, 100)] {
            self.inner.pull_progress.lock().unwrap().push((done, total));
            on_progress(done, total);
        }
        if spec == "fails/model" {
            return Err(EngineError::internal("download failed"));
        }
        Ok(PathBuf::from("C:/models/a--b"))
    }

    fn delete_model(&self, spec: &str) -> Result<(), EngineError> {
        if spec == "outside/model" {
            return Err(EngineError::bad_request(
                "refusing to delete: it is outside the models folder",
            ));
        }
        self.inner.deletes.lock().unwrap().push(spec.into());
        Ok(())
    }

    fn check_model(
        &self,
        spec: &str,
        role: ModelRole,
        _hf_token: Option<&str>,
    ) -> Result<ModelCheck, EngineError> {
        self.inner
            .checks
            .lock()
            .unwrap()
            .push((spec.into(), role_name(role).into()));
        if spec == "bad/model" {
            return Err(EngineError::model_unsupported("no GGUF model inside"));
        }
        Ok(ModelCheck {
            ok: true,
            bytes: 123,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{ChatMessage, Priority};

    #[test]
    fn the_fake_answers_and_records() {
        let fake = FakeEngine::new();
        let request = ChatRequest {
            model: "a/b".into(),
            messages: vec![ChatMessage::user("hi")],
            priority: Some(Priority::Background),
            ..ChatRequest::default()
        };
        let mut seen = Vec::new();
        let answer = fake
            .chat(&request, &AtomicBool::new(false), &mut |text| {
                seen.push(text.to_string());
                true
            })
            .unwrap();
        assert_eq!(seen, ["Привет", ", ", "мир"]);
        assert_eq!(answer.content, "Привет, мир");
        let recorded = fake.chats();
        assert_eq!(
            (
                recorded[0].model.as_str(),
                recorded[0].messages,
                recorded[0].interactive
            ),
            ("a/b", 1, false)
        );
    }

    #[test]
    fn the_fake_can_fail_on_purpose() {
        let fake = FakeEngine::new();
        let request = ChatRequest {
            model: "invalid".into(),
            ..ChatRequest::default()
        };
        assert_eq!(
            fake.chat(&request, &AtomicBool::new(false), &mut |_| true)
                .unwrap_err()
                .kind,
            "bad_request"
        );
        let unsupported = ChatRequest {
            model: "unsupported/model".into(),
            messages: vec![ChatMessage::user("hi")],
            ..ChatRequest::default()
        };
        assert_eq!(
            fake.chat(&unsupported, &AtomicBool::new(false), &mut |_| true)
                .unwrap_err()
                .kind,
            "model_unsupported"
        );
        assert_eq!(
            fake.inspect("missing/model", None).unwrap_err().kind,
            "not_found"
        );
        assert_eq!(
            fake.pull("fails/model", None, Arc::new(|_, _| {}))
                .unwrap_err()
                .kind,
            "internal"
        );
        assert_eq!(
            fake.delete_model("outside/model").unwrap_err().kind,
            "bad_request"
        );
        assert_eq!(
            fake.check_model("bad/model", ModelRole::Agent, None)
                .unwrap_err()
                .kind,
            "model_unsupported"
        );
    }

    #[test]
    fn the_fake_orders_rerank_scores() {
        let fake = FakeEngine::new();
        let request = RerankRequest {
            model: "a/b".into(),
            query: "q".into(),
            documents: vec!["one".into(), "two".into(), "three".into()],
            ..RerankRequest::default()
        };
        let results = fake.rerank(&request, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            results,
            [
                RerankResult {
                    index: 1,
                    relevance_score: 0.9
                },
                RerankResult {
                    index: 2,
                    relevance_score: 0.5
                },
                RerankResult {
                    index: 0,
                    relevance_score: 0.2
                }
            ]
        );
    }
}
