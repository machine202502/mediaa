use super::{
    BatchStatus, ConversionController, ConversionJob, ConversionOutcome, ConversionTaskResult,
};
use crate::features::jobs::Job;
use mediaa_core::paths::OutputPath;
use mediaa_media::convert::Target;
use std::sync::atomic::{AtomicBool, AtomicI32};
use std::sync::Arc;
use std::time::{Duration, Instant};

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
    let wake = mediaa_core::wake::Wake::default();
    (0..100)
        .find_map(|_| {
            let outcome = controller.poll_job(&wake);
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
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("result.mp4");
    std::fs::write(&output, b"partial").unwrap();
    let mut controller = ConversionController {
        job: Some(conversion_job(
            output.clone(),
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
    assert!(!output.exists(), "the partial output is removed");
}

#[test]
fn selecting_a_supported_file_clears_the_previous_unsupported_state() {
    let mut controller = ConversionController::default();
    controller.set_input(std::path::PathBuf::from("notes.txt"));
    assert!(controller.probed.is_some());
    controller.set_input(std::path::PathBuf::from("capture.png"));
    assert!(controller.probed.is_none());
}

#[test]
fn mixed_batches_use_the_intersection_of_targets() {
    let mut controller = ConversionController::default();
    controller.set_inputs(vec!["a.mp4".into(), "b.mp3".into()]);
    assert!(controller
        .targets()
        .iter()
        .all(|target| matches!(target, Target::Audio(_))));

    controller.set_inputs(vec!["a.mp4".into(), "b.png".into()]);
    assert_eq!(controller.targets().len(), 5);
    assert!(controller
        .targets()
        .iter()
        .all(|target| matches!(target, Target::Still(_))));

    controller.set_inputs(vec!["a.mp3".into(), "b.png".into()]);
    assert!(controller.targets().is_empty());
}

#[test]
fn duplicate_inputs_are_removed_and_queue_keeps_the_rest() {
    let mut controller = ConversionController::default();
    controller.set_inputs(vec!["a.mp4".into(), "a.mp4".into(), "b.mp4".into()]);
    assert_eq!(controller.batch_total, 2);
    assert_eq!(controller.queue.len(), 1);
    assert_eq!(
        controller.input.as_deref(),
        Some(std::path::Path::new("a.mp4"))
    );
}

#[test]
fn completed_batch_items_keep_their_status() {
    let mut controller = ConversionController::default();
    controller.set_inputs(vec!["a.mp4".into(), "b.mp4".into()]);
    controller.job = Some(conversion_job(
        std::path::PathBuf::from("result.mp4"),
        Arc::default(),
        ConversionTaskResult::Completed,
    ));
    let outcome = wait_for_outcome(&mut controller);
    assert!(matches!(outcome, ConversionOutcome::Finished(_)));
    assert_eq!(controller.batch.len(), 2);
    assert!(
        controller
            .batch
            .iter()
            .any(|item| item.path == std::path::Path::new("a.mp4")
                && item.status == BatchStatus::Done)
    );
}

#[test]
fn a_failed_item_can_be_retried_and_an_item_can_be_removed() {
    let mut controller = ConversionController::default();
    controller.set_inputs(vec!["a.mp4".into(), "b.mp4".into()]);
    controller.mark_batch(
        std::path::Path::new("a.mp4"),
        BatchStatus::Failed,
        Some("boom".into()),
    );
    controller.retry_batch(std::path::Path::new("a.mp4"));
    assert_eq!(
        controller.input.as_deref(),
        Some(std::path::Path::new("a.mp4"))
    );
    assert_eq!(controller.batch[0].status, BatchStatus::Pending);
    controller.remove_batch(std::path::Path::new("a.mp4"));
    assert!(controller
        .batch
        .iter()
        .all(|item| item.path != std::path::Path::new("a.mp4")));
    assert_eq!(
        controller.input.as_deref(),
        Some(std::path::Path::new("b.mp4"))
    );
}

#[test]
fn an_unsupported_batch_file_is_skipped_too() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.png");
    image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]))
        .save(&good)
        .unwrap();
    let unsupported = dir.path().join("notes.txt");
    let wake = mediaa_core::wake::Wake::default();
    let mut controller = ConversionController::default();
    controller.set_inputs(vec![unsupported.clone(), good.clone()]);
    assert!(
        controller.probed.is_some(),
        "the unsupported file is reported at once"
    );
    for _ in 0..400 {
        controller.poll_probe(&wake);
        if controller.input.as_deref() == Some(good.as_path())
            && controller
                .probed
                .as_ref()
                .is_some_and(|probe| probe.is_ok())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(controller.input.as_deref(), Some(good.as_path()));
    assert_eq!(controller.batch[0].status, BatchStatus::Failed);
    assert!(controller.batch[0].error.is_some());
}

#[test]
fn a_missing_batch_file_fails_alone_and_the_batch_moves_on() {
    let dir = tempfile::tempdir().unwrap();
    let good = dir.path().join("good.png");
    image::RgbaImage::from_pixel(4, 3, image::Rgba([10, 20, 30, 255]))
        .save(&good)
        .unwrap();
    let missing = dir.path().join("missing.png");
    let wake = mediaa_core::wake::Wake::default();
    let mut controller = ConversionController::default();
    controller.set_inputs(vec![missing.clone(), good.clone()]);
    for _ in 0..400 {
        controller.poll_probe(&wake);
        if controller.input.as_deref() == Some(good.as_path())
            && controller
                .probed
                .as_ref()
                .is_some_and(|probe| probe.is_ok())
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        controller.input.as_deref(),
        Some(good.as_path()),
        "the missing file is skipped"
    );
    assert_eq!(controller.batch[0].status, BatchStatus::Failed);
    assert!(
        controller.batch[0]
            .error
            .as_deref()
            .is_some_and(|error| !error.is_empty()),
        "the failed file keeps the reason"
    );
    assert_eq!(controller.batch[1].status, BatchStatus::Pending);
    assert!(controller
        .probed
        .as_ref()
        .is_some_and(|probe| probe.is_ok()));
}

#[test]
fn a_failed_item_advances_to_the_next_without_leaving_the_batch() {
    let mut controller = ConversionController::default();
    controller.set_inputs(vec!["a.mp4".into(), "b.mp4".into()]);
    controller.job = Some(conversion_job(
        std::path::PathBuf::from("result.mp4"),
        Arc::default(),
        ConversionTaskResult::Failed {
            kind: mediaa_media::error::MediaErrorKind::Internal,
            message: "boom".into(),
        },
    ));
    let outcome = wait_for_outcome(&mut controller);
    assert!(matches!(outcome, ConversionOutcome::Failed { .. }));
    assert_eq!(controller.batch.len(), 2);
    assert_eq!(controller.batch[0].status, BatchStatus::Failed);
    assert_eq!(
        controller.input.as_deref(),
        Some(std::path::Path::new("b.mp4"))
    );
}

#[test]
fn restarting_a_batch_skips_done_files_and_keeps_their_status() {
    let mut controller = ConversionController::default();
    controller.test_batch(
        &[
            ("a.mp4", BatchStatus::Done, None),
            ("b.mp4", BatchStatus::Failed, Some("failed")),
        ],
        Some(Target::Video(mediaa_media::convert::VideoFormat::Mp4)),
    );
    controller.prepare_batch_run();
    assert_eq!(controller.batch[0].status, BatchStatus::Done);
    assert_eq!(controller.batch[1].status, BatchStatus::Failed);
    assert_eq!(controller.batch_done, 1);
    assert!(controller.queue.is_empty());
    assert_eq!(
        controller.input.as_deref(),
        Some(std::path::Path::new("b.mp4"))
    );
}

#[test]
fn changing_conversion_settings_resets_all_batch_statuses() {
    let mut controller = ConversionController::default();
    controller.test_batch(
        &[
            ("a.mp4", BatchStatus::Done, None),
            ("b.mp4", BatchStatus::Failed, Some("failed")),
        ],
        Some(Target::Video(mediaa_media::convert::VideoFormat::Mp4)),
    );
    controller.reset_batch_statuses();
    assert!(controller
        .batch
        .iter()
        .all(|item| { item.status == BatchStatus::Pending && item.error.is_none() }));
    assert_eq!(controller.batch_done, 0);
}

#[test]
fn clearing_a_batch_removes_files_and_resets_the_form() {
    let mut controller = ConversionController::default();
    controller.test_batch(
        &[("a.mp4", BatchStatus::Pending, None)],
        Some(Target::Video(mediaa_media::convert::VideoFormat::Mp4)),
    );
    controller.clear_batch();
    assert!(controller.batch.is_empty());
    assert_eq!(controller.batch_total, 0);
    assert!(controller.input.is_none());
    assert!(controller.target.is_none());
}

#[test]
fn batch_progress_stays_visible_between_files() {
    let mut controller = ConversionController::default();
    controller.test_batch(
        &[
            ("a.png", BatchStatus::Pending, None),
            ("b.png", BatchStatus::Pending, None),
        ],
        Some(Target::Still(mediaa_media::convert::StillFormat::Png)),
    );
    controller.job = Some(conversion_job(
        std::path::PathBuf::from("a.png"),
        Arc::default(),
        ConversionTaskResult::Completed,
    ));
    let outcome = wait_for_outcome(&mut controller);
    assert!(matches!(outcome, ConversionOutcome::Finished(_)));
    assert!(controller.job.is_none());
    assert!(controller.batch_auto);
    assert!(controller.probe_loading);
    assert!(
        controller.probed.is_none(),
        "the next file is probed before its job starts"
    );
    assert!(controller.is_busy());
    let status = controller.status().expect("the batch is still running");
    assert_eq!((status.done, status.total), (1, 2));
    assert!((status.progress.unwrap() - 0.5).abs() < 1e-6);
}

#[test]
fn cancelling_between_files_stops_the_queue() {
    let mut controller = ConversionController::default();
    controller.test_batch(
        &[
            ("a.png", BatchStatus::Pending, None),
            ("b.png", BatchStatus::Pending, None),
        ],
        Some(Target::Still(mediaa_media::convert::StillFormat::Png)),
    );
    controller.job = Some(conversion_job(
        std::path::PathBuf::from("a.png"),
        Arc::default(),
        ConversionTaskResult::Completed,
    ));
    let outcome = wait_for_outcome(&mut controller);
    assert!(matches!(outcome, ConversionOutcome::Finished(_)));
    assert!(controller.batch_auto);
    controller.cancel();
    assert!(!controller.is_busy());
    assert!(!controller.batch_auto);
    assert!(controller.queue.is_empty());
    assert!(controller.status().is_none());
}

#[test]
fn adding_inputs_deduplicates_by_path() {
    let mut controller = ConversionController::default();
    controller.set_inputs(vec![
        std::path::PathBuf::from("a.png"),
        std::path::PathBuf::from("a.png"),
    ]);
    controller.add_inputs(vec![
        std::path::PathBuf::from("a.png"),
        std::path::PathBuf::from("b.png"),
        std::path::PathBuf::from("b.png"),
    ]);
    let paths: Vec<_> = controller
        .batch
        .iter()
        .map(|item| item.path.clone())
        .collect();
    assert_eq!(
        paths,
        vec![
            std::path::PathBuf::from("a.png"),
            std::path::PathBuf::from("b.png"),
        ]
    );
    assert_eq!(controller.batch_total, 2);
    assert_eq!(controller.input, Some(std::path::PathBuf::from("a.png")));
}
