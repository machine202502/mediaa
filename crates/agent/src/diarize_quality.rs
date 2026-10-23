use crate::transcribe::SAMPLE_RATE;
use anyhow::Context;
use mediaa_core::agent::AgentSettings;
use mediaa_core::transcript::Turn;
use mediaa_engine::{Diarization, DiarizeRequest, Engine};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub const COLLAR_SECONDS: f64 = 0.25;
const FRAME_SECONDS: f64 = 0.01;
const CLONE_MIN_SECONDS: f64 = 2.0;
const CLONE_MIN_SHARE: f64 = 0.10;

#[derive(Debug, Clone, PartialEq)]
pub struct RefTurn {
    pub start: f64,
    pub end: f64,
    pub speaker: String,
}

pub fn parse_rttm(text: &str) -> anyhow::Result<Vec<RefTurn>> {
    let mut turns = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.first() != Some(&"SPEAKER") {
            continue;
        }
        let line_no = index + 1;
        anyhow::ensure!(fields.len() >= 8, "RTTM line {line_no}: expected at least 8 fields, got {}", fields.len());
        let start: f64 = fields[3].parse().with_context(|| format!("RTTM line {line_no}: start {:?}", fields[3]))?;
        let duration: f64 = fields[4].parse().with_context(|| format!("RTTM line {line_no}: duration {:?}", fields[4]))?;
        anyhow::ensure!(
            start.is_finite() && duration.is_finite() && start >= 0.0 && duration > 0.0,
            "RTTM line {line_no}: bad start {start} or duration {duration}"
        );
        turns.push(RefTurn { start, end: start + duration, speaker: fields[7].to_string() });
    }
    turns.sort_by(|a, b| a.start.total_cmp(&b.start).then_with(|| a.speaker.cmp(&b.speaker)));
    Ok(turns)
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Der {
    pub score: f64,
    pub missed: f64,
    pub false_alarm: f64,
    pub speaker_error: f64,
    pub reference: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Clone {
    pub reference: String,
    pub found: Vec<u32>,
}

impl std::fmt::Display for Clone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}→{:?}", self.reference, self.found)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Scores {
    pub der: Der,
    pub clones: Vec<Clone>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Reference {
    pub turns: Option<Vec<RefTurn>>,
    pub speakers: Option<usize>,
}

impl Reference {
    pub fn read(dir: &Path) -> anyhow::Result<Reference> {
        let rttm = dir.join("speakers.rttm");
        if rttm.is_file() {
            let text = std::fs::read_to_string(&rttm).with_context(|| rttm.display().to_string())?;
            let turns = parse_rttm(&text)?;
            let speakers = turns.iter().map(|turn| turn.speaker.as_str()).collect::<BTreeSet<_>>().len();
            return Ok(Reference { turns: Some(turns), speakers: Some(speakers) });
        }
        let json = dir.join("reference.json");
        if json.is_file() {
            let text = std::fs::read_to_string(&json).with_context(|| json.display().to_string())?;
            let value: serde_json::Value = serde_json::from_str(&text).with_context(|| json.display().to_string())?;
            let speakers = value.get("speakers").and_then(serde_json::Value::as_u64).map(|n| n as usize);
            return Ok(Reference { turns: None, speakers });
        }
        Ok(Reference::default())
    }
}

