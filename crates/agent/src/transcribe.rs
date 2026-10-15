//! The `transcribe` queue task: the sound of a file → voice activity → speech recognition,
//! phrase by phrase into the file's transcript (`mediaa_core::transcript`), checkpointing the
//! time recognised up to; then (optionally) diarization and the dialog's replies. A stopped or
//! crashed task continues from its last phrase.

use crate::hub;
use crate::scheduler::{ModelNeed, Scheduler};
use crate::speech::{self, Recognizer, Vad, SAMPLE_RATE};
use crate::worker::{Checkpoints, TaskHandler};
use mediaa_core::agent::{AgentSettings, Device, ModelSource, WorkKind};
use mediaa_core::queue::Task;
use mediaa_core::transcript::{self, Transcript};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const KIND: &str = "transcribe";

/// What to transcribe and with which models (chosen when the task is created).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
    pub recognizer: ModelSource,
    /// Speaker model; `None`: no diarization.
    pub diarizer: Option<ModelSource>,
    /// Number of speakers when known.
    #[serde(default)]
    pub speakers: Option<u32>,
}

/// Where a task stopped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Cursor {
    /// Seconds of the recording recognised.
    t: f64,
    /// Recognition is complete (diarization next).
    recognized: bool,
}

/// Share of the progress bar for recognition when diarization follows.
const RECOGNITION_SHARE: f32 = 0.85;

/// The local file or folder of a model, downloading a Hugging Face one if needed.
pub fn model_path(ai_dir: &Path, source: &ModelSource, settings: &AgentSettings) -> anyhow::Result<PathBuf> {
    match source {
        ModelSource::Local { path } => Ok(path.clone()),
        ModelSource::HuggingFace { repo } => {
            let dir = hub::download(ai_dir, repo, settings.hugging_face(), |_, _| {})?;
            Ok(match hub::split_spec(repo).1 {
                Some(file) => dir.join(file),
                None => dir,
            })
        }
        ModelSource::Provider { .. } => anyhow::bail!("speech models run locally, not at a provider"),
    }
}

fn companion(ai_dir: &Path, spec: &str, settings: &AgentSettings) -> anyhow::Result<PathBuf> {
    model_path(ai_dir, &ModelSource::HuggingFace { repo: spec.into() }, settings)
}

/// Bytes a model takes on disk (a file or a folder's files): its memory need, roughly.
fn size_of(path: &Path) -> u64 {
    if path.is_file() {
        return path.metadata().map_or(0, |m| m.len());
    }
    std::fs::read_dir(path).map_or(0, |d| d.flatten().filter_map(|e| e.metadata().ok()).filter(|m| m.is_file()).map(|m| m.len()).sum())
}

/// The model files of a task, on disk.
struct Models {
    recognizer: PathBuf,
    vad: PathBuf,
    /// Speaker embedding and segmentation models.
    diarizer: Option<(PathBuf, PathBuf)>,
}

impl Models {
    /// Downloads what is missing.
    fn get(ai_dir: &Path, input: &Input, settings: &AgentSettings) -> anyhow::Result<Models> {
        let diarizer = match &input.diarizer {
            Some(d) => Some((model_path(ai_dir, d, settings)?, companion(ai_dir, speech::SEGMENTATION_MODEL, settings)?)),
            None => None,
        };
        Ok(Models { recognizer: model_path(ai_dir, &input.recognizer, settings)?, vad: companion(ai_dir, speech::VAD_MODEL, settings)?, diarizer })
    }

    /// Memory they take loaded: their size and half again for working memory.
    fn memory(&self) -> u64 {
        let files = size_of(&self.recognizer) + size_of(&self.vad) + self.diarizer.as_ref().map_or(0, |(e, s)| size_of(e) + size_of(s));
        files + files / 2
    }
}

