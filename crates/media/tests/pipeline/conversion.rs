use super::*;

#[test]
fn conversions_between_popular_formats() {
    use mediaa_media::convert::{StillFormat, Target, VideoFormat};
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(96, 64)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let mp4 = record(dir.path(), req, 1.2).remove(0);
    let out = dir.path().join("conv");
    let cancel = AtomicBool::new(false);
    let run = |input: &Path, target: Target, s: &ConvertSettings| {
        let source = mediaa_core::paths::SourcePath::new(input);
        let output = convert::output_path(
            &source,
            target,
            None,
            &mediaa_core::paths::MediaDir::new(out.clone()),
        );
        let mut progress = Vec::new();
        convert::convert(
            &source,
            &output,
            target,
            s,
            &mut |p| progress.push(p),
            &cancel,
        )
        .unwrap_or_else(|e| panic!("{target:?}: {e:#}"));
        let measured: Vec<_> = progress.into_iter().flatten().collect();
        assert!(
            measured.windows(2).all(|pair| pair[0] <= pair[1]),
            "{target:?}: {measured:?}"
        );
        assert!(
            measured.last().is_none_or(|value| *value == 1.0),
            "{target:?}: {measured:?}"
        );
        assert_eq!(output.extension().unwrap(), target.extension());
        output.into_path()
    };
    let s = opts();

    for (format, video, audio) in [
        (VideoFormat::Mp4, "h264", "aac"),
        (VideoFormat::Mov, "h264", "aac"),
        (VideoFormat::Mkv, "h264", "aac"),
        (VideoFormat::Webm, "vp9", "opus"),
        (VideoFormat::Avi, "mpeg4", "mp3"),
    ] {
        let p = run(&mp4, Target::Video(format), &s);
        let info = probe(&p).unwrap();
        assert_eq!(info.video_codec.as_deref(), Some(video), "{format:?}");
        assert_eq!(info.audio_codec.as_deref(), Some(audio), "{format:?}");
        assert_eq!((info.width, info.height), (96, 64), "{format:?}");
    }
    let small = run(
        &mp4,
        Target::Video(VideoFormat::Mp4),
        &ConvertSettings {
            remove_audio: true,
            max_height: Some(32),
            quality: 30,
            ..opts()
        },
    );
    let info = probe(&small).unwrap();
    assert!(!info.has_audio());
    assert_eq!(
        (info.width, info.height),
        (48, 32),
        "downscaled, aspect kept"
    );
    assert_eq!(title(&small), "Recording test");

    let gif = run(&mp4, Target::Anim(AnimFormat::Gif), &s);
    assert_eq!(
        (probe(&gif).unwrap().width, probe(&gif).unwrap().height),
        (50, 33),
        "height limit applied"
    );
    let webp = run(&mp4, Target::Anim(AnimFormat::Webp), &s);
    assert_eq!(MediaKind::detect(&webp), Some(MediaKind::Animation));
    let from_gif = run(&gif, Target::Video(VideoFormat::Webm), &s);
    let info = probe(&from_gif).unwrap();
    assert_eq!(info.video_codec.as_deref(), Some("vp9"));
    assert_eq!(
        (info.width, info.height),
        (50, 34),
        "even height after padding"
    );
    assert!(probe(&run(&webp, Target::Video(VideoFormat::Mp4), &s))
        .unwrap()
        .has_video());
    assert!(probe(&run(&webp, Target::Anim(AnimFormat::Gif), &s))
        .unwrap()
        .has_video());

    let png = dir.path().join("shot_2026-01-01_10-00-00.png");
    image::RgbaImage::from_fn(120, 80, |x, y| image::Rgba([x as u8, y as u8, 200, 255]))
        .save(&png)
        .unwrap();
    for format in StillFormat::ALL {
        let p = run(&png, Target::Still(format), &s);
        let img = image::open(&p).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        assert_eq!((img.width(), img.height()), (120, 80), "{format:?}");
    }
    let half = run(
        &png,
        Target::Still(StillFormat::Jpeg),
        &ConvertSettings {
            max_height: Some(40),
            quality: 50,
            ..opts()
        },
    );
    assert_eq!(image::image_dimensions(&half).unwrap(), (60, 40));
    for (input, size) in [(&mp4, (96, 64)), (&gif, (50, 33)), (&webp, (50, 33))] {
        for format in [StillFormat::Png, StillFormat::Jpeg] {
            let p = run(input, Target::Still(format), &s);
            assert_eq!(
                image::image_dimensions(&p).unwrap(),
                size,
                "{} → {format:?}",
                input.display()
            );
        }
    }

    for format in mediaa_core::formats::AudioFormat::ALL {
        let p = run(&mp4, Target::Audio(format), &s);
        let info = probe(&p).unwrap();
        assert!(
            info.has_audio() && !info.has_video(),
            "{format:?}: {info:?}"
        );
        let again = run(
            &p,
            Target::Audio(mediaa_core::formats::AudioFormat::Mp3),
            &s,
        );
        assert_eq!(
            probe(&again).unwrap().audio_codec.as_deref(),
            Some("mp3"),
            "{format:?} → MP3"
        );
        assert_eq!(MediaKind::detect(&p), Some(MediaKind::Audio));
    }

    let hi = run(
        &png,
        Target::Still(StillFormat::Jpeg),
        &ConvertSettings {
            quality: 100,
            ..opts()
        },
    );
    let lo = run(
        &png,
        Target::Still(StillFormat::Jpeg),
        &ConvertSettings {
            quality: 5,
            ..opts()
        },
    );
    assert!(
        std::fs::metadata(&lo).unwrap().len() < std::fs::metadata(&hi).unwrap().len(),
        "lower quality, smaller file"
    );
}

