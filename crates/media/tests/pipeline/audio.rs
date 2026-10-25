use super::*;

#[test]
fn audio_only_recordings_in_every_format() {
    use mediaa_core::formats::AudioFormat;
    for (format, codec) in [
        (AudioFormat::Mp3, "mp3"),
        (AudioFormat::M4a, "aac"),
        (AudioFormat::Wav, "pcm_s16le"),
        (AudioFormat::Flac, "flac"),
        (AudioFormat::Ogg, "opus"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let req = request(
            dir.path(),
            vec![],
            vec![Sine::boxed(2), Sine::boxed(1)],
            OutputKind::Audio(format),
        );
        let paths = record(dir.path(), req, 1.2);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].extension().unwrap(), format.extension());
        assert_eq!(title(&paths[0]), "Recording test");
        let info = probe(&paths[0]).unwrap();
        assert!(!info.has_video(), "{format:?}");
        assert_eq!(
            (info.audio_streams, info.audio_codec.as_deref()),
            (1, Some(codec)),
            "{format:?}"
        );
        assert_duration(&paths[0], 1.2, 0.5);
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(RecordingRequest::builder(
        OutputKind::Audio(AudioFormat::Mp3),
        vec![],
        vec![],
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(dir.path().join("work")),
        "Audio".into(),
    )
    .build()
    .is_err());
    assert!(RecordingRequest::builder(
        OutputKind::Audio(AudioFormat::Mp3),
        vec![pattern(64, 48)],
        vec![Sine::boxed(2)],
        mediaa_core::paths::MediaDir::new(dir.path().join("out")),
        mediaa_core::paths::WorkDir::new(dir.path().join("work")),
        "Audio".into(),
    )
    .build()
    .is_err());
}

