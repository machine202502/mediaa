use anyhow::Context;
use sherpa_onnx::{
    FastClusteringConfig, OfflineNemoEncDecCtcModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineSpeakerDiarization,
    OfflineSpeakerDiarizationConfig, OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    OfflineTransducerModelConfig, SileroVadModelConfig, SpeakerEmbeddingExtractorConfig, VadModelConfig, VoiceActivityDetector,
};
pub use mediaa_core::transcript::{Phrase, Turn, Word};
use crate::engine::{Diarization, Voice};
use std::path::{Path, PathBuf};

pub const SAMPLE_RATE: u32 = 16_000;
pub const DEFAULT_CLUSTERING_THRESHOLD: f32 = 0.4;
pub const DEFAULT_MERGE_THRESHOLD: f32 = 0.80;
pub const MIN_SPEAKER_SECONDS: f64 = 2.0;
pub const VAD_MODEL: &str = "csukuangfj/vad/silero_vad.onnx";
pub const SEGMENTATION_MODEL: &str = "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/model.onnx";
const MAX_SPEECH_SECONDS: f32 = 20.0;

fn threads() -> i32 {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 8)) as i32
}

fn path_str(p: &Path) -> anyhow::Result<String> {
    p.to_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("{}: not a UTF-8 path", p.display()))
}

#[derive(Debug, Clone, PartialEq)]
pub enum ModelFiles {
    Ctc { model: PathBuf, tokens: PathBuf },
    Transducer { encoder: PathBuf, decoder: PathBuf, joiner: PathBuf, tokens: PathBuf },
}

pub fn model_files(dir: &Path) -> anyhow::Result<ModelFiles> {
    anyhow::ensure!(dir.is_dir(), "{}: a speech recognition model is a folder (sherpa-onnx layout)", dir.display());
    let tokens = dir.join("tokens.txt");
    anyhow::ensure!(tokens.is_file(), "{}: no tokens.txt (not a sherpa-onnx model)", dir.display());
    let mut onnx: Vec<String> = std::fs::read_dir(dir)?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| n.to_ascii_lowercase().ends_with(".onnx"))
        .collect();
    onnx.sort();
    let pick = |stem: &str| {
        let named: Vec<&String> = onnx.iter().filter(|n| n.to_ascii_lowercase().starts_with(stem)).collect();
        named.iter().find(|n| n.contains("int8")).or(named.first()).map(|n| dir.join(n))
    };
    if let (Some(encoder), Some(decoder), Some(joiner)) = (pick("encoder"), pick("decoder"), pick("joiner")) {
        return Ok(ModelFiles::Transducer { encoder, decoder, joiner, tokens });
    }
    match pick("model") {
        Some(model) => Ok(ModelFiles::Ctc { model, tokens }),
        None => anyhow::bail!("{}: no model.onnx or encoder / decoder / joiner", dir.display()),
    }
}

pub fn words(tokens: &[String], starts: &[f32], offset: f64, end: f64) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut separated = true;
    for (i, token) in tokens.iter().enumerate() {
        let at = offset + f64::from(starts.get(i).copied().unwrap_or(0.0));
        let new_word = separated || token.starts_with(['▁', ' ']);
        let piece = token.trim_start_matches(['▁', ' ']);
        if new_word {
            if let Some(last) = words.last_mut() {
                last.end = last.end.max(at);
            }
        }
        separated = piece.is_empty();
        match words.last_mut() {
            _ if piece.is_empty() => {}
            Some(last) if !new_word => last.text.push_str(piece),
            _ => words.push(Word { text: piece.to_string(), start: at, end: at }),
        }
    }
    if let Some(last) = words.last_mut() {
        last.end = end.max(last.start);
    }
    for w in &mut words {
        w.end = w.end.min(w.start + 2.0);
    }
    words
}

pub struct Recognizer(OfflineRecognizer);