#[test]
fn webp_conversion_uses_activity_instead_of_fake_percentage() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(96, 64)],
        vec![],
        OutputKind::Video(VideoContainer::Mkv),
    );
    let mkv = record(dir.path(), req, 1.2).remove(0);
    let output = dir.path().join("segmented.webp");
    let mut progress = Vec::new();
    convert::convert(
        &mediaa_core::paths::SourcePath::new(&mkv),
        &mediaa_core::paths::OutputPath::new(&output),
        Target::Anim(AnimFormat::Webp),
        &opts(),
        &mut |p| progress.push(p),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(!progress.is_empty());
    assert!(progress.iter().all(Option::is_none), "{progress:?}");
    let decoder = image_webp::WebPDecoder::new(std::io::BufReader::new(
        std::fs::File::open(&output).unwrap(),
    ))
    .unwrap();
    let info = probe(&output).unwrap();
    let duration = info.duration_secs().unwrap();
    assert!((0.9..1.8).contains(&duration), "{info:?}");
    let expected_frames = duration * 10.0;
    assert!(
        (decoder.num_frames() as f64 - expected_frames).abs() <= 2.0,
        "{} vs {expected_frames}",
        decoder.num_frames()
    );
}

#[test]
fn sound_is_detected_in_videos_and_audio() {
    use mediaa_core::formats::AudioFormat;
    use mediaa_media::info::has_sound;
    let dir = tempfile::tempdir().unwrap();
    let with = record(
        dir.path(),
        request(
            dir.path(),
            vec![pattern(64, 48)],
            vec![Sine::boxed(2)],
            OutputKind::Video(VideoContainer::Mp4),
        ),
        0.6,
    );
    assert!(has_sound(&with[0]).unwrap());
    let silent = record(
        dir.path(),
        request(
            dir.path(),
            vec![pattern(64, 48)],
            vec![],
            OutputKind::Video(VideoContainer::Mkv),
        ),
        0.6,
    );
    assert!(!has_sound(&silent[0]).unwrap());
    let audio = record(
        dir.path(),
        request(
            dir.path(),
            vec![],
            vec![Sine::boxed(1)],
            OutputKind::Audio(AudioFormat::Mp3),
        ),
        0.6,
    );
    assert!(has_sound(&audio[0]).unwrap());
    assert!(has_sound(&dir.path().join("missing.mp4")).is_err());
}

#[test]
fn cancelled_conversion_removes_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(64, 48)],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let mp4 = record(dir.path(), req, 0.5).remove(0);
    let out = dir.path().join("cancelled.gif");
    let cancel = AtomicBool::new(true);
    let err = convert::convert(
        &mediaa_core::paths::SourcePath::new(&mp4),
        &mediaa_core::paths::OutputPath::new(&out),
        Target::Anim(AnimFormat::Gif),
        &opts(),
        &mut |_| {},
        &cancel,
    )
    .unwrap_err();
    assert!(mediaa_media::job::is_cancelled(&err), "{err:#}");
    assert!(!out.exists());
}

#[test]
fn thumbnails_for_video_animation_and_image() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(160, 96)],
        vec![],
        OutputKind::Anim(AnimFormat::Gif),
    );
    let gif = record(dir.path(), req, 0.6).remove(0);
    let t = mediaa_media::thumbnail::thumbnail(&gif, MediaKind::Animation, 80).unwrap();
    assert_eq!((t.width, t.height), (80, 48));

    let png = dir.path().join("still.png");
    mediaa_storage::screenshot::save(
        &Frame::filled(200, 100, [1, 2, 3, 255]),
        mediaa_core::ImageFormat::Png,
        &png,
    )
    .unwrap();
    let t = mediaa_media::thumbnail::thumbnail(&png, MediaKind::Image, 50).unwrap();
    assert_eq!((t.width, t.height), (50, 25));
}

#[test]
fn output_height_downscales_keeping_aspect() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(
        dir.path(),
        vec![pattern(320, 192)],
        vec![],
        OutputKind::Video(VideoContainer::Mp4),
    );
    req.set_max_height(Some(96));
    let out = record(dir.path(), req, 0.6).remove(0);
    let info = probe(&out).unwrap();
    assert_eq!((info.width, info.height), (160, 96));

    let dir = tempfile::tempdir().unwrap();
    let mut req = request(
        dir.path(),
        vec![pattern(128, 72)],
        vec![],
        OutputKind::Video(VideoContainer::Mkv),
    );
    req.set_max_height(Some(1080));
    let out = record(dir.path(), req, 0.6).remove(0);
    assert_eq!(probe(&out).unwrap().height, 72);
}

