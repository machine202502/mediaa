use crate::engine::{
    ChatAnswer, ChatRequest, DiarizeRequest, EmbedKind, EmbeddingsRequest, Engine, EngineError,
    ModelCheck, ModelInfo, RerankRequest, RerankResult, SpeechBody, TranscribeRequest, Usage,
    VoiceRequest,
};
use mediaa_core::agent::ModelRole;
use mediaa_core::transcript::{Phrase, Turn, Word};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct RecordedChat {
    pub model: String,
    pub messages: usize,
    pub texts: Vec<String>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub json_schema: Option<String>,
    pub thinking: Option<String>,
    pub tools: usize,
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
    embeddings: AtomicU64,
    phrases: Mutex<Vec<Phrase>>,
    turns: Mutex<Vec<Turn>>,
    voices: Mutex<Vec<Vec<f32>>>,
    models: Mutex<Vec<ModelInfo>>,
    checks: Mutex<Vec<(String, String)>>,
    transcribes: Mutex<Vec<String>>,
    diarizes: Mutex<Vec<String>>,
    voice_calls: Mutex<Vec<usize>>,
    fail_next: Mutex<Option<String>>,
}

impl FakeEngine {
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

    pub fn script_phrases(&self, phrases: Vec<Phrase>) {
        *self.inner.phrases.lock().unwrap() = phrases;
    }

    pub fn script_turns(&self, turns: Vec<Turn>) {
        *self.inner.turns.lock().unwrap() = turns;
    }

    pub fn script_voices(&self, voices: Vec<Vec<f32>>) {
        *self.inner.voices.lock().unwrap() = voices;
    }

    pub fn set_models(&self, ids: &[&str]) {
        *self.inner.models.lock().unwrap() = ids
            .iter()
            .map(|id| ModelInfo {
                id: (*id).to_string(),
                name: (*id).to_string(),
            })
            .collect();
    }

    pub fn fail_next(&self, message: &str) {
        *self.inner.fail_next.lock().unwrap() = Some(message.to_string());
    }

    pub fn transcribes(&self) -> Vec<String> {
        self.inner.transcribes.lock().unwrap().clone()
    }

    pub fn diarizes(&self) -> Vec<String> {
        self.inner.diarizes.lock().unwrap().clone()
    }

    pub fn voice_calls(&self) -> Vec<usize> {
        self.inner.voice_calls.lock().unwrap().clone()
    }

    pub fn checks(&self) -> Vec<(String, String)> {
        self.inner.checks.lock().unwrap().clone()
    }
}

