use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

struct Capture;

impl log::Log for Capture {
    fn enabled(&self, _: &log::Metadata) -> bool {
        true
    }

    fn log(&self, record: &log::Record) {
        if record.level() <= log::Level::Error && record.target().starts_with("ez_ffmpeg") {
            LOGS.lock()
                .unwrap()
                .push(format!("{}: {}", record.target(), record.args()));
        }
    }

    fn flush(&self) {}
}

static LOGGER: Capture = Capture;

fn tone(dir: &std::path::Path) -> std::path::PathBuf {
    let wav = dir.join("tone.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for i in 0..48_000 * 3 {
        let v = ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    wav
}

#[test]
fn stopping_the_audio_decoder_early_logs_no_ffmpeg_error() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Trace);
    let dir = tempfile::tempdir().unwrap();
    let wav = tone(dir.path());

    let fed = Arc::new(AtomicUsize::new(0));
    let f = fed.clone();
    mediaa_media::preview::decode_audio(
        &wav,
        0.0,
        1.0,
        48_000,
        Arc::new(AtomicBool::new(false)),
        move |samples| {
            assert!(!samples.is_empty());
            f.fetch_add(1, Ordering::Relaxed);
            false
        },
    )
    .unwrap();
    assert!(
        fed.load(Ordering::Relaxed) > 0,
        "the job never reached the sink"
    );

    let started = Arc::new(AtomicBool::new(false));
    let s = started.clone();
    mediaa_media::preview::decode_audio(
        &wav,
        1.0,
        1.5,
        48_000,
        Arc::new(AtomicBool::new(true)),
        move |_| {
            s.store(true, Ordering::Relaxed);
            true
        },
    )
    .unwrap();
    assert!(
        !started.load(Ordering::Relaxed),
        "a cancelled job must not feed the sink"
    );

    {
        let logs = LOGS.lock().unwrap();
        assert!(
            logs.is_empty(),
            "FFmpeg errors logged while stopping on purpose: {logs:?}"
        );
    }

    let missing = dir.path().join("missing.wav");
    assert!(mediaa_media::preview::decode_audio(
        &missing,
        0.0,
        1.0,
        48_000,
        Arc::new(AtomicBool::new(false)),
        |_| true
    )
    .is_err());
}
