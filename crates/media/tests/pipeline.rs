//! End-to-end tests of the media pipeline with synthetic capture sources.
//! They run real FFmpeg encodes (statically linked into the test binary).

use mediaa_core::audio::AudioSink;
use mediaa_core::formats::{AnimFormat, MediaKind, OutputKind, VideoContainer};
use mediaa_core::settings::AnimSettings;
use mediaa_core::source::{AudioSource, FrameSource, SourceFactory};
use mediaa_core::{naming, Frame};
use mediaa_media::convert::{self, ConvertSettings, Target};
use mediaa_media::info::probe;
use mediaa_media::recorder::{RecordingRequest, RecordingSession, SessionEvent};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// A moving square on a gradient, so encoders see real motion.
struct Pattern {
    w: u32,
    h: u32,
    n: u32,
}

impl FrameSource for Pattern {
    fn name(&self) -> String {
        format!("Pattern {}x{}", self.w, self.h)
    }
    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }
    fn grab(&mut self) -> anyhow::Result<Frame> {
        self.n += 1;
        let mut data = Vec::with_capacity((self.w * self.h * 4) as usize);
        let sq = (self.n * 4) % self.w;
        for y in 0..self.h {
            for x in 0..self.w {
                let inside = x >= sq && x < sq + 16 && (8..24).contains(&y);
                if inside {
                    data.extend_from_slice(&[255, 255, 255, 255]);
                } else {
                    data.extend_from_slice(&[(x * 255 / self.w) as u8, (y * 255 / self.h) as u8, 90, 255]);
                }
            }
        }
        Ok(Frame::new(self.w, self.h, data))
    }
}

fn pattern(w: u32, h: u32) -> SourceFactory {
    Box::new(move || Ok(Box::new(Pattern { w, h, n: 0 }) as Box<dyn FrameSource>))
}

/// 440 Hz sine generated on a background thread, like a real device callback.
struct Sine {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    channels: u16,
}

impl Sine {
    fn boxed(channels: u16) -> Box<dyn AudioSource> {
        Box::new(Sine {
            stop: Arc::new(AtomicBool::new(false)),
            thread: None,
            channels,
        })
    }
}