impl Engine for FakeEngine {
    fn chat(
        &self,
        request: &ChatRequest,
        cancel: &AtomicBool,
        on_text: &mut dyn FnMut(&str) -> bool,
    ) -> Result<ChatAnswer, EngineError> {
        if let Some(message) = self.inner.fail_next.lock().unwrap().take() {
            return Err(EngineError::internal(message));
        }
        self.inner.chats.lock().unwrap().push(RecordedChat {
            model: request.source.model.trim().to_string(),
            messages: request.messages.len(),
            texts: request
                .messages
                .iter()
                .map(|message| message.content.clone())
                .collect(),
            max_tokens: request.max_tokens,
            temperature: request.temperature,
            json_schema: request
                .json_schema
                .as_ref()
                .map(|schema| schema.to_string()),
            thinking: request.thinking.map(|thinking| {
                match thinking.depth {
                    mediaa_core::agent::ReasoningDepth::Low => "low",
                    mediaa_core::agent::ReasoningDepth::Medium => "medium",
                    mediaa_core::agent::ReasoningDepth::High => "high",
                    mediaa_core::agent::ReasoningDepth::XHigh => "xhigh",
                }
                .to_string()
            }),
            tools: request.tools.len(),
        });
        if request.source.model.trim() == "slow/model" {
            loop {
                if cancel.load(Ordering::Relaxed) {
                    return Err(EngineError::cancelled("cancelled"));
                }
                if !on_text("x") {
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
        Ok(ChatAnswer {
            content,
            usage,
            ..ChatAnswer::default()
        })
    }

    fn embeddings(
        &self,
        request: &EmbeddingsRequest,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        self.inner.kinds.lock().unwrap().push(request.kind);
        self.inner.embeddings.fetch_add(1, Ordering::Relaxed);
        let scripted = self.inner.vectors.lock().unwrap().clone();
        if let Some(vectors) = scripted {
            if vectors.len() == request.input.len() {
                return Ok(vectors);
            }
            if let Some(first) = vectors.first() {
                return Ok(vec![first.clone(); request.input.len()]);
            }
        }
        Ok(request
            .input
            .iter()
            .map(|text| vec![text.len() as f32, 1.0])
            .collect())
    }

    fn rerank(
        &self,
        request: &RerankRequest,
        _cancel: &AtomicBool,
    ) -> Result<Vec<RerankResult>, EngineError> {
        let scores = self.inner.scores.lock().unwrap().clone();
        Ok(request
            .documents
            .iter()
            .enumerate()
            .map(|(index, _)| RerankResult {
                index,
                relevance_score: scores
                    .as_ref()
                    .and_then(|scores| scores.get(index).copied())
                    .unwrap_or(1.0 / (index as f32 + 1.0)),
            })
            .collect())
    }

    fn transcribe(
        &self,
        request: &TranscribeRequest,
        _cancel: &AtomicBool,
    ) -> Result<SpeechBody, EngineError> {
        if let Some(message) = self.inner.fail_next.lock().unwrap().take() {
            return Err(EngineError::internal(message));
        }
        self.inner
            .transcribes
            .lock()
            .unwrap()
            .push(format!("{}:{}", request.from, request.to));
        let phrases = self.inner.phrases.lock().unwrap().clone();
        let phrases = if phrases.is_empty() {
            vec![Phrase {
                start: request.from,
                end: request.to,
                words: vec![Word {
                    text: "word".into(),
                    start: request.from,
                    end: request.to.min(request.from + 0.4),
                }],
            }]
        } else {
            phrases
                .into_iter()
                .filter(|phrase| phrase.start >= request.from && phrase.start < request.to)
                .collect()
        };
        let turns = if request.speakers {
            self.inner.turns.lock().unwrap().clone()
        } else {
            Vec::new()
        };
        let voices = if request.voiceprint {
            self.inner.voices.lock().unwrap().clone()
        } else {
            Vec::new()
        };
        Ok(SpeechBody {
            phrases,
            turns,
            voices,
        })
    }

    fn diarize(
        &self,
        request: &DiarizeRequest,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Turn>, EngineError> {
        self.inner
            .diarizes
            .lock()
            .unwrap()
            .push(request.path.display().to_string());
        Ok(self.inner.turns.lock().unwrap().clone())
    }

    fn voice_embeddings(
        &self,
        request: &VoiceRequest,
        _cancel: &AtomicBool,
    ) -> Result<Vec<Vec<f32>>, EngineError> {
        self.inner
            .voice_calls
            .lock()
            .unwrap()
            .push(request.clips.len());
        let voices = self.inner.voices.lock().unwrap().clone();
        if voices.len() != request.clips.len() {
            return Err(EngineError::bad_request(format!(
                "the command returned {} vectors for {} files",
                voices.len(),
                request.clips.len()
            )));
        }
        Ok(voices)
    }

    fn models(&self, _connection: &str) -> Result<Vec<ModelInfo>, EngineError> {
        Ok(self.inner.models.lock().unwrap().clone())
    }

    fn check(&self, source: &ModelSource, role: ModelRole) -> Result<ModelCheck, EngineError> {
        self.inner
            .checks
            .lock()
            .unwrap()
            .push((source.key(), format!("{role:?}")));
        Ok(ModelCheck {
            ok: true,
            detail: source.model.clone(),
        })
    }
}

use mediaa_core::agent::ModelSource;
