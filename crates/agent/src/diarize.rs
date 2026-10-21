//! The `diarize` queue task: who speaks when in a recording whose phrases are already
//! recognised, written onto the dialog's replies. Its own task after `transcribe`, so the
//! replies are usable before it and its failure (or a missing model) only loses speakers: the
//! task then finishes with a warning, and the index and topics still run.

use crate::slots::Slots;
use crate::transcribe::SAMPLE_RATE;
use crate::worker::{Checkpoints, TaskHandler};
use mediaa_core::agent::{AgentSettings, WorkKind};
use mediaa_core::queue::Task;
use mediaa_core::transcript::{self, Transcript};
use mediaa_engine::{DiarizeRequest, Engine};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
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
    slots: Slots,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Diarize {
    pub fn new(ai_dir: &std::path::Path, slots: Slots, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Diarize {
        Diarize { ai_dir: ai_dir.to_path_buf(), slots, settings }
    }

    /// Decodes the whole recording and asks the engine who speaks when.
    fn turns(&self, engine: &dyn Engine, input: &Input, model: &str, hf_token: Option<&str>) -> anyhow::Result<Vec<transcript::Turn>> {
        let samples = Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let sink = samples.clone();
        mediaa_media::preview::decode_mono(&input.path, 0.0, SAMPLE_RATE, Arc::new(AtomicBool::new(false)), move |chunk| {
            sink.lock().expect("samples lock").extend_from_slice(chunk);
            true
        })?;
        let samples = std::mem::take(&mut *samples.lock().expect("samples lock"));
        let request = DiarizeRequest {
            segmentation: SEGMENTATION_MODEL.to_string(),
            embedding: model.to_string(),
            speakers: input.speakers,
            hf_token: hf_token.map(str::to_string),
        };
        Ok(engine.diarize(&request, &samples)?)
    }
}

impl TaskHandler for Diarize {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(Transcript::exists(&self.ai_dir, &id), "{} has no transcript", input.path.display());
        let mut transcript = Transcript::open(&self.ai_dir, &id)?;
        let phrases = transcript.phrases()?;
        if phrases.is_empty() {
            return Ok(json!({ "file_id": id, "warning": "there is no recognised speech to attribute" }));
        }
        let settings = (self.settings)();
        let Some(model) = settings.models.diarization.as_ref().and_then(|source| crate::models::local_spec(source).ok()) else {
            log::warn!("{}: no diarization model; the replies stay without speakers", input.path.display());
            return Ok(json!({ "file_id": id, "warning": "the diarization model is not chosen" }));
        };
        let hf_token = crate::assistant::hf_token(&settings);
        let engine = cp.engine();

        // Diarization needs the same memory as recognition.
        let _lease = self.slots.request(WorkKind::Transcription, false).wait(cp.stopping())?;
        let turns = match self.turns(engine.as_ref(), &input, &model, hf_token.as_deref()) {
            Ok(turns) => turns,
            Err(e) => {
                // The words are usable without speakers: the chain goes on.
                log::warn!("{}: diarization failed: {e:#}", input.path.display());
                return Ok(json!({ "file_id": id, "warning": format!("{e:#}") }));
            }
        };
        let replies = transcript::replies(&phrases, &turns);
        let speakers = replies.iter().filter_map(|r| r.speaker).collect::<BTreeSet<_>>().len();
        transcript.set_replies(&replies)?;
        transcript.set_meta("diarized", if turns.is_empty() { "0" } else { "1" })?;
        Ok(json!({ "file_id": id, "replies": replies.len(), "speakers": speakers }))
    }
}
