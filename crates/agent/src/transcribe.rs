//! The `transcribe` queue task: the sound of a file → voice activity → speech recognition,
//! phrase by phrase into the file's transcript (`mediaa_core::transcript`), checkpointing the
//! time recognised up to, then the dialog's replies without speakers. A stopped or crashed task
//! continues from its last phrase. Speakers are a task of their own (`diarize`), the index and
//! the topics too: the pipeline links them by dependencies.

use crate::slots::Slots;
use crate::worker::{Checkpoints, TaskHandler};
use mediaa_core::agent::{AgentSettings, WorkKind};
use mediaa_core::queue::Task;
use mediaa_core::transcript::{self, Transcript};
use mediaa_engine::{Engine, SpeechSegment};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const KIND: &str = "transcribe";

/// The engine's sample rate of speech models: 16 kHz mono PCM f32.
pub const SAMPLE_RATE: u32 = 16_000;

/// Voice activity detection (engine model spec).
pub const VAD_MODEL: &str = "csukuangfj/vad/silero_vad.onnx";

/// What to transcribe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
}

/// Where a task stopped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Cursor {
    /// Seconds of the recording recognised.
    t: f64,
}

/// The engine's model specs of a task.
struct Specs {
    recognizer: String,
    vad: String,
}

impl Specs {
    fn get(settings: &AgentSettings) -> anyhow::Result<Specs> {
        let source = settings.models.speech_recognition.as_ref().ok_or_else(|| anyhow::anyhow!("the speech recognition model is not chosen"))?;
        Ok(Specs { recognizer: crate::models::local_spec(source)?, vad: VAD_MODEL.to_string() })
    }
}

pub struct Transcribe {
    ai_dir: PathBuf,
    slots: Slots,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Transcribe {
    pub fn new(ai_dir: &std::path::Path, slots: Slots, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Transcribe {
        Transcribe { ai_dir: ai_dir.to_path_buf(), slots, settings }
    }

    /// Recognises from `cursor.t` to the end, a checkpoint after every phrase.
    #[allow(clippy::too_many_arguments)]
    fn recognize(
        &self,
        engine: &dyn Engine,
        input: &Input,
        specs: &Specs,
        duration: f64,
        transcript: &mut Transcript,
        cursor: &mut Cursor,
        cp: &mut Checkpoints,
        hf_token: Option<&str>,
    ) -> anyhow::Result<()> {
        let mut session = engine.vad_open(&specs.vad, cursor.t, hf_token)?;

        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let (path, start, s) = (input.path.clone(), cursor.t, stop.clone());
        let decoder = std::thread::Builder::new()
            .name("transcribe-decode".into())
            .spawn(move || mediaa_media::preview::decode_mono(&path, start, SAMPLE_RATE, s, move |samples| tx.send(samples.to_vec()).is_ok()))?;

        let mut step = |speech: SpeechSegment, cursor: &mut Cursor, cp: &mut Checkpoints| -> anyhow::Result<()> {
            let phrase = engine.recognize(&specs.recognizer, speech.start, &speech.samples, hf_token)?;
            let phrases = if phrase.words.is_empty() { vec![] } else { vec![phrase.clone()] };
            transcript.put_phrases(phrase.start, &phrases)?;
            cursor.t = phrase.end;
            let progress = if duration > 0.0 { (phrase.end / duration).min(1.0) as f32 } else { 0.0 };
            cp.save(&serde_json::to_value(&*cursor)?, &Value::Null, progress)
        };
        let mut result = Ok(());
        'decode: for chunk in rx.iter() {
            for speech in session.push(&chunk) {
                if let Err(e) = step(speech, cursor, cp) {
                    result = Err(e);
                    break 'decode;
                }
            }
        }
        if result.is_ok() {
            for speech in session.finish() {
                result = step(speech, cursor, cp);
                if result.is_err() {
                    break;
                }
            }
        }
        // Stop the decoder when recognition stopped early (it is blocked on the full channel).
        stop.store(result.is_err(), Ordering::Relaxed);
        drop(rx);
        let decoded = decoder.join().map_err(|_| anyhow::anyhow!("the audio decoder crashed"))?;
        result?;
        decoded
    }
}

impl TaskHandler for Transcribe {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let fresh = task.cursor.is_null();
        let mut cursor: Cursor = serde_json::from_value(task.cursor.clone()).unwrap_or_default();
        let id = transcript::file_id(&input.path)?;
        let mut transcript = Transcript::open(&self.ai_dir, &id)?;
        if fresh {
            transcript.clear()?;
        }
        transcript.set_meta("path", &input.path.to_string_lossy())?;
        let duration = mediaa_media::info::probe(&input.path)?.duration_secs().unwrap_or(0.0);
        let settings = (self.settings)();
        let specs = Specs::get(&settings)?;
        let hf_token = crate::assistant::hf_token(&settings);
        let engine = cp.engine();

