use mediaa_core::source::FrameSource;

fn main() -> anyhow::Result<()> {
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
            let pid: Option<u32> = title.strip_prefix("pid:").and_then(|p| p.parse().ok());
            let w = mediaa_platform::screen::windows()?
                .into_iter()
                .find(|w| pid.map_or(w.title.contains(title) || w.app.contains(title), |p| w.pid == p))
                .ok_or_else(|| {
                    if let Some(p) = pid {
                        return anyhow::anyhow!("no window of process {p}");
                    }
                    let all = mediaa_platform::screen::windows().unwrap_or_default();
                    anyhow::anyhow!("no window matching {title}; windows: {:?}", all.iter().map(|w| (&w.title, &w.app)).collect::<Vec<_>>())
                })?;
            println!("window {} of process {} ({}x{} reported)", w.title, w.pid, w.width, w.height);
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
