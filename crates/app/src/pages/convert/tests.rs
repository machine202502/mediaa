use super::{ConversionOutcome, ConversionTaskResult, ConvertUi, Estimate, Job};
use crate::app::tasks::{TaskKind, TaskSupervisor};
use mediaa_core::paths::OutputPath;
use mediaa_media::convert::{ConvertSettings, Target};
use std::sync::atomic::AtomicI32;
use std::sync::Arc;
use std::time::Instant;

fn job(
    output: std::path::PathBuf,
    done: crossbeam_channel::Receiver<ConversionTaskResult>,
    thread: std::thread::JoinHandle<()>,
    tasks: &mut TaskSupervisor,
) -> Job {
    let task = tasks.manage(TaskKind::Conversion, thread, None);
    Job {
        output: OutputPath::new(output),
        progress: Arc::new(AtomicI32::new(-1)),
        started: Instant::now(),
        cancel: Arc::default(),
        done,
        task,
    }
}

#[test]
fn conversion_finishes_even_when_the_convert_page_is_not_visible() {
    let ctx = eframe::egui::Context::default();
    let output = std::path::PathBuf::from("result.mp4");
    let (tx, rx) = crossbeam_channel::bounded(1);
    let mut tasks = TaskSupervisor::default();
    let thread = std::thread::spawn(move || {
        tx.send(ConversionTaskResult::Completed).unwrap();
    });
    let mut ui = ConvertUi {
        job: Some(job(output.clone(), rx, thread, &mut tasks)),
        ..ConvertUi::default()
    };
    assert!(ui.is_busy());
    let outcome = (0..100)
        .find_map(|_| {
            let outcome = ui.poll_job(&mut tasks, &ctx);
            if outcome.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            outcome
        })
        .expect("conversion should finish");
    assert!(matches!(
        outcome,
        ConversionOutcome::Finished(path) if path.as_ref() == output
    ));
    assert!(!ui.is_busy());
    assert_eq!(ui.last, Some(output));
}

#[test]
fn a_dead_conversion_worker_reports_a_terminal_error() {
    let ctx = eframe::egui::Context::default();
    let (tx, rx) = crossbeam_channel::bounded::<ConversionTaskResult>(1);
    drop(tx);
    let mut tasks = TaskSupervisor::default();
    let thread = std::thread::spawn(|| panic!("worker failed"));
    let mut ui = ConvertUi {
        job: Some(job(
            std::path::PathBuf::from("result.mp4"),
            rx,
            thread,
            &mut tasks,
        )),
        ..ConvertUi::default()
    };
    assert!(matches!(
        ui.poll_job(&mut tasks, &ctx),
        Some(ConversionOutcome::Failed { .. })
    ));
    assert!(!ui.is_busy());
}

#[test]
fn cancellation_is_a_typed_conversion_outcome() {
    let ctx = eframe::egui::Context::default();
    let (tx, rx) = crossbeam_channel::bounded(1);
    let mut tasks = TaskSupervisor::default();
    let thread = std::thread::spawn(move || {
        tx.send(ConversionTaskResult::Cancelled).unwrap();
    });
    let mut ui = ConvertUi {
        job: Some(job(
            std::path::PathBuf::from("result.mp4"),
            rx,
            thread,
            &mut tasks,
        )),
        ..ConvertUi::default()
    };
    let outcome = (0..100)
        .find_map(|_| {
            let outcome = ui.poll_job(&mut tasks, &ctx);
            if outcome.is_none() {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            outcome
        })
        .expect("cancelled conversion should finish");
    assert!(matches!(outcome, ConversionOutcome::Cancelled));
    assert!(!ui.is_busy());
}

#[test]
fn canceling_an_estimate_signals_its_worker() {
    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut ui = ConvertUi {
        estimate: Some(Estimate {
            key: (
                std::path::PathBuf::from("source.mp4"),
                Target::Video(mediaa_media::convert::VideoFormat::Mp4),
                ConvertSettings::default(),
            ),
            since: Instant::now(),
            cancel: cancel.clone(),
            rx: None,
            result: None,
            previous: None,
            task: None,
        }),
        ..ConvertUi::default()
    };
    ui.cancel_estimate();
    assert!(ui.estimate.is_none());
    assert!(cancel.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn selecting_a_supported_file_clears_the_previous_unsupported_state() {
    let mut ui = ConvertUi::default();
    ui.set_input(std::path::PathBuf::from("notes.txt"));
    assert!(ui.probed.is_some());
    ui.set_input(std::path::PathBuf::from("capture.png"));
    assert!(ui.probed.is_none());
}
