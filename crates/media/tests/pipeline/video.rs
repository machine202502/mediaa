use super::*;

fn last_webp_pixel(path: &Path) -> [u8; 4] {
    let mut decoder =
        image_webp::WebPDecoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
            .unwrap();
    let size = decoder.output_buffer_size().unwrap();
    let mut buffer = vec![0; size];
    let mut pixel = [0; 4];
    for _ in 0..decoder.num_frames() {
        decoder.read_image(&mut buffer).unwrap();
        pixel.copy_from_slice(&buffer[..4]);
    }
    pixel
}

#[test]
fn sources_starting_at_different_times_have_equal_duration() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            delayed_pattern(64, 48, Duration::from_millis(0)),
            delayed_pattern(96, 64, Duration::from_millis(250)),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let paths = record(dir.path(), req, 1.2);
    assert_eq!(paths.len(), 2);
    let durations: Vec<f64> = paths
        .iter()
        .map(|path| probe(path).unwrap().duration_secs().unwrap_or(0.0))
        .collect();
    assert!(
        durations.iter().all(|duration| *duration > 0.8),
        "{durations:?}"
    );
    assert!(
        (durations[0] - durations[1]).abs() <= 1.0 / 20.0 + 0.01,
        "captures must share one duration: {durations:?}"
    );
}

#[test]
fn animated_sources_have_equal_duration_too() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            delayed_pattern(64, 48, Duration::from_millis(0)),
            delayed_pattern(96, 64, Duration::from_millis(250)),
        ],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let paths = record(dir.path(), req, 1.2);
    let durations: Vec<f64> = paths
        .iter()
        .map(|path| probe(path).unwrap().duration_secs().unwrap_or(0.0))
        .collect();
    assert!(
        (durations[0] - durations[1]).abs() <= 1.0 / 10.0 + 0.01,
        "animated captures must share one duration: {durations:?}"
    );
}

#[test]
fn a_slow_animated_source_keeps_the_shared_duration() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            pattern(64, 48),
            slow_pattern(96, 64, Duration::from_millis(90)),
        ],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let paths = record(dir.path(), req, 1.2);
    let durations: Vec<f64> = paths
        .iter()
        .map(|path| probe(path).unwrap().duration_secs().unwrap_or(0.0))
        .collect();
    assert!(
        (durations[0] - durations[1]).abs() <= 1.0 / 10.0 + 0.01,
        "slow animated captures must share one duration: {durations:?}"
    );
}

#[test]
fn a_slow_gif_source_keeps_the_shared_duration() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            pattern(64, 48),
            slow_pattern(96, 64, Duration::from_millis(90)),
        ],
        vec![],
        OutputKind::Anim(AnimFormat::Gif),
    );
    let paths = record(dir.path(), req, 1.2);
    let durations: Vec<f64> = paths
        .iter()
        .map(|path| probe(path).unwrap().duration_secs().unwrap_or(0.0))
        .collect();
    assert!(
        (durations[0] - durations[1]).abs() <= 1.0 / 10.0 + 0.01,
        "slow GIF captures must share one duration: {durations:?}"
    );
}

#[test]
fn a_slow_source_is_trimmed_to_the_shared_duration() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            pattern(64, 48),
            slow_pattern(96, 64, Duration::from_millis(90)),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let paths = record(dir.path(), req, 1.2);
    let durations: Vec<f64> = paths
        .iter()
        .map(|path| probe(path).unwrap().duration_secs().unwrap_or(0.0))
        .collect();
    assert!(
        (durations[0] - durations[1]).abs() <= 1.0 / 20.0 + 0.01,
        "a slow source must not shorten the set: {durations:?}"
    );
}

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
fn a_source_that_fails_before_start_aborts_the_whole_recording() {
    let dir = tempfile::tempdir().unwrap();
    let failing: SourceFactory = Box::new(|| anyhow::bail!("window is minimized"));
    let req = request(
        dir.path(),
        vec![failing, pattern(96, 64)],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let Err(error) = RecordingSession::start(req) else {
        panic!("recording started although a source was unavailable")
    };
    assert!(format!("{error:#}").contains("window is minimized"));
    assert_eq!(
        std::fs::read_dir(dir.path().join("out")).unwrap().count(),
        0
    );
}

#[test]
fn a_hidden_window_is_replaced_with_the_no_signal_card() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![hidden_pattern(64, 48), pattern(96, 64)],
        vec![],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let paths = record(dir.path(), req, 1.2);
    assert_eq!(paths.len(), 2);
    let durations: Vec<f64> = paths
        .iter()
        .map(|path| probe(path).unwrap().duration_secs().unwrap_or(0.0))
        .collect();
    assert!(
        durations.iter().all(|duration| *duration > 0.8),
        "{durations:?}"
    );
}