pub struct Transcribe {
    ai_dir: PathBuf,
    scheduler: Scheduler,
    /// The current settings (Hugging Face token and proxy for downloads).
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Transcribe {
    pub fn new(ai_dir: &Path, scheduler: Scheduler, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Transcribe {
        Transcribe { ai_dir: ai_dir.to_path_buf(), scheduler, settings }
    }

    /// Recognises from `cursor.t` to the end, a checkpoint after every phrase.
    fn recognize(&self, input: &Input, models: &Models, progress: (f64, f32), transcript: &mut Transcript, cursor: &mut Cursor, cp: &mut Checkpoints) -> anyhow::Result<()> {
        let (duration, share) = progress;
        let (model, vad_model) = (&models.recognizer, &models.vad);
        let recognizer = Recognizer::load(model)?;
        let mut vad = Vad::load(vad_model, cursor.t)?;

        let (tx, rx) = crossbeam_channel::bounded::<Vec<f32>>(16);
        let stop = Arc::new(AtomicBool::new(false));
        let (path, start, s) = (input.path.clone(), cursor.t, stop.clone());
        let decoder = std::thread::Builder::new()
            .name("transcribe-decode".into())
            .spawn(move || mediaa_media::preview::decode_mono(&path, start, SAMPLE_RATE, s, move |samples| tx.send(samples.to_vec()).is_ok()))?;

        let mut step = |speech: speech::Speech, cursor: &mut Cursor, cp: &mut Checkpoints| -> anyhow::Result<()> {
            let phrase = recognizer.recognize(&speech.samples, speech.start)?;
            let phrases = if phrase.words.is_empty() { vec![] } else { vec![phrase.clone()] };
            transcript.put_phrases(phrase.start, &phrases)?;
            cursor.t = phrase.end;
            let progress = if duration > 0.0 { (phrase.end / duration).min(1.0) as f32 * share } else { 0.0 };
            cp.save(&serde_json::to_value(&*cursor)?, &Value::Null, progress)
        };
        let mut result = Ok(());
        'decode: for chunk in rx.iter() {
            for speech in vad.push(&chunk) {
                if let Err(e) = step(speech, cursor, cp) {
                    result = Err(e);
                    break 'decode;
                }
            }
        }
        if result.is_ok() {
            for speech in vad.finish() {
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

    /// Who speaks when, over the whole recording.
    fn diarize(&self, input: &Input, embedding: &Path, segmentation: &Path) -> anyhow::Result<Vec<speech::Turn>> {
        let samples = Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let s = samples.clone();
        mediaa_media::preview::decode_mono(&input.path, 0.0, SAMPLE_RATE, Arc::new(AtomicBool::new(false)), move |chunk| {
            s.lock().expect("samples lock").extend_from_slice(chunk);
            true
        })?;
        let samples = std::mem::take(&mut *samples.lock().expect("samples lock"));
        speech::diarize(segmentation, embedding, &samples, input.speakers)
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
        let share = if input.diarizer.is_some() { RECOGNITION_SHARE } else { 1.0 };
        let models = Models::get(&self.ai_dir, &input, &(self.settings)())?;

        // Transcriptions at once and the memory of their models (on the CPU) per the budget.
        let need = ModelNeed { key: format!("speech:{}", models.recognizer.display()), bytes: models.memory(), device: Device::Cpu };
        let lease = self.scheduler.request(WorkKind::Transcription, Some(need), false).wait(cp.stopping())?;
        if !cursor.recognized {
            self.recognize(&input, &models, (duration, share), &mut transcript, &mut cursor, cp)?;
            cursor.recognized = true;
            cp.save(&serde_json::to_value(&cursor)?, &Value::Null, share)?;
        }
        let phrases = transcript.phrases()?;
        let turns = match &models.diarizer {
            Some((embedding, segmentation)) if !phrases.is_empty() => self.diarize(&input, embedding, segmentation)?,
            _ => Vec::new(),
        };
        drop(lease);
        log::info!("{}: {} phrases, {} speaker turns", input.path.display(), phrases.len(), turns.len());
        let replies = transcript::replies(&phrases, &turns);
        transcript.set_replies(&replies)?;
        transcript.set_meta("diarized", if turns.is_empty() { "0" } else { "1" })?;
        // Searchable next (a task of its own: the indexing budget, resumable).
        crate::index::enqueue(&self.ai_dir, &input.path)?;
        let speakers = replies.iter().filter_map(|r| r.speaker).collect::<std::collections::BTreeSet<_>>().len();
        Ok(json!({ "file_id": id, "replies": replies.len(), "speakers": speakers }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::Budget;
    use mediaa_core::queue::{Queue, TaskStatus};

    /// A real transcription through the queue: GigaAM + VAD on a two-speaker sample, diarized.
    /// Models are taken from `MEDIAA_TEST_AI` (see `speech` tests) or downloaded.
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
        let input = Input { path: media.clone(), recognizer: defaults.models.speech_recognition.clone().unwrap(), diarizer: defaults.models.diarization.clone(), speakers: None };
        let id = q.enqueue(KIND, "talk.wav", &serde_json::to_value(&input).unwrap(), 0).unwrap();

        let runtime = crate::runtime::Runtime::new(&ai, Budget::default());
        let settings: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(AgentSettings::default);
        let handlers: Vec<Box<dyn TaskHandler>> = vec![
            Box::new(Transcribe::new(&ai, runtime.scheduler().clone(), settings.clone())),
            Box::new(crate::index::Index::new(&ai, runtime.clone(), settings)),
        ];
        let stop = AtomicBool::new(false);
        assert!(crate::worker::run_next(&q, &handlers, &stop).unwrap());
        let task = q.get(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        let t = Transcript::open(&ai, &transcript::file_id(&media).unwrap()).unwrap();
        let dialog = t.dialog().unwrap();
        eprintln!("{dialog:#?}");
        assert!(!dialog.is_empty());
        assert!(dialog.iter().all(|r| r.speaker.is_some()), "diarized");
        assert!(dialog.iter().any(|r| r.text().to_lowercase().contains("лукоморья")));

        // Indexing was queued and runs next; then a question in other words finds the place.
        assert!(crate::worker::run_next(&q, &handlers, &stop).unwrap());
        let index = q.list().unwrap().into_iter().find(|t| t.kind == crate::index::KIND).unwrap();
        assert_eq!(index.status, TaskStatus::Done, "{:?}", index.error);
        assert!(t.chunks().unwrap().iter().all(|(_, _, embedded)| *embedded));
        if let Some(ModelSource::HuggingFace { repo }) = &defaults.models.reranker {
            hub::download(&ai, repo, None, |_, _| {}).unwrap();
        }
        let (hits, trace) = crate::search::search(&runtime, &defaults, &ai, &transcript::file_id(&media).unwrap(), "где растёт дерево у моря?", 3, true).unwrap();
        assert!(trace.by_meaning && trace.reranked && trace.not_indexed == 0, "{trace:?}");
        assert!(hits[0].chunk.text.to_lowercase().contains("дуб"), "{hits:?}");
    }
}