impl Recognizer {
    pub fn load(dir: &Path) -> anyhow::Result<Recognizer> {
        let mut config = OfflineRecognizerConfig::default();
        let m = &mut config.model_config;
        match model_files(dir)? {
            ModelFiles::Ctc { model, tokens } => {
                m.nemo_ctc = OfflineNemoEncDecCtcModelConfig { model: Some(path_str(&model)?) };
                m.tokens = Some(path_str(&tokens)?);
            }
            ModelFiles::Transducer { encoder, decoder, joiner, tokens } => {
                m.transducer = OfflineTransducerModelConfig { encoder: Some(path_str(&encoder)?), decoder: Some(path_str(&decoder)?), joiner: Some(path_str(&joiner)?) };
                m.tokens = Some(path_str(&tokens)?);
                m.model_type = Some("nemo_transducer".into());
            }
        }
        m.num_threads = threads();
        m.provider = Some("cpu".into());
        config.decoding_method = Some("greedy_search".into());
        let r = OfflineRecognizer::create(&config).ok_or_else(|| anyhow::anyhow!("{}: the speech recognition model does not load", dir.display()))?;
        Ok(Recognizer(r))
    }

    pub fn recognize(&self, samples: &[f32], offset: f64) -> anyhow::Result<Phrase> {
        let stream = self.0.create_stream();
        stream.accept_waveform(SAMPLE_RATE as i32, samples);
        mediaa_core::pulse::beat();
        self.0.decode(&stream);
        mediaa_core::pulse::beat();
        let result = stream.get_result().context("no recognition result")?;
        let end = offset + samples.len() as f64 / f64::from(SAMPLE_RATE);
        let starts = result.timestamps.unwrap_or_default();
        Ok(Phrase { start: offset, end, words: words(&result.tokens, &starts, offset, end) })
    }
}

pub struct Speech {
    pub start: f64,
    pub samples: Vec<f32>,
}

pub struct Vad {
    inner: VoiceActivityDetector,
    offset: f64,
}

impl Vad {
    pub fn load(model: &Path, offset: f64) -> anyhow::Result<Vad> {
        let config = VadModelConfig {
            silero_vad: SileroVadModelConfig {
                model: Some(path_str(model)?),
                threshold: 0.5,
                min_silence_duration: 0.5,
                min_speech_duration: 0.25,
                window_size: 512,
                max_speech_duration: MAX_SPEECH_SECONDS,
            },
            sample_rate: SAMPLE_RATE as i32,
            num_threads: 1,
            provider: Some("cpu".into()),
            ..VadModelConfig::default()
        };
        let inner = VoiceActivityDetector::create(&config, 60.0).ok_or_else(|| anyhow::anyhow!("{}: the voice activity model does not load", model.display()))?;
        Ok(Vad { inner, offset })
    }

    pub fn push(&mut self, samples: &[f32]) -> Vec<Speech> {
        self.inner.accept_waveform(samples);
        mediaa_core::pulse::beat();
        self.take()
    }

    pub fn finish(&mut self) -> Vec<Speech> {
        self.inner.flush();
        mediaa_core::pulse::beat();
        self.take()
    }

    fn take(&mut self) -> Vec<Speech> {
        let mut out = Vec::new();
        while let Some(s) = self.inner.front() {
            out.push(Speech { start: self.offset + f64::from(s.start()) / f64::from(SAMPLE_RATE), samples: s.samples().to_vec() });
            self.inner.pop();
        }
        out
    }
}