#[test]
fn compression_options_and_size_estimates() {
    use mediaa_core::formats::AudioFormat;
    use mediaa_media::convert::{Effort, StillFormat, VideoFormat};
    let dir = tempfile::tempdir().unwrap();
    let cancel = AtomicBool::new(false);
    let size = |p: &Path| std::fs::metadata(p).unwrap().len();
    let run = |input: &Path, target: Target, s: &ConvertSettings, name: &str| {
        let out = dir.path().join(format!("{name}.{}", target.extension()));
        let input = mediaa_core::paths::SourcePath::new(input);
        let output = mediaa_core::paths::OutputPath::new(&out);
        convert::convert(&input, &output, target, s, &mut |_| {}, &cancel)
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
        out
    };

    let noise = |x: u32, y: u32, k: u32| {
        (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ k.wrapping_mul(83_492_791)) % 24
    };
    let src = image::RgbaImage::from_fn(300, 200, |x, y| {
        let c = |base: u32, k| (base + noise(x, y, k)).min(255) as u8;
        if x < 10 {
            image::Rgba([0; 4])
        } else {
            image::Rgba([c(x * 200 / 300, 1), c(y, 2), c((x + y) / 3, 3), 255])
        }
    });
    let png = dir.path().join("src.png");
    src.save(&png).unwrap();
    let lossless = run(
        &png,
        Target::Still(StillFormat::Png),
        &ConvertSettings {
            effort: Effort::Max,
            ..ConvertSettings::default()
        },
        "lossless",
    );
    assert!(
        image::open(&lossless).unwrap().to_rgba8() == src,
        "lossless PNG keeps every pixel"
    );
    let palette = run(
        &png,
        Target::Still(StillFormat::Png),
        &ConvertSettings {
            png_colors: Some(32),
            ..ConvertSettings::default()
        },
        "palette",
    );
    assert_eq!(image::image_dimensions(&palette).unwrap(), (300, 200));
    assert!(
        size(&palette) < size(&lossless) / 2,
        "palette {} vs {}",
        size(&palette),
        size(&lossless)
    );
    let webp = run(
        &png,
        Target::Still(StillFormat::Webp),
        &ConvertSettings {
            webp_lossless: true,
            ..ConvertSettings::default()
        },
        "lossless_webp",
    );
    assert!(
        image::open(&webp).unwrap().to_rgba8() == src,
        "lossless WebP keeps every pixel"
    );
    let s = ConvertSettings {
        png_colors: Some(32),
        ..ConvertSettings::default()
    };
    assert_eq!(
        convert::estimate_size(
            &mediaa_core::paths::SourcePath::new(&png),
            Target::Still(StillFormat::Png),
            &s,
            &cancel
        )
        .unwrap(),
        size(&palette)
    );

    let req = request(
        dir.path(),
        vec![pattern(96, 64)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mp4),
    );
    let mp4 = record(dir.path(), req, 1.2).remove(0);
    for effort in Effort::ALL {
        for format in [VideoFormat::Mp4, VideoFormat::Webm, VideoFormat::Avi] {
            let s = ConvertSettings {
                effort,
                max_fps: Some(5),
                mono: true,
                audio_kbps: 96,
                ..ConvertSettings::default()
            };
            let p = run(
                &mp4,
                Target::Video(format),
                &s,
                &format!("{effort:?}_{format:?}"),
            );
            let info = probe(&p).unwrap();
            assert!(
                info.has_audio() && info.fps <= 5.5,
                "{effort:?} {format:?}: {info:?}"
            );
        }
    }
    let s = ConvertSettings::default();
    let actual = size(&run(&mp4, Target::Video(VideoFormat::Mp4), &s, "estimated"));
    let estimate = convert::estimate_size(
        &mediaa_core::paths::SourcePath::new(&mp4),
        Target::Video(VideoFormat::Mp4),
        &s,
        &cancel,
    )
    .unwrap();
    assert!(
        estimate.abs_diff(actual) * 10 < actual,
        "short files are converted whole: {estimate} vs {actual}"
    );

    let wav = dir.path().join("long.wav");
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for i in 0..48_000 * 20 {
        let v = ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    let stereo = run(
        &wav,
        Target::Audio(AudioFormat::Wav),
        &ConvertSettings::default(),
        "stereo",
    );
    let mono = run(
        &wav,
        Target::Audio(AudioFormat::Wav),
        &ConvertSettings {
            mono: true,
            ..ConvertSettings::default()
        },
        "mono",
    );
    assert!(
        size(&mono).abs_diff(size(&stereo) / 2) < 1000,
        "{} vs {}",
        size(&mono),
        size(&stereo)
    );
    for effort in Effort::ALL {
        run(
            &wav,
            Target::Audio(AudioFormat::Flac),
            &ConvertSettings {
                effort,
                ..ConvertSettings::default()
            },
            &format!("{effort:?}"),
        );
    }
    let s = ConvertSettings {
        audio_kbps: 128,
        ..ConvertSettings::default()
    };
    let estimate = convert::estimate_size(
        &mediaa_core::paths::SourcePath::new(&wav),
        Target::Audio(AudioFormat::Mp3),
        &s,
        &cancel,
    )
    .unwrap();
    assert!((300_000..345_000).contains(&estimate), "{estimate}");
}

#[test]
fn decodes_mono_for_speech() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("stereo.wav");
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
    let decoded = |start: f64| {
        let got = Arc::new(std::sync::Mutex::new(Vec::<f32>::new()));
        let g = got.clone();
        mediaa_media::preview::decode_mono(
            &wav,
            start,
            16_000,
            Arc::new(AtomicBool::new(false)),
            move |s| {
                g.lock().unwrap().extend_from_slice(s);
                true
            },
        )
        .unwrap();
        let v = got.lock().unwrap().clone();
        v
    };
    let all = decoded(0.0);
    assert!(all.len().abs_diff(48_000) < 400, "{}", all.len());
    assert!(all.iter().any(|s| s.abs() > 0.2), "the tone is there");
    let rest = decoded(2.0);
    assert!(rest.len().abs_diff(16_000) < 400, "{}", rest.len());
}