impl AudioSource for Sine {
    fn name(&self) -> String {
        "sine".into()
    }
    fn start(&mut self, open: &mut dyn FnMut(u32, u16) -> anyhow::Result<AudioSink>) -> anyhow::Result<()> {
        let sink = open(48_000, self.channels)?;
        let stop = self.stop.clone();
        let ch = self.channels as usize;
        self.thread = Some(std::thread::spawn(move || {
            let mut t = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let mut buf = Vec::with_capacity(480 * ch);
                for _ in 0..480 {
                    let v = (t as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 0.3;
                    buf.extend(std::iter::repeat_n(v, ch));
                    t += 1;
                }
                sink.push(&buf);
                std::thread::sleep(Duration::from_millis(10));
            }
        }));
        Ok(())
    }
    fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Failing;
impl AudioSource for Failing {
    fn name(&self) -> String {
        "broken mic".into()
    }
    fn start(&mut self, _: &mut dyn FnMut(u32, u16) -> anyhow::Result<AudioSink>) -> anyhow::Result<()> {
        anyhow::bail!("device unplugged")
    }
    fn stop(&mut self) {}
}

/// Title part of a MediaA file name.
fn title(p: &Path) -> String {
    naming::parse_path(p).unwrap_or_else(|| panic!("not a media name: {}", p.display())).title
}

fn request(dir: &Path, sources: Vec<SourceFactory>, audio: Vec<Box<dyn AudioSource>>, output: OutputKind) -> RecordingRequest {
    RecordingRequest {
        sources,
        audio,
        output,
        fps: 20,
        video_quality: 70,
        max_height: None,
        audio_in_every_file: false,
        audio_kbps: 128,
        anim: AnimSettings { fps: 10, max_height: 0, ..AnimSettings::default() },
        out_dir: dir.join("out"),
        work_dir: dir.join("work"),
        title: "Recording test".into(),
    }
}

/// Waits for the terminal event, collecting warnings.
fn finish(session: RecordingSession, save: bool) -> (SessionEvent, Vec<String>) {
    let events = session.events();
    session.stop(save, Arc::new(AtomicBool::new(false))).join().unwrap();
    let mut warnings = Vec::new();
    loop {
        match events.recv_timeout(Duration::from_secs(60)).expect("terminal event") {
            SessionEvent::Warning(w) => warnings.push(w),
            SessionEvent::Finalizing(_) => {}
            other => return (other, warnings),
        }
    }
}

fn record(dir: &Path, req: RecordingRequest, secs: f32) -> Vec<PathBuf> {
    let _ = dir;
    let picture = !matches!(req.output, OutputKind::Audio(_));
    let session = RecordingSession::start(req).expect("start");
    std::thread::sleep(Duration::from_secs_f32(secs));
    assert_eq!(session.frames_captured() > 0, picture, "frames only for picture recordings");
    match finish(session, true) {
        (SessionEvent::Finished(paths), _) => paths,
        (other, _) => panic!("unexpected {other:?}"),
    }
}

fn opts() -> ConvertSettings {
    ConvertSettings {
        anim: AnimSettings { fps: 10, max_height: 33, ..AnimSettings::default() },
        quality: 60,
        ..ConvertSettings::default()
    }
}

fn assert_duration(path: &Path, expected: f64, tolerance: f64) {
    let info = probe(path).unwrap();
    let d = info.duration_secs().expect("duration");
    assert!((d - expected).abs() <= tolerance, "{}: duration {d}, expected ~{expected}", path.display());
}

#[test]
fn static_build_has_required_codecs() {
    for (name, ok) in mediaa_media::capabilities() {
        assert!(ok, "{name} missing from the FFmpeg build");
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
    assert_eq!(info.audio_streams, 1, "two sources are mixed into one track");
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
    assert_eq!(names, ["Recording test [1 of 2] - Pattern 64x48 (audio)", "Recording test [2 of 2] - Pattern 96x64"]);
    assert_eq!(naming::parse_path(&paths[0]).unwrap().taken, naming::parse_path(&paths[1]).unwrap().taken, "one start date for the set");
    assert!(probe(&paths[0]).unwrap().has_audio(), "sound goes into the first file");
    assert!(!probe(&paths[1]).unwrap().has_audio(), "and only there");
    assert_eq!(probe(&paths[1]).unwrap().width, 96);
}

#[test]
fn audio_can_go_into_every_linked_file() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(
        dir.path(),
        vec![pattern(64, 48), pattern(96, 64)],
        vec![Sine::boxed(2)],
        OutputKind::Video(VideoContainer::Mkv),
    );
    req.audio_in_every_file = true;
    let mut paths = record(dir.path(), req, 1.0);
    paths.sort();
    let names: Vec<_> = paths.iter().map(|p| title(p)).collect();
    assert_eq!(names, ["Recording test [1 of 2] - Pattern 64x48", "Recording test [2 of 2] - Pattern 96x64"]);
    assert!(paths.iter().all(|p| probe(p).unwrap().has_audio()), "every file has the sound");
}

#[test]
fn records_gif_and_webp_directly() {
    for (fmt, codec) in [(AnimFormat::Gif, "gif"), (AnimFormat::Webp, "webp")] {
        let dir = tempfile::tempdir().unwrap();
        let req = request(dir.path(), vec![pattern(80, 48)], vec![Sine::boxed(2)], OutputKind::Anim(fmt));
        let paths = record(dir.path(), req, 1.0);
        assert_eq!(paths[0].extension().unwrap(), fmt.extension());
        assert_eq!(MediaKind::detect(&paths[0]), Some(MediaKind::Animation));
        let info = probe(&paths[0]).unwrap();
        assert!(info.video_codec.as_deref().unwrap().contains(codec), "{info:?}");
        assert_eq!((info.width, info.height), (80, 48), "{info:?}");
        assert!(info.duration_secs().is_some_and(|d| d > 0.5), "{info:?}");
    }
}

#[test]
fn pause_is_excluded_from_duration() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), vec![pattern(64, 48)], vec![Sine::boxed(2)], OutputKind::Video(VideoContainer::Mkv));
    let session = RecordingSession::start(req).unwrap();
    std::thread::sleep(Duration::from_millis(800));
    session.pause();
    assert!(session.is_paused());
    std::thread::sleep(Duration::from_millis(1500));
    session.resume();
    std::thread::sleep(Duration::from_millis(800));
    let elapsed = session.elapsed().as_secs_f64();
    assert!((elapsed - 1.6).abs() < 0.3, "elapsed {elapsed}");
    let (event, _) = finish(session, true);
    let SessionEvent::Finished(paths) = event else { panic!("{event:?}") };
    assert_duration(&paths[0], 1.6, 0.4);
}

