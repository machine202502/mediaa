use super::{
    concat_chunks, join_with_timeout, linked_title, preserve_recordings,
    recover_incomplete_sessions, wait_for_source, AudioLayout, RecordingLimits, RecordingRequest,
    RecordingSession, RecordingStartCancelled, SourceReady, SourceReadyResult, TimedJoin,
};
use chrono::{Local, TimeZone};
use ez_ffmpeg::{FfmpegContext, Input, Output};
use mediaa_core::formats::{OutputKind, VideoContainer};
use mediaa_core::settings::{AnimSettings, WebpMode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[test]
fn capture_join_timeout_returns_without_waiting_for_a_stuck_worker() {
    let worker = std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(500));
    });
    let started = Instant::now();
    let outcome = join_with_timeout(worker, Duration::from_millis(20));
    assert!(matches!(outcome, TimedJoin::TimedOut));
    assert!(started.elapsed() < Duration::from_millis(400));
}

#[test]
fn capture_join_distinguishes_values_and_panics() {
    let finished = std::thread::spawn(|| 7);
    let outcome = join_with_timeout(finished, Duration::from_millis(100));
    assert!(matches!(outcome, TimedJoin::Finished(7)));

    let panicked = std::thread::spawn(|| -> usize { panic!("capture panic") });
    let outcome = join_with_timeout(panicked, Duration::from_millis(100));
    assert!(matches!(outcome, TimedJoin::Panicked));
}

#[test]
fn linked_names() {
    assert_eq!(linked_title("Rec", 0, 1, "Display 1", true), "Rec");
    assert_eq!(
        linked_title("Rec", 0, 2, "Display 1", true),
        "Rec [1 of 2] - Display 1 (audio)"
    );
    assert_eq!(
        linked_title("Rec", 1, 2, "Code: main.rs", false),
        "Rec [2 of 2] - Code main.rs"
    );
}

#[test]
fn recording_request_builder_rejects_impossible_requests() {
    let dir = tempfile::tempdir().unwrap();
    let video = || {
        RecordingRequest::builder(
            OutputKind::Video(VideoContainer::Mp4),
            Vec::new(),
            Vec::new(),
            mediaa_core::paths::MediaDir::new(dir.path().join("out")),
            mediaa_core::paths::WorkDir::new(dir.path().join("work")),
            "Recording".into(),
        )
        .build()
    };
    assert!(video().is_err());

    let audio = RecordingRequest::builder(
        OutputKind::Audio(mediaa_core::formats::AudioFormat::Mp3),
        Vec::new(),
        Vec::new(),
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(dir.path().join("work")),
        "Audio".into(),
    )
    .build();
    assert!(audio.is_err());

    let fps = RecordingRequest::builder(
        OutputKind::Video(VideoContainer::Mp4),
        vec![Box::new(|| anyhow::bail!("unused"))],
        Vec::new(),
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(dir.path().join("work")),
        "Recording".into(),
    )
    .video(0, 80, None)
    .build();
    assert!(fps.is_err());
}

#[test]
fn recording_limits_can_be_overridden_and_are_validated() {
    let dir = tempfile::tempdir().unwrap();
    let limits = RecordingLimits {
        source_ready_timeout: Duration::from_millis(250),
        ready_poll_interval: Duration::from_millis(10),
        capture_stop_timeout: Duration::from_secs(1),
    };
    let request = RecordingRequest::builder(
        OutputKind::Video(VideoContainer::Mp4),
        vec![Box::new(|| anyhow::bail!("unused"))],
        Vec::new(),
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(dir.path().join("work")),
        "Recording".into(),
    )
    .limits(limits)
    .build()
    .unwrap();
    assert_eq!(request.limits, limits);

    let invalid = RecordingRequest::builder(
        OutputKind::Video(VideoContainer::Mp4),
        vec![Box::new(|| anyhow::bail!("unused"))],
        Vec::new(),
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(dir.path().join("work")),
        "Recording".into(),
    )
    .limits(RecordingLimits {
        source_ready_timeout: Duration::from_millis(10),
        ready_poll_interval: Duration::from_millis(20),
        capture_stop_timeout: Duration::from_secs(1),
    })
    .build();
    assert!(invalid.is_err());

    let invalid_stop = RecordingRequest::builder(
        OutputKind::Video(VideoContainer::Mp4),
        vec![Box::new(|| anyhow::bail!("unused"))],
        Vec::new(),
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(dir.path().join("work")),
        "Recording".into(),
    )
    .limits(RecordingLimits {
        source_ready_timeout: Duration::from_secs(1),
        ready_poll_interval: Duration::from_millis(10),
        capture_stop_timeout: Duration::ZERO,
    })
    .build();
    assert!(invalid_stop.is_err());
}

#[test]
fn failed_finalize_keeps_source_recordings_in_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let source = work.join("video-0.mkv");
    std::fs::write(&source, b"recorded frames").unwrap();
    let started = Local.with_ymd_and_hms(2026, 10, 25, 6, 0, 0).unwrap();
    let kept = preserve_recordings(
        &[super::RecordedVideo {
            index: 0,
            name: "Display 1".into(),
            files: vec![source.clone()],
        }],
        &out,
        "Recording",
        started,
    );
    assert_eq!(kept.len(), 1);
    assert!(!source.exists());
    assert_eq!(
        kept[0].extension().and_then(|ext| ext.to_str()),
        Some("mkv")
    );
    assert_eq!(std::fs::read(&kept[0]).unwrap(), b"recorded frames");
    assert!(kept[0].starts_with(&out));
    let expected: PathBuf = out.join("Recording (source)_2026-10-25_06-00-00.mkv");
    assert_eq!(kept[0], expected);
}

