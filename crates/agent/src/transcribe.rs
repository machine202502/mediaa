use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use mediaa_core::agent::AgentSettings;
use mediaa_core::transcript::{self, Transcript};
use mediaa_queue::Task;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const KIND: &str = "transcribe";

pub const SAMPLE_RATE: u32 = 16_000;

pub const WINDOW_SECONDS: f64 = 600.0;
pub const OVERLAP_SECONDS: f64 = 2.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Cursor {
    t: f64,
}

pub struct Transcribe {
    ai_dir: PathBuf,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    window_seconds: f64,
    overlap_seconds: f64,
}

impl Transcribe {
    pub fn new(
        ai_dir: &std::path::Path,
        settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    ) -> Transcribe {
        Transcribe {
            ai_dir: ai_dir.to_path_buf(),
            settings,
            window_seconds: WINDOW_SECONDS,
            overlap_seconds: OVERLAP_SECONDS,
        }
    }
}

fn window_at(start: f64, duration: f64, window: f64, overlap: f64) -> Option<(f64, f64)> {
    if !(duration.is_finite() && duration > 0.0) || start >= duration - 0.001 {
        return None;
    }
    let end = (start + window).min(duration);
    if end <= start {
        return None;
    }
    let _ = overlap;
    Some((start, end))
}

fn next_start(start: f64, end: f64, duration: f64, overlap: f64) -> Option<f64> {
    if end >= duration - 0.001 {
        return None;
    }
    Some((end - overlap).max(start))
}

fn owned_from(start: f64, overlap: f64) -> f64 {
    if start <= 0.0 {
        start
    } else {
        start + overlap
    }
}

fn keep_words(
    phrases: Vec<mediaa_core::transcript::Phrase>,
    start: f64,
    overlap: f64,
) -> Vec<mediaa_core::transcript::Phrase> {
    let cutoff = owned_from(start, overlap);
    phrases
        .into_iter()
        .filter_map(|mut phrase| {
            phrase.words.retain(|word| word.start >= cutoff);
            if phrase.words.is_empty() {
                return None;
            }
            phrase.start = phrase.words[0].start;
            phrase.end = phrase
                .words
                .last()
                .map(|word| word.end)
                .unwrap_or(phrase.start);
            Some(phrase)
        })
        .collect()
}

fn write_wav(path: &std::path::Path, samples: &[f32]) -> anyhow::Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec)?;
    for sample in samples {
        let clipped = (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16;
        writer.write_sample(clipped)?;
    }
    writer.finalize()?;
    Ok(())
}

