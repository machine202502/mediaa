//! Dev helper: `cargo run -p mediaa-platform --example snap -- out.png [window-title-substring]`
//! saves a screenshot of the primary display (or of the first window whose title or app matches);
//! `--cursor` instead of a title grabs the primary display as a recording frame (with cursor).
//! `--record` after the title grabs the window through its recording source instead.
use mediaa_core::source::FrameSource;

fn main() -> anyhow::Result<()> {
    // Physical pixels everywhere, like the app (winit makes it per-monitor DPI aware).
    #[cfg(windows)]
    // SAFETY: process-wide setting, called before any window or capture exists.
    unsafe {
        use windows_sys::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
        SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
    let mut args = std::env::args().skip(1);
    let out = args.next().unwrap_or_else(|| "snap.png".into());
    let frame = match args.next().as_deref() {
        Some("--cursor") => {
            let m = mediaa_platform::screen::monitors()?.remove(0);
            if let Some(c) = mediaa_platform::cursor::current() {
                println!("cursor at ({}, {}), {}x{} px", c.x - m.x, c.y - m.y, c.image.image.width, c.image.image.height);
            }
            mediaa_platform::screen::MonitorSource::new(m.id, None, true)?.grab()?
        }
        Some(title) => {
            let w = mediaa_platform::screen::windows()?
                .into_iter()
                .find(|w| w.title.contains(title) || w.app.contains(title))
                .ok_or_else(|| {
                    let all = mediaa_platform::screen::windows().unwrap_or_default();
                    anyhow::anyhow!("no window matching {title}; windows: {:?}", all.iter().map(|w| (&w.title, &w.app)).collect::<Vec<_>>())
                })?;
            println!("window {} ({}x{} reported)", w.title, w.width, w.height);
            if args.next().as_deref() == Some("--record") {
                let mut src = mediaa_platform::screen::window_source(w.id, false)?;
                src.grab()?
            } else {
                mediaa_platform::screen::capture_window(w.id)?
            }
        }
        None => {
            let m = mediaa_platform::screen::monitors()?.remove(0);
            mediaa_platform::screen::capture_monitor(m.id)?
        }
    };
    mediaa_core::screenshot::save(&frame, mediaa_core::ImageFormat::Png, std::path::Path::new(&out))?;
    println!("{}x{} -> {out}", frame.width, frame.height);
    Ok(())
}