#[test]
fn failed_chunk_conversion_keeps_every_source_part() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let out = dir.path().join("out");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::create_dir_all(&out).unwrap();
    let first = work.join("chunk-000000.mkv");
    let second = work.join("chunk-000001.mkv");
    std::fs::write(&first, b"first").unwrap();
    std::fs::write(&second, b"second").unwrap();
    let started = Local.with_ymd_and_hms(2026, 10, 25, 6, 0, 0).unwrap();
    let kept = preserve_recordings(
        &[super::RecordedVideo {
            index: 0,
            name: "Display 1".into(),
            files: vec![first, second],
        }],
        &out,
        "Recording",
        started,
    );
    assert_eq!(kept.len(), 2);
    assert!(kept.iter().all(|path| path.starts_with(&out)));
    assert!(kept[0].to_string_lossy().contains("part 1 of 2"));
    assert!(kept[1].to_string_lossy().contains("part 2 of 2"));
    assert_eq!(std::fs::read(&kept[0]).unwrap(), b"first");
    assert_eq!(std::fs::read(&kept[1]).unwrap(), b"second");
}

#[test]
fn startup_recovery_moves_incomplete_session_sources_into_the_library() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let out = dir.path().join("out");
    let session = work.join("20261025-060000-42");
    std::fs::create_dir_all(&session).unwrap();
    std::fs::write(session.join("video-0.mkv"), b"video").unwrap();
    std::fs::write(session.join("audio-0.wav"), b"audio").unwrap();

    let report = recover_incomplete_sessions(&work, &out);
    assert!(report.failures.is_empty(), "{:?}", report.failures);
    assert_eq!(report.files.len(), 2);
    assert!(!session.exists());
    let contents: Vec<_> = report
        .files
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    assert!(contents.contains(&b"audio".to_vec()));
    assert!(contents.contains(&b"video".to_vec()));
    assert!(report.files.iter().all(|path| path.starts_with(&out)));
}

#[test]
fn canceled_start_returns_without_touching_the_work_directory() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    let request = RecordingRequest::builder(
        OutputKind::Video(VideoContainer::Mp4),
        vec![Box::new(|| anyhow::bail!("must not start"))],
        Vec::new(),
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(work.clone()),
        "Recording".into(),
    )
    .video(30, 80, None)
    .audio_layout(AudioLayout::EveryFile)
    .audio_kbps(192)
    .animation(AnimSettings::default())
    .webp_mode(WebpMode::Fast)
    .build()
    .unwrap();
    let error = RecordingSession::start_with_cancel(request, &AtomicBool::new(true))
        .err()
        .expect("canceled start must fail");
    assert!(error.is::<RecordingStartCancelled>());
    assert!(!work.exists());
}

#[test]
fn cancellation_interrupts_a_source_readiness_wait() {
    let (_tx, rx) = crossbeam_channel::bounded::<Result<SourceReadyResult, anyhow::Error>>(1);
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = cancel.clone();
    let canceller = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(30));
        worker_cancel.store(true, Ordering::Relaxed);
    });
    let started = Instant::now();
    let result = wait_for_source(
        &rx,
        &cancel,
        Duration::from_secs(5),
        Duration::from_millis(20),
    );
    canceller.join().unwrap();
    assert!(matches!(result, SourceReady::Cancelled));
    assert!(started.elapsed() < Duration::from_millis(500));
}

#[test]
fn finalization_progress_is_shared_without_event_queue_traffic() {
    let progress = super::RecordingProgress::default();
    assert_eq!(progress.fraction(), None);
    progress.set(Some(0.425));
    assert_eq!(progress.fraction(), Some(0.425));
    progress.set(Some(1.5));
    assert_eq!(progress.fraction(), Some(1.0));
    progress.set(Some(-1.0));
    assert_eq!(progress.fraction(), Some(0.0));
    progress.set(None);
    assert_eq!(progress.fraction(), None);
}

#[test]
fn chunk_concat_keeps_both_parts_in_order() {
    let dir = tempfile::tempdir().unwrap();
    let mut chunks = Vec::new();
    for (index, color) in ["red", "blue"].into_iter().enumerate() {
        let path = dir.path().join(format!("part-{index}.mkv"));
        let context = FfmpegContext::builder()
            .input(Input::from(format!("color=c={color}:s=64x48:r=10:d=0.4")).set_format("lavfi"))
            .filter_desc("[0:v]format=bgr0[v]")
            .output(
                Output::from(path.to_string_lossy().into_owned())
                    .set_format("matroska")
                    .set_video_codec("libx264rgb")
                    .set_video_codec_opt("preset", "ultrafast")
                    .set_video_codec_opt("qp", "0")
                    .add_stream_map("[v]"),
            )
            .build()
            .unwrap();
        crate::job::run(context, None, &mut |_| {}, &AtomicBool::new(false)).unwrap();
        chunks.push(path);
    }
    let combined = dir.path().join("combined.mkv");
    concat_chunks(&chunks, &combined, &AtomicBool::new(false)).unwrap();
    let info = crate::info::probe(&combined).unwrap();
    assert_eq!((info.width, info.height), (64, 48));
    assert!(
        info.duration_secs()
            .is_some_and(|duration| (0.7..0.9).contains(&duration)),
        "{info:?}"
    );
}