pub fn diarize(
    segmentation: &Path,
    embedding: &Path,
    samples: &[f32],
    speakers: Option<u32>,
    clustering_threshold: Option<f32>,
) -> anyhow::Result<Diarization> {
    let config = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig { model: Some(path_str(segmentation)?), ..Default::default() },
            num_threads: threads(),
            provider: Some("cpu".into()),
            ..Default::default()
        },
        embedding: SpeakerEmbeddingExtractorConfig { model: Some(path_str(embedding)?), num_threads: threads(), ..Default::default() },
        clustering: FastClusteringConfig {
            num_clusters: speakers.map_or(-1, |n| n as i32),
            threshold: clustering_threshold.unwrap_or(DEFAULT_CLUSTERING_THRESHOLD),
            ..Default::default()
        },
        ..Default::default()
    };
    let at = std::time::Instant::now();
    mediaa_core::pulse::beat();
    let d = OfflineSpeakerDiarization::create(&config).ok_or_else(|| anyhow::anyhow!("the diarization models do not load"))?;
    log::info!("diarize: the models loaded in {:.1} s", at.elapsed().as_secs_f32());
    let at = std::time::Instant::now();
    mediaa_core::pulse::beat();
    let result = d.process(samples).ok_or_else(|| anyhow::anyhow!("diarization failed"))?;
    mediaa_core::pulse::beat();
    log::info!("diarize: segmentation, embeddings and clustering took {:.1} s", at.elapsed().as_secs_f32());
    let mut turns: Vec<Turn> = result
        .sort_by_start_time()
        .into_iter()
        .map(|s| Turn { start: f64::from(s.start), end: f64::from(s.end), speaker: s.speaker.max(0) as u32 })
        .collect();
    let speakers = compact(&mut turns);
    let at = std::time::Instant::now();
    let voices = voices(embedding, samples, &turns, speakers)?;
    log::info!("diarize: the voices of {speakers} speakers took {:.1} s", at.elapsed().as_secs_f32());
    Ok(Diarization { turns, voices })
}

fn compact(turns: &mut [Turn]) -> usize {
    let mut ids = std::collections::HashMap::new();
    for turn in turns {
        let next = ids.len() as u32;
        turn.speaker = *ids.entry(turn.speaker).or_insert(next);
    }
    ids.len()
}

const VOICE_SECONDS: f64 = 30.0;
const VOICE_TURN_SECONDS: f64 = 10.0;

fn voices(embedding: &Path, samples: &[f32], turns: &[Turn], count: usize) -> anyhow::Result<Vec<Voice>> {
    let mut voices = vec![Voice { embedding: Vec::new(), seconds: 0.0 }; count];
    if count == 0 {
        return Ok(voices);
    }
    let mut audio = vec![Vec::<f32>::new(); count];
    for turn in turns {
        let length = (turn.end - turn.start).max(0.0);
        voices[turn.speaker as usize].seconds += length;
        let taken = audio[turn.speaker as usize].len() as f64 / f64::from(SAMPLE_RATE);
        if taken >= VOICE_SECONDS {
            continue;
        }
        let low = (turn.start * f64::from(SAMPLE_RATE)).round().clamp(0.0, samples.len() as f64) as usize;
        let high = (turn.end * f64::from(SAMPLE_RATE)).round().clamp(0.0, samples.len() as f64) as usize;
        let limit = low + (VOICE_TURN_SECONDS.min(VOICE_SECONDS - taken) * f64::from(SAMPLE_RATE)) as usize;
        audio[turn.speaker as usize].extend_from_slice(&samples[low..high.max(low).min(limit)]);
    }
    let config = SpeakerEmbeddingExtractorConfig { model: Some(path_str(embedding)?), num_threads: threads(), ..Default::default() };
    let extractor = sherpa_onnx::SpeakerEmbeddingExtractor::create(&config)
        .ok_or_else(|| anyhow::anyhow!("{}: the speaker model does not load", embedding.display()))?;
    for (voice, audio) in voices.iter_mut().zip(&audio) {
        if audio.is_empty() {
            continue;
        }
        let Some(stream) = extractor.create_stream() else { continue };
        stream.accept_waveform(SAMPLE_RATE as i32, audio);
        mediaa_core::pulse::beat();
        if !extractor.is_ready(&stream) {
            continue;
        }
        voice.embedding = extractor.compute(&stream).and_then(|v| unit(&v)).unwrap_or_default();
        mediaa_core::pulse::beat();
    }
    Ok(voices)
}

fn unit(vector: &[f32]) -> Option<Vec<f32>> {
    let norm = vector.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>().sqrt();
    (norm > 1e-8).then(|| vector.iter().map(|v| (*v as f64 / norm) as f32).collect())
}

fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() || left.is_empty() {
        return -1.0;
    }
    left.iter().zip(right).map(|(a, b)| a * b).sum()
}

struct Cluster {
    embedding: Option<Vec<f32>>,
    seconds: f64,
    members: Vec<usize>,
}

impl Cluster {
    fn new(index: usize, voice: &Voice) -> Cluster {
        Cluster { embedding: unit(&voice.embedding), seconds: voice.seconds, members: vec![index] }
    }

    fn merge(&mut self, other: Cluster) {
        let weight = self.seconds.max(0.0);
        let other_weight = other.seconds.max(0.0);
        if let (Some(mine), Some(theirs)) = (&self.embedding, &other.embedding) {
            let total = weight + other_weight;
            if total > 0.0 {
                let mixed: Vec<f32> = mine
                    .iter()
                    .zip(theirs)
                    .map(|(a, b)| ((f64::from(*a) * weight + f64::from(*b) * other_weight) / total) as f32)
                    .collect();
                self.embedding = unit(&mixed);
            }
        }
        self.seconds += other.seconds;
        self.members.extend(other.members);
    }
}

pub fn merge_voices(turns: &[Turn], voices: &[Voice], threshold: f32, min_seconds: f64) -> (Vec<Turn>, Vec<Voice>) {
    merge(turns, voices, &[], Some(threshold), Some(min_seconds), None)
}

pub fn merge_voices_linked(turns: &[Turn], voices: &[Voice], links: &[(usize, usize)], threshold: f32, min_seconds: f64) -> (Vec<Turn>, Vec<Voice>) {
    merge(turns, voices, links, Some(threshold), Some(min_seconds), None)
}

pub fn merge_voices_to(turns: &[Turn], voices: &[Voice], count: usize) -> (Vec<Turn>, Vec<Voice>) {
    merge(turns, voices, &[], None, None, Some(count))
}