#[test]
fn cancel_discards_everything() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), vec![pattern(64, 48)], vec![Sine::boxed(2)], OutputKind::Video(VideoContainer::Mp4));
    let session = RecordingSession::start(req).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let (event, _) = finish(session, false);
    assert_eq!(event, SessionEvent::Cancelled);
    assert_eq!(std::fs::read_dir(dir.path().join("out")).unwrap().count(), 0);
    assert!(!dir.path().join("work").exists());
}

#[test]
fn failing_audio_source_is_a_warning_not_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), vec![pattern(64, 48)], vec![Box::new(Failing)], OutputKind::Video(VideoContainer::Mp4));
    let session = RecordingSession::start(req).unwrap();
    assert!(!session.has_audio());
    std::thread::sleep(Duration::from_millis(500));
    let (event, warnings) = finish(session, true);
    assert!(matches!(event, SessionEvent::Finished(_)), "{event:?}");
    assert!(warnings.iter().any(|w| w.contains("broken mic")), "{warnings:?}");
}

#[test]
fn source_that_fails_to_open_aborts_start() {
    let dir = tempfile::tempdir().unwrap();
    let bad: SourceFactory = Box::new(|| anyhow::bail!("window was closed"));
    let err = RecordingSession::start(request(dir.path(), vec![bad], vec![], OutputKind::default()))
        .err()
        .expect("must fail");
    assert!(format!("{err:#}").contains("window was closed"));
}

