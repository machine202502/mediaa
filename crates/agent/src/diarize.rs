use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use crate::transcribe::SAMPLE_RATE;
use crate::voices::{self, Voice};
use mediaa_core::agent::{AgentSettings, ModelSource};
use mediaa_core::transcript::{self, Transcript, Turn};
use mediaa_engine::DiarizeRequest;
use mediaa_queue::Task;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const KIND: &str = "diarize";

pub const CHUNK_SECONDS: f64 = 480.0;
pub const OVERLAP_SECONDS: f64 = 15.0;
const OVERLAP_LINK_SECONDS: f64 = 1.0;
pub const LINK_THRESHOLD: f32 = 0.75;
pub const LINK_MIN_SECONDS: f64 = 3.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
    #[serde(default)]
    pub speakers: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub start: f64,
    pub end: f64,
    pub turns: Vec<Turn>,
    pub voices: Vec<ChunkVoice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkVoice {
    pub embedding: Vec<f32>,
    pub seconds: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diarization {
    pub turns: Vec<Turn>,
    pub voices: Vec<Voice>,
}

impl Chunk {
    pub fn new(start: f64, end: f64, mut turns: Vec<Turn>, voices: Vec<ChunkVoice>) -> Chunk {
        for turn in &mut turns {
            turn.start += start;
            turn.end += start;
        }
        Chunk {
            start,
            end,
            turns,
            voices,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Cursor {
    #[serde(default)]
    chunks: Vec<Chunk>,
    #[serde(default)]
    voice_source: String,
}

pub fn plan(total: f64, chunk: f64, overlap: f64) -> Vec<(f64, f64)> {
    if total <= 0.0 || chunk <= 0.0 || !total.is_finite() || !chunk.is_finite() {
        return Vec::new();
    }
    let overlap = overlap.max(0.0).min(chunk / 2.0);
    let mut ranges = Vec::new();
    let mut start = 0.0;
    loop {
        let end = (start + chunk).min(total);
        ranges.push((start, end));
        if end >= total {
            return ranges;
        }
        start = end - overlap;
    }
}

pub fn link(
    chunks: &[Chunk],
    threshold: f32,
    min_seconds: f64,
    speakers: Option<u32>,
) -> Diarization {
    if chunks.is_empty() {
        return Diarization::default();
    }
    let mut bases = Vec::with_capacity(chunks.len());
    let mut base = 0usize;
    for chunk in chunks {
        bases.push(base);
        base += chunk.voices.len();
    }
    let links = overlap_links(chunks, &bases);
    let mut turns = Vec::new();
    let mut voices = Vec::with_capacity(base);
    for (index, chunk) in chunks.iter().enumerate() {
        let own_end = chunks.get(index + 1).map_or(chunk.end, |next| next.start);
        let mut seconds = vec![0.0_f64; chunk.voices.len()];
        for turn in &chunk.turns {
            let end = turn.end.min(own_end);
            if turn.start >= own_end || end <= turn.start {
                continue;
            }
            if let Some(total) = seconds.get_mut(turn.speaker as usize) {
                *total += end - turn.start;
            }
            turns.push(Turn {
                start: turn.start,
                end,
                speaker: bases[index] as u32 + turn.speaker,
            });
        }
        for (voice, seconds) in chunk.voices.iter().zip(seconds) {
            voices.push(Voice {
                embedding: voice.embedding.clone(),
                seconds,
            });
        }
    }
    let (turns, voices) =
        voices::merge_voices_linked(&turns, &voices, &links, threshold, min_seconds);
    let found = Diarization { turns, voices };
    match speakers {
        Some(speakers) if found.voices.len() > speakers as usize => {
            let (turns, voices) =
                voices::merge_voices_to(&found.turns, &found.voices, speakers as usize);
            Diarization { turns, voices }
        }
        _ => found,
    }
}

fn overlap_links(chunks: &[Chunk], bases: &[usize]) -> Vec<(usize, usize)> {
    let mut links = Vec::new();
    for index in 0..chunks.len().saturating_sub(1) {
        let (left, right) = (&chunks[index], &chunks[index + 1]);
        let low = left.start.max(right.start);
        let high = left.end.min(right.end);
        if high - low < OVERLAP_LINK_SECONDS {
            continue;
        }
        let coincidence = |a: usize, b: usize| -> f64 {
            let mut seconds = 0.0;
            for one in left.turns.iter().filter(|turn| turn.speaker as usize == a) {
                for other in right.turns.iter().filter(|turn| turn.speaker as usize == b) {
                    let start = one.start.max(other.start).max(low);
                    let end = one.end.min(other.end).min(high);
                    if end > start {
                        seconds += end - start;
                    }
                }
            }
            seconds
        };
        let mut pairs: Vec<(f64, usize, usize)> = Vec::new();
        for a in 0..left.voices.len() {
            for b in 0..right.voices.len() {
                let seconds = coincidence(a, b);
                if seconds >= OVERLAP_LINK_SECONDS {
                    pairs.push((seconds, a, b));
                }
            }
        }
        pairs.sort_by(|one, other| other.0.total_cmp(&one.0));
        let (mut used_left, mut used_right) = (BTreeSet::new(), BTreeSet::new());
        for (_, a, b) in pairs {
            if used_left.contains(&a) || used_right.contains(&b) {
                continue;
            }
            used_left.insert(a);
            used_right.insert(b);
            links.push((bases[index] + a, bases[index + 1] + b));
        }
    }
    links
}

fn decode_range(
    path: &Path,
    start: f64,
    end: f64,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<Vec<f32>> {
    let want = (((end - start).max(0.0)) * f64::from(SAMPLE_RATE)).round() as usize;
    let samples = Arc::new(Mutex::new(Vec::<f32>::with_capacity(want)));
    let sink = Arc::clone(&samples);
    let stop = Arc::new(AtomicBool::new(false));
    let user = Arc::clone(cancel);
    let pulse = mediaa_core::pulse::current();
    let at = std::time::Instant::now();
    mediaa_media::preview::decode_mono(path, start, SAMPLE_RATE, stop, move |chunk| {
        if let Some(pulse) = &pulse {
            pulse.beat();
        }
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
    let samples = std::mem::take(&mut *samples.lock().expect("samples lock"));
    log::info!(
        "diarize: [{start:.0}..{end:.0}] s decoded as {:.1} s of sound in {:.1} s",
        samples.len() as f64 / f64::from(SAMPLE_RATE),
        at.elapsed().as_secs_f32()
    );
    Ok(samples)
}

fn write_wav(path: &Path, samples: &[f32]) -> anyhow::Result<()> {
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

fn tmp_dir(ai_dir: &Path, task: mediaa_queue::TaskId) -> PathBuf {
    ai_dir.join("tmp").join(task.to_string())
}

fn voices_of(
    engine: &dyn mediaa_engine::Engine,
    source: Option<&ModelSource>,
    samples: &[f32],
    turns: &[Turn],
    count: usize,
    dir: &Path,
    cancel: &Arc<AtomicBool>,
) -> anyhow::Result<Vec<ChunkVoice>> {
    let Some(source) = source else {
        return Ok((0..count)
            .map(|_| ChunkVoice {
                embedding: Vec::new(),
                seconds: 0.0,
            })
            .collect());
    };
    let clips = voices::clips(samples, turns, count);
    let mut paths = Vec::new();
    for (index, clip) in clips.iter().enumerate() {
        let path = dir.join(format!("voice-{index}.wav"));
        write_wav(&path, clip)?;
        paths.push(path);
    }
    let vectors = engine
        .voice_embeddings(
            &mediaa_engine::VoiceRequest {
                source: source.clone(),
                clips: paths.clone(),
            },
            cancel,
        )
        .map_err(|error| anyhow::anyhow!("{error}"))?;
    for path in &paths {
        let _ = std::fs::remove_file(path);
    }
    if vectors.len() != paths.len() {
        anyhow::bail!(
            "the command returned {} vectors for {} files",
            vectors.len(),
            paths.len()
        );
    }
    Ok(clips
        .iter()
        .zip(vectors)
        .map(|(clip, vector)| ChunkVoice {
            embedding: voices::unit(&vector).unwrap_or_default(),
            seconds: clip.len() as f64 / f64::from(SAMPLE_RATE),
        })
        .collect())
}

pub struct Diarize {
    ai_dir: PathBuf,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    chunk_seconds: f64,
    overlap_seconds: f64,
}

impl Diarize {
    pub fn new(
        ai_dir: &std::path::Path,
        settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    ) -> Diarize {
        Diarize {
            ai_dir: ai_dir.to_path_buf(),
            settings,
            chunk_seconds: CHUNK_SECONDS,
            overlap_seconds: OVERLAP_SECONDS,
        }
    }
}

impl Stage for Diarize {
    fn handles(&self, kind: &str) -> bool {
        kind == KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(
            Transcript::exists(&self.ai_dir, &id),
            "{} has no transcript",
            input.path.display()
        );
        let mut transcript = Transcript::open(&self.ai_dir, &id)?;
        let phrases = transcript.phrases()?;
        if phrases.is_empty() {
            return Ok(Work::Done(
                json!({ "file_id": id, "warning": "there is no recognised speech to attribute" }),
            ));
        }
        let settings = (self.settings)();
        let Some(source) = settings.models.diarization.clone() else {
            log::warn!(
                "{}: no diarization model; the replies stay without speakers",
                input.path.display()
            );
            return Ok(Work::Done(
                json!({ "file_id": id, "warning": "the diarization model is not chosen" }),
            ));
        };
        let voice = settings.models.voice_embedding.clone();
        let voice_key = voice
            .as_ref()
            .map(|source| source.key())
            .unwrap_or_default();
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let total = phrases
            .iter()
            .map(|phrase| phrase.end)
            .fold(0.0_f64, f64::max);
        let ranges = plan(total, self.chunk_seconds, self.overlap_seconds);
        let mut cursor: Cursor = if task.cursor.is_null() {
            Cursor::default()
        } else {
            serde_json::from_value(task.cursor.clone())?
        };
        let dir = tmp_dir(&self.ai_dir, task.id);
        std::fs::create_dir_all(&dir)?;
        let scope = Scope::attach(
            host.store.clone(),
            task.id,
            host.engine.clone(),
            task.scope.clone(),
        );
        let engine = scope.engine();
        if cursor.voice_source != voice_key {
            for (index, chunk) in cursor.chunks.iter_mut().enumerate() {
                let samples = decode_range(&input.path, chunk.start, chunk.end, cancel)?;
                if cancel.load(Ordering::Relaxed) {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Ok(Work::Stopped);
                }
                let local: Vec<Turn> = chunk
                    .turns
                    .iter()
                    .map(|turn| Turn {
                        start: (turn.start - chunk.start).max(0.0),
                        end: (turn.end - chunk.start).max(0.0),
                        speaker: turn.speaker,
                    })
                    .collect();
                let count = chunk.voices.len();
                chunk.voices = voices_of(
                    engine.as_ref(),
                    voice.as_ref(),
                    &samples,
                    &local,
                    count,
                    &dir,
                    cancel,
                )?;
                let _ = index;
            }
            cursor.voice_source = voice_key.clone();
        }
        let reused = cursor.chunks.len();
        if let Some(&(start, end)) = ranges.get(reused) {
            let samples = decode_range(&input.path, start, end, cancel)?;
            if cancel.load(Ordering::Relaxed) {
                let _ = std::fs::remove_dir_all(&dir);
                return Ok(Work::Stopped);
            }
            let wav = dir.join(format!("chunk-{reused}.wav"));
            write_wav(&wav, &samples)?;
            let found = match engine.diarize(
                &DiarizeRequest {
                    source: source.clone(),
                    path: wav.clone(),
                },
                cancel,
            ) {
                Ok(found) => found,
                Err(_) if cancel.load(Ordering::Relaxed) => {
                    let _ = std::fs::remove_file(&wav);
                    let _ = std::fs::remove_dir_all(&dir);
                    return Ok(Work::Stopped);
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&wav);
                    let _ = std::fs::remove_dir_all(&dir);
                    log::warn!("{}: diarization failed: {e:#}", input.path.display());
                    return Ok(Work::Done(
                        json!({ "file_id": id, "warning": format!("{e:#}") }),
                    ));
                }
            };
            let _ = std::fs::remove_file(&wav);
            if cancel.load(Ordering::Relaxed) {
                let _ = std::fs::remove_dir_all(&dir);
                return Ok(Work::Stopped);
            }
            let mut turns = found;
            let count = voices::compact(&mut turns);
            let voices = match voices_of(
                engine.as_ref(),
                voice.as_ref(),
                &samples,
                &turns,
                count,
                &dir,
                cancel,
            ) {
                Ok(voices) => voices,
                Err(error) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    return Err(error);
                }
            };
            cursor.chunks.push(Chunk::new(start, end, turns, voices));
            cursor.voice_source = voice_key;
            log::info!(
                "diarize: chunk {}/{} [{start:.0}..{end:.0}] s is done",
                cursor.chunks.len(),
                ranges.len()
            );
            let progress = (end / total).clamp(0.0, 1.0) as f32;
            return Ok(Work::More {
                cursor: serde_json::to_value(&cursor)?,
                partial: json!({ "chunks": cursor.chunks.len(), "of": ranges.len() }),
                progress,
            });
        }
        if cancel.load(Ordering::Relaxed) {
            let _ = std::fs::remove_dir_all(&dir);
            return Ok(Work::Stopped);
        }
        let found = link(
            &cursor.chunks,
            LINK_THRESHOLD,
            LINK_MIN_SECONDS,
            input.speakers,
        );
        log::info!(
            "diarize: {} chunks ({} reused from the cursor) linked into {} speakers",
            cursor.chunks.len(),
            reused,
            found.voices.len()
        );
        let replies = transcript::replies(&phrases, &found.turns);
        let speakers = replies
            .iter()
            .filter_map(|r| r.speaker)
            .collect::<BTreeSet<_>>()
            .len();
        transcript.set_replies(&replies)?;
        transcript.set_meta("diarized", if found.turns.is_empty() { "0" } else { "1" })?;
        let _ = std::fs::remove_dir_all(&dir);
        let mut result = json!({ "file_id": id, "replies": replies.len(), "speakers": speakers, "chunks": cursor.chunks.len(), "reused": reused });
        if voice.is_none() {
            result["warning"] = json!("speakers are linked only by the overlaps");
        }
        Ok(Work::Done(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_engine::testing::FakeEngine;
    use mediaa_engine::Engine;

    #[test]
    #[ignore]
    fn diarization_quality_on_eval_data() {
        let settings = AgentSettings::default();
        let engine = crate::live::engine();
        let data = eval_data();
        let reports = crate::diarize_quality::measure_all(engine.as_ref(), &settings, &data)
            .expect("measuring evals/data");
        assert!(!reports.is_empty(), "no recordings in {}", data.display());
        for report in &reports {
            println!("{report}");
        }
    }

    #[test]
    fn overlapped_speech_keeps_every_word_in_the_replies() {
        use mediaa_core::transcript::{replies, Phrase, Turn, Word};
        let words = [
            (1.0, "a"),
            (3.0, "b"),
            (7.5, "c"),
            (8.2, "d"),
            (9.6, "e"),
            (12.0, "f"),
            (14.0, "g"),
        ];
        let phrases: Vec<Phrase> = words
            .chunks(3)
            .map(|chunk| Phrase {
                start: chunk[0].0,
                end: chunk[chunk.len() - 1].0 + 0.4,
                words: chunk
                    .iter()
                    .map(|&(start, text)| Word {
                        text: text.to_string(),
                        start,
                        end: start + 0.4,
                    })
                    .collect(),
            })
            .collect();
        let turns = [
            Turn {
                start: 0.0,
                end: 10.0,
                speaker: 0,
            },
            Turn {
                start: 8.0,
                end: 15.0,
                speaker: 1,
            },
        ];
        let replies = replies(&phrases, &turns);
        let kept: Vec<&str> = replies
            .iter()
            .flat_map(|reply| reply.words.iter().map(|word| word.text.as_str()))
            .collect();
        assert_eq!(
            kept,
            words.iter().map(|&(_, text)| text).collect::<Vec<_>>(),
            "no word of the overlap is lost"
        );
        let speakers: BTreeSet<u32> = replies.iter().filter_map(|reply| reply.speaker).collect();
        assert_eq!(speakers, BTreeSet::from([0, 1]));
        assert!(replies
            .iter()
            .filter(|reply| reply.speaker == Some(1))
            .flat_map(|reply| &reply.words)
            .any(|word| word.text == "g"));
    }

    #[test]
    #[ignore]
    fn diarization_short_speakers_on_eval_data() {
        let settings = AgentSettings::default();
        let engine = crate::live::engine();
        let data = eval_data();
        let run = crate::diarize_quality::Run {
            model: Some("csukuangfj/speaker-embedding-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx".to_string()),
            clustering_threshold: Some(0.5),
            merge_threshold: None,
            ..crate::diarize_quality::Run::default()
        };
        for audio in recordings(&data) {
            let found = match crate::diarize_quality::run_recording(
                engine.as_ref(),
                &settings,
                &audio,
                &run,
            ) {
                Ok(found) => found,
                Err(e) => {
                    println!("{}: {e:#}", audio.display());
                    continue;
                }
            };
            for min in [2.0, 4.0, 6.0, 8.0] {
                let (turns, voices) = voices::merge_voices(
                    &found.diarization.turns,
                    &found.diarization.voices,
                    LINK_THRESHOLD,
                    min,
                );
                let scored = crate::diarize_quality::score(&crate::diarize_quality::Found {
                    diarization: Diarization { turns, voices },
                    ..found.clone()
                })
                .expect("scoring");
                println!("min_speaker_seconds={min:.0} | {scored}");
            }
        }
    }

    const GRID_MODELS: [&str; 3] = [
        "csukuangfj/speaker-embedding-models/wespeaker_en_voxceleb_resnet34_LM.onnx",
        "csukuangfj/speaker-embedding-models/3dspeaker_speech_campplus_sv_zh_en_16k-common_advanced.onnx",
        "csukuangfj/speaker-embedding-models/3dspeaker_speech_eres2netv2_sv_zh-cn_16k-common.onnx",
    ];
    const GRID_CLUSTERING: [f32; 4] = [0.3, 0.4, 0.5, 0.6];
    const GRID_MERGES: [Option<f32>; 5] = [None, Some(0.70), Some(0.75), Some(0.80), Some(0.85)];

    fn eval_data() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("evals")
            .join("data")
    }

    fn recordings(data: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut folders: Vec<std::path::PathBuf> = std::fs::read_dir(data)
            .expect("evals/data")
            .flatten()
            .map(|entry| entry.path().join("audio.opus"))
            .filter(|audio| audio.is_file())
            .collect();
        folders.sort();
        folders
    }

    #[test]
    #[ignore]
    fn diarization_model_grid_on_eval_data() {
        let settings = AgentSettings::default();
        let engine = crate::live::engine();
        let data = eval_data();
        for model in GRID_MODELS {
            let short = model
                .rsplit('/')
                .next()
                .unwrap_or(model)
                .trim_end_matches(".onnx")
                .to_string();
            for clustering in GRID_CLUSTERING {
                let run = crate::diarize_quality::Run {
                    model: Some(model.to_string()),
                    clustering_threshold: Some(clustering),
                    merge_threshold: None,
                    ..crate::diarize_quality::Run::default()
                };
                for audio in recordings(&data) {
                    let found = match crate::diarize_quality::run_recording(
                        engine.as_ref(),
                        &settings,
                        &audio,
                        &run,
                    ) {
                        Ok(found) => found,
                        Err(e) => {
                            println!(
                                "{short} clustering={clustering}: {}: {e:#}",
                                audio.display()
                            );
                            continue;
                        }
                    };
                    let clusters = found.diarization.voices.len();
                    for merge in GRID_MERGES {
                        let diarization = match merge {
                            None => found.diarization.clone(),
                            Some(threshold) => {
                                let (turns, voices) = voices::merge_voices(
                                    &found.diarization.turns,
                                    &found.diarization.voices,
                                    threshold,
                                    LINK_MIN_SECONDS,
                                );
                                Diarization { turns, voices }
                            }
                        };
                        let scored =
                            crate::diarize_quality::score(&crate::diarize_quality::Found {
                                diarization,
                                ..found.clone()
                            });
                        match scored {
                            Ok(recording) => {
                                let merge = merge.map_or("—".to_string(), |threshold| {
                                    format!("{threshold:.2}")
                                });
                                println!("{short} clustering={clustering:.2} merge={merge} clusters={clusters} | {recording}");
                            }
                            Err(e) => println!(
                                "{short} clustering={clustering}: {}: {e:#}",
                                audio.display()
                            ),
                        }
                    }
                }
            }
        }
    }

    #[test]
    #[ignore]
    fn diarization_chunking_on_eval_data() {
        let settings = AgentSettings::default();
        let engine = crate::live::engine();
        let data = eval_data();
        let mut grid = vec![(None, LINK_THRESHOLD, None)];
        for merge in [LINK_THRESHOLD, LINK_THRESHOLD] {
            grid.push((Some(CHUNK_SECONDS), merge, None));
        }
        for (chunk, merge, overlap) in grid {
            let label = match (chunk, overlap) {
                (None, _) => "whole".to_string(),
                (Some(length), None) => format!("{length:.0} s chunks, link {merge:.2}"),
                (Some(length), Some(overlap)) => {
                    format!("{length:.0} s chunks, link {merge:.2}, overlap {overlap:.0} s")
                }
            };
            let run = crate::diarize_quality::Run {
                merge_threshold: Some(merge),
                chunk_seconds: chunk,
                overlap_seconds: overlap,
                ..crate::diarize_quality::Run::default()
            };
            for audio in recordings(&data) {
                let started = std::time::Instant::now();
                match crate::diarize_quality::run_recording(
                    engine.as_ref(),
                    &settings,
                    &audio,
                    &run,
                ) {
                    Ok(found) => {
                        let mut seconds: Vec<f64> = found
                            .diarization
                            .voices
                            .iter()
                            .map(|voice| voice.seconds)
                            .collect();
                        seconds.sort_by(|one, other| other.total_cmp(one));
                        let voices: Vec<String> = seconds
                            .iter()
                            .map(|seconds| format!("{seconds:.1}"))
                            .collect();
                        match crate::diarize_quality::score(&found) {
                            Ok(recording) => println!(
                                "{label} | {recording} | voices {} s | {:.0} s total",
                                voices.join(" "),
                                started.elapsed().as_secs_f64()
                            ),
                            Err(e) => println!("{label} | {}: {e:#}", audio.display()),
                        }
                    }
                    Err(e) => println!("{label} | {}: {e:#}", audio.display()),
                }
            }
        }
    }

    #[test]
    #[ignore]
    fn diarization_short_speakers_after_chunks_on_eval_data() {
        let settings = AgentSettings::default();
        let engine = crate::live::engine();
        let data = eval_data();
        let run = crate::diarize_quality::Run {
            merge_threshold: Some(LINK_THRESHOLD),
            chunk_seconds: Some(CHUNK_SECONDS),
            ..crate::diarize_quality::Run::default()
        };
        for audio in recordings(&data) {
            let (found, chunks) = match crate::diarize_quality::run_recording_chunks(
                engine.as_ref(),
                &settings,
                &audio,
                &run,
            ) {
                Ok(run) => run,
                Err(e) => {
                    println!("{}: {e:#}", audio.display());
                    continue;
                }
            };
            let speech: f64 = chunks
                .iter()
                .flat_map(|chunk| &chunk.turns)
                .map(|turn| turn.end - turn.start)
                .sum();
            let rows = [
                ("2 s (before)", 2.0),
                ("3 s", 3.0),
                ("5 s", 5.0),
                ("8 s", 8.0),
                ("12 s", 12.0),
                ("1 % of speech", 0.01 * speech),
            ];
            for (label, min_seconds) in rows {
                let diarization = link(&chunks, LINK_THRESHOLD, min_seconds, None);
                let mut seconds: Vec<f64> = diarization
                    .voices
                    .iter()
                    .map(|voice| voice.seconds)
                    .collect();
                seconds.sort_by(|one, other| other.total_cmp(one));
                let voices: Vec<String> = seconds
                    .iter()
                    .map(|seconds| format!("{seconds:.1}"))
                    .collect();
                let scored = crate::diarize_quality::score(&crate::diarize_quality::Found {
                    diarization,
                    ..found.clone()
                })
                .expect("scoring");
                println!(
                    "{label} | min {min_seconds:.1} s | {scored} | voices {} s",
                    voices.join(" ")
                );
            }
        }
    }

    #[test]
    fn chunks_overlap_and_cover_the_whole_recording() {
        let ranges = plan(100.0, 30.0, 5.0);
        assert_eq!(
            ranges,
            [(0.0, 30.0), (25.0, 55.0), (50.0, 80.0), (75.0, 100.0)]
        );
        assert!(
            ranges
                .windows(2)
                .all(|pair| (pair[0].1 - pair[1].0 - 5.0).abs() < 1e-9),
            "neighbours share 5 s"
        );
        assert_eq!(
            plan(10.0, 30.0, 5.0),
            [(0.0, 10.0)],
            "a recording shorter than a chunk is one chunk"
        );
        assert!(plan(0.0, 30.0, 5.0).is_empty());
        assert!(plan(10.0, 0.0, 5.0).is_empty());
        assert_eq!(
            plan(20.0, 10.0, 50.0),
            [(0.0, 10.0), (5.0, 15.0), (10.0, 20.0)]
        );
    }

    fn one_voice(start: f64, end: f64, voice: [f32; 2], turns: &[(f64, f64)]) -> Chunk {
        Chunk {
            start,
            end,
            turns: turns
                .iter()
                .map(|&(start, end)| Turn {
                    start,
                    end,
                    speaker: 0,
                })
                .collect(),
            voices: vec![ChunkVoice {
                embedding: voice.to_vec(),
                seconds: turns.iter().map(|&(start, end)| end - start).sum(),
            }],
        }
    }

    #[test]
    fn the_same_voice_in_two_chunks_is_one_speaker() {
        let same = vec![
            one_voice(0.0, 60.0, [1.0, 0.0], &[(0.0, 30.0)]),
            one_voice(60.0, 120.0, [1.0, 0.0], &[(60.0, 90.0)]),
        ];
        let found = link(&same, LINK_THRESHOLD, 0.0, None);
        assert_eq!(found.voices.len(), 1, "{found:?}");
        assert_eq!(
            found
                .turns
                .iter()
                .map(|turn| turn.speaker)
                .collect::<Vec<_>>(),
            [0, 0],
            "one number for the person"
        );

        let different = vec![
            one_voice(0.0, 60.0, [1.0, 0.0], &[(0.0, 30.0)]),
            one_voice(60.0, 120.0, [0.0, 1.0], &[(60.0, 90.0)]),
        ];
        let found = link(&different, LINK_THRESHOLD, 0.0, None);
        assert_eq!(found.voices.len(), 2, "{found:?}");
        assert_eq!(
            found
                .turns
                .iter()
                .map(|turn| turn.speaker)
                .collect::<Vec<_>>(),
            [0, 1]
        );

        let close = vec![
            one_voice(0.0, 60.0, [1.0, 0.0], &[(0.0, 30.0)]),
            one_voice(60.0, 120.0, [0.78, 0.626], &[(60.0, 90.0)]),
        ];
        assert_eq!(
            link(&close, LINK_THRESHOLD, 0.0, None).voices.len(),
            1,
            "cosine ≈ 0.78"
        );
        assert_eq!(link(&close, 0.80, 0.0, None).voices.len(), 2);
    }

    #[test]
    fn a_short_voice_of_one_chunk_goes_to_its_nearest_voice() {
        let chunks = vec![
            one_voice(0.0, 60.0, [1.0, 0.0], &[(0.0, 30.0)]),
            Chunk {
                start: 60.0,
                end: 120.0,
                turns: vec![
                    Turn {
                        start: 60.0,
                        end: 90.0,
                        speaker: 0,
                    },
                    Turn {
                        start: 90.0,
                        end: 93.8,
                        speaker: 1,
                    },
                ],
                voices: vec![
                    ChunkVoice {
                        embedding: vec![1.0, 0.0],
                        seconds: 30.0,
                    },
                    ChunkVoice {
                        embedding: vec![0.0, 1.0],
                        seconds: 3.8,
                    },
                ],
            },
        ];
        assert_eq!(
            link(&chunks, LINK_THRESHOLD, LINK_MIN_SECONDS, None)
                .voices
                .len(),
            2,
            "3.8 s stands on its own at 3 s"
        );
        assert_eq!(
            link(&chunks, LINK_THRESHOLD, 5.0, None).voices.len(),
            1,
            "5 s sends it to the nearest voice"
        );
    }

    #[test]
    fn talking_at_once_in_the_overlap_links_the_chunks_whatever_the_voices_say() {
        let chunks = vec![
            one_voice(0.0, 50.0, [1.0, 0.0], &[(14.0, 50.0)]),
            one_voice(40.0, 100.0, [0.0, 1.0], &[(40.0, 60.0)]),
        ];
        let found = link(&chunks, LINK_THRESHOLD, 0.0, None);
        assert_eq!(found.voices.len(), 1, "{found:?}");
        assert_eq!(
            found.turns,
            [
                Turn {
                    start: 14.0,
                    end: 40.0,
                    speaker: 0
                },
                Turn {
                    start: 40.0,
                    end: 60.0,
                    speaker: 0
                }
            ]
        );

        let apart = vec![
            one_voice(0.0, 50.0, [1.0, 0.0], &[(0.0, 20.0)]),
            one_voice(40.0, 100.0, [0.0, 1.0], &[(40.0, 60.0)]),
        ];
        assert_eq!(link(&apart, LINK_THRESHOLD, 0.0, None).voices.len(), 2);
    }

    fn wav(path: &Path, seconds: f64) {
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

    fn library(seconds: f64) -> (tempfile::TempDir, PathBuf, Arc<dyn mediaa_queue::Store>) {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        std::fs::create_dir_all(&ai).unwrap();
        let media = tmp.path().join("talk.wav");
        wav(&media, seconds);
        let id = transcript::file_id(&media).unwrap();
        let mut transcript = Transcript::open(&ai, &id).unwrap();
        let phrases: Vec<mediaa_core::transcript::Phrase> = (0..seconds as usize)
            .map(|second| {
                let start = second as f64;
                mediaa_core::transcript::Phrase {
                    start,
                    end: start + 1.0,
                    words: vec![mediaa_core::transcript::Word {
                        text: format!("w{second}"),
                        start: start + 0.1,
                        end: start + 0.9,
                    }],
                }
            })
            .collect();
        transcript.put_phrases(0.0, &phrases).unwrap();
        let store = crate::run::open_store(&ai).unwrap();
        (tmp, media, store)
    }

    fn short_chunks(ai: &Path, chunk: f64, overlap: f64) -> (Diarize, AgentSettings) {
        let mut settings = AgentSettings::default();
        settings.models.diarization = Some(ModelSource::new("c", "dia"));
        let current = settings.clone();
        (
            Diarize {
                ai_dir: ai.to_path_buf(),
                settings: Arc::new(move || current.clone()),
                chunk_seconds: chunk,
                overlap_seconds: overlap,
            },
            settings,
        )
    }

    fn stage_host(
        ai: &Path,
        engine: Arc<dyn Engine>,
        store: Arc<dyn mediaa_queue::Store>,
        settings: &AgentSettings,
    ) -> Host {
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

    fn diarize_task(store: &dyn mediaa_queue::Store, media: &Path) -> Task {
        let id = store
            .begin_inline(KIND, "diarize", &json!({ "path": media }), None, None)
            .unwrap();
        store.get(id).unwrap().unwrap()
    }

    fn run_to_end(
        stage: &Diarize,
        task: &mut Task,
        host: &Host,
        cancel: &Arc<AtomicBool>,
    ) -> serde_json::Value {
        for _ in 0..64 {
            match stage.chunk(task, cancel, host).unwrap() {
                Work::More { cursor, .. } => task.cursor = cursor,
                Work::Done(result) => return result,
                Work::Stopped => panic!("the task stopped"),
            }
        }
        panic!("the task did not finish");
    }

    #[test]
    fn a_stopped_diarization_goes_on_from_the_next_chunk_with_the_same_replies() {
        let ranges = plan(10.0, 3.0, 0.5);
        assert_eq!(ranges.len(), 4);

        let (first, media, store) = library(10.0);
        let ai = first.path().join(".ai");
        let id = transcript::file_id(&media).unwrap();
        let fake = FakeEngine::new();
        let (stage, settings) = short_chunks(&ai, 3.0, 0.5);
        let host = stage_host(&ai, Arc::new(fake.clone()), store.clone(), &settings);
        let mut task = diarize_task(store.as_ref(), &media);
        let cancel = Arc::new(AtomicBool::new(false));
        run_to_end(&stage, &mut task, &host, &cancel);
        assert_eq!(
            fake.diarizes().len(),
            ranges.len(),
            "one engine call per chunk"
        );
        let uninterrupted = Transcript::open(&ai, &id).unwrap().dialog().unwrap();

        let (second, media, store) = library(10.0);
        let ai = second.path().join(".ai");
        let fake = FakeEngine::new();
        let (stage, settings) = short_chunks(&ai, 3.0, 0.5);
        let host = stage_host(&ai, Arc::new(fake.clone()), store.clone(), &settings);
        let mut task = diarize_task(store.as_ref(), &media);
        let cancel = Arc::new(AtomicBool::new(false));
        match stage.chunk(&task, &cancel, &host).unwrap() {
            Work::More {
                cursor, partial, ..
            } => {
                task.cursor = cursor;
                assert_eq!(partial["chunks"].as_u64(), Some(1));
            }
            other => panic!("the first chunk: {other:?}"),
        }
        cancel.store(true, Ordering::Relaxed);
        assert!(matches!(
            stage.chunk(&task, &cancel, &host).unwrap(),
            Work::Stopped
        ));
        assert_eq!(
            fake.diarizes().len(),
            1,
            "a stopped task asks the engine for nothing"
        );
        cancel.store(false, Ordering::Relaxed);
        let result = run_to_end(&stage, &mut task, &host, &cancel);
        assert_eq!(
            fake.diarizes().len(),
            ranges.len(),
            "the finished chunk is not diarized again"
        );
        assert_eq!(result["chunks"].as_u64(), Some(4));
        assert_eq!(
            result["reused"].as_u64(),
            Some(4),
            "the resumed run reused the chunks of the cursor"
        );
        let resumed = Transcript::open(&ai, &id).unwrap().dialog().unwrap();
        assert_eq!(
            resumed, uninterrupted,
            "a stop and a restart change nothing in the replies"
        );
    }

    #[test]
    #[ignore]
    fn diarize_a_long_video_in_chunks_across_a_restart() {
        use std::time::{Duration, Instant};
        let video = Path::new(
            "C:/shared-data/reps/hh-hack/source-1/videos/Как подтвердить опыт без трудовой ⧸ Хабр против работяг [1280x720].mp4",
        );
        assert!(video.is_file(), "{}: not found", video.display());
        let duration = mediaa_media::info::probe(video)
            .expect("probing the video")
            .duration_secs()
            .unwrap_or(1620.0);
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let id = transcript::file_id(video).unwrap();
        {
            let mut transcript = Transcript::open(&ai, &id).unwrap();
            let phrases: Vec<mediaa_core::transcript::Phrase> = (0..(duration / 5.0).max(1.0)
                as usize)
                .map(|index| {
                    let start = (index * 5) as f64;
                    let end = (start + 5.0).min(duration);
                    mediaa_core::transcript::Phrase {
                        start,
                        end,
                        words: vec![mediaa_core::transcript::Word {
                            text: format!("w{index}"),
                            start: start + 0.5,
                            end: end - 0.5,
                        }],
                    }
                })
                .collect();
            transcript.put_phrases(0.0, &phrases).unwrap();
        }

        let settings = AgentSettings::default();
        let open = || {
            let sched = crate::run::open_scheduler(&ai, &settings.budget).unwrap();
            let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> =
                Arc::new(AgentSettings::default);
            crate::run::register_stages(
                &sched,
                &ai,
                crate::live::engine(),
                crate::slots::Slots::new(settings.budget.clone()),
                current,
                Arc::new(crate::run::Streams::default()),
            );
            sched
        };
        let sched = open();
        let task = sched
            .enqueue(
                mediaa_queue::NewTask::new(KIND, "chunked diarization", &json!({ "path": video }))
                    .resource(mediaa_queue::Resource::Speech),
            )
            .unwrap();
        let started = Instant::now();
        let limit = Duration::from_secs(45 * 60);
        let chunks_of = |sched: &mediaa_queue::Scheduler| {
            sched
                .store()
                .get(task)
                .unwrap()
                .unwrap()
                .cursor
                .get("chunks")
                .and_then(serde_json::Value::as_array)
                .map_or(0, Vec::len)
        };
        let (chunks_at_pause, calls_at_pause) = loop {
            assert!(started.elapsed() < limit, "no second chunk in 45 minutes");
            sched.tick().unwrap();
            if chunks_of(&sched) >= 2 {
                sched.pause(task).unwrap();
                loop {
                    assert!(started.elapsed() < limit, "the pause did not settle");
                    sched.tick().unwrap();
                    if sched.store().get(task).unwrap().unwrap().status
                        == mediaa_queue::Status::Paused
                    {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(200));
                }
                let paused = sched.store().get(task).unwrap().unwrap();
                break (chunks_of(&sched), paused.stats.model_calls);
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        println!(
            "paused after {chunks_at_pause} chunks and {calls_at_pause} engine calls, {:.1} s after the start",
            started.elapsed().as_secs_f64()
        );
        assert!(chunks_at_pause >= 2);
        assert!(
            calls_at_pause <= chunks_at_pause as u64 + 1,
            "{calls_at_pause} calls for {chunks_at_pause} chunks"
        );
        drop(sched);

        let sched = open();
        assert!(sched.resume(task).unwrap());
        let resumed_at = Instant::now();
        crate::run::drain(&sched, limit);
        let done = sched.store().get(task).unwrap().unwrap();
        let chunks = chunks_of(&sched);
        let calls = done.stats.model_calls;
        println!(
            "resumed at {:.1} s: chunks {chunks_at_pause} → {chunks}, engine calls {calls_at_pause} → {calls}, {:.1} s after the resume",
            resumed_at.duration_since(started).as_secs_f64(),
            resumed_at.elapsed().as_secs_f64()
        );
        println!("result: {:?}", done.result);
        assert_eq!(done.status, mediaa_queue::Status::Done, "{:?}", done.error);
        assert!(chunks > chunks_at_pause, "the recording is finished");
        assert_eq!(
            calls,
            chunks as u64 + calls_at_pause - chunks_at_pause as u64,
            "only the chunk lost to the pause is diarized twice"
        );
        assert_eq!(
            done.result
                .as_ref()
                .and_then(|result| result["reused"].as_u64()),
            Some(chunks as u64)
        );
    }
}
