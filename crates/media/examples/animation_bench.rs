use mediaa_core::formats::AnimFormat;
use mediaa_core::settings::AnimSettings;
use mediaa_media::animation_backends::{self, AnimationEncodeRequest};
use mediaa_media::convert::Effort;
use mediaa_media::info::probe;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    install_logger();
    let mut args = std::env::args().skip(1);
    let input = args.next().expect("input MKV");
    let fps = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(60);
    let max_height = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(720);
    let quality = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(80);
    let settings = AnimSettings {
        fps,
        max_height,
        webp_quality: quality,
        ..AnimSettings::default()
    };
    let dir = tempfile::tempdir()?;
    for (effort, label) in [(Effort::Fastest, "fast"), (Effort::Balanced, "quality")] {
        let output = dir.path().join(format!("{label}.webp"));
        let request = AnimationEncodeRequest {
            input: Path::new(&input),
            output: &output,
            window: None,
            format: AnimFormat::Webp,
            settings: &settings,
            effort,
            threads: None,
        };
        let started = Instant::now();
        let mut saw_percentage = false;
        let result = animation_backends::encode(
            &request,
            &mut |progress| saw_percentage |= progress.is_some(),
            &AtomicBool::new(false),
        );
        let elapsed = started.elapsed().as_secs_f64();
        match result {
            Ok(()) => {
                let size = std::fs::metadata(&output)?.len();
                let info = probe(&output)?;
                println!(
                    "{label}: {elapsed:.3}s, {} bytes, {}x{}, {:.3}s, measured_progress={saw_percentage}",
                    size,
                    info.width,
                    info.height,
                    info.duration_secs().unwrap_or_default()
                );
            }
            Err(error) => println!("{label}: failed after {elapsed:.3}s: {error:#}"),
        }
    }
    Ok(())
}

struct StderrLogger;

static LOGGER: StderrLogger = StderrLogger;

impl log::Log for StderrLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.level() <= log::Level::Warn
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!("{}: {}", record.level(), record.args());
        }
    }

    fn flush(&self) {}
}

fn install_logger() {
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Warn);
}