fn merge(
    turns: &[Turn],
    voices: &[Voice],
    links: &[(usize, usize)],
    threshold: Option<f32>,
    min_seconds: Option<f64>,
    target: Option<usize>,
) -> (Vec<Turn>, Vec<Voice>) {
    let mut clusters: Vec<Cluster> = voices.iter().enumerate().map(|(index, voice)| Cluster::new(index, voice)).collect();
    let first_talk = |cluster: &Cluster| {
        cluster
            .members
            .iter()
            .filter_map(|&index| turns.iter().filter(|turn| turn.speaker as usize == index).map(|turn| turn.start).reduce(f64::min))
            .reduce(f64::min)
            .unwrap_or(f64::INFINITY)
    };
    clusters.sort_by(|left, right| first_talk(left).total_cmp(&first_talk(right)));
    for &(a, b) in links {
        let left = clusters.iter().position(|cluster| cluster.members.contains(&a));
        let right = clusters.iter().position(|cluster| cluster.members.contains(&b));
        let (Some(left), Some(right)) = (left, right) else { continue };
        if left == right {
            continue;
        }
        let other = clusters.remove(right.max(left));
        clusters[right.min(left)].merge(other);
    }
    if let Some(threshold) = threshold {
        loop {
            let mut best: Option<(f32, usize, usize)> = None;
            for left in 0..clusters.len() {
                for right in left + 1..clusters.len() {
                    let (Some(a), Some(b)) = (&clusters[left].embedding, &clusters[right].embedding) else { continue };
                    let score = cosine(a, b);
                    if score >= threshold && best.is_none_or(|(best, _, _)| score > best) {
                        best = Some((score, left, right));
                    }
                }
            }
            let Some((_, left, right)) = best else { break };
            let other = clusters.remove(right);
            clusters[left].merge(other);
        }
    }
    if let Some(min_seconds) = min_seconds {
        while clusters.len() > 1 {
            let smallest = (0..clusters.len())
                .filter(|&index| clusters[index].seconds < min_seconds && clusters[index].embedding.is_some())
                .min_by(|&a, &b| clusters[a].seconds.total_cmp(&clusters[b].seconds));
            let Some(smallest) = smallest else { break };
            let embedding = clusters[smallest].embedding.clone().expect("filtered by having one");
            let nearest = (0..clusters.len())
                .filter(|&index| index != smallest)
                .filter_map(|index| clusters[index].embedding.as_ref().map(|other| (cosine(&embedding, other), index)))
                .max_by(|a, b| a.0.total_cmp(&b.0));
            let Some((_, nearest)) = nearest else { break };
            let other = clusters.remove(smallest);
            let target = if smallest < nearest { nearest - 1 } else { nearest };
            clusters[target].merge(other);
        }
    }
    if let Some(target) = target {
        while clusters.len() > target {
            let mut best: Option<(f32, usize, usize)> = None;
            for left in 0..clusters.len() {
                for right in left + 1..clusters.len() {
                    let (Some(a), Some(b)) = (&clusters[left].embedding, &clusters[right].embedding) else { continue };
                    let score = cosine(a, b);
                    if best.is_none_or(|(best, _, _)| score > best) {
                        best = Some((score, left, right));
                    }
                }
            }
            let Some((_, left, right)) = best else { break };
            let other = clusters.remove(right);
            clusters[left].merge(other);
        }
    }
    let mut order: Vec<usize> = (0..clusters.len()).collect();
    let first = |index: usize| turns.iter().filter(|t| clusters[index].members.contains(&(t.speaker as usize))).map(|t| t.start).fold(f64::INFINITY, f64::min);
    order.sort_by(|&a, &b| first(a).total_cmp(&first(b)));
    let mut ids = vec![0u32; clusters.len()];
    for (new, &old) in order.iter().enumerate() {
        ids[old] = new as u32;
    }
    let name = |speaker: u32| {
        let cluster = clusters.iter().position(|c| c.members.contains(&(speaker as usize)));
        cluster.map_or(speaker, |cluster| ids[cluster])
    };
    let turns = turns.iter().map(|turn| Turn { speaker: name(turn.speaker), ..*turn }).collect::<Vec<_>>();
    let voices = order
        .iter()
        .map(|&index| Voice { embedding: clusters[index].embedding.clone().unwrap_or_default(), seconds: clusters[index].seconds })
        .collect();
    (turns, voices)
}

