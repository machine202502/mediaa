use super::*;

#[test]
fn static_build_has_required_codecs() {
    use ez_ffmpeg::capabilities as caps;
    for name in ["libx264", "libx264rgb", "aac", "gif", "libwebp_anim"] {
        assert!(
            caps::is_encoder_available(name),
            "{name} encoder missing from the FFmpeg build"
        );
    }
    for name in ["palettegen", "paletteuse", "amix"] {
        assert!(
            caps::is_filter_available(name),
            "{name} filter missing from the FFmpeg build"
        );
    }
}

#[test]
fn records_mp4_with_mixed_audio() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(160, 96)],
        vec![Sine::boxed(2), Sine::boxed(1)],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let paths = record(dir.path(), req, 1.5);
    assert_eq!(paths.len(), 1);
    assert_eq!(title(&paths[0]), "Recording test");
    assert_eq!(paths[0].extension().unwrap(), "mp4");
    let info = probe(&paths[0]).unwrap();
    assert_eq!((info.width, info.height), (160, 96));
    assert_eq!(info.video_codec.as_deref(), Some("h264"));
    assert_eq!(
        info.audio_streams, 1,
        "two sources are mixed into one track"
    );
    assert_duration(&paths[0], 1.5, 0.5);
    assert!(!dir.path().join("work").exists(), "scratch dir removed");
}

#[test]
fn records_several_sources_into_separate_mkv_files() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(64, 48), pattern(96, 64)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let mut paths = record(dir.path(), req, 1.0);
    paths.sort();
    assert_eq!(paths.len(), 2);
    let names: Vec<_> = paths.iter().map(|p| title(p)).collect();
    assert_eq!(
        names,
        [
            "Recording test [1 of 2] - Pattern 64x48 (audio)",
            "Recording test [2 of 2] - Pattern 96x64"
        ]
    );
    assert_eq!(
        naming::parse_path(&paths[0]).unwrap().taken,
        naming::parse_path(&paths[1]).unwrap().taken,
        "one start date for the set"
    );
    assert!(
        probe(&paths[0]).unwrap().has_audio(),
        "sound goes into the first file"
    );
    assert!(!probe(&paths[1]).unwrap().has_audio(), "and only there");
    assert_eq!(probe(&paths[1]).unwrap().width, 96);
}

#[test]
fn audio_can_go_into_every_linked_file() {
    let dir = tempfile::tempdir().unwrap();
    let req = request_with_layout(
        dir.path(),
        vec![pattern(64, 48), pattern(96, 64)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mkv),
        AudioLayout::EveryFile,
    );
    let mut paths = record(dir.path(), req, 1.0);
    paths.sort();
    let names: Vec<_> = paths.iter().map(|p| title(p)).collect();
    assert_eq!(
        names,
        [
            "Recording test [1 of 2] - Pattern 64x48",
            "Recording test [2 of 2] - Pattern 96x64"
        ]
    );
    assert!(
        paths.iter().all(|p| probe(p).unwrap().has_audio()),
        "every file has the sound"
    );
}

#[test]
fn records_gif_and_webp_directly() {
    for (fmt, codec) in [(AnimFormat::Gif, "gif"), (AnimFormat::Webp, "webp")] {
        let dir = tempfile::tempdir().unwrap();
        let req = request(
            dir.path(),
            vec![pattern(80, 48)],
            vec![Sine::boxed(2)],
            OutputKind::Anim(fmt),
        );
        let paths = record(dir.path(), req, 1.0);
        assert_eq!(paths[0].extension().unwrap(), fmt.extension());
        assert_eq!(MediaKind::detect(&paths[0]), Some(MediaKind::Animation));
        let info = probe(&paths[0]).unwrap();
        assert!(
            info.video_codec.as_deref().unwrap().contains(codec),
            "{info:?}"
        );
        assert_eq!((info.width, info.height), (80, 48), "{info:?}");
        assert!(info.duration_secs().is_some_and(|d| d > 0.5), "{info:?}");
    }
}

#[test]
fn webp_encoding_finishes_from_live_chunks_without_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(96, 64)],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let session = RecordingSession::start(req).unwrap();
    std::thread::sleep(Duration::from_millis(2300));
    let (event, warnings) = finish(session, StopAction::Save);
    let SessionEvent::Finished(paths) = event else {
        panic!("{event:?}")
    };
    assert_eq!(paths.len(), 1);
    assert!(
        warnings.iter().all(|warning| !warning.contains("WebP")),
        "live encoding unexpectedly fell back: {warnings:?}"
    );
    let info = probe(&paths[0]).unwrap();
    let duration = info.duration_secs().expect("WebP duration");
    assert!((1.9..2.8).contains(&duration), "{info:?}");
    let decoder = image_webp::WebPDecoder::new(std::io::BufReader::new(
        std::fs::File::open(&paths[0]).unwrap(),
    ))
    .unwrap();
    let expected = duration * 10.0;
    assert!(
        (decoder.num_frames() as f64 - expected).abs() <= 3.0,
        "{} frames, expected about {expected}",
        decoder.num_frames()
    );
    assert!(!dir.path().join("work").exists());
}