#[test]
fn conversions_between_popular_formats() {
    use mediaa_media::convert::{StillFormat, Target, VideoFormat};
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), vec![pattern(96, 64)], vec![Sine::boxed(2)], OutputKind::Video(VideoContainer::Mp4));
    let mp4 = record(dir.path(), req, 1.2).remove(0);
    let out = dir.path().join("conv");
    let cancel = AtomicBool::new(false);
    let run = |input: &Path, target: Target, s: &ConvertSettings| {
        let output = convert::output_path(input, target, None, &out);
        let mut last = 0.0f32;
        convert::convert(input, &output, target, s, &mut |p| last = p, &cancel).unwrap_or_else(|e| panic!("{target:?}: {e:#}"));
        assert_eq!(last, 1.0);
        assert_eq!(output.extension().unwrap(), target.extension());
        output
    };
    let s = opts();

    // Video → every video format, with the codecs that format calls for.
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
    // Same format, smaller and silent.
    let small = run(&mp4, Target::Video(VideoFormat::Mp4), &ConvertSettings { remove_audio: true, max_height: Some(32), quality: 30, ..opts() });
    let info = probe(&small).unwrap();
    assert!(!info.has_audio());
    assert_eq!((info.width, info.height), (48, 32), "downscaled, aspect kept");
    assert_eq!(title(&small), "Recording test");

    // Video → animations → video.
    let gif = run(&mp4, Target::Anim(AnimFormat::Gif), &s);
    assert_eq!((probe(&gif).unwrap().width, probe(&gif).unwrap().height), (50, 33), "height limit applied");
    let webp = run(&mp4, Target::Anim(AnimFormat::Webp), &s);
    assert_eq!(MediaKind::detect(&webp), Some(MediaKind::Animation));
    let from_gif = run(&gif, Target::Video(VideoFormat::Webm), &s);
    let info = probe(&from_gif).unwrap();
    assert_eq!(info.video_codec.as_deref(), Some("vp9"));
    // 96x64 limited to height 33 -> 50x33, padded to even 50x34 for video.
    assert_eq!((info.width, info.height), (50, 34), "even height after padding");
    assert!(probe(&run(&webp, Target::Video(VideoFormat::Mp4), &s)).unwrap().has_video());
    assert!(probe(&run(&webp, Target::Anim(AnimFormat::Gif), &s)).unwrap().has_video());

    // Image → every image format.
    let png = dir.path().join("shot_2026-01-01_10-00-00.png");
    image::RgbaImage::from_fn(120, 80, |x, y| image::Rgba([x as u8, y as u8, 200, 255])).save(&png).unwrap();
    for format in StillFormat::ALL {
        let p = run(&png, Target::Still(format), &s);
        let img = image::open(&p).unwrap_or_else(|e| panic!("{format:?}: {e}"));
        assert_eq!((img.width(), img.height()), (120, 80), "{format:?}");
    }
    let half = run(&png, Target::Still(StillFormat::Jpeg), &ConvertSettings { max_height: Some(40), quality: 50, ..opts() });
    assert_eq!(image::image_dimensions(&half).unwrap(), (60, 40));
    // Video / animation → image of the first frame.
    for (input, size) in [(&mp4, (96, 64)), (&gif, (50, 33)), (&webp, (50, 33))] {
        for format in [StillFormat::Png, StillFormat::Jpeg] {
            let p = run(input, Target::Still(format), &s);
            assert_eq!(image::image_dimensions(&p).unwrap(), size, "{} → {format:?}", input.display());
        }
    }

    // Video → sound only; sound → other sound formats.
    for format in mediaa_core::formats::AudioFormat::ALL {
        let p = run(&mp4, Target::Audio(format), &s);
        let info = probe(&p).unwrap();
        assert!(info.has_audio() && !info.has_video(), "{format:?}: {info:?}");
        let again = run(&p, Target::Audio(mediaa_core::formats::AudioFormat::Mp3), &s);
        assert_eq!(probe(&again).unwrap().audio_codec.as_deref(), Some("mp3"), "{format:?} → MP3");
        assert_eq!(MediaKind::detect(&p), Some(MediaKind::Audio));
    }

    let hi = run(&png, Target::Still(StillFormat::Jpeg), &ConvertSettings { quality: 100, ..opts() });
    let lo = run(&png, Target::Still(StillFormat::Jpeg), &ConvertSettings { quality: 5, ..opts() });
    assert!(std::fs::metadata(&lo).unwrap().len() < std::fs::metadata(&hi).unwrap().len(), "lower quality, smaller file");
}

#[test]
fn sound_is_detected_in_videos_and_audio() {
    use mediaa_core::formats::AudioFormat;
    use mediaa_media::info::has_sound;
    let dir = tempfile::tempdir().unwrap();
    let with = record(dir.path(), request(dir.path(), vec![pattern(64, 48)], vec![Sine::boxed(2)], OutputKind::Video(VideoContainer::Mp4)), 0.6);
    assert!(has_sound(&with[0]).unwrap());
    let silent = record(dir.path(), request(dir.path(), vec![pattern(64, 48)], vec![], OutputKind::Video(VideoContainer::Mkv)), 0.6);
    assert!(!has_sound(&silent[0]).unwrap());
    let audio = record(dir.path(), request(dir.path(), vec![], vec![Sine::boxed(1)], OutputKind::Audio(AudioFormat::Mp3)), 0.6);
    assert!(has_sound(&audio[0]).unwrap());
    assert!(has_sound(&dir.path().join("missing.mp4")).is_err());
}

