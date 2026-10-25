use mediaa_core::formats::AnimFormat;
use mediaa_core::settings::AnimSettings;
use mediaa_media::animation_backends::{
    self, AnimationEncodeRequest, AnimationOrigin, WebpSegmentation,
};
use mediaa_media::convert::Effort;
use mediaa_media::info::probe;
use mediaa_webp_pipeline::PipelineTuning;
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
    let mode = args.next().unwrap_or_else(|| "both".into());
    let budget_mb: u64 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(512);
    let settings = AnimSettings {
        fps,
        max_height,
        webp_quality: quality,
        ..AnimSettings::default()
    };
    let dir = tempfile::tempdir()?;
    for (origin, label) in match mode.as_str() {
        "direct" => vec![(AnimationOrigin::Conversion, "direct")],
        "segmented" => vec![(AnimationOrigin::Recording, "segmented")],
        _ => vec![
            (AnimationOrigin::Conversion, "direct"),
            (AnimationOrigin::Recording, "segmented"),
        ],
    } {
        let output = dir.path().join(format!("{label}.webp"));
        let segmented = origin == AnimationOrigin::Recording;
        let request = AnimationEncodeRequest {
            input: Path::new(&input),
            output: &output,
            window: None,
            format: AnimFormat::Webp,
            settings: &settings,
            effort: Effort::Balanced,
            origin,
            segmentation: if segmented {
                WebpSegmentation::Segmented
            } else {
                WebpSegmentation::Direct
            },
            scheduling: PipelineTuning {
                memory_bytes: budget_mb * 1024 * 1024,
                max_workers: 8,
                ..PipelineTuning::default()
            },
            threads: None,
            trace_id: None,
        };
        let started = Instant::now();
        let mut saw_percentage = false;
        let result = animation_backends::encode_with(
            animation_backends::backend(AnimFormat::Webp, origin, segmented),
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
