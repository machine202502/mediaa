use mediaa_core::formats::{AnimFormat, OutputKind, VideoContainer};
use mediaa_core::settings::AnimSettings;
use mediaa_core::source::{FrameSource, SourceFactory};
use mediaa_media::recorder::{
    AudioLayout, RecordingRequest, RecordingSession, SessionEvent, StopAction, WebpMode,
};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn main() -> anyhow::Result<()> {
    #[cfg(windows)]
    // SAFETY: process-wide setting, called before any capture exists.
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{
            SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
        };
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    mediaa_core::logging::init(
        &std::env::temp_dir().join("mediaa-capture-bench"),
        log::LevelFilter::Info,
    );
    let m = mediaa_platform::screen::monitors()?.remove(0);
    println!("display {}x{}", m.width, m.height);
    let mut screenshots = mediaa_platform::screen::MonitorSource::new(m.id, None, true)?;
    let mut backend = mediaa_platform::screen::monitor_source(m.id, None, true)?;
    for (label, src) in [
        ("screenshots", &mut screenshots as &mut dyn FrameSource),
        ("recording backend", backend.as_mut()),
    ] {
        let n = 30;
        let t = Instant::now();
        for _ in 0..n {
            src.grab()?;
        }
        let per = t.elapsed() / n;
        println!(
            "{label}: {per:?} per grab = {:.0} grabs/s",
            1.0 / per.as_secs_f64()
        );
    }
    drop(backend);

    let output = match std::env::args().nth(1).as_deref() {
        Some("gif") => OutputKind::Anim(AnimFormat::Gif),
        Some("webp") => OutputKind::Anim(AnimFormat::Webp),
        _ => OutputKind::Video(VideoContainer::Mp4),
    };
    let dir = std::env::temp_dir().join("mediaa-capture-bench");
    let id = m.id;
    let window = std::env::var("WINDOW")
        .ok()
        .map(|t| -> anyhow::Result<u32> {
            let w = mediaa_platform::screen::windows()?
                .into_iter()
                .find(|w| w.title.contains(&t));
            Ok(w.ok_or_else(|| anyhow::anyhow!("no window matching {t}"))?
                .id)
        });
    let factory: SourceFactory = match window.transpose()? {
        Some(wid) => Box::new(move || mediaa_platform::screen::window_source(wid, true)),
        None => {
            let region =
                std::env::var_os("REGION").map(|_| mediaa_core::Rect::new(100, 100, 800, 600));
            Box::new(move || mediaa_platform::screen::monitor_source(id, region, true))
        }
    };
    let session = RecordingSession::start(
        RecordingRequest::builder(
            output,
            vec![factory],
            if std::env::var_os("AUDIO").is_some() {
                vec![Box::new(mediaa_platform::audio::CpalSource::new(
                    mediaa_platform::audio::CpalTarget::System,
                ))
                    as Box<dyn mediaa_core::source::AudioSource>]
            } else {
                vec![]
            },
            mediaa_core::paths::MediaDir::new(dir.join("out")),
            mediaa_core::paths::WorkDir::new(dir.join(format!("work-{}", std::process::id()))),
            "bench".into(),
        )
        .video(
            60,
            100,
            if std::env::var_os("NATIVE").is_some() {
                None
            } else {
                Some(1080)
            },
        )
        .audio_layout(AudioLayout::EveryFile)
        .audio_kbps(192)
        .animation(AnimSettings::default())
        .webp_mode(WebpMode::SegmentedParallel)
        .build()?,
    )?;
    let _tone = std::env::var_os("AUDIO")
        .and_then(|_| mediaa_platform::audio::AudioOut::open().ok())
        .inspect(|out| {
            let rate = out.sample_rate() as f32;
            let tone: Vec<f32> = (0..(rate as usize) * 3)
                .flat_map(|i| {
                    let v = (i as f32 / rate * 440.0 * std::f32::consts::TAU).sin() * 0.08;
                    [v, v]
                })
                .collect();
            out.push(&tone);
        });
    std::thread::sleep(Duration::from_secs(3));
    println!("captured {} frames in 3 s", session.frames_captured());
    let events = session.events();
    let t = Instant::now();
    match session.stop(StopAction::Save, Arc::new(AtomicBool::new(false))) {
        Ok(handle) => {
            let _ = handle.join();
        }
        Err(error) => log::warn!("finalizer ran inline: {error:#}"),
    }
    for e in events.iter() {
        match e {
            SessionEvent::Finished(paths) => {
                let info = mediaa_media::info::probe(&paths[0])?;
                println!(
                    "{} saved in {:?}: {info:?}",
                    paths[0].display(),
                    t.elapsed()
                );
                if let Ok(out) = std::env::var("THUMB") {
                    let kind = mediaa_core::MediaKind::detect(&paths[0])
                        .unwrap_or(mediaa_core::MediaKind::Video);
                    let frame = mediaa_media::thumbnail::thumbnail(&paths[0], kind, 1600)?;
                    mediaa_storage::screenshot::save(
                        &frame,
                        mediaa_core::ImageFormat::Png,
                        std::path::Path::new(&out),
                    )?;
                }
                break;
            }
            SessionEvent::Failed(failure) => anyhow::bail!(failure.error),
            SessionEvent::CaptureFailed(error) => anyhow::bail!(error),
            _ => {}
        }
    }
    Ok(())
}