fn decode_window(
    path: &std::path::Path,
    start: f64,
    end: f64,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<Vec<f32>> {
    let want = (((end - start).max(0.0)) * f64::from(SAMPLE_RATE)).round() as usize;
    let samples = Arc::new(std::sync::Mutex::new(Vec::<f32>::with_capacity(want)));
    let sink = Arc::clone(&samples);
    let stop = Arc::new(AtomicBool::new(false));
    let user = Arc::clone(cancel);
    mediaa_media::preview::decode_mono(path, start, SAMPLE_RATE, stop, move |chunk| {
        if user.load(Ordering::Relaxed) {
            return false;
        }
        let mut buffer = sink.lock().expect("samples lock");
        if buffer.len() < want {
            let room = want - buffer.len();
            buffer.extend_from_slice(&chunk[..chunk.len().min(room)]);
        }
        buffer.len() < want
    })?;
    let taken = std::mem::take(&mut *samples.lock().expect("samples lock"));
    Ok(taken)
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
        let duration = mediaa_media::info::probe(&input.path)?
            .duration_secs()
            .unwrap_or(0.0);
        let settings = (self.settings)();
        let source = settings
            .models
            .speech_recognition
            .clone()
            .ok_or_else(|| anyhow::anyhow!("the speech recognition model is not chosen"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let Some((start, end)) = window_at(
            cursor.t,
            duration,
            self.window_seconds,
            self.overlap_seconds,
        ) else {
            let phrases = transcript.phrases()?;
            log::info!("{}: {} phrases", input.path.display(), phrases.len());
            let replies = transcript::replies(&phrases, &[]);
            transcript.set_replies(&replies)?;
            transcript.remove_meta("diarized")?;
            return Ok(Work::Done(
                json!({ "file_id": id, "replies": replies.len() }),
            ));
        };
        let samples = decode_window(&input.path, start, end, cancel)?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let wav = self.ai_dir.join(format!("transcribe-{}.wav", task.id));
        write_wav(&wav, &samples)?;
        let scope = Scope::attach(
            host.store.clone(),
            task.id,
            host.engine.clone(),
            task.scope.clone(),
        );
        let engine = scope.engine();
        let recognized = engine.transcribe(
            &mediaa_engine::TranscribeRequest {
                source,
                path: wav.clone(),
                from: start,
                to: end,
                language: settings.ui_language.clone(),
            },
            cancel,
        );
        let _ = std::fs::remove_file(&wav);
        let recognized = recognized.map_err(|error| anyhow::anyhow!("{error}"))?;
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let phrases = keep_words(recognized, start, self.overlap_seconds);
        transcript.put_phrases(owned_from(start, self.overlap_seconds), &phrases)?;
        let Some(next) = next_start(start, end, duration, self.overlap_seconds) else {
            let phrases = transcript.phrases()?;
            let replies = transcript::replies(&phrases, &[]);
            transcript.set_replies(&replies)?;
            transcript.remove_meta("diarized")?;
            return Ok(Work::Done(
                json!({ "file_id": id, "replies": replies.len() }),
            ));
        };
        cursor.t = next;
        let progress = if duration > 0.0 {
            (end / duration).min(1.0) as f32
        } else {
            0.0
        };
        Ok(Work::More {
            cursor: serde_json::to_value(&cursor)?,
            partial: Value::Null,
            progress,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::Host;
    use mediaa_core::agent::{AgentSettings, AnalysisOptions, Budget, ModelChoices, ModelSource};
    use mediaa_core::transcript::Word;
    use mediaa_engine::testing::FakeEngine;
    use mediaa_engine::Engine;
    use mediaa_queue::{Status as TaskStatus, Store};

    fn wav(path: &std::path::Path, seconds: f64) {
        let rate = 8_000u32;
        let data = (seconds * f64::from(rate)) as u32 * 2;
        let mut bytes = Vec::with_capacity(44 + data as usize);
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + data).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2u16.to_le_bytes());
        bytes.extend_from_slice(&16u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&data.to_le_bytes());
        bytes.resize(44 + data as usize, 0);
        std::fs::write(path, bytes).unwrap();
    }

    fn phrase(start: f64, text: &str) -> mediaa_core::transcript::Phrase {
        mediaa_core::transcript::Phrase {
            start,
            end: start + 0.4,
            words: vec![Word {
                text: text.into(),
                start,
                end: start + 0.4,
            }],
        }
    }

    fn stage(ai: &std::path::Path) -> (Transcribe, AgentSettings) {
        let settings = AgentSettings {
            models: ModelChoices {
                speech_recognition: Some(ModelSource::new("c", "asr")),
                ..ModelChoices::default()
            },
            ..AgentSettings::default()
        };
        let current = settings.clone();
        (
            Transcribe {
                ai_dir: ai.to_path_buf(),
                settings: Arc::new(move || current.clone()),
                window_seconds: 6.0,
                overlap_seconds: 2.0,
            },
            settings,
        )
    }

    fn host(ai: &std::path::Path, engine: Arc<dyn Engine>, settings: &AgentSettings) -> Host {
        let store = crate::run::open_store(ai).unwrap();
        let current = settings.clone();
        Host {
            ai_dir: ai.to_path_buf(),
            engine,
            store,
            slots: crate::slots::Slots::new(settings.budget.clone()),
            settings: Arc::new(move || current.clone()),
            streams: Arc::new(crate::run::Streams::default()),
        }
    }

    fn task(store: &dyn Store, media: &std::path::Path) -> mediaa_queue::Task {
        let id = store
            .begin_inline(KIND, "transcribe", &json!({ "path": media }), None, None)
            .unwrap();
        store.get(id).unwrap().unwrap()
    }

    #[test]
    fn windows_cover_the_recording_without_duplicate_overlap_words() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path().join(".ai");
        std::fs::create_dir_all(&ai).unwrap();
        let media = dir.path().join("talk.wav");
        wav(&media, 10.0);
        let fake = FakeEngine::new();
        fake.script_phrases(vec![
            phrase(1.0, "one"),
            phrase(5.0, "five"),
            phrase(7.0, "seven"),
        ]);
        let (stage, settings) = stage(&ai);
        let host = host(&ai, Arc::new(fake.clone()), &settings);
        let mut task = task(host.store.as_ref(), &media);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut windows = Vec::new();
        for _ in 0..8 {
            match stage.chunk(&task, &cancel, &host).unwrap() {
                Work::More { cursor, .. } => {
                    windows.push(fake.transcribes().last().cloned());
                    task.cursor = cursor;
                }
                Work::Done(_) => break,
                Work::Stopped => panic!("stopped"),
            }
        }
        assert_eq!(
            fake.transcribes(),
            ["0:6", "4:10"],
            "the windows cover 10 s with a 2 s overlap"
        );
        let id = transcript::file_id(&media).unwrap();
        let words: Vec<String> = Transcript::open(&ai, &id)
            .unwrap()
            .phrases()
            .unwrap()
            .into_iter()
            .flat_map(|phrase| phrase.words)
            .map(|word| word.text)
            .collect();
        assert_eq!(words, ["one", "five", "seven"]);
        let _ = windows;
    }

    #[test]
    fn stop_and_resume_continue_the_same_window() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path().join(".ai");
        std::fs::create_dir_all(&ai).unwrap();
        let media = dir.path().join("talk.wav");
        wav(&media, 10.0);
        let fake = FakeEngine::new();
        let (stage, settings) = stage(&ai);
        let host = host(&ai, Arc::new(fake.clone()), &settings);
        let mut task = task(host.store.as_ref(), &media);
        let cancel = Arc::new(AtomicBool::new(true));
        assert!(matches!(
            stage.chunk(&task, &cancel, &host).unwrap(),
            Work::Stopped
        ));
        assert!(fake.transcribes().is_empty());
        cancel.store(false, Ordering::Relaxed);
        match stage.chunk(&task, &cancel, &host).unwrap() {
            Work::More { cursor, .. } => {
                assert_eq!(fake.transcribes(), ["0:6"]);
                task.cursor = cursor;
            }
            other => panic!("{other:?}"),
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            stage.chunk(&task, &cancel, &host).unwrap(),
            Work::Stopped
        ));
        assert_eq!(
            fake.transcribes(),
            ["0:6"],
            "the next window was not started"
        );
        cancel.store(false, Ordering::Relaxed);
        let again = stage.chunk(&task, &cancel, &host).unwrap();
        assert!(matches!(again, Work::Done(_)));
        assert_eq!(fake.transcribes(), ["0:6", "4:10"]);
    }

    #[test]
    fn a_window_error_fails_the_task_with_its_text() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path().join(".ai");
        std::fs::create_dir_all(&ai).unwrap();
        let media = dir.path().join("talk.wav");
        wav(&media, 3.0);
        let fake = FakeEngine::new();
        fake.fail_next("the window broke");
        let (stage, settings) = stage(&ai);
        let host = host(&ai, Arc::new(fake), &settings);
        let task = task(host.store.as_ref(), &media);
        let err = stage
            .chunk(&task, &Arc::new(AtomicBool::new(false)), &host)
            .unwrap_err()
            .to_string();
        assert!(err.contains("the window broke"), "{err}");
    }

    #[test]
    #[ignore]
    fn transcribes_through_the_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = std::env::var_os("MEDIAA_TEST_AI").map_or(tmp.path().to_path_buf(), PathBuf::from);
        let wav = ai.join("example.wav");
        if !wav.is_file() {
            let bytes = ureq::Agent::new_with_defaults()
                .get("https://huggingface.co/csukuangfj/tmp-files/resolve/main/GigaAM/example.wav")
                .call()
                .unwrap()
                .into_body()
                .read_to_vec()
                .unwrap();
            std::fs::write(&wav, &bytes).unwrap();
        }
        let library = tempfile::tempdir().unwrap();
        let media = library.path().join("talk.wav");
        std::fs::copy(&wav, &media).unwrap();
        let defaults = AgentSettings::default();
        let options = AnalysisOptions {
            diarize: true,
            speakers: None,
            index: true,
            topics: false,
            details: false,
        };
        let sched = crate::run::open_scheduler(&ai, &defaults.budget).unwrap();
        for t in sched.store().list().unwrap() {
            sched.remove(t.id).unwrap();
        }
        let id = crate::pipeline::start(&sched, &ai, &media, options).unwrap();
        let engine = crate::live::engine();
        let slots = crate::slots::Slots::new(Budget::default());
        let settings: Arc<dyn Fn() -> AgentSettings + Send + Sync> =
            Arc::new(AgentSettings::default);
        crate::run::register_stages(
            &sched,
            &ai,
            engine.clone(),
            slots.clone(),
            settings,
            Arc::new(crate::run::Streams::default()),
        );
        crate::run::drain(&sched, std::time::Duration::from_secs(600));
        let task = sched.store().get(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        let t = Transcript::open(&ai, &transcript::file_id(&media).unwrap()).unwrap();
        let dialog = t.dialog().unwrap();
        eprintln!("{dialog:#?}");
        assert!(!dialog.is_empty());
        assert!(dialog.iter().all(|r| r.speaker.is_some()), "diarized");
        assert!(dialog
            .iter()
            .any(|r| r.text().to_lowercase().contains("лукоморья")));
        assert!(t.chunks().unwrap().iter().all(|(_, _, embedded)| *embedded));
        let (hits, trace) = crate::search::search(
            engine.as_ref(),
            &slots,
            &defaults,
            &ai,
            &transcript::file_id(&media).unwrap(),
            "где растёт дерево у моря?",
            3,
            true,
        )
        .unwrap();
        assert!(
            trace.by_meaning && trace.reranked && trace.not_indexed == 0,
            "{trace:?}"
        );
        assert!(
            hits[0].chunk.text.to_lowercase().contains("дуб"),
            "{hits:?}"
        );
    }
}
