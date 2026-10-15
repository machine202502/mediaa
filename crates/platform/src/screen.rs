//! Monitor and window enumeration/capture backed by `xcap`
//! (Windows GDI/PrintWindow, macOS CoreGraphics, Linux X11 and Wayland portals).

use anyhow::{anyhow, Context};
use mediaa_core::source::FrameSource;
use mediaa_core::{Frame, Rect};

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: u32,
    /// Stable system device name (e.g. `\\.\DISPLAY1` on Windows), to remember a display across runs.
    pub device: String,
    /// Position in the OS enumeration order (matches winit's `available_monitors()`,
    /// used to open the selection overlay on this monitor).
    pub index: usize,
    pub name: String,
    /// Position on the virtual desktop, physical pixels.
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub primary: bool,
}

impl MonitorInfo {
    pub fn label(&self) -> String {
        format!("{} ({}×{})", self.name, self.width, self.height)
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.width as i32, self.height as i32)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowInfo {
    pub id: u32,
    pub title: String,
    pub app: String,
    pub width: u32,
    pub height: u32,
}

impl WindowInfo {
    pub fn label(&self) -> String {
        if self.app.is_empty() || self.title.contains(&self.app) {
            self.title.clone()
        } else {
            format!("{} — {}", self.title, self.app)
        }
    }
}

fn monitor_info(m: &xcap::Monitor, index: usize) -> anyhow::Result<MonitorInfo> {
    // Built-in panels often report no EDID name ("Unknown Monitor 65537").
    let name = m
        .friendly_name()
        .ok()
        .filter(|n| !n.trim().is_empty() && !n.starts_with("Unknown"))
        .unwrap_or_else(|| format!("Display {}", index + 1));
    Ok(MonitorInfo {
        id: m.id()?,
        device: m.name().unwrap_or_default(),
        index,
        name,
        x: m.x()?,
        y: m.y()?,
        width: m.width()?,
        height: m.height()?,
        scale: m.scale_factor().unwrap_or(1.0),
        primary: m.is_primary().unwrap_or(false),
    })
}

/// All monitors, primary first.
pub fn monitors() -> anyhow::Result<Vec<MonitorInfo>> {
    let mut list: Vec<MonitorInfo> = xcap::Monitor::all()?
        .iter()
        .enumerate()
        .filter_map(|(i, m)| monitor_info(m, i).ok())
        .collect();
    list.sort_by_key(|m| (!m.primary, m.x, m.y));
    Ok(list)
}

/// Id of the monitor containing the virtual-desktop point `(x, y)` (physical pixels).
pub fn monitor_at(x: i32, y: i32) -> Option<u32> {
    xcap::Monitor::from_point(x, y).ok().and_then(|m| m.id().ok())
}

fn find_monitor(id: u32) -> anyhow::Result<xcap::Monitor> {
    xcap::Monitor::all()?
        .into_iter()
        .find(|m| m.id().ok() == Some(id))
        .ok_or_else(|| anyhow!("monitor {id} is no longer connected"))
}

fn find_window(id: u32) -> anyhow::Result<xcap::Window> {
    xcap::Window::all()?
        .into_iter()
        .find(|w| w.id().ok() == Some(id))
        .ok_or_else(|| anyhow!("window {id} was closed"))
}

pub fn capture_monitor(id: u32) -> anyhow::Result<Frame> {
    let img = find_monitor(id)?
        .capture_image()
        .context("screen capture failed")?;
    Ok(Frame::from_image(img))
}

/// Top-level windows worth recording: titled, visible, not minimized, not our own.
pub fn windows() -> anyhow::Result<Vec<WindowInfo>> {
    let own_pid = std::process::id();
    let mut list = Vec::new();
    for w in xcap::Window::all()? {
        if w.is_minimized().unwrap_or(true) || w.pid().ok() == Some(own_pid) {
            continue;
        }
        let title = w.title().unwrap_or_default();
        let (width, height) = (w.width().unwrap_or(0), w.height().unwrap_or(0));
        if title.trim().is_empty() || width < 16 || height < 16 {
            continue;
        }
        list.push(WindowInfo {
            id: w.id()?,
            title,
            app: w.app_name().unwrap_or_default(),
            width,
            height,
        });
    }
    Ok(list)
}

pub fn capture_window(id: u32) -> anyhow::Result<Frame> {
    // xcap rescales and crops Windows window shots by the invisible resize borders, cutting
    // off the bottom / right (maximized windows come out blank); WGC returns the window as shown.
    #[cfg(windows)]
    match crate::wgc::WgcSource::window_frame(id) {
        Ok(frame) => return Ok(frame),
        Err(e) => log::warn!("{e:#}; window screenshot through GDI instead"),
    }
    let img = find_window(id)?
        .capture_image()
        .context("window capture failed")?;
    Ok(Frame::from_image(img))
}

/// Records a whole monitor or a region of it.
pub struct MonitorSource {
    monitor: xcap::Monitor,
    name: String,
    region: Option<Rect>,
    size: (u32, u32),
    /// Top-left of the recorded area on the virtual desktop and the monitor scale (for the
    /// cursor overlay); `None` when the cursor is not recorded.
    cursor: Option<((i32, i32), f32)>,
}

impl MonitorSource {
    /// `region` is in the monitor's pixel space; it is clamped and made even-sized
    /// (yuv420p encoders need even dimensions).
    pub fn new(monitor_id: u32, region: Option<Rect>, with_cursor: bool) -> anyhow::Result<Self> {
        let monitor = find_monitor(monitor_id)?;
        let info = monitor_info(&monitor, 0)?;
        let region = match region {
            Some(r) => {
                let r = r.clamp_to(&info.bounds()).even();
                anyhow::ensure!(r.w >= 16 && r.h >= 16, "selected area is too small");
                Some(r)
            }
            None => None,
        };
        let size = match region {
            Some(r) => (r.w as u32, r.h as u32),
            None => (info.width & !1, info.height & !1),
        };
        let name = if region.is_some() {
            "Region".to_string()
        } else {
            info.name.clone()
        };
        let (rx, ry) = region.map_or((0, 0), |r| (r.x, r.y));
        let origin = (info.x + logical_to_desktop(rx, info.scale), info.y + logical_to_desktop(ry, info.scale));
        let cursor = with_cursor.then_some((origin, info.scale));
        Ok(Self {
            monitor,
            name,
            region,
            size,
            cursor,
        })
    }
}

impl FrameSource for MonitorSource {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        let img = match self.region {
            Some(r) => self
                .monitor
                .capture_region(r.x as u32, r.y as u32, r.w as u32, r.h as u32)?,
            None => self.monitor.capture_image()?,
        };
        let frame = Frame::from_image(img);
        let (w, h) = self.size;
        let mut frame = if (frame.width, frame.height) == (w, h) {
            frame
        } else {
            frame.fit_to(w, h, [0, 0, 0, 255])
        };
        if let Some((origin, scale)) = self.cursor {
            draw_cursor(&mut frame, origin, scale);
        }
        Ok(frame)
    }
}