#[test]
fn cancelled_conversion_removes_partial_output() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), vec![pattern(64, 48)], vec![], OutputKind::Video(VideoContainer::Mp4));
    let mp4 = record(dir.path(), req, 0.5).remove(0);
    let out = dir.path().join("cancelled.gif");
    let cancel = AtomicBool::new(true);
    let err = convert::convert(&mp4, &out, Target::Anim(AnimFormat::Gif), &opts(), &mut |_| {}, &cancel).unwrap_err();
    assert!(mediaa_media::job::is_cancelled(&err), "{err:#}");
    assert!(!out.exists());
}

#[test]
fn thumbnails_for_video_animation_and_image() {
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), vec![pattern(160, 96)], vec![], OutputKind::Anim(AnimFormat::Gif));
    let gif = record(dir.path(), req, 0.6).remove(0);
    let t = mediaa_media::thumbnail::thumbnail(&gif, MediaKind::Animation, 80).unwrap();
    assert_eq!((t.width, t.height), (80, 48));

    let png = dir.path().join("still.png");
    mediaa_core::screenshot::save(&Frame::filled(200, 100, [1, 2, 3, 255]), mediaa_core::ImageFormat::Png, &png).unwrap();
    let t = mediaa_media::thumbnail::thumbnail(&png, MediaKind::Image, 50).unwrap();
    assert_eq!((t.width, t.height), (50, 25));
}

#[test]
fn output_height_downscales_keeping_aspect() {
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), vec![pattern(320, 192)], vec![], OutputKind::Video(VideoContainer::Mp4));
    req.max_height = Some(96);
    let out = record(dir.path(), req, 0.6).remove(0);
    let info = probe(&out).unwrap();
    assert_eq!((info.width, info.height), (160, 96));

    // Never upscales: a limit above the source height keeps the source size.
    let dir = tempfile::tempdir().unwrap();
    let mut req = request(dir.path(), vec![pattern(128, 72)], vec![], OutputKind::Video(VideoContainer::Mkv));
    req.max_height = Some(1080);
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
        let req = request(dir.path(), vec![pattern(160, 96)], vec![Sine::boxed(2)], output);
        let path = record(dir.path(), req, 1.5).remove(0);
        let kind = MediaKind::detect(&path).unwrap();
        let info = details(&path, kind).unwrap();
        assert_eq!((info.width, info.height), (160, 96), "{output:?}: {info:?}");
        assert!(info.duration_secs().is_some_and(|d| d > 1.0), "{output:?}: {info:?}");
        assert_eq!(info.has_audio(), output == OutputKind::Video(VideoContainer::Mp4), "{output:?}");

        let mut frames = Vec::new();
        preview_frames(&path, 80, 0.1, 0.0, &AtomicBool::new(false), &mut |f| {
            frames.push((f.at, f.frame.width));
            true
        })
        .unwrap();
        assert!(frames.len() >= 5, "{output:?}: {} frames", frames.len());
        assert!(frames.iter().all(|(_, w)| *w == 80), "{output:?}: scaled to 80 px");
        assert!(frames.windows(2).all(|p| p[1].0 > p[0].0), "{output:?}: increasing times");
        assert_eq!(frames[0].0, 0.0);

        // Cancelling stops early.
        let mut n = 0;
        preview_frames(&path, 80, 0.1, 0.0, &AtomicBool::new(false), &mut |_| {
            n += 1;
            n < 2
        })
        .unwrap();
        assert_eq!(n, 2);

        // Starting in the middle (where a hover preview was left): times stay clip times.
        let mut later = Vec::new();
        preview_frames(&path, 80, 0.1, 0.8, &AtomicBool::new(false), &mut |f| {
            later.push(f.at);
            true
        })
        .unwrap();
        assert!(later.first().is_some_and(|t| (0.75..1.0).contains(t)), "{output:?}: {later:?}");
        assert!(later.len() < frames.len(), "{output:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    let png = dir.path().join("shot.png");
    image::RgbaImage::new(30, 20).save(&png).unwrap();
    // Thumbnails of every recorded format (animated WebP goes through image-webp).
    for output in formats {
        let dir = tempfile::tempdir().unwrap();
        let req = request(dir.path(), vec![pattern(160, 96)], vec![], output);
        let path = record(dir.path(), req, 1.0).remove(0);
        let thumb = mediaa_media::thumbnail::thumbnail(&path, MediaKind::detect(&path).unwrap(), 64).unwrap();
        assert_eq!(thumb.width.max(thumb.height), 64, "{output:?}");
    }
    let info = details(&png, MediaKind::Image).unwrap();
    assert_eq!((info.width, info.height, info.duration_us), (30, 20, None));
}

