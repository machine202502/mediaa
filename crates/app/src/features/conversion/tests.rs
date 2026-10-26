use super::{
    ConversionController, ConversionJob, ConversionOutcome, ConversionTaskResult, Estimate,
};
use crate::jobs::Job;
use mediaa_core::paths::OutputPath;
use mediaa_media::convert::{ConvertSettings, Target};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::Instant;

fn conversion_job(
    output: std::path::PathBuf,
    cancel: Arc<AtomicBool>,
    result: ConversionTaskResult,
) -> ConversionJob {
    ConversionJob {
        output: OutputPath::new(output),
        progress: Arc::new(AtomicI32::new(-1)),
        started: Instant::now(),
        job: Job::spawn("conversion-test", cancel, move || result).unwrap(),
    }
}

fn wait_for_outcome(controller: &mut ConversionController) -> ConversionOutcome {
    let ctx = eframe::egui::Context::default();
    (0..100)
        .find_map(|_| {
            let outcome = controller.poll_job(&ctx);
            if outcome.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            outcome
        })
        .expect("conversion should finish")
}

#[test]
fn conversion_finishes_even_when_the_convert_page_is_not_visible() {
    let output = std::path::PathBuf::from("result.mp4");
    let mut controller = ConversionController {
        job: Some(conversion_job(
            output.clone(),
            Arc::default(),
            ConversionTaskResult::Completed,
        )),
        ..ConversionController::default()
    };
    assert!(controller.is_busy());
    let outcome = wait_for_outcome(&mut controller);
    assert!(matches!(
        outcome,
        ConversionOutcome::Finished(path) if path.as_ref() == output
    ));
    assert!(!controller.is_busy());
    assert_eq!(controller.last, Some(output));
}

#[test]
fn a_panicked_conversion_worker_reports_a_terminal_error() {
    let cancel = Arc::new(AtomicBool::new(false));
    let job = Job::spawn("panic-conversion", cancel, || -> ConversionTaskResult {
        panic!("worker failed")
    })
    .unwrap();
    let mut controller = ConversionController {
        job: Some(ConversionJob {
            output: OutputPath::new("result.mp4"),
            progress: Arc::new(AtomicI32::new(-1)),
            started: Instant::now(),
            job,
        }),
        ..ConversionController::default()
    };
    assert!(matches!(
        wait_for_outcome(&mut controller),
        ConversionOutcome::Failed { .. }
    ));
    assert!(!controller.is_busy());
}

#[test]
fn cancellation_is_a_typed_conversion_outcome() {
    let mut controller = ConversionController {
        job: Some(conversion_job(
            "result.mp4".into(),
            Arc::default(),
            ConversionTaskResult::Cancelled,
        )),
        ..ConversionController::default()
    };
    assert!(matches!(
        wait_for_outcome(&mut controller),
        ConversionOutcome::Cancelled
    ));
    assert!(!controller.is_busy());
}

#[test]
fn canceling_an_estimate_signals_its_owned_job() {
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let job = Job::spawn("estimate-test", cancel.clone(), move || {
        while !worker_cancel.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(0)
    })
    .unwrap();
    let mut controller = ConversionController {
        estimate: Some(Estimate {
            key: (
                std::path::PathBuf::from("source.mp4"),
                Target::Video(mediaa_media::convert::VideoFormat::Mp4),
                ConvertSettings::default(),
            ),
            since: Instant::now(),
            cancel: cancel.clone(),
            job: Some(job),
            result: None,
            previous: None,
            started: Some(Instant::now()),
        }),
        ..ConversionController::default()
    };
    controller.cancel_estimate();
    assert!(controller.estimate.is_none());
    assert!(cancel.load(Ordering::Relaxed));
}

#[test]
fn selecting_a_supported_file_clears_the_previous_unsupported_state() {
    let mut controller = ConversionController::default();
    controller.set_input(std::path::PathBuf::from("notes.txt"));
    assert!(controller.probed.is_some());
    controller.set_input(std::path::PathBuf::from("capture.png"));
    assert!(controller.probed.is_none());
}