/// Records one window; its frame size is fixed at start (resizes are padded/cropped).
pub struct WindowSource {
    window: xcap::Window,
    name: String,
    size: (u32, u32),
    with_cursor: bool,
}

impl WindowSource {
    pub fn new(window_id: u32, with_cursor: bool) -> anyhow::Result<Self> {
        let window = find_window(window_id)?;
        let first = window.capture_image().context("window capture failed")?;
        let size = (first.width() & !1, first.height() & !1);
        anyhow::ensure!(size.0 >= 16 && size.1 >= 16, "window is too small to record");
        Ok(Self {
            name: window.title().unwrap_or_else(|_| "Window".into()),
            window,
            size,
            with_cursor,
        })
    }
}

impl FrameSource for WindowSource {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        let frame = Frame::from_image(self.window.capture_image()?);
        let mut frame = frame.fit_to(self.size.0, self.size.1, [0, 0, 0, 255]);
        if self.with_cursor {
            if let (Ok(x), Ok(y)) = (self.window.x(), self.window.y()) {
                draw_cursor(&mut frame, (x, y), 1.0);
            }
        }
        Ok(frame)
    }
}

/// Region offsets are monitor pixels; the virtual desktop uses points on macOS.
fn logical_to_desktop(v: i32, scale: f32) -> i32 {
    if cfg!(target_os = "macos") {
        (v as f32 / scale).round() as i32
    } else {
        v
    }
}