#[test]
fn viewer_audio_follows_start_and_speed() {
    use mediaa_media::preview::decode_audio;
    let dir = tempfile::tempdir().unwrap();
    let req = request(dir.path(), vec![pattern(64, 48)], vec![Sine::boxed(2)], OutputKind::Video(VideoContainer::Mp4));
    let path = record(dir.path(), req, 2.0).remove(0);
    let secs = |start: f64, speed: f64| {
        let count = Arc::new(std::sync::Mutex::new(0usize));
        let c = count.clone();
        decode_audio(&path, start, speed, 48_000, Arc::new(AtomicBool::new(false)), move |s| {
            *c.lock().unwrap() += s.len();
            true
        })
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
    // Stopping from the sink ends decoding quietly.
    decode_audio(&path, 0.0, 1.0, 48_000, Arc::new(AtomicBool::new(false)), |_| false).unwrap();
}

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
        // Two sources (like microphone + system sound) mixed into one file.
        let req = request(dir.path(), vec![], vec![Sine::boxed(2), Sine::boxed(1)], OutputKind::Audio(format));
        let paths = record(dir.path(), req, 1.2);
        assert_eq!(paths.len(), 1);
        assert_eq!(paths[0].extension().unwrap(), format.extension());
        assert_eq!(title(&paths[0]), "Recording test");
        let info = probe(&paths[0]).unwrap();
        assert!(!info.has_video(), "{format:?}");
        assert_eq!((info.audio_streams, info.audio_codec.as_deref()), (1, Some(codec)), "{format:?}");
        assert_duration(&paths[0], 1.2, 0.5);
    }
    // Audio recordings need sound sources and no picture.
    let dir = tempfile::tempdir().unwrap();
    assert!(RecordingSession::start(request(dir.path(), vec![], vec![], OutputKind::Audio(AudioFormat::Mp3))).is_err());
    assert!(RecordingSession::start(request(dir.path(), vec![pattern(64, 48)], vec![Sine::boxed(2)], OutputKind::Audio(AudioFormat::Mp3))).is_err());
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
        convert::convert(input, &out, target, s, &mut |_| {}, &cancel).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        out
    };

    // PNG: oxipng keeps every pixel; a palette makes it much smaller.
    // Photo-like: gradients with noise (hard to compress losslessly), a transparent strip.
    let noise = |x: u32, y: u32, k: u32| (x.wrapping_mul(73_856_093) ^ y.wrapping_mul(19_349_663) ^ k.wrapping_mul(83_492_791)) % 24;
    let src = image::RgbaImage::from_fn(300, 200, |x, y| {
        let c = |base: u32, k| (base + noise(x, y, k)).min(255) as u8;
        if x < 10 { image::Rgba([0; 4]) } else { image::Rgba([c(x * 200 / 300, 1), c(y, 2), c((x + y) / 3, 3), 255]) }
    });
    let png = dir.path().join("src.png");
    src.save(&png).unwrap();
    let lossless = run(&png, Target::Still(StillFormat::Png), &ConvertSettings { effort: Effort::Max, ..ConvertSettings::default() }, "lossless");
    assert!(image::open(&lossless).unwrap().to_rgba8() == src, "lossless PNG keeps every pixel");
    let palette = run(&png, Target::Still(StillFormat::Png), &ConvertSettings { png_colors: Some(32), ..ConvertSettings::default() }, "palette");
    assert_eq!(image::image_dimensions(&palette).unwrap(), (300, 200));
    assert!(size(&palette) < size(&lossless) / 2, "palette {} vs {}", size(&palette), size(&lossless));
    // Lossless WebP keeps every pixel too.
    let webp = run(&png, Target::Still(StillFormat::Webp), &ConvertSettings { webp_lossless: true, ..ConvertSettings::default() }, "lossless_webp");
    assert!(image::open(&webp).unwrap().to_rgba8() == src, "lossless WebP keeps every pixel");
    // Images are estimated by converting them whole: exact.
    let s = ConvertSettings { png_colors: Some(32), ..ConvertSettings::default() };
    assert_eq!(convert::estimate_size(&png, Target::Still(StillFormat::Png), &s, &cancel).unwrap(), size(&palette));

    // Video: every effort, a frame-rate cap and mono sound.
    let req = request(dir.path(), vec![pattern(96, 64)], vec![Sine::boxed(2)], OutputKind::Video(VideoContainer::Mp4));
    let mp4 = record(dir.path(), req, 1.2).remove(0);
    for effort in Effort::ALL {
        for format in [VideoFormat::Mp4, VideoFormat::Webm, VideoFormat::Avi] {
            let s = ConvertSettings { effort, max_fps: Some(5), mono: true, audio_kbps: 96, ..ConvertSettings::default() };
            let p = run(&mp4, Target::Video(format), &s, &format!("{effort:?}_{format:?}"));
            let info = probe(&p).unwrap();
            assert!(info.has_audio() && info.fps <= 5.5, "{effort:?} {format:?}: {info:?}");
        }
    }
    let s = ConvertSettings::default();
    let actual = size(&run(&mp4, Target::Video(VideoFormat::Mp4), &s, "estimated"));
    let estimate = convert::estimate_size(&mp4, Target::Video(VideoFormat::Mp4), &s, &cancel).unwrap();
    assert!(estimate.abs_diff(actual) * 10 < actual, "short files are converted whole: {estimate} vs {actual}");

    // Sound: mono WAV is half the stereo one; FLAC at every effort.
    let wav = dir.path().join("long.wav");
    let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for i in 0..48_000 * 20 {
        let v = ((i as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
    let stereo = run(&wav, Target::Audio(AudioFormat::Wav), &ConvertSettings::default(), "stereo");
    let mono = run(&wav, Target::Audio(AudioFormat::Wav), &ConvertSettings { mono: true, ..ConvertSettings::default() }, "mono");
    assert!(size(&mono).abs_diff(size(&stereo) / 2) < 1000, "{} vs {}", size(&mono), size(&stereo));
    for effort in Effort::ALL {
        run(&wav, Target::Audio(AudioFormat::Flac), &ConvertSettings { effort, ..ConvertSettings::default() }, &format!("{effort:?}"));
    }
    // A 20 s file is estimated from a 5 s piece: 128 kbit/s × 20 s ≈ 320 kB.
    let s = ConvertSettings { audio_kbps: 128, ..ConvertSettings::default() };
    let estimate = convert::estimate_size(&wav, Target::Audio(AudioFormat::Mp3), &s, &cancel).unwrap();
    assert!((300_000..345_000).contains(&estimate), "{estimate}");
}

/// Speech models take 16 kHz mono: a 48 kHz stereo file decodes to a third of its frames,
/// from any starting point.
#[test]
fn decodes_mono_for_speech() {
    let dir = tempfile::tempdir().unwrap();
    let wav = dir.path().join("stereo.wav");
    let spec = hound::WavSpec { channels: 2, sample_rate: 48_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
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
        mediaa_media::preview::decode_mono(&wav, start, 16_000, Arc::new(AtomicBool::new(false)), move |s| {
            g.lock().unwrap().extend_from_slice(s);
            true
        })
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
