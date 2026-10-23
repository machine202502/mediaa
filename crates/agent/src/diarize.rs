//! The `diarize` queue task: who speaks when in a recording whose phrases are already
//! recognised, written onto the dialog's replies. Its own task after `transcribe`, so the
//! replies are usable before it and its failure (or a missing model) only loses speakers: the
//! task then finishes with a warning, and the index and topics still run.

use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use crate::transcribe::SAMPLE_RATE;
use mediaa_core::agent::AgentSettings;
use mediaa_queue::Task;
use mediaa_core::transcript::{self, Transcript};
use mediaa_engine::{DiarizeRequest, Engine};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const KIND: &str = "diarize";

/// Speaker segmentation for diarization (engine model spec).
pub const SEGMENTATION_MODEL: &str = "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/model.onnx";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
    /// How many people speak when known.
    #[serde(default)]
    pub speakers: Option<u32>,
}

pub struct Diarize {
    ai_dir: PathBuf,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Diarize {
    pub fn new(ai_dir: &std::path::Path, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Diarize {
        Diarize { ai_dir: ai_dir.to_path_buf(), settings }
    }

    /// Decodes the whole recording and asks the engine who speaks when.
    fn turns(&self, engine: &dyn Engine, input: &Input, model: &str, hf_token: Option<&str>, cancel: &Arc<AtomicBool>) -> anyhow::Result<Vec<transcript::Turn>> {
        let samples = Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let sink = samples.clone();
        let at = std::time::Instant::now();
        let pulse = mediaa_core::pulse::current();
        mediaa_media::preview::decode_mono(&input.path, 0.0, SAMPLE_RATE, Arc::clone(cancel), move |chunk| {
            if let Some(pulse) = &pulse {
                pulse.beat();
            }
            sink.lock().expect("samples lock").extend_from_slice(chunk);
            true
        })?;
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("stopped");
        }
        let samples = std::mem::take(&mut *samples.lock().expect("samples lock"));
        log::info!("diarize: decoded {:.0} s of sound in {:.0} s", samples.len() as f64 / f64::from(SAMPLE_RATE), at.elapsed().as_secs_f32());
        let request = DiarizeRequest {
            segmentation: SEGMENTATION_MODEL.to_string(),
            embedding: model.to_string(),
            speakers: input.speakers,
            hf_token: hf_token.map(str::to_string),
        };
        let at = std::time::Instant::now();
        let turns = engine.diarize(&request, &samples)?;
        log::info!("diarize: the engine found {} turns in {:.0} s", turns.len(), at.elapsed().as_secs_f32());
        Ok(turns)
    }
}

impl Stage for Diarize {
    fn handles(&self, kind: &str) -> bool {
        kind == KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(Transcript::exists(&self.ai_dir, &id), "{} has no transcript", input.path.display());
        let mut transcript = Transcript::open(&self.ai_dir, &id)?;
        let phrases = transcript.phrases()?;
        if phrases.is_empty() {
            return Ok(Work::Done(json!({ "file_id": id, "warning": "there is no recognised speech to attribute" })));
        }
        let settings = (self.settings)();
        let Some(model) = settings.models.diarization.as_ref().and_then(|source| crate::models::local_spec(source).ok()) else {
            log::warn!("{}: no diarization model; the replies stay without speakers", input.path.display());
            return Ok(Work::Done(json!({ "file_id": id, "warning": "the diarization model is not chosen" })));
        };
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let hf_token = crate::assistant::hf_token(&settings);
        let scope = Scope::attach(host.store.clone(), task.id, host.engine.clone(), task.scope.clone());
        let engine = scope.engine();
        // One chunk: the engine's diarize call has no cancel of its own. Decode stops at once;
        // a stop during the engine call is noticed when it returns and the replies are not written.
        let turns = match self.turns(engine.as_ref(), &input, &model, hf_token.as_deref(), cancel) {
            Ok(turns) if cancel.load(Ordering::Relaxed) => {
                let _ = turns;
                return Ok(Work::Stopped);
            }
            Ok(turns) => turns,
            Err(e) if cancel.load(Ordering::Relaxed) || e.to_string().contains("stopped") => return Ok(Work::Stopped),
            Err(e) => {
                log::warn!("{}: diarization failed: {e:#}", input.path.display());
                return Ok(Work::Done(json!({ "file_id": id, "warning": format!("{e:#}") })));
            }
        };
        let replies = transcript::replies(&phrases, &turns);
        let speakers = replies.iter().filter_map(|r| r.speaker).collect::<BTreeSet<_>>().len();
        transcript.set_replies(&replies)?;
        transcript.set_meta("diarized", if turns.is_empty() { "0" } else { "1" })?;
        Ok(Work::Done(json!({ "file_id": id, "replies": replies.len(), "speakers": speakers })))
    }
}
