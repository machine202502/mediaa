use anyhow::{anyhow, Context};
use mediaa_core::source::FrameSource;
use mediaa_core::{Frame, Rect};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Available,
    Minimized,
    Hidden,
    Missing,
    Mismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorState {
    Available,
    Disconnected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonitorId(u32);

impl MonitorId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CaptureWindowId(u32);

impl CaptureWindowId {
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u32 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    pub id: MonitorId,
    pub device: String,
    pub index: usize,
    pub name: String,
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
    pub id: CaptureWindowId,
    pub title: String,
    pub app: String,
    pub pid: u32,
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
    let name = m
        .friendly_name()
        .ok()
        .filter(|n| !n.trim().is_empty() && !n.starts_with("Unknown"))
        .unwrap_or_else(|| format!("Display {}", index + 1));
    Ok(MonitorInfo {
        id: MonitorId::new(m.id()?),
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

pub fn monitors() -> anyhow::Result<Vec<MonitorInfo>> {
    let mut list: Vec<MonitorInfo> = xcap::Monitor::all()?
        .iter()
        .enumerate()
        .filter_map(|(i, m)| monitor_info(m, i).ok())
        .collect();
    list.sort_by_key(|m| (!m.primary, m.x, m.y));
    Ok(list)
}

pub fn monitor_at(x: i32, y: i32) -> Option<MonitorId> {
    xcap::Monitor::from_point(x, y)
        .ok()
        .and_then(|m| m.id().ok())
        .map(MonitorId::new)
}

fn find_monitor(id: MonitorId) -> anyhow::Result<xcap::Monitor> {
    xcap::Monitor::all()?
        .into_iter()
        .find(|m| m.id().ok() == Some(id.get()))
        .ok_or_else(|| anyhow!("monitor {} is no longer connected", id.get()))
}

fn find_window(id: CaptureWindowId) -> anyhow::Result<xcap::Window> {
    xcap::Window::all()?
        .into_iter()
        .find(|w| w.id().ok() == Some(id.get()))
        .ok_or_else(|| anyhow!("window {} was closed", id.get()))
}

pub fn capture_monitor(id: MonitorId) -> anyhow::Result<Frame> {
    let img = find_monitor(id)?
        .capture_image()
        .context("screen capture failed")?;
    Ok(Frame::from_image(img))
}

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
            id: CaptureWindowId::new(w.id()?),
            title,
            app: w.app_name().unwrap_or_default(),
            pid: w.pid().unwrap_or(0),
            width,
            height,
        });
    }
    Ok(list)
}

pub fn window_available(id: CaptureWindowId) -> bool {
    window_status(id, 0, "").is_available()
}

pub fn window_status(id: CaptureWindowId, pid: u32, app: &str) -> WindowState {
    let Ok(window) = find_window(id) else {
        return WindowState::Missing;
    };
    if pid != 0 && window.pid().ok() != Some(pid) {
        return WindowState::Mismatch;
    }
    if !app.is_empty() && window.app_name().ok().is_some_and(|name| name != app) {
        return WindowState::Mismatch;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::HWND;
        use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
        let hwnd = id.get() as HWND;
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return WindowState::Hidden;
        }
        if unsafe { IsIconic(hwnd) } != 0 {
            return WindowState::Minimized;
        }
        WindowState::Available
    }
    #[cfg(not(windows))]
    {
        if window.is_minimized().unwrap_or(true) {
            WindowState::Minimized
        } else {
            WindowState::Available
        }
    }
}

impl WindowState {
    pub fn is_available(self) -> bool {
        self == Self::Available
    }
}

pub fn monitor_state(id: MonitorId) -> MonitorState {
    if find_monitor(id).is_ok() {
        MonitorState::Available
    } else {
        MonitorState::Disconnected
    }
}

pub fn monitor_available(id: MonitorId) -> bool {
    monitor_state(id) == MonitorState::Available
}