        // Transcriptions at once per the budget.
        let _lease = self.slots.request(WorkKind::Transcription, false).wait(cp.stopping())?;
        self.recognize(engine.as_ref(), &input, &specs, duration, &mut transcript, &mut cursor, cp, hf_token.as_deref())?;
        let phrases = transcript.phrases()?;
        log::info!("{}: {} phrases", input.path.display(), phrases.len());
        let replies = transcript::replies(&phrases, &[]);
        transcript.set_replies(&replies)?;
        transcript.remove_meta("diarized")?;
        Ok(json!({ "file_id": id, "replies": replies.len() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{AgentSettings, AnalysisOptions, Budget};
    use mediaa_core::queue::{Queue, TaskStatus};

    /// A real transcription through the queue with models in `MEDIAA_TEST_MODELS` (the app's
    /// folder by default): GigaAM + VAD on a two-speaker sample, diarized. The engine downloads
    /// the models on first use.
    #[test]
    #[ignore]
    fn transcribes_through_the_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = std::env::var_os("MEDIAA_TEST_AI").map_or(tmp.path().to_path_buf(), PathBuf::from);
        let wav = ai.join("example.wav");
        if !wav.is_file() {
            let bytes = reqwest::blocking::get("https://huggingface.co/csukuangfj/tmp-files/resolve/main/GigaAM/example.wav").unwrap().bytes().unwrap();
            std::fs::write(&wav, &bytes).unwrap();
        }
        let library = tempfile::tempdir().unwrap();
        let media = library.path().join("talk.wav");
        std::fs::copy(&wav, &media).unwrap();
        // Models (shared between test runs), the queue and the transcript live in `ai`.
        let q = Queue::open(&ai).unwrap();
        for t in q.list().unwrap() {
            q.remove(t.id).unwrap();
        }
        let defaults = AgentSettings::default();
        let options = AnalysisOptions { diarize: true, speakers: None, index: true, topics: false, details: false };
        let id = crate::pipeline::start(&ai, &media, options).unwrap();

        let engine = crate::live::engine();
        let slots = Slots::new(Budget::default());
        let settings: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(AgentSettings::default);
        let handlers: Vec<Box<dyn TaskHandler>> = vec![
            Box::new(Transcribe::new(&ai, slots.clone(), settings.clone())),
            Box::new(crate::diarize::Diarize::new(&ai, slots.clone(), settings.clone())),
            Box::new(crate::index::Index::new(&ai, slots.clone(), settings)),
        ];
        let stop = AtomicBool::new(false);
        while crate::worker::run_next(&q, &engine, &handlers, &stop).unwrap() {}
        let task = q.get(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        let t = Transcript::open(&ai, &transcript::file_id(&media).unwrap()).unwrap();
        let dialog = t.dialog().unwrap();
        eprintln!("{dialog:#?}");
        assert!(!dialog.is_empty());
        assert!(dialog.iter().all(|r| r.speaker.is_some()), "diarized");
        assert!(dialog.iter().any(|r| r.text().to_lowercase().contains("лукоморья")));
        assert!(t.chunks().unwrap().iter().all(|(_, _, embedded)| *embedded));
        let (hits, trace) = crate::search::search(engine.as_ref(), &slots, &defaults, &ai, &transcript::file_id(&media).unwrap(), "где растёт дерево у моря?", 3, true).unwrap();
        assert!(trace.by_meaning && trace.reranked && trace.not_indexed == 0, "{trace:?}");
        assert!(hits[0].chunk.text.to_lowercase().contains("дуб"), "{hits:?}");
    }
}