#[test]
fn the_no_signal_card_is_written_into_the_output() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![hidden_pattern(64, 48), pattern(96, 64)],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let paths = record(dir.path(), req, 1.0);
    let hidden = last_webp_pixel(&paths[0]);
    let live = last_webp_pixel(&paths[1]);
    assert!(
        hidden[0] > 100 && hidden[1] > 100 && hidden[2] > 100,
        "expected the light no-signal card for the hidden source, got {hidden:?}"
    );
    assert!(
        live[0] < 100,
        "the live source must not be replaced by the TV card: {live:?}"
    );
}

#[test]
fn disabling_the_no_signal_card_keeps_the_last_frame() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            fading_pattern(64, 48, 3, [220, 20, 30, 255]),
            pattern(96, 64),
        ],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    )
    .no_signal(false);
    let paths = record(dir.path(), req, 1.0);
    let pixel = last_webp_pixel(&paths[0]);
    assert!(
        pixel[0] > 100 && pixel[1] < 100,
        "expected the last red frame, got {pixel:?}"
    );
}

#[test]
fn a_lost_source_is_filled_with_a_placeholder_for_the_whole_track() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![failing_pattern(64, 48, 5), pattern(96, 64)],
        vec![],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let paths = record(dir.path(), req, 1.2);
    assert_eq!(paths.len(), 2);
    let durations: Vec<f64> = paths
        .iter()
        .map(|path| probe(path).unwrap().duration_secs().unwrap_or(0.0))
        .collect();
    assert!(
        durations.iter().all(|duration| *duration > 0.8),
        "{durations:?}"
    );
    assert!(
        (durations[0] - durations[1]).abs() <= 1.0 / 20.0 + 0.01,
        "the lost source must still have the full duration: {durations:?}"
    );
}

#[test]
fn a_source_can_be_lost_and_restored_during_recording() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![recovering_pattern(64, 48, 5), pattern(96, 64)],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let session = RecordingSession::start(req).unwrap();
    let events = session.events();
    let lost = events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(lost, SessionEvent::SourceLost(_)), "{lost:?}");
    let restored = events.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        matches!(restored, SessionEvent::SourceRestored(_)),
        "{restored:?}"
    );
    let (event, warnings) = finish_with_timeout(session, StopAction::Save, Duration::from_secs(60));
    let SessionEvent::Finished(paths) = event else {
        panic!("{event:?} {warnings:?}")
    };
    assert_eq!(paths.len(), 2);
    assert!(probe(&paths[0]).unwrap().duration_secs().unwrap_or(0.0) > 0.5);
}

#[test]
fn a_resized_window_keeps_the_initial_canvas() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            resized_recovering_pattern(64, 48, 5, 96, 64),
            pattern(96, 64),
        ],
        vec![],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let paths = record(dir.path(), req, 1.2);
    assert_eq!(probe(&paths[0]).unwrap().width, 64);
    assert_eq!(probe(&paths[0]).unwrap().height, 48);
    assert_eq!(
        probe(&paths[1]).unwrap().width,
        96,
        "the second source keeps its own canvas"
    );
    assert_eq!(probe(&paths[1]).unwrap().height, 64);
}

#[test]
fn one_failing_live_source_saves_the_remaining_animation() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            failing_pattern(640, 360, 8),
            video_source("fake-window.webp", 640, 360),
        ],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let paths = record(dir.path(), req, 1.2);
    assert_eq!(paths.len(), 2);
    assert!(paths
        .iter()
        .all(|path| probe(path).unwrap().duration_secs().is_some()));
}

#[test]
fn closing_one_video_source_finishes_without_hanging() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![
            video_source("fake-window.webp", 640, 360),
            closing_video_source("fake-window.webp", 640, 360, 20),
        ],
        vec![],
        OutputKind::Anim(AnimFormat::Webp),
    );
    let session = RecordingSession::start(req).unwrap();
    std::thread::sleep(Duration::from_millis(1800));
    let (event, warnings) = finish_with_timeout(session, StopAction::Save, Duration::from_secs(60));
    assert!(
        matches!(
            event,
            SessionEvent::Finished(_)
                | SessionEvent::Failed(_)
                | SessionEvent::Cancelled
                | SessionEvent::CaptureFailed(_)
        ),
        "{event:?} {warnings:?}"
    );
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