pub fn capture_window(id: CaptureWindowId) -> anyhow::Result<Frame> {
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

pub struct DeferredSource {
    name: String,
    size: (u32, u32),
    probe: Arc<dyn Fn() -> bool + Send + Sync>,
    factory: Arc<dyn Fn() -> anyhow::Result<Box<dyn FrameSource>> + Send + Sync>,
    state: Arc<Mutex<DeferredState>>,
    alive: Arc<AtomicBool>,
}

struct DeferredState {
    latest: Option<Frame>,
    last: Option<Frame>,
    live: bool,
    started: bool,
}

impl DeferredSource {
    fn new(
        name: String,
        size: (u32, u32),
        probe: Arc<dyn Fn() -> bool + Send + Sync>,
        factory: Arc<dyn Fn() -> anyhow::Result<Box<dyn FrameSource>> + Send + Sync>,
    ) -> anyhow::Result<Self> {
        anyhow::ensure!(size.0 >= 16 && size.1 >= 16, "source is too small");
        Ok(Self {
            name,
            size: (size.0 & !1, size.1 & !1),
            probe,
            factory,
            state: Arc::new(Mutex::new(DeferredState {
                latest: None,
                last: None,
                live: false,
                started: false,
            })),
            alive: Arc::new(AtomicBool::new(true)),
        })
    }
}

impl Drop for DeferredSource {
    fn drop(&mut self) {
        self.alive.store(false, Ordering::SeqCst);
    }
}

fn deferred_worker(
    name: String,
    probe: Arc<dyn Fn() -> bool + Send + Sync>,
    factory: Arc<dyn Fn() -> anyhow::Result<Box<dyn FrameSource>> + Send + Sync>,
    state: Arc<Mutex<DeferredState>>,
    alive: Arc<AtomicBool>,
) {
    let mut inner: Option<Box<dyn FrameSource>> = None;
    while alive.load(Ordering::SeqCst) {
        if inner.is_none() {
            if (probe)() {
                match factory() {
                    Ok(source) => inner = Some(source),
                    Err(error) => {
                        log::warn!("deferred source {name} failed: {error:#}");
                        std::thread::sleep(Duration::from_secs(1));
                        continue;
                    }
                }
            } else {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
        }
        if let Some(source) = inner.as_mut() {
            if source.available() {
                match source.grab() {
                    Ok(frame) => {
                        if let Ok(mut state) = state.lock() {
                            state.last = Some(frame.clone());
                            state.latest = Some(frame);
                            state.live = true;
                        }
                    }
                    Err(_) => {
                        inner = None;
                        if let Ok(mut state) = state.lock() {
                            state.live = false;
                        }
                    }
                }
            } else if let Ok(mut state) = state.lock() {
                state.live = false;
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

impl FrameSource for DeferredSource {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn available(&self) -> bool {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.started {
            state.started = true;
            let (name, probe, factory, shared, alive) = (
                self.name.clone(),
                self.probe.clone(),
                self.factory.clone(),
                self.state.clone(),
                self.alive.clone(),
            );
            std::thread::spawn(move || deferred_worker(name, probe, factory, shared, alive));
        }
        state.live
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow!("deferred source is poisoned"))?;
        if let Some(frame) = state.latest.take() {
            return Ok(frame);
        }
        state
            .last
            .clone()
            .context("deferred source has no frame yet")
    }
}

pub fn deferred_window_source(
    window_id: CaptureWindowId,
    expected: (u32, u32),
    pid: u32,
    app: String,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    let probe =
        Arc::new(move || matches!(window_status(window_id, pid, &app), WindowState::Available));
    let factory = Arc::new(move || window_source(window_id, with_cursor));
    Ok(Box::new(DeferredSource::new(
        format!("Window {} (waiting)", window_id.get()),
        expected,
        probe,
        factory,
    )?))
}

pub fn deferred_monitor_source(
    monitor_id: MonitorId,
    expected: (u32, u32),
    region: Option<Rect>,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    let probe = Arc::new(move || monitor_available(monitor_id));
    let factory = Arc::new(move || monitor_source(monitor_id, region, with_cursor));
    Ok(Box::new(DeferredSource::new(
        format!("Monitor {} (waiting)", monitor_id.get()),
        expected,
        probe,
        factory,
    )?))
}

pub struct MonitorSource {
    monitor: xcap::Monitor,
    name: String,
    region: Option<Rect>,
    size: (u32, u32),
    cursor: Option<((i32, i32), f32)>,
}

impl MonitorSource {
    pub fn new(
        monitor_id: MonitorId,
        region: Option<Rect>,
        with_cursor: bool,
    ) -> anyhow::Result<Self> {
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
        let origin = (
            info.x + logical_to_desktop(rx, info.scale),
            info.y + logical_to_desktop(ry, info.scale),
        );
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

pub struct WindowSource {
    window: xcap::Window,
    id: CaptureWindowId,
    name: String,
    size: (u32, u32),
    with_cursor: bool,
}

impl WindowSource {
    pub fn new(window_id: CaptureWindowId, with_cursor: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(window_available(window_id), "window is minimized or hidden");
        let window = find_window(window_id)?;
        let first = window.capture_image().context("window capture failed")?;
        let size = (first.width() & !1, first.height() & !1);
        anyhow::ensure!(
            size.0 >= 16 && size.1 >= 16,
            "window is too small to record"
        );
        Ok(Self {
            name: window.title().unwrap_or_else(|_| "Window".into()),
            window,
            id: window_id,
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

    fn available(&self) -> bool {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::HWND;
            use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
            let hwnd = self.id.get() as HWND;
            unsafe { IsWindowVisible(hwnd) != 0 && IsIconic(hwnd) == 0 }
        }
        #[cfg(not(windows))]
        {
            !self.window.is_minimized().unwrap_or(false)
        }
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

fn logical_to_desktop(v: i32, scale: f32) -> i32 {
    if cfg!(target_os = "macos") {
        (v as f32 / scale).round() as i32
    } else {
        v
    }
}

fn draw_cursor(frame: &mut Frame, origin: (i32, i32), scale: f32) {
    let Some(c) = crate::cursor::current() else {
        return;
    };
    let k = if cfg!(target_os = "macos") {
        scale
    } else {
        1.0
    };
    let x = ((c.x - origin.0) as f32 * k).round() as i32;
    let y = ((c.y - origin.1) as f32 * k).round() as i32;
    crate::cursor_overlay::draw(frame, &c.image, x, y);
}

pub fn monitor_source(
    monitor_id: MonitorId,
    region: Option<Rect>,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    let src = MonitorSource::new(monitor_id, region, with_cursor)?;
    #[cfg(windows)]
    let fast =
        crate::wgc::WgcSource::monitor(monitor_id, src.name(), src.size(), src.region, with_cursor)
            .map(|s| Box::new(s) as Box<dyn FrameSource>);
    #[cfg(target_os = "linux")]
    let fast = if crate::x11::X11Source::available() {
        let info = monitor_info(&src.monitor, 0)?;
        let r = src.region.unwrap_or(info.bounds());
        let area = Rect::new(info.x + r.x, info.y + r.y, r.w, r.h);
        crate::x11::X11Source::new(crate::x11::Target::Area(area), src.name(), with_cursor)
            .map(|s| Box::new(s) as Box<dyn FrameSource>)
    } else {
        crate::stream::StreamSource::monitor(&src.monitor, src.name(), src.region)
            .map(|s| Box::new(s) as Box<dyn FrameSource>)
    };
    #[cfg(target_os = "macos")]
    let fast = crate::stream::StreamSource::monitor(&src.monitor, src.name(), src.region)
        .map(|s| Box::new(s) as Box<dyn FrameSource>);
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    match fast {
        Ok(s) => return Ok(s),
        Err(e) => log::warn!("{e:#}; recording with screenshots instead"),
    }
    Ok(Box::new(src))
}

pub fn window_source_checked(
    window_id: CaptureWindowId,
    pid: u32,
    app: &str,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    anyhow::ensure!(
        window_status(window_id, pid, app).is_available(),
        "window is closed, hidden or belongs to another process"
    );
    window_source(window_id, with_cursor)
}

pub fn window_source(
    window_id: CaptureWindowId,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    anyhow::ensure!(window_available(window_id), "window is minimized or hidden");
    #[cfg(windows)]
    {
        let name = find_window(window_id)?
            .title()
            .unwrap_or_else(|_| "Window".into());
        match crate::wgc::WgcSource::window(window_id, name, with_cursor) {
            Ok(wgc) => return Ok(Box::new(wgc)),
            Err(e) => log::warn!("{e:#}; recording with screenshots instead"),
        }
    }
    #[cfg(target_os = "linux")]
    if crate::x11::X11Source::available() {
        let name = find_window(window_id)?
            .title()
            .unwrap_or_else(|_| "Window".into());
        match crate::x11::X11Source::new(crate::x11::Target::Window(window_id), name, with_cursor) {
            Ok(s) => return Ok(Box::new(s)),
            Err(e) => log::warn!("{e:#}; recording with screenshots instead"),
        }
    }
    Ok(Box::new(WindowSource::new(window_id, with_cursor)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct FakeSource {
        size: (u32, u32),
        frame: Frame,
    }

    impl FrameSource for FakeSource {
        fn name(&self) -> String {
            "fake".into()
        }
        fn size(&self) -> (u32, u32) {
            self.size
        }
        fn grab(&mut self) -> anyhow::Result<Frame> {
            Ok(self.frame.clone())
        }
    }

    #[test]
    fn deferred_source_keeps_its_size_until_the_source_appears() {
        let ready = Arc::new(AtomicBool::new(false));
        let builds = Arc::new(AtomicUsize::new(0));
        let probe_ready = ready.clone();
        let factory_ready = ready.clone();
        let factory_builds = builds.clone();
        let expected = (400, 300);
        let mut source = DeferredSource::new(
            "deferred".into(),
            expected,
            Arc::new(move || probe_ready.load(Ordering::SeqCst)),
            Arc::new(move || {
                anyhow::ensure!(factory_ready.load(Ordering::SeqCst), "not ready");
                factory_builds.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(FakeSource {
                    size: expected,
                    frame: Frame::new(400, 300, vec![0; 400 * 300 * 4]),
                }) as Box<dyn FrameSource>)
            }),
        )
        .unwrap();
        assert_eq!(source.size(), expected);
        assert!(!source.available());
        assert_eq!(builds.load(Ordering::SeqCst), 0);
        ready.store(true, Ordering::SeqCst);
        for _ in 0..100 {
            if source.available() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(source.available());
        assert_eq!(builds.load(Ordering::SeqCst), 1);
        assert_eq!(source.grab().unwrap().width, 400);
    }

    #[test]
    fn unknown_window_and_monitor_are_unavailable() {
        assert_eq!(
            window_status(CaptureWindowId::new(0), 0, ""),
            WindowState::Missing
        );
        assert_eq!(
            monitor_state(MonitorId::new(u32::MAX)),
            MonitorState::Disconnected
        );
    }
}