/// Draws the current mouse cursor into a frame whose top-left is at `origin` on the virtual
/// desktop (`scale` converts desktop units to frame pixels on macOS).
fn draw_cursor(frame: &mut Frame, origin: (i32, i32), scale: f32) {
    let Some(c) = crate::cursor::current() else { return };
    let k = if cfg!(target_os = "macos") { scale } else { 1.0 };
    let x = ((c.x - origin.0) as f32 * k).round() as i32;
    let y = ((c.y - origin.1) as f32 * k).round() as i32;
    mediaa_core::cursor::draw(frame, &c.image, x, y);
}

/// Recording source for a display or a region of it: Windows Graphics Capture on Windows
/// (falls back to `xcap` screenshots if unavailable), `xcap` elsewhere.
pub fn monitor_source(monitor_id: u32, region: Option<Rect>, with_cursor: bool) -> anyhow::Result<Box<dyn FrameSource>> {
    let src = MonitorSource::new(monitor_id, region, with_cursor)?;
    #[cfg(windows)]
    let fast = crate::wgc::WgcSource::monitor(monitor_id, src.name(), src.size(), src.region, with_cursor)
        .map(|s| Box::new(s) as Box<dyn FrameSource>);
    #[cfg(target_os = "linux")]
    let fast = if crate::x11::X11Source::available() {
        let info = monitor_info(&src.monitor, 0)?;
        let r = src.region.unwrap_or(info.bounds());
        let area = Rect::new(info.x + r.x, info.y + r.y, r.w, r.h);
        crate::x11::X11Source::new(crate::x11::Target::Area(area), src.name(), with_cursor)
            .map(|s| Box::new(s) as Box<dyn FrameSource>)
    } else {
        // Wayland: PipeWire stream (the compositor decides about the cursor).
        crate::stream::StreamSource::monitor(&src.monitor, src.name(), src.region).map(|s| Box::new(s) as Box<dyn FrameSource>)
    };
    // macOS: AVFoundation stream; it always shows the cursor.
    #[cfg(target_os = "macos")]
    let fast = crate::stream::StreamSource::monitor(&src.monitor, src.name(), src.region).map(|s| Box::new(s) as Box<dyn FrameSource>);
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    match fast {
        Ok(s) => return Ok(s),
        Err(e) => log::warn!("{e:#}; recording with screenshots instead"),
    }
    Ok(Box::new(src))
}

/// Recording source for a window (see [`monitor_source`]).
pub fn window_source(window_id: u32, with_cursor: bool) -> anyhow::Result<Box<dyn FrameSource>> {
    #[cfg(windows)]
    {
        let name = find_window(window_id)?.title().unwrap_or_else(|_| "Window".into());
        match crate::wgc::WgcSource::window(window_id, name, with_cursor) {
            Ok(wgc) => return Ok(Box::new(wgc)),
            Err(e) => log::warn!("{e:#}; recording with screenshots instead"),
        }
    }
    #[cfg(target_os = "linux")]
    if crate::x11::X11Source::available() {
        let name = find_window(window_id)?.title().unwrap_or_else(|_| "Window".into());
        match crate::x11::X11Source::new(crate::x11::Target::Window(window_id), name, with_cursor) {
            Ok(s) => return Ok(Box::new(s)),
            Err(e) => log::warn!("{e:#}; recording with screenshots instead"),
        }
    }
    Ok(Box::new(WindowSource::new(window_id, with_cursor)?))
}
