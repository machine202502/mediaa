//! The `transcribe` queue task: the sound of a file → voice activity → speech recognition,
//! phrase by phrase into the file's transcript (`mediaa_core::transcript`), checkpointing the
//! time recognised up to, then the dialog's replies without speakers. A stopped or crashed task
//! continues from its last phrase. Speakers are a task of their own (`diarize`), the index and
//! the topics too: the pipeline links them by dependencies.

use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use mediaa_core::agent::AgentSettings;
use mediaa_core::transcript::{self, Transcript};
use mediaa_queue::Task;
use mediaa_engine::Engine;
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

/// The engine's model specs of a task and the token to download the model files.
struct Specs {
    recognizer: String,
    vad: String,
    hf_token: Option<String>,
}

impl Specs {
    fn get(settings: &AgentSettings) -> anyhow::Result<Specs> {
        let source = settings.models.speech_recognition.as_ref().ok_or_else(|| anyhow::anyhow!("the speech recognition model is not chosen"))?;
        Ok(Specs {
            recognizer: crate::models::local_spec(source)?,
            vad: VAD_MODEL.to_string(),
            hf_token: crate::assistant::hf_token(settings),
        })
    }
}

pub struct Transcribe {
    ai_dir: PathBuf,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Transcribe {
    pub fn new(ai_dir: &std::path::Path, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Transcribe {
        Transcribe { ai_dir: ai_dir.to_path_buf(), settings }
    }

    /// One phrase from `cursor.t`. `None` when the recording has no more speech.
    fn one_phrase(
        &self,
        engine: &dyn Engine,
        input: &Input,
        specs: &Specs,
        cursor: &Cursor,
        transcript: &mut Transcript,
        cancel: &Arc<AtomicBool>,
    ) -> anyhow::Result<Option<f64>> {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let hf_token = specs.hf_token.as_deref();
        let mut session = engine.vad_open(&specs.vad, cursor.t, hf_token)?;
        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let (path, start, decode_stop) = (input.path.clone(), cursor.t, Arc::clone(&stop));
        let (watch_stop, watch_cancel) = (Arc::clone(&stop), Arc::clone(cancel));
        let watcher = std::thread::Builder::new().name("transcribe-cancel".into()).spawn(move || {
            while !watch_stop.load(Ordering::Relaxed) {
                if watch_cancel.load(Ordering::Relaxed) {
                    watch_stop.store(true, Ordering::Relaxed);
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        })?;
        let decoder = std::thread::Builder::new().name("transcribe-decode".into()).spawn(move || {
            // The decoding runs on this thread; its progress is the chunk's pulse too.
            let pulse = mediaa_core::pulse::current();
            mediaa_media::preview::decode_mono(&path, start, SAMPLE_RATE, decode_stop, move |samples| {
                if let Some(pulse) = &pulse {
                    pulse.beat();
                }
                tx.send(samples.to_vec()).is_ok()
            })
        })?;
        let mut found: Option<f64> = None;
        let mut failed: Option<anyhow::Error> = None;
        'decode: for chunk in rx.iter() {
            mediaa_core::pulse::beat();
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            for speech in session.push(&chunk) {
                match engine.recognize(&specs.recognizer, speech.start, &speech.samples, hf_token) {
                    Ok(phrase) => {
                        // A segment that does not move past the cursor is behind us (a new
                        // session can report it again). Skipping it is what lets the next
                        // chunk reach the end of the file.
                        if phrase.end <= cursor.t {
                            continue;
                        }
                        let phrases = if phrase.words.is_empty() { vec![] } else { vec![phrase.clone()] };
                        if let Err(error) = transcript.put_phrases(phrase.start, &phrases) {
                            failed = Some(error);
                            break 'decode;
                        }
                        found = Some(phrase.end);
                        break 'decode;
                    }
                    Err(error) => {
                        failed = Some(error.into());
                        break 'decode;
                    }
                }
            }
        }
        if found.is_none() && failed.is_none() && !cancel.load(Ordering::Relaxed) {
            for speech in session.finish() {
                let phrase = engine.recognize(&specs.recognizer, speech.start, &speech.samples, hf_token)?;
                if phrase.end <= cursor.t {
                    continue;
                }
                let phrases = if phrase.words.is_empty() { vec![] } else { vec![phrase.clone()] };
                transcript.put_phrases(phrase.start, &phrases)?;
                found = Some(phrase.end);
                break;
            }
        }
        stop.store(true, Ordering::Relaxed);
        drop(rx);
        decoder.join().map_err(|_| anyhow::anyhow!("the audio decoder crashed"))??;
        let _ = watcher.join();
        if let Some(error) = failed {
            return Err(error);
        }
        if cancel.load(Ordering::Relaxed) && found.is_none() {
            return Ok(None);
        }
        Ok(found)
    }
}

impl Stage for Transcribe {
    fn handles(&self, kind: &str) -> bool {
        kind == KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
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
        let scope = Scope::attach(host.store.clone(), task.id, host.engine.clone(), task.scope.clone());
        let engine = scope.engine();
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let Some(end) = self.one_phrase(engine.as_ref(), &input, &specs, &cursor, &mut transcript, cancel)? else {
            if cancel.load(Ordering::Relaxed) {
                return Ok(Work::Stopped);
            }
            let phrases = transcript.phrases()?;
            log::info!("{}: {} phrases", input.path.display(), phrases.len());
            let replies = transcript::replies(&phrases, &[]);
            transcript.set_replies(&replies)?;
            transcript.remove_meta("diarized")?;
            return Ok(Work::Done(json!({ "file_id": id, "replies": replies.len() })));
        };
        cursor.t = end;
        let progress = if duration > 0.0 { (end / duration).min(1.0) as f32 } else { 0.0 };
        Ok(Work::More { cursor: serde_json::to_value(&cursor)?, partial: Value::Null, progress })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{AgentSettings, AnalysisOptions, Budget};
    use mediaa_queue::Status as TaskStatus;

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
        let defaults = AgentSettings::default();
        let options = AnalysisOptions { diarize: true, speakers: None, index: true, topics: false, details: false };
        let sched = crate::run::open_scheduler(&ai, &defaults.budget).unwrap();
        for t in sched.store().list().unwrap() {
            sched.remove(t.id).unwrap();
        }
        let id = crate::pipeline::start(&sched, &ai, &media, options).unwrap();
        let engine = crate::live::engine();
        let slots = crate::slots::Slots::new(Budget::default());
        let settings: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(AgentSettings::default);
        crate::run::register_stages(&sched, &ai, engine.clone(), slots.clone(), settings, Arc::new(crate::run::Streams::default()));
        crate::run::drain(&sched, std::time::Duration::from_secs(600));
        let task = sched.store().get(id).unwrap().unwrap();
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
