use mediaa_core::settings::{AnimSettings, AnimationQuality, WebpMode};
use mediaa_core::source::SourceFactory;
use mediaa_core::{AnimFormat, OutputKind, Rect};
use mediaa_media::recorder::{
    RecordingRequest, RecordingSession, SessionEvent, SessionNotice, StopAction,
};
use mediaa_platform::screen::{self, MonitorInfo, WindowInfo};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scenario {
    Full,
    TwoScreens,
    TwoWindows,
    OneWindow,
    Small,
    Medium,
    Large,
}

impl Scenario {
    fn parse(value: &str) -> anyhow::Result<Self> {
        Ok(match value {
            "full" => Self::Full,
            "two-screens" => Self::TwoScreens,
            "two-windows" => Self::TwoWindows,
            "one-window" => Self::OneWindow,
            "small" => Self::Small,
            "medium" => Self::Medium,
            "large" => Self::Large,
            other => anyhow::bail!("unknown scenario: {other}"),
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::TwoScreens => "two-screens",
            Self::TwoWindows => "two-windows",
            Self::OneWindow => "one-window",
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Quality,
    Fast,
}

impl Mode {
    fn parse(value: &str) -> anyhow::Result<Self> {
        Ok(match value {
            "quality" | "direct" => Self::Quality,
            "fast" => Self::Fast,
            other => anyhow::bail!("unknown mode: {other}"),
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Quality => "quality",
            Self::Fast => "fast",
        }
    }

    fn webp_mode(self) -> WebpMode {
        match self {
            Self::Quality => WebpMode::Quality,
            Self::Fast => WebpMode::Fast,
        }
    }
}

fn format_name(format: AnimFormat) -> &'static str {
    match format {
        AnimFormat::Webp => "webp",
        AnimFormat::Gif => "gif",
    }
}

struct Args {
    scenario: Scenario,
    seconds: u64,
    mode: Mode,
    format: AnimFormat,
    fps: u32,
    height: Option<u32>,
    window: Option<String>,
    window2: Option<String>,
    tag: String,
    keep: bool,
}

impl Args {
    fn parse() -> anyhow::Result<Self> {
        let mut args = std::env::args().skip(1);
        let mut scenario = Scenario::Full;
        let mut seconds = 3;
        let mut mode = Mode::Quality;
        let mut format = AnimFormat::Webp;
        let mut fps = 60;
        let mut height = None;
        let mut window = None;
        let mut window2 = None;
        let mut tag = String::new();
        let mut keep = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--scenario" => {
                    scenario = Scenario::parse(
                        &args
                            .next()
                            .ok_or_else(|| anyhow::anyhow!("missing scenario"))?,
                    )?;
                }
                "--seconds" => {
                    seconds = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("missing seconds"))?
                        .parse()?;
                }
                "--mode" => {
                    mode =
                        Mode::parse(&args.next().ok_or_else(|| anyhow::anyhow!("missing mode"))?)?;
                }
                "--format" => {
                    format = match args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("missing format"))?
                        .as_str()
                    {
                        "webp" => AnimFormat::Webp,
                        "gif" => AnimFormat::Gif,
                        other => anyhow::bail!("unknown format: {other}"),
                    };
                }
                "--fps" => {
                    fps = args
                        .next()
                        .ok_or_else(|| anyhow::anyhow!("missing fps"))?
                        .parse()?;
                }
                "--height" => {
                    height = Some(
                        args.next()
                            .ok_or_else(|| anyhow::anyhow!("missing height"))?
                            .parse()?,
                    );
                }
                "--window" => window = args.next(),
                "--window2" => window2 = args.next(),
                "--tag" => {
                    tag = args.next().ok_or_else(|| anyhow::anyhow!("missing tag"))?;
                }
                "--keep" => keep = true,
                other => anyhow::bail!("unknown argument: {other}"),
            }
        }
        anyhow::ensure!(seconds > 0 && seconds <= 600, "seconds out of range");
        if tag.is_empty() {
            tag = format!(
                "{}-{}-{}-{}s",
                format_name(format),
                scenario.name(),
                mode.name(),
                seconds
            );
        }
        Ok(Self {
            scenario,
            seconds,
            mode,
            format,
            fps,
            height,
            window,
            window2,
            tag,
            keep,
        })
    }
}