#[test]
fn library_details_and_hover_preview_frames() {
    use mediaa_media::preview::{details, preview_frames};
    let formats = [
        OutputKind::Video(VideoContainer::Mp4),
        OutputKind::Anim(AnimFormat::Gif),
        OutputKind::Anim(AnimFormat::Webp),
    ];
    for output in formats {
        let dir = tempfile::tempdir().unwrap();
        let req = request(
            dir.path(),
            vec![pattern(160, 96)],
            vec![Sine::boxed(2)],
            output,
        );
        let path = record(dir.path(), req, 1.5).remove(0);
        let kind = MediaKind::detect(&path).unwrap();
        let info = details(&path, kind).unwrap();
        assert_eq!((info.width, info.height), (160, 96), "{output:?}: {info:?}");
        assert!(
            info.duration_secs().is_some_and(|d| d > 1.0),
            "{output:?}: {info:?}"
        );
        assert_eq!(
            info.has_audio(),
            output == OutputKind::Video(VideoContainer::Mp4),
            "{output:?}"
        );
        if matches!(output, OutputKind::Anim(_)) {
            let proxy = mediaa_media::preview_cache::ready_proxy(&path)
                .unwrap_or_else(|| panic!("{output:?}: preview proxy missing"));
            let proxy_info = probe(&proxy).unwrap();
            assert!(proxy_info.has_video(), "{output:?}: {proxy_info:?}");
            assert!(
                proxy_info.duration_secs().is_some_and(|d| d > 1.0),
                "{output:?}: {proxy_info:?}"
            );
        }

        let mut frames = Vec::new();
        preview_frames(&path, 80, 0.1, 0.0, &AtomicBool::new(false), &mut |f| {
            frames.push((f.at, f.frame.width));
            true
        })
        .unwrap();
        assert!(frames.len() >= 5, "{output:?}: {} frames", frames.len());
        assert!(
            frames.iter().all(|(_, w)| *w == 80),
            "{output:?}: scaled to 80 px"
        );
        assert!(
            frames.windows(2).all(|p| p[1].0 > p[0].0),
            "{output:?}: increasing times"
        );
        assert_eq!(frames[0].0, 0.0);

        let mut n = 0;
        preview_frames(&path, 80, 0.1, 0.0, &AtomicBool::new(false), &mut |_| {
            n += 1;
            n < 2
        })
        .unwrap();
        assert_eq!(n, 2);

        let mut later = Vec::new();
        preview_frames(&path, 80, 0.1, 0.8, &AtomicBool::new(false), &mut |f| {
            later.push(f.at);
            true
        })
        .unwrap();
        assert!(
            later.first().is_some_and(|t| (0.75..1.0).contains(t)),
            "{output:?}: {later:?}"
        );
        assert!(later.len() < frames.len(), "{output:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("shot.png");
    image::RgbaImage::new(30, 20).save(&png).unwrap();
    for output in formats {
        let dir = tempfile::tempdir().unwrap();
        let req = request(dir.path(), vec![pattern(160, 96)], vec![], output);
        let path = record(dir.path(), req, 1.0).remove(0);
        let thumb =
            mediaa_media::thumbnail::thumbnail(&path, MediaKind::detect(&path).unwrap(), 64)
                .unwrap();
        assert_eq!(thumb.width.max(thumb.height), 64, "{output:?}");
    }
    let info = details(&png, MediaKind::Image).unwrap();
    assert_eq!((info.width, info.height, info.duration_us), (30, 20, None));
}

#[test]
fn viewer_audio_follows_start_and_speed() {
    use mediaa_media::preview::decode_audio;
    let dir = tempfile::tempdir().unwrap();
    let req = request(
        dir.path(),
        vec![pattern(64, 48)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let path = record(dir.path(), req, 2.0).remove(0);
    let secs = |start: f64, speed: f64| {
        let count = Arc::new(std::sync::Mutex::new(0usize));
        let c = count.clone();
        decode_audio(
            &path,
            start,
            speed,
            48_000,
            Arc::new(AtomicBool::new(false)),
            move |s| {
                *c.lock().unwrap() += s.len();
                true
            },
        )
        .unwrap();
        let n = *count.lock().unwrap();
        n as f64 / 2.0 / 48_000.0
    };
    let full = secs(0.0, 1.0);
    assert!((1.6..2.5).contains(&full), "{full}");
    let fast = secs(0.0, 2.0);
    assert!((fast - full / 2.0).abs() < 0.2, "{fast} vs {full}");
    let tail = secs(1.0, 1.0);
    assert!((tail - (full - 1.0)).abs() < 0.2, "{tail} vs {full}");
    decode_audio(
        &path,
        0.0,
        1.0,
        48_000,
        Arc::new(AtomicBool::new(false)),
        |_| false,
    )
    .unwrap();
}