#[test]
fn segmented_webp_accepts_still_sections() {
    let dir = tempfile::tempdir().unwrap();
    let mkv = record(
        dir.path(),
        request(
            dir.path(),
            vec![still(96, 64)],
            vec![],
            OutputKind::Video(VideoContainer::Mkv),
        ),
        1.4,
    )
    .remove(0);
    let output = dir.path().join("still.webp");
    let animation = AnimSettings {
        fps: 10,
        max_height: 0,
        ..AnimSettings::default()
    };
    animation_backends::encode(
        &AnimationEncodeRequest {
            input: &mkv,
            output: &output,
            window: None,
            format: AnimFormat::Webp,
            settings: &animation,
            effort: Effort::Balanced,
            origin: AnimationOrigin::Recording,
            segmentation: WebpSegmentation::Segmented,
            scheduling: PipelineTuning {
                max_workers: 2,
                max_attempts: 2,
                retry_backoff: Duration::from_millis(1),
                ..PipelineTuning::default()
            },
            threads: None,
            trace_id: None,
        },
        &mut |_| {},
        &AtomicBool::new(false),
    )
    .unwrap();
    let info = probe(&output).unwrap();
    assert!(
        info.duration_secs().is_some_and(|duration| duration > 1.0),
        "{info:?}"
    );
}

#[test]
fn failed_live_start_falls_back_to_offline_webp() {
    let dir = tempfile::tempdir().unwrap();
    let work = dir.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("live-webp"), b"blocks the live encoder directory").unwrap();
    let req = request(
        dir.path(),
        vec![pattern(96, 64)],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let session = RecordingSession::start(req).unwrap();
    std::thread::sleep(Duration::from_millis(1300));
    let (event, warnings) = finish(session, StopAction::Save);
    let SessionEvent::Finished(paths) = event else {
        panic!("{event:?}")
    };
    assert_eq!(paths.len(), 1);
    let notice = warnings
        .iter()
        .find(|warning| warning.starts_with("WebpFallback"))
        .unwrap_or_else(|| panic!("{warnings:?}"));
    assert!(notice.contains("webp-"), "{notice}");
    let info = probe(&paths[0]).unwrap();
    assert!(
        info.duration_secs().is_some_and(|duration| duration > 0.8),
        "{info:?}"
    );
}

#[test]
fn pause_is_excluded_from_duration() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(64, 48)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let session = RecordingSession::start(req).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    session.pause();
    assert!(session.is_paused());
    std::thread::sleep(Duration::from_millis(1500));
    session.resume();
    std::thread::sleep(Duration::from_millis(800));
    let elapsed = session.elapsed().as_secs_f64();
    assert!((elapsed - 1.6).abs() < 0.3, "elapsed {elapsed}");
    let (event, _) = finish(session, StopAction::Save);
    let SessionEvent::Finished(paths) = event else {
        panic!("{event:?}")
    };
    assert_duration(&paths[0], 1.6, 0.4);
}

#[test]
fn cancel_discards_everything() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(64, 48)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let session = RecordingSession::start(req).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let (event, _) = finish(session, StopAction::Discard);
    assert_eq!(event, SessionEvent::Cancelled);
    assert_eq!(
        std::fs::read_dir(dir.path().join("out")).unwrap().count(),
        0
    );
    assert!(!dir.path().join("work").exists());
}

#[test]
fn failing_audio_source_is_a_warning_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(64, 48)],
        vec![Box::new(Failing)],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let session = RecordingSession::start(req).unwrap();
    assert!(!session.has_audio());
    std::thread::sleep(Duration::from_millis(500));
    let (event, warnings) = finish(session, StopAction::Save);
    assert!(matches!(event, SessionEvent::Finished(_)), "{event:?}");
    assert!(
        warnings.iter().any(|w| w.contains("broken mic")),
        "{warnings:?}"
    );
}

#[test]
fn source_that_fails_to_open_aborts_start() {
    let dir = tempfile::tempdir().unwrap();
    let bad: SourceFactory = Box::new(|| anyhow::bail!("window was closed"));
    let err = RecordingSession::start(request(
        dir.path(),
        vec![bad],
        vec![],
        OutputKind::default(),
    ))
    .err()
    .expect("must fail");
    assert!(format!("{err:#}").contains("window was closed"));
}