fn dpi_aware() {
    #[cfg(windows)]
    // SAFETY: process-wide setting, called before any capture exists.
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn region(size: (u32, u32), monitor: &MonitorInfo) -> Rect {
    let w = size.0.min(monitor.width) as i32 & !1;
    let h = size.1.min(monitor.height) as i32 & !1;
    Rect::new(
        ((monitor.width as i32 - w) / 2).max(0),
        ((monitor.height as i32 - h) / 2).max(0),
        w,
        h,
    )
}

fn monitor_factory(monitor: &MonitorInfo, crop: Option<Rect>) -> SourceFactory {
    let id = monitor.id;
    Box::new(move || screen::monitor_source(id, crop, true))
}

fn window_factory(window: &WindowInfo) -> SourceFactory {
    let id = window.id;
    Box::new(move || screen::window_source(id, true))
}

fn is_console_window(window: &WindowInfo) -> bool {
    let text = format!("{} {}", window.title, window.app).to_lowercase();
    [
        "webp_matrix",
        "capture_bench",
        "powershell",
        "pwsh",
        "windows terminal",
        "conhost",
        "cargo",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn pick_window(windows: &[WindowInfo], needle: Option<&str>) -> anyhow::Result<WindowInfo> {
    if let Some(needle) = needle {
        return windows
            .iter()
            .find(|w| w.title.contains(needle) || w.app.contains(needle))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no window matching {needle}"));
    }
    windows
        .iter()
        .filter(|w| !is_console_window(w) && w.width >= 800 && w.height >= 600)
        .max_by_key(|w| u64::from(w.width) * u64::from(w.height))
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("no suitable window"))
}

fn sources(
    args: &Args,
    monitors: &[MonitorInfo],
    windows: &[WindowInfo],
) -> anyhow::Result<Vec<SourceFactory>> {
    let monitor = monitors
        .first()
        .ok_or_else(|| anyhow::anyhow!("no monitor"))?;
    Ok(match args.scenario {
        Scenario::Full => vec![monitor_factory(monitor, None)],
        Scenario::TwoScreens => {
            anyhow::ensure!(monitors.len() >= 2, "only one monitor is connected");
            vec![
                monitor_factory(&monitors[0], None),
                monitor_factory(&monitors[1], None),
            ]
        }
        Scenario::TwoWindows => {
            let first = pick_window(windows, args.window.as_deref())?;
            let second = match args.window2.as_deref() {
                Some(needle) => pick_window(windows, Some(needle))?,
                None => windows
                    .iter()
                    .filter(|w| w.id != first.id && !is_console_window(w))
                    .max_by_key(|w| u64::from(w.width) * u64::from(w.height))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("no second window"))?,
            };
            anyhow::ensure!(first.id != second.id, "the same window was selected twice");
            vec![window_factory(&first), window_factory(&second)]
        }
        Scenario::OneWindow => vec![window_factory(&pick_window(
            windows,
            args.window.as_deref(),
        )?)],
        Scenario::Small => vec![monitor_factory(monitor, Some(region((640, 400), monitor)))],
        Scenario::Medium => vec![monitor_factory(
            monitor,
            Some(region((1600, 1000), monitor)),
        )],
        Scenario::Large => vec![monitor_factory(
            monitor,
            Some(region((2560, 1600), monitor)),
        )],
    })
}

fn wait_finished(
    events: &crossbeam_channel::Receiver<SessionEvent>,
) -> anyhow::Result<(Vec<PathBuf>, bool)> {
    let mut fallback = false;
    loop {
        match events.recv_timeout(Duration::from_secs(7200))? {
            SessionEvent::Finished(paths) => return Ok((paths, fallback)),
            SessionEvent::Notice(SessionNotice::WebpFallback { detail, .. }) => {
                fallback = true;
                eprintln!("warning: segmented WebP fallback: {detail}");
            }
            SessionEvent::Warning(warning) => eprintln!("warning: {warning}"),
            SessionEvent::Failed(failure) => anyhow::bail!("{}", failure.error),
            SessionEvent::CaptureFailed(error) => anyhow::bail!("capture failed: {error}"),
            SessionEvent::Cancelled => anyhow::bail!("recording cancelled"),
            _ => {}
        }
    }
}

fn main() -> anyhow::Result<()> {
    dpi_aware();
    mediaa_media::preview_cache::set_full_previews_enabled(false);
    mediaa_media::preview_cache::set_animated_previews_enabled(false);
    let args = Args::parse()?;
    let monitors = screen::monitors()?;
    let windows = screen::windows()?;
    let factories = sources(&args, &monitors, &windows)?;

    let root = std::env::current_dir()?.join("target/bench");
    std::fs::create_dir_all(&root)?;
    let run_dir = root.join(format!("{}-{}", args.tag, std::process::id()));
    let out_dir = run_dir.join("out");
    let work_dir = run_dir.join("work");
    std::fs::create_dir_all(&out_dir)?;
    std::fs::create_dir_all(&work_dir)?;

    let (fps, max_height) = match args.mode {
        Mode::Quality => (args.fps, args.height),
        Mode::Fast => (
            args.fps.min(30),
            Some(args.height.unwrap_or(1080).min(1080)),
        ),
    };
    let animation = AnimSettings::for_recording(fps, max_height, &AnimationQuality::default());
    let request = RecordingRequest::builder(
        OutputKind::Anim(args.format),
        factories,
        vec![],
        mediaa_core::paths::MediaDir::new(out_dir.clone()),
        mediaa_core::paths::WorkDir::new(work_dir.clone()),
        args.tag.clone(),
    )
    .video(fps, 100, max_height)
    .animation(animation)
    .webp_mode(args.mode.webp_mode())
    .build()?;

    let started = Instant::now();
    let session = RecordingSession::start(request)?;
    let start_ms = started.elapsed().as_millis();
    let events = session.events();
    let capture_started = Instant::now();
    std::thread::sleep(Duration::from_secs(args.seconds));
    let capture_ms = capture_started.elapsed().as_millis();
    let frames = session.frames_captured();
    let names = session.source_names();
    let cancel = Arc::new(AtomicBool::new(false));
    let stopped = Instant::now();
    let handle = session.stop(StopAction::Save, cancel)?;
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("finalizer panicked"))?;
    let finalize_ms = stopped.elapsed().as_millis();
    let (paths, fallback) = wait_finished(&events)?;
    let total_ms = started.elapsed().as_millis();
    let bytes: u64 = paths
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| meta.len())
        .sum();

    println!(
        "format={} scenario={} mode={} seconds={} sources={} frames={} start_ms={} capture_ms={} finalize_ms={} total_ms={} outputs={} bytes={} fallback={}",
        format_name(args.format),
        args.scenario.name(),
        args.mode.name(),
        args.seconds,
        names.join("|"),
        frames,
        start_ms,
        capture_ms,
        finalize_ms,
        total_ms,
        paths.len(),
        bytes,
        fallback
    );
    let header = "format,scenario,mode,seconds,sources,frames,start_ms,capture_ms,finalize_ms,total_ms,outputs,bytes,fallback";
    let csv = root.join(format!("{}-results.csv", format_name(args.format)));
    let new = !csv.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&csv)?;
    use std::io::Write;
    if new {
        writeln!(file, "{header}")?;
    }
    writeln!(
        file,
        "{},{},{},{},\"{}\",{},{},{},{},{},{},{},{}",
        format_name(args.format),
        args.scenario.name(),
        args.mode.name(),
        args.seconds,
        names.join("|"),
        frames,
        start_ms,
        capture_ms,
        finalize_ms,
        total_ms,
        paths.len(),
        bytes,
        fallback
    )?;

    if !args.keep {
        let _ = std::fs::remove_dir_all(&run_dir);
    }
    Ok(())
}
