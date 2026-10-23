//! Speech through sherpa-onnx: voice activity (Silero), recognition (GigaAM and other NeMo
//! CTC / transducer models in the sherpa-onnx layout), diarization (pyannote segmentation +
//! speaker embeddings). What they produce is stored by `mediaa_core::transcript`.

use anyhow::Context;
use sherpa_onnx::{
    FastClusteringConfig, OfflineNemoEncDecCtcModelConfig, OfflineRecognizer, OfflineRecognizerConfig, OfflineSpeakerDiarization,
    OfflineSpeakerDiarizationConfig, OfflineSpeakerSegmentationModelConfig, OfflineSpeakerSegmentationPyannoteModelConfig,
    OfflineTransducerModelConfig, SileroVadModelConfig, SpeakerEmbeddingExtractorConfig, VadModelConfig, VoiceActivityDetector,
};
pub use mediaa_core::transcript::{Phrase, Turn, Word};
use std::path::{Path, PathBuf};

/// Sample rate every model here takes (mono).
pub const SAMPLE_RATE: u32 = 16_000;
/// Voice activity model, downloaded with the speech recognition model.
pub const VAD_MODEL: &str = "csukuangfj/vad/silero_vad.onnx";
/// Speaker segmentation model, downloaded with the diarization (speaker embedding) model.
pub const SEGMENTATION_MODEL: &str = "csukuangfj/sherpa-onnx-pyannote-segmentation-3-0/model.onnx";
/// Longest piece of speech recognised at once (GigaAM works best on up to ~25 s).
const MAX_SPEECH_SECONDS: f32 = 20.0;

fn threads() -> i32 {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(1, 8)) as i32
}

fn path_str(p: &Path) -> anyhow::Result<String> {
    p.to_str().map(str::to_string).ok_or_else(|| anyhow::anyhow!("{}: not a UTF-8 path", p.display()))
}

/// The files of a recognition model folder.
#[derive(Debug, Clone, PartialEq)]
pub enum ModelFiles {
    /// NeMo CTC (GigaAM CTC): `model.onnx`.
    Ctc { model: PathBuf, tokens: PathBuf },
    /// NeMo transducer (GigaAM RNN-T): `encoder`, `decoder`, `joiner`.
    Transducer { encoder: PathBuf, decoder: PathBuf, joiner: PathBuf, tokens: PathBuf },
}

/// Finds the model files in `dir`, preferring int8 weights.
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

/// Words from recognised tokens: `▁` starts a word, other tokens continue it. `starts` are the
/// token times from the start of the piece (seconds), `offset` where the piece begins and `end`
/// where it ends in the recording.
pub fn words(tokens: &[String], starts: &[f32], offset: f64, end: f64) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    // After a bare separator token the next piece starts a word.
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
    // A word ends where the next begins, but no word lasts longer than a few seconds of silence.
    for w in &mut words {
        w.end = w.end.min(w.start + 2.0);
    }
    words
}

/// A loaded speech recognition model.
pub struct Recognizer(OfflineRecognizer);

impl Recognizer {
    /// Loads the model in `dir` (see [`model_files`]) on the CPU.
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

    /// Recognises `samples` (16 kHz mono) that start `offset` seconds into the recording.
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

/// Speech found by the voice activity detector: where it starts (seconds) and its samples.
pub struct Speech {
    pub start: f64,
    pub samples: Vec<f32>,
}

/// Voice activity detection over a stream of 16 kHz mono samples.
pub struct Vad {
    inner: VoiceActivityDetector,
    /// Seconds into the recording where the stream began.
    offset: f64,
}

impl Vad {
    /// `offset`: where in the recording the samples fed to it begin.
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

    /// Feeds samples; returns the speech completed so far.
    pub fn push(&mut self, samples: &[f32]) -> Vec<Speech> {
        self.inner.accept_waveform(samples);
        mediaa_core::pulse::beat();
        self.take()
    }

    /// The end of the stream: returns the rest of the speech.
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

/// Who speaks when in `samples` (a whole recording, 16 kHz mono). `speakers`: how many there
/// are when known.
pub fn diarize(segmentation: &Path, embedding: &Path, samples: &[f32], speakers: Option<u32>) -> anyhow::Result<Vec<Turn>> {
    let config = OfflineSpeakerDiarizationConfig {
        segmentation: OfflineSpeakerSegmentationModelConfig {
            pyannote: OfflineSpeakerSegmentationPyannoteModelConfig { model: Some(path_str(segmentation)?), ..Default::default() },
            num_threads: threads(),
            provider: Some("cpu".into()),
            ..Default::default()
        },
        embedding: SpeakerEmbeddingExtractorConfig { model: Some(path_str(embedding)?), num_threads: threads(), ..Default::default() },
        clustering: FastClusteringConfig { num_clusters: speakers.map_or(-1, |n| n as i32), ..Default::default() },
        ..Default::default()
    };
    let at = std::time::Instant::now();
    mediaa_core::pulse::beat();
    let d = OfflineSpeakerDiarization::create(&config).ok_or_else(|| anyhow::anyhow!("the diarization models do not load"))?;
    log::info!("diarize: the models loaded in {:.1} s", at.elapsed().as_secs_f32());
    let at = std::time::Instant::now();
    // The whole job is one native call: a beat before it starts (and the speech stale limit is
    // wide enough for a long recording — see the queue's per-resource limits).
    mediaa_core::pulse::beat();
    let result = d.process(samples).ok_or_else(|| anyhow::anyhow!("diarization failed"))?;
    mediaa_core::pulse::beat();
    log::info!("diarize: segmentation, embeddings and clustering took {:.1} s", at.elapsed().as_secs_f32());
    Ok(result
        .sort_by_start_time()
        .into_iter()
        .map(|s| Turn { start: f64::from(s.start), end: f64::from(s.end), speaker: s.speaker.max(0) as u32 })
        .collect())
}

/// Checks that a speaker embedding model loads.
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
        // Character models without word marks: a space token separates words.
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

    /// Downloads GigaAM v3 (~225 MB) and the VAD into `MEDIAA_TEST_AI` (or a temporary folder)
    /// and transcribes the model's sample. Network: `cargo test -p mediaa-engine -- --ignored`.
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

    /// Diarizes a two-speaker sample with the default speaker model. Network, see above.
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
        let turns = diarize(&segmentation, &embedding, wave.samples(), None).unwrap();
        eprintln!("{turns:?}");
        let speakers: std::collections::BTreeSet<u32> = turns.iter().map(|t| t.speaker).collect();
        assert_eq!(speakers.len(), 2, "{turns:?}");
        let known = diarize(&segmentation, &embedding, wave.samples(), Some(2)).unwrap();
        assert_eq!(known.iter().map(|t| t.speaker).collect::<std::collections::BTreeSet<_>>().len(), 2);
    }

    #[test]
    fn linked() {
        assert!(sherpa_onnx::version().starts_with("1."));
    }
}
