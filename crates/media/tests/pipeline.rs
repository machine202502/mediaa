use mediaa_core::audio::AudioSink;
use mediaa_core::formats::{AnimFormat, MediaKind, OutputKind, VideoContainer};
use mediaa_core::paths::{MediaDir, WorkDir};
use mediaa_core::settings::{AnimSettings, WebpMode};
use mediaa_core::source::{AudioSource, FrameSource, SourceFactory};
use mediaa_core::{naming, Frame};
use mediaa_media::animation_backends::{
    self, AnimationEncodeRequest, AnimationOrigin, WebpSegmentation,
};
use mediaa_media::convert::{self, ConvertSettings, Effort, Target};
use mediaa_media::info::probe;
use mediaa_media::recorder::{
    AudioLayout, RecordingRequest, RecordingSession, SessionEvent, StopAction,
};
use mediaa_webp_pipeline::PipelineTuning;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

static LOGS: std::sync::OnceLock<std::sync::Mutex<Vec<String>>> = std::sync::OnceLock::new();

struct CaptureLogger;

impl log::Log for CaptureLogger {
    fn enabled(&self, _metadata: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        if let Some(logs) = LOGS.get() {
            logs.lock().unwrap().push(record.args().to_string());
        }
    }

    fn flush(&self) {}
}

static CAPTURE_LOGGER: CaptureLogger = CaptureLogger;

fn log_position() -> usize {
    let logs = LOGS.get_or_init(|| std::sync::Mutex::new(Vec::new()));
    let _ = log::set_logger(&CAPTURE_LOGGER);
    log::set_max_level(log::LevelFilter::Info);
    logs.lock().unwrap().len()
}

fn logs_since(position: usize) -> Vec<String> {
    LOGS.get()
        .map(|logs| logs.lock().unwrap()[position..].to_vec())
        .unwrap_or_default()
}

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
                    data.extend_from_slice(&[
                        (x * 255 / self.w) as u8,
                        (y * 255 / self.h) as u8,
                        90,
                        255,
                    ]);
                }
            }
        }
        Ok(Frame::new(self.w, self.h, data))
    }
}

struct Still {
    w: u32,
    h: u32,
}

impl FrameSource for Still {
    fn name(&self) -> String {
        format!("Still {}x{}", self.w, self.h)
    }

    fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        let mut data = Vec::with_capacity((self.w * self.h * 4) as usize);
        for y in 0..self.h {
            for x in 0..self.w {
                data.extend_from_slice(&[
                    (x * 255 / self.w) as u8,
                    (y * 255 / self.h) as u8,
                    90,
                    255,
                ]);
            }
        }
        Ok(Frame::new(self.w, self.h, data))
    }
}

fn pattern(w: u32, h: u32) -> SourceFactory {
    Box::new(move || Ok(Box::new(Pattern { w, h, n: 0 }) as Box<dyn FrameSource>))
}

fn still(w: u32, h: u32) -> SourceFactory {
    Box::new(move || Ok(Box::new(Still { w, h }) as Box<dyn FrameSource>))
}

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
    fn start(
        &mut self,
        open: &mut dyn FnMut(u32, u16) -> anyhow::Result<AudioSink>,
    ) -> anyhow::Result<()> {
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
    fn start(
        &mut self,
        _: &mut dyn FnMut(u32, u16) -> anyhow::Result<AudioSink>,
    ) -> anyhow::Result<()> {
        anyhow::bail!("device unplugged")
    }
    fn stop(&mut self) {}
}

fn title(p: &Path) -> String {
    naming::parse_path(p)
        .unwrap_or_else(|| panic!("not a media name: {}", p.display()))
        .title
}

fn request(
    dir: &Path,
    sources: Vec<SourceFactory>,
    audio: Vec<Box<dyn AudioSource>>,
    output: OutputKind,
) -> RecordingRequest {
    request_with_layout(dir, sources, audio, output, AudioLayout::FirstFileOnly)
}

fn request_with_layout(
    dir: &Path,
    sources: Vec<SourceFactory>,
    audio: Vec<Box<dyn AudioSource>>,
    output: OutputKind,
    audio_layout: AudioLayout,
) -> RecordingRequest {
    RecordingRequest::builder(
        output,
        sources,
        audio,
        MediaDir::new(dir.join("out")),
        WorkDir::new(dir.join("work")),
        "Recording test".into(),
    )
    .video(20, 70, None)
    .audio_layout(audio_layout)
    .audio_kbps(128)
    .animation(AnimSettings {
        fps: 10,
        max_height: 0,
        ..AnimSettings::default()
    })
    .webp_mode(WebpMode::SegmentedParallel)
    .build()
    .unwrap()
}

fn finish(session: RecordingSession, action: StopAction) -> (SessionEvent, Vec<String>) {
    let events = session.events();
    session
        .stop(action, Arc::new(AtomicBool::new(false)))
        .unwrap()
        .join()
        .unwrap();
    let mut warnings = Vec::new();
    loop {
        match events
            .recv_timeout(Duration::from_secs(60))
            .expect("terminal event")
        {
            SessionEvent::Warning(w) => warnings.push(w),
            SessionEvent::Notice(notice) => warnings.push(format!("{notice:?}")),
            other => return (other, warnings),
        }
    }
}

fn record(dir: &Path, req: RecordingRequest, secs: f32) -> Vec<PathBuf> {
    let _ = dir;
    let picture = !matches!(req.output(), OutputKind::Audio(_));
    let session = RecordingSession::start(req).expect("start");
    std::thread::sleep(Duration::from_secs_f32(secs));
    assert_eq!(
        session.frames_captured() > 0,
        picture,
        "frames only for picture recordings"
    );
    match finish(session, StopAction::Save) {
        (SessionEvent::Finished(paths), _) => paths,
        (other, _) => panic!("unexpected {other:?}"),
    }
}

fn opts() -> ConvertSettings {
    ConvertSettings {
        anim: AnimSettings {
            fps: 10,
            max_height: 33,
            ..AnimSettings::default()
        },
        quality: 60,
        ..ConvertSettings::default()
    }
}

fn assert_duration(path: &Path, expected: f64, tolerance: f64) {
    let info = probe(path).unwrap();
    let d = info.duration_secs().expect("duration");
    assert!(
        (d - expected).abs() <= tolerance,
        "{}: duration {d}, expected ~{expected}",
        path.display()
    );
}

#[path = "pipeline/audio.rs"]
mod audio;
#[path = "pipeline/conversion.rs"]
mod conversion;
#[path = "pipeline/video.rs"]
mod video;