pub fn check_embedding(model: &Path) -> anyhow::Result<()> {
    let config = SpeakerEmbeddingExtractorConfig { model: Some(path_str(model)?), ..Default::default() };
    sherpa_onnx::SpeakerEmbeddingExtractor::create(&config).map(drop).ok_or_else(|| anyhow::anyhow!("{}: the speaker model does not load", model.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn words_from_subword_tokens() {
        let w = words(&toks(&["▁при", "вет", ",", "▁мир", "."]), &[0.0, 0.25, 0.5, 0.75, 0.875], 10.0, 11.0);
        assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["привет,", "мир."]);
        assert_eq!((w[0].start, w[0].end), (10.0, 10.75));
        assert_eq!((w[1].start, w[1].end), (10.75, 11.0));
        assert!(words(&[], &[], 0.0, 1.0).is_empty());
        let w = words(&toks(&["д", "а", " ", "н", "е", "т"]), &[0.0, 0.1, 0.2, 0.3, 0.4, 0.5], 0.0, 0.6);
        assert_eq!(w.iter().map(|w| w.text.as_str()).collect::<Vec<_>>(), ["да", "нет"]);
    }

    #[test]
    fn model_layouts() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        assert!(model_files(d).is_err(), "no tokens");
        std::fs::write(d.join("tokens.txt"), "a 0").unwrap();
        assert!(model_files(d).is_err(), "no model");
        std::fs::write(d.join("model.onnx"), "x").unwrap();
        std::fs::write(d.join("model.int8.onnx"), "x").unwrap();
        assert_eq!(model_files(d).unwrap(), ModelFiles::Ctc { model: d.join("model.int8.onnx"), tokens: d.join("tokens.txt") });
        for f in ["encoder.onnx", "decoder.onnx", "joiner.onnx"] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        assert!(matches!(model_files(d).unwrap(), ModelFiles::Transducer { .. }));
    }

    #[test]
    #[ignore]
    fn transcribes_a_russian_sample() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = std::env::var_os("MEDIAA_TEST_AI").map_or(tmp.path().to_path_buf(), PathBuf::from);
        let repo = mediaa_core::agent::ModelChoices::default().speech_recognition.map(|s| match s {
            mediaa_core::agent::ModelSource::HuggingFace { repo } => repo,
            other => panic!("{other:?}"),
        });
        let model = crate::hub::download(&ai, &repo.unwrap(), None, |_, _| {}).unwrap();
        let vad = crate::hub::download(&ai, VAD_MODEL, None, |_, _| {}).unwrap().join("silero_vad.onnx");
        let wav = ai.join("example.wav");
        if !wav.is_file() {
            let bytes = reqwest::blocking::get("https://huggingface.co/csukuangfj/tmp-files/resolve/main/GigaAM/example.wav").unwrap().bytes().unwrap();
            std::fs::write(&wav, &bytes).unwrap();
        }
        let wave = sherpa_onnx::Wave::read(wav.to_str().unwrap()).unwrap();
        assert_eq!(wave.sample_rate(), SAMPLE_RATE as i32);

        let recognizer = Recognizer::load(&model).unwrap();
        let mut vad = Vad::load(&vad, 0.0).unwrap();
        let mut speech = Vec::new();
        for chunk in wave.samples().chunks(4000) {
            speech.extend(vad.push(chunk));
        }
        speech.extend(vad.finish());
        assert!(!speech.is_empty());
        let phrases: Vec<Phrase> = speech.iter().map(|s| recognizer.recognize(&s.samples, s.start).unwrap()).collect();
        let text = phrases.iter().map(Phrase::text).collect::<Vec<_>>().join(" ");
        eprintln!("{text}");
        for p in &phrases {
            eprintln!("{:.2}-{:.2}: {}", p.start, p.end, p.words.iter().map(|w| format!("{}[{:.2}]", w.text, w.start)).collect::<Vec<_>>().join(" "));
        }
        assert!(text.chars().any(|c| ('а'..='я').contains(&c)), "{text}");
        assert!(text.contains(['.', ',', '?']), "punctuation: {text}");
        let all: Vec<&Word> = phrases.iter().flat_map(|p| &p.words).collect();
        assert!(all.windows(2).all(|w| w[0].start <= w[1].start), "words in time order");
    }

    #[test]
    #[ignore]
    fn tells_two_speakers_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = std::env::var_os("MEDIAA_TEST_AI").map_or(tmp.path().to_path_buf(), PathBuf::from);
        let Some(mediaa_core::agent::ModelSource::HuggingFace { repo }) = mediaa_core::agent::ModelRole::Diarization.default_source() else { panic!() };
        let (_, file) = crate::hub::split_spec(&repo);
        let embedding = crate::hub::download(&ai, &repo, None, |_, _| {}).unwrap().join(file.unwrap());
        let segmentation = crate::hub::download(&ai, SEGMENTATION_MODEL, None, |_, _| {}).unwrap().join("model.onnx");
        check_embedding(&embedding).unwrap();
        let wav = ai.join("two-speakers.wav");
        if !wav.is_file() {
            let url = "https://huggingface.co/csukuangfj/speaker-embedding-models/resolve/main/1-two-speakers-en.wav";
            std::fs::write(&wav, reqwest::blocking::get(url).unwrap().bytes().unwrap()).unwrap();
        }
        let wave = sherpa_onnx::Wave::read(wav.to_str().unwrap()).unwrap();
        assert_eq!(wave.sample_rate(), SAMPLE_RATE as i32);
        let turns = diarize(&segmentation, &embedding, wave.samples(), None, None).unwrap();
        eprintln!("{turns:?}");
        let speakers: std::collections::BTreeSet<u32> = turns.turns.iter().map(|t| t.speaker).collect();
        assert_eq!(speakers.len(), 2, "{turns:?}");
        assert_eq!(turns.voices.len(), 2, "one voice per found speaker: {turns:?}");
        let known = diarize(&segmentation, &embedding, wave.samples(), Some(2), None).unwrap();
        assert_eq!(known.turns.iter().map(|t| t.speaker).collect::<std::collections::BTreeSet<_>>().len(), 2);
    }

    #[test]
    fn voices_merge_by_cosine_and_short_speakers_go_to_the_nearest() {
        let (a, b, c) = (unit(&[1.0, 0.0, 0.0]).unwrap(), unit(&[0.98, 0.2, 0.0]).unwrap(), unit(&[0.2, 0.0, 0.98]).unwrap());
        assert!(cosine(&a, &b) > 0.97);
        assert!(cosine(&a, &c) < 0.25);
        let voices = vec![
            Voice { embedding: a.clone(), seconds: 6.0 },
            Voice { embedding: b.clone(), seconds: 4.0 },
            Voice { embedding: c.clone(), seconds: 0.5 },
        ];
        let turns = vec![
            Turn { start: 0.0, end: 6.0, speaker: 0 },
            Turn { start: 6.0, end: 10.0, speaker: 1 },
            Turn { start: 10.0, end: 10.5, speaker: 2 },
        ];
        let (merged, found) = merge_voices(&turns, &voices, 0.9, 2.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(merged.iter().all(|turn| turn.speaker == 0));
        assert!((found[0].seconds - 10.5).abs() < 1e-9);

        let (merged, found) = merge_voices(&turns, &voices, 0.999, 2.0);
        assert_eq!(found.len(), 2, "0 and 1 stay apart; the short 2 has no voice of its own: {found:?}");
        assert_eq!(merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 1, 0]);
        assert!((merged[2].start - 10.0).abs() < 1e-9, "the turn of the short speaker is kept: {merged:?}");

        let (merged, found) = merge_voices(&turns, &voices, 0.999, 0.0);
        assert_eq!(found.len(), 3);
        assert_eq!(merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 1, 2]);
        assert!(found[2].seconds < 1.0);
    }

    #[test]
    fn merging_renumbers_the_speakers_in_the_order_they_talk() {
        let voices = vec![
            Voice { embedding: vec![1.0, 0.0], seconds: 5.0 },
            Voice { embedding: vec![0.0, 1.0], seconds: 5.0 },
        ];
        let turns = vec![Turn { start: 1.0, end: 6.0, speaker: 1 }, Turn { start: 6.0, end: 11.0, speaker: 0 }];
        let (merged, found) = merge_voices(&turns, &voices, 0.99, 0.0);
        assert_eq!(merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 1]);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].embedding, vec![0.0, 1.0]);
        let (merged, _) = merge_voices(&turns, &voices, 0.5, 0.0);
        assert_eq!(merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 1]);
    }

    #[test]
    fn the_gaps_of_the_clustering_numbering_are_closed_in_the_order_speakers_talk() {
        let mut turns = vec![
            Turn { start: 0.0, end: 1.0, speaker: 9 },
            Turn { start: 1.0, end: 2.0, speaker: 4 },
            Turn { start: 2.0, end: 3.0, speaker: 11 },
            Turn { start: 3.0, end: 4.0, speaker: 9 },
        ];
        assert_eq!(compact(&mut turns), 3);
        assert_eq!(turns.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 1, 2, 0]);
        let voices = vec![
            Voice { embedding: vec![1.0, 0.0], seconds: 2.0 },
            Voice { embedding: vec![0.0, 1.0], seconds: 1.0 },
            Voice { embedding: vec![0.5, 0.5], seconds: 1.0 },
        ];
        let (merged, found) = merge_voices(&turns, &voices, 0.99, 0.0);
        assert_eq!(found.len(), 3, "{found:?}");
        assert_eq!(merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 1, 2, 0]);
    }

    #[test]
    fn merging_does_not_depend_on_how_the_clustering_numbered_the_speakers() {
        let (a, b, c) = (unit(&[1.0, 0.0, 0.0]).unwrap(), unit(&[0.97, 0.24, 0.0]).unwrap(), unit(&[0.0, 0.0, 1.0]).unwrap());
        let turns = vec![
            Turn { start: 0.0, end: 2.0, speaker: 0 },
            Turn { start: 2.0, end: 4.0, speaker: 1 },
            Turn { start: 4.0, end: 6.0, speaker: 2 },
            Turn { start: 6.0, end: 8.0, speaker: 1 },
        ];
        let voices = vec![Voice { embedding: a, seconds: 2.0 }, Voice { embedding: b, seconds: 4.0 }, Voice { embedding: c, seconds: 2.0 }];
        let (merged, found) = merge_voices(&turns, &voices, 0.9, 2.0);
        assert_eq!(merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 0, 1, 0], "{merged:?}");
        assert_eq!(found.len(), 2);
        let swapped = turns.iter().map(|turn| Turn { speaker: [2, 1, 0][turn.speaker as usize], ..*turn }).collect::<Vec<_>>();
        let swapped_voices = vec![voices[2].clone(), voices[1].clone(), voices[0].clone()];
        let (also, also_found) = merge_voices(&swapped, &swapped_voices, 0.9, 2.0);
        assert_eq!(also, merged);
        assert_eq!(also_found.len(), found.len());
        for (left, right) in found.iter().zip(&also_found) {
            assert!((left.seconds - right.seconds).abs() < 1e-9);
            assert!(left.embedding.iter().zip(&right.embedding).all(|(l, r)| (l - r).abs() < 1e-6));
        }
    }

    #[test]
    fn linked_voices_merge_whatever_their_cosine_says() {
        let turns = vec![Turn { start: 0.0, end: 5.0, speaker: 0 }, Turn { start: 5.0, end: 10.0, speaker: 1 }];
        let voices = vec![Voice { embedding: vec![1.0, 0.0], seconds: 5.0 }, Voice { embedding: vec![0.0, 1.0], seconds: 5.0 }];
        let (merged, found) = merge_voices_linked(&turns, &voices, &[(0, 1)], 0.8, 0.0);
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(merged.iter().all(|turn| turn.speaker == 0), "{merged:?}");
        assert!((found[0].seconds - 10.0).abs() < 1e-9);
        let (_, found) = merge_voices_linked(&turns, &voices, &[(7, 9)], 0.8, 0.0);
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn merging_to_a_known_count_takes_the_closest_voices_first() {
        let (a, b, c) = (unit(&[1.0, 0.0]).unwrap(), unit(&[0.9, 0.1]).unwrap(), unit(&[0.0, 1.0]).unwrap());
        let voices = vec![Voice { embedding: a, seconds: 2.0 }, Voice { embedding: b, seconds: 2.0 }, Voice { embedding: c, seconds: 4.0 }];
        let turns = vec![
            Turn { start: 0.0, end: 2.0, speaker: 0 },
            Turn { start: 2.0, end: 4.0, speaker: 1 },
            Turn { start: 4.0, end: 8.0, speaker: 2 },
        ];
        let (merged, found) = merge_voices_to(&turns, &voices, 2);
        assert_eq!(found.len(), 2, "{found:?}");
        assert_eq!(merged.iter().map(|turn| turn.speaker).collect::<Vec<_>>(), [0, 0, 1], "a and b are one speaker");
        assert!((found[0].seconds - 4.0).abs() < 1e-9 && (found[1].seconds - 4.0).abs() < 1e-9);

        let (_, found) = merge_voices_to(&turns, &voices, 3);
        assert_eq!(found.len(), 3);
        let (_, found) = merge_voices_to(&turns, &voices, 9);
        assert_eq!(found.len(), 3);
    }

    #[test]
    fn linked() {
        assert!(sherpa_onnx::version().starts_with("1."));
    }
}