pub fn evaluate(reference: &[RefTurn], hypothesis: &[Turn]) -> anyhow::Result<Scores> {
    let raster = Raster::new(reference, hypothesis)?;
    Ok(Scores { der: raster.der(), clones: raster.clones() })
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recording {
    pub name: String,
    pub audio_seconds: f64,
    pub turns: usize,
    pub found: usize,
    pub expected: Option<usize>,
    pub der: Option<Der>,
    pub clones: Vec<Clone>,
    pub decode_seconds: f64,
    pub diarize_seconds: f64,
}

impl std::fmt::Display for Recording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let speakers = match self.expected {
            Some(expected) if expected == self.found => format!("{} of {} speakers", self.found, expected),
            Some(expected) => format!("{} of {} speakers ({:+})", self.found, expected, self.found as i64 - expected as i64),
            None => format!("{} speakers (speaker count unknown)", self.found),
        };
        let der = match &self.der {
            Some(der) => format!(
                "DER {:.1}% (missed {:.1} s, false alarm {:.1} s, confusion {:.1} s)",
                der.score * 100.0,
                der.missed,
                der.false_alarm,
                der.speaker_error
            ),
            None => "DER — (no timed reference)".to_string(),
        };
        let clones = if self.der.is_none() {
            "clones —".to_string()
        } else if self.clones.is_empty() {
            "clones none".to_string()
        } else {
            format!("clones {}", self.clones.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))
        };
        write!(
            f,
            "{}: {} · {} · {} · {} turns · {:.1} s audio, {:.1} s decode, {:.1} s diarize",
            self.name, speakers, der, clones, self.turns, self.audio_seconds, self.decode_seconds, self.diarize_seconds
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Run {
    pub model: Option<String>,
    pub clustering_threshold: Option<f32>,
    pub merge_threshold: Option<f32>,
    pub chunk_seconds: Option<f64>,
    pub overlap_seconds: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Found {
    pub name: String,
    pub audio_seconds: f64,
    pub decode_seconds: f64,
    pub diarize_seconds: f64,
    pub reference: Reference,
    pub diarization: Diarization,
}

pub fn run_recording(engine: &dyn Engine, settings: &AgentSettings, audio: &Path, options: &Run) -> anyhow::Result<Found> {
    let (found, _) = run_recording_chunks(engine, settings, audio, options)?;
    Ok(found)
}

pub(crate) fn run_recording_chunks(
    engine: &dyn Engine,
    settings: &AgentSettings,
    audio: &Path,
    options: &Run,
) -> anyhow::Result<(Found, Vec<crate::diarize::Chunk>)> {
    let name = audio
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("?")
        .to_string();
    let reference = Reference::read(audio.parent().unwrap_or(Path::new(".")))?;

    let started = Instant::now();
    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let sink = Arc::clone(&samples);
    let cancel = Arc::new(AtomicBool::new(false));
    mediaa_media::preview::decode_mono(audio, 0.0, SAMPLE_RATE, cancel, move |chunk| {
        sink.lock().expect("samples lock").extend_from_slice(chunk);
        true
    })
    .with_context(|| format!("decoding {}", audio.display()))?;
    let samples = std::mem::take(&mut *samples.lock().expect("samples lock"));
    let audio_seconds = samples.len() as f64 / f64::from(SAMPLE_RATE);
    let decode_seconds = started.elapsed().as_secs_f64();

    let model = match &options.model {
        Some(model) => model.clone(),
        None => {
            let source = settings
                .models
                .diarization
                .as_ref()
                .context("no diarization model is chosen in the settings")?;
            crate::models::local_spec(source)?
        }
    };
    let started = Instant::now();
    let mut chunks = Vec::new();
    let diarization = match options.chunk_seconds {
        Some(chunk_seconds) => {
            let overlap = options.overlap_seconds.unwrap_or(crate::diarize::OVERLAP_SECONDS);
            let ranges = crate::diarize::plan(audio_seconds, chunk_seconds, overlap);
            chunks.reserve(ranges.len());
            for (start, end) in ranges {
                let low = (start * f64::from(SAMPLE_RATE)).round() as usize;
                let high = ((end * f64::from(SAMPLE_RATE)).round() as usize).min(samples.len());
                let request = DiarizeRequest {
                    segmentation: crate::diarize::SEGMENTATION_MODEL.to_string(),
                    embedding: model.clone(),
                    speakers: None,
                    clustering_threshold: options.clustering_threshold,
                    merge_threshold: None,
                    min_speaker_seconds: None,
                    hf_token: crate::assistant::hf_token(settings),
                };
                let found = engine.diarize(&request, &samples[low..high]).with_context(|| format!("diarizing {}", audio.display()))?;
                chunks.push(crate::diarize::Chunk::new(start, end, found));
            }
            crate::diarize::link(
                &chunks,
                options.merge_threshold.unwrap_or(mediaa_engine::speech::DEFAULT_MERGE_THRESHOLD),
                crate::diarize::LINK_MIN_SECONDS,
                None,
            )
        }
        None => {
            let request = DiarizeRequest {
                segmentation: crate::diarize::SEGMENTATION_MODEL.to_string(),
                embedding: model,
                speakers: None,
                clustering_threshold: options.clustering_threshold,
                merge_threshold: options.merge_threshold,
                min_speaker_seconds: Some(mediaa_engine::speech::MIN_SPEAKER_SECONDS),
                hf_token: crate::assistant::hf_token(settings),
            };
            engine.diarize(&request, &samples).with_context(|| format!("diarizing {}", audio.display()))?
        }
    };
    let diarize_seconds = started.elapsed().as_secs_f64();
    Ok((Found { name, audio_seconds, decode_seconds, diarize_seconds, reference, diarization }, chunks))
}

pub fn score(found: &Found) -> anyhow::Result<Recording> {
    let turns = &found.diarization.turns;
    let speakers = turns.iter().map(|turn| turn.speaker).collect::<BTreeSet<_>>().len();
    let scores = match &found.reference.turns {
        Some(reference) => Some(evaluate(reference, turns)?),
        None => None,
    };
    Ok(Recording {
        name: found.name.clone(),
        audio_seconds: found.audio_seconds,
        turns: turns.len(),
        found: speakers,
        expected: found.reference.speakers,
        der: scores.as_ref().map(|scores| scores.der),
        clones: scores.map(|scores| scores.clones).unwrap_or_default(),
        decode_seconds: found.decode_seconds,
        diarize_seconds: found.diarize_seconds,
    })
}

pub fn measure_recording(engine: &dyn Engine, settings: &AgentSettings, audio: &Path) -> anyhow::Result<Recording> {
    let found = run_recording(
        engine,
        settings,
        audio,
        &Run {
            merge_threshold: Some(crate::diarize::LINK_THRESHOLD),
            chunk_seconds: Some(crate::diarize::CHUNK_SECONDS),
            ..Run::default()
        },
    )?;
    score(&found)
}

pub fn measure_all(engine: &dyn Engine, settings: &AgentSettings, data: &Path) -> anyhow::Result<Vec<Recording>> {
    anyhow::ensure!(data.is_dir(), "{}: no such data folder", data.display());
    let mut folders: Vec<PathBuf> = std::fs::read_dir(data)
        .with_context(|| data.display().to_string())?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.join("audio.opus").is_file())
        .collect();
    folders.sort();
    anyhow::ensure!(!folders.is_empty(), "{}: no recordings with audio.opus", data.display());
    folders
        .iter()
        .map(|folder| measure_recording(engine, settings, &folder.join("audio.opus")))
        .collect()
}

struct Raster {
    reference: Vec<u128>,
    hypothesis: Vec<u128>,
    reference_speakers: Vec<String>,
    hypothesis_speakers: Vec<u32>,
    frames: usize,
    boundaries: Vec<f64>,
}

impl Raster {
    fn new(reference: &[RefTurn], hypothesis: &[Turn]) -> anyhow::Result<Raster> {
        let reference_speakers: Vec<String> = reference.iter().map(|turn| turn.speaker.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        let hypothesis_speakers: Vec<u32> = hypothesis.iter().map(|turn| turn.speaker).collect::<BTreeSet<_>>().into_iter().collect();
        anyhow::ensure!(reference_speakers.len() <= 128 && hypothesis_speakers.len() <= 128, "too many speakers to measure");
        let duration = reference
            .iter()
            .map(|turn| turn.end)
            .chain(hypothesis.iter().map(|turn| turn.end))
            .fold(0.0_f64, f64::max);
        anyhow::ensure!(duration.is_finite() && duration >= 0.0, "bad recording duration {duration}");
        let frames = (duration / FRAME_SECONDS).ceil() as usize;
        let mut raster = Raster {
            reference: vec![0; frames],
            hypothesis: vec![0; frames],
            reference_speakers,
            hypothesis_speakers,
            frames,
            boundaries: reference.iter().flat_map(|turn| [turn.start, turn.end]).collect(),
        };
        for turn in reference {
            anyhow::ensure!(
                turn.start.is_finite() && turn.end.is_finite() && turn.start >= 0.0 && turn.end > turn.start,
                "bad reference turn {:?}",
                turn
            );
            let speaker = raster.reference_speakers.binary_search(&turn.speaker).expect("collected from the same turns");
            paint(&mut raster.reference, turn.start, turn.end, speaker);
        }
        for turn in hypothesis {
            anyhow::ensure!(
                turn.start.is_finite() && turn.end.is_finite() && turn.start >= 0.0 && turn.end > turn.start,
                "bad hypothesis turn {turn:?}"
            );
            let speaker = raster.hypothesis_speakers.binary_search(&turn.speaker).expect("collected from the same turns");
            paint(&mut raster.hypothesis, turn.start, turn.end, speaker);
        }
        Ok(raster)
    }

    fn der(&self) -> Der {
        let counted = self.counted();
        let mut counts = vec![vec![0u64; self.hypothesis_speakers.len()]; self.reference_speakers.len()];
        let (mut total, mut missed, mut false_alarm, mut both) = (0u64, 0u64, 0u64, 0u64);
        for (frame, included) in counted.iter().enumerate() {
            if !included {
                continue;
            }
            let reference = self.reference[frame];
            let hypothesis = self.hypothesis[frame];
            let (ref_speakers, hyp_speakers) = (reference.count_ones(), hypothesis.count_ones());
            total += u64::from(ref_speakers);
            if ref_speakers == 0 {
                false_alarm += u64::from(hyp_speakers);
            } else if hyp_speakers == 0 {
                missed += u64::from(ref_speakers);
            } else {
                both += u64::from(ref_speakers);
                for r in indices(reference) {
                    for h in indices(hypothesis) {
                        counts[r][h] += 1;
                    }
                }
            }
        }
        let matched: u64 = best_mapping(&counts).iter().enumerate().filter_map(|(r, h)| h.map(|h| counts[r][h])).sum();
        let speaker_error = both.saturating_sub(matched);
        let frame = FRAME_SECONDS;
        let reference = total as f64 * frame;
        Der {
            score: if reference > 0.0 { (missed + false_alarm + speaker_error) as f64 * frame / reference } else { 0.0 },
            missed: missed as f64 * frame,
            false_alarm: false_alarm as f64 * frame,
            speaker_error: speaker_error as f64 * frame,
            reference,
        }
    }

    fn clones(&self) -> Vec<Clone> {
        let mut counts = vec![vec![0u64; self.hypothesis_speakers.len()]; self.reference_speakers.len()];
        let mut totals = vec![0u64; self.reference_speakers.len()];
        for frame in 0..self.frames {
            for r in indices(self.reference[frame]) {
                totals[r] += 1;
                for h in indices(self.hypothesis[frame]) {
                    counts[r][h] += 1;
                }
            }
        }
        let mut clones = Vec::new();
        for (r, total) in totals.iter().enumerate() {
            let least = CLONE_MIN_SECONDS.max(CLONE_MIN_SHARE * *total as f64 * FRAME_SECONDS);
            let found: Vec<u32> = (0..self.hypothesis_speakers.len())
                .filter(|&h| counts[r][h] as f64 * FRAME_SECONDS >= least)
                .map(|h| self.hypothesis_speakers[h])
                .collect();
            if found.len() >= 2 {
                clones.push(Clone { reference: self.reference_speakers[r].clone(), found });
            }
        }
        clones
    }

    fn counted(&self) -> Vec<bool> {
        let mut counted = vec![true; self.frames];
        if COLLAR_SECONDS <= 0.0 {
            return counted;
        }
        for boundary in &self.boundaries {
            let low = ((boundary - COLLAR_SECONDS) / FRAME_SECONDS - 0.5).ceil().max(0.0) as usize;
            let high = ((boundary + COLLAR_SECONDS) / FRAME_SECONDS - 0.5).floor();
            if high >= 0.0 {
                let high = (high as usize).min(self.frames.saturating_sub(1));
                for frame in counted.iter_mut().take(high + 1).skip(low) {
                    *frame = false;
                }
            }
        }
        counted
    }
}

fn paint(masks: &mut [u128], start: f64, end: f64, speaker: usize) {
    let low = (start / FRAME_SECONDS).ceil().clamp(0.0, masks.len() as f64) as usize;
    let high = (end / FRAME_SECONDS).ceil().clamp(0.0, masks.len() as f64) as usize;
    for mask in masks.iter_mut().take(high).skip(low) {
        *mask |= 1u128 << speaker;
    }
}

fn indices(mut mask: u128) -> impl Iterator<Item = usize> {
    std::iter::from_fn(move || {
        if mask == 0 {
            return None;
        }
        let index = mask.trailing_zeros() as usize;
        mask &= mask - 1;
        Some(index)
    })
}

fn best_mapping(counts: &[Vec<u64>]) -> Vec<Option<usize>> {
    let reference = counts.len();
    let hypothesis = counts.first().map_or(0, Vec::len);
    if reference == 0 || hypothesis == 0 {
        return vec![None; reference];
    }
    if reference <= 8 && hypothesis <= 20 {
        let mut best = (0u64, vec![None; reference]);
        let mut current = vec![None; reference];
        assign(counts, 0, 0, 0, &mut current, &mut best);
        return best.1;
    }
    let mut cells: Vec<(u64, usize, usize)> = counts
        .iter()
        .enumerate()
        .flat_map(|(r, row)| row.iter().enumerate().map(move |(h, &count)| (count, r, h)))
        .collect();
    cells.sort_by_key(|cell| std::cmp::Reverse(cell.0));
    let mut taken = vec![false; hypothesis];
    let mut mapping = vec![None; reference];
    for (count, r, h) in cells {
        if count == 0 {
            break;
        }
        if mapping[r].is_none() && !taken[h] {
            mapping[r] = Some(h);
            taken[h] = true;
        }
    }
    mapping
}

fn assign(counts: &[Vec<u64>], r: usize, used: u64, score: u64, current: &mut Vec<Option<usize>>, best: &mut (u64, Vec<Option<usize>>)) {
    if r == counts.len() {
        if score > best.0 {
            *best = (score, current.clone());
        }
        return;
    }
    for h in 0..counts[r].len() {
        if used & (1u64 << h) == 0 {
            current[r] = Some(h);
            assign(counts, r + 1, used | (1u64 << h), score + counts[r][h], current, best);
        }
    }
    current[r] = None;
    assign(counts, r + 1, used, score, current, best);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(turns: &[(f64, f64, &str)]) -> Vec<RefTurn> {
        turns.iter().map(|&(start, end, speaker)| RefTurn { start, end, speaker: speaker.into() }).collect()
    }

    fn hyps(turns: &[(f64, f64, u32)]) -> Vec<Turn> {
        turns.iter().map(|&(start, end, speaker)| Turn { start, end, speaker }).collect()
    }

    #[test]
    fn rttm_reads_speaker_lines_and_skips_comments_and_other_records() {
        let text = "\
# reference
SPEAKER mix 1 0.000 1.500 <NA> <NA> a <NA> <NA>
SPEAKER mix 1 1.200 2.000 <NA> <NA> b <NA> <NA>
\n";
        let turns = parse_rttm(text).unwrap();
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0], RefTurn { start: 0.0, end: 1.5, speaker: "a".into() });
        assert_eq!(turns[1], RefTurn { start: 1.2, end: 3.2, speaker: "b".into() });
        assert!(parse_rttm("SPEAKER mix 1 0 1\n").is_err());
        assert!(parse_rttm("SPEAKER mix 1 x 1 <NA> <NA> a <NA> <NA>\n").is_err());
        assert!(parse_rttm("SPEAKER mix 1 0 0 <NA> <NA> a <NA> <NA>\n").is_err());
    }

    #[test]
    fn der_counts_missed_false_alarm_and_confusion_without_a_collar() {
        let reference = refs(&[(0.0, 10.0, "a"), (10.0, 20.0, "b")]);
        let hypothesis = hyps(&[(0.0, 8.0, 0), (10.0, 20.0, 1), (20.0, 22.0, 2)]);
        let mut raster = Raster::new(&reference, &hypothesis).unwrap();
        raster.boundaries.clear();
        let der = raster.der();
        assert!((der.missed - 2.0).abs() < 1e-9, "{der:?}");
        assert!((der.false_alarm - 2.0).abs() < 1e-9, "{der:?}");
        assert!((der.reference - 20.0).abs() < 1e-9, "{der:?}");
        assert!((der.speaker_error - 0.0).abs() < 1e-9, "speech with no hypothesis at all is missed, not confused: {der:?}");
        assert!((der.score - 4.0 / 20.0).abs() < 1e-9, "{der:?}");
        assert!(raster.clones().is_empty());
    }

    #[test]
    fn der_does_not_punish_a_permuted_speaker_numbering() {
        let reference = refs(&[(0.0, 10.0, "a"), (10.0, 20.0, "b")]);
        let hypothesis = hyps(&[(0.0, 10.0, 7), (10.0, 20.0, 3)]);
        let mut raster = Raster::new(&reference, &hypothesis).unwrap();
        raster.boundaries.clear();
        let der = raster.der();
        assert!(der.score < 1e-12, "{der:?}");
        assert_eq!(der.reference, 20.0);
    }

    #[test]
    fn a_merged_speaker_is_confusion_not_missed_speech() {
        let reference = refs(&[(0.0, 10.0, "a"), (10.0, 20.0, "b")]);
        let hypothesis = hyps(&[(0.0, 20.0, 0)]);
        let mut raster = Raster::new(&reference, &hypothesis).unwrap();
        raster.boundaries.clear();
        let der = raster.der();
        assert_eq!(der.missed, 0.0);
        assert_eq!(der.false_alarm, 0.0);
        assert!((der.speaker_error - 10.0).abs() < 1e-9, "{der:?}");
        assert!((der.score - 0.5).abs() < 1e-9, "{der:?}");
    }

    #[test]
    fn the_collar_ignores_speech_near_reference_boundaries() {
        let reference = refs(&[(0.0, 10.0, "a")]);
        let hypothesis = hyps(&[(0.1, 9.9, 0)]);
        assert!(evaluate(&reference, &hypothesis).unwrap().der.score < 1e-12, "with the collar");
        let mut raster = Raster::new(&reference, &hypothesis).unwrap();
        raster.boundaries.clear();
        let der = raster.der();
        assert!((der.missed - 0.2).abs() < 0.03, "{der:?}");
    }

    #[test]
    fn overlapping_reference_speech_counts_for_both_speakers() {
        let reference = refs(&[(0.0, 10.0, "a"), (5.0, 15.0, "b")]);
        let hypothesis = hyps(&[(0.0, 15.0, 0)]);
        let mut raster = Raster::new(&reference, &hypothesis).unwrap();
        raster.boundaries.clear();
        let der = raster.der();
        assert!((der.reference - 20.0).abs() < 1e-9, "10 s of each speaker, the 5 s overlap twice: {der:?}");
        assert!((der.speaker_error - 10.0).abs() < 1e-9, "the second speaker has no hypothesis speaker: {der:?}");
        assert!((der.score - 10.0 / 20.0).abs() < 1e-9, "{der:?}");
    }

    #[test]
    fn a_reference_speaker_found_as_two_is_a_clone() {
        let reference = refs(&[(0.0, 20.0, "a")]);
        let hypothesis = hyps(&[(0.0, 10.0, 4), (10.0, 20.0, 9)]);
        let clones = evaluate(&reference, &hypothesis).unwrap().clones;
        assert_eq!(clones, vec![Clone { reference: "a".into(), found: vec![4, 9] }]);
        assert_eq!(clones[0].to_string(), "a→[4, 9]");
        assert!(evaluate(&reference, &hyps(&[(0.0, 20.0, 0)])).unwrap().clones.is_empty());
        assert!(evaluate(&reference, &hyps(&[(0.0, 1.0, 0), (19.0, 20.0, 1)])).unwrap().clones.is_empty());
    }

    #[test]
    fn reference_reads_rttm_first_then_the_speaker_count_json() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Reference::read(dir.path()).unwrap(), Reference::default());
        std::fs::write(dir.path().join("reference.json"), r#"{"speakers": 3, "note": "count only"}"#).unwrap();
        assert_eq!(Reference::read(dir.path()).unwrap(), Reference { turns: None, speakers: Some(3) });
        std::fs::write(
            dir.path().join("speakers.rttm"),
            "SPEAKER x 1 0.0 1.0 <NA> <NA> a <NA> <NA>\nSPEAKER x 1 1.0 1.0 <NA> <NA> b <NA> <NA>\n",
        )
        .unwrap();
        let reference = Reference::read(dir.path()).unwrap();
        assert_eq!(reference.speakers, Some(2));
        assert_eq!(reference.turns.unwrap().len(), 2);
    }

    #[test]
    fn a_recording_line_says_the_count_der_and_time() {
        let recording = Recording {
            name: "mix".into(),
            audio_seconds: 688.5,
            turns: 40,
            found: 5,
            expected: Some(4),
            der: Some(Der { score: 0.047, missed: 1.0, false_alarm: 0.5, speaker_error: 2.0, reference: 660.0 }),
            clones: vec![Clone { reference: "a".into(), found: vec![0, 3] }],
            decode_seconds: 2.0,
            diarize_seconds: 61.0,
        };
        let line = recording.to_string();
        assert!(line.contains("5 of 4 speakers (+1)"), "{line}");
        assert!(line.contains("DER 4.7%"), "{line}");
        assert!(line.contains("clones a→[0, 3]"), "{line}");
        assert!(line.contains("688.5 s audio"), "{line}");
    }
}
