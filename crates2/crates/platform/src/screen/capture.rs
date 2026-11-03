use super::types::{CaptureWindowId, MonitorId, MonitorInfo, WindowInfo};
use anyhow::{anyhow, Context};
use mediaa2_core::frame::Frame;
use mediaa2_core::geometry::Rect;
use mediaa2_core::source::FrameSource;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub(crate) fn monitor_info(m: &xcap::Monitor, index: usize) -> anyhow::Result<MonitorInfo> {
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

pub(crate) fn monitors() -> anyhow::Result<Vec<MonitorInfo>> {
    let mut list: Vec<MonitorInfo> = xcap::Monitor::all()?
        .iter()
        .enumerate()
        .filter_map(|(i, m)| monitor_info(m, i).ok())
        .collect();
    list.sort_by_key(|m| (!m.primary, m.x, m.y));
    Ok(list)
}

pub(crate) fn monitor_at(x: i32, y: i32) -> Option<MonitorId> {
    xcap::Monitor::from_point(x, y)
        .ok()
        .and_then(|m| m.id().ok())
        .map(MonitorId::new)
}

pub(crate) fn find_monitor(id: MonitorId) -> anyhow::Result<xcap::Monitor> {
    xcap::Monitor::all()?
        .into_iter()
        .find(|m| m.id().ok() == Some(id.get()))
        .ok_or_else(|| anyhow!("monitor {} is no longer connected", id.get()))
}

pub(crate) fn find_window(id: CaptureWindowId) -> anyhow::Result<xcap::Window> {
    xcap::Window::all()?
        .into_iter()
        .find(|w| w.id().ok() == Some(id.get()))
        .ok_or_else(|| anyhow!("window {} was closed", id.get()))
}

pub(crate) fn capture_monitor(id: MonitorId) -> anyhow::Result<Frame> {
    let img = find_monitor(id)?
        .capture_image()
        .context("screen capture failed")?;
    Ok(Frame::from_image(img))
}

pub(crate) fn capture_window(id: CaptureWindowId) -> anyhow::Result<Frame> {
    let img = find_window(id)?
        .capture_image()
        .context("window capture failed")?;
    Ok(Frame::from_image(img))
}

pub(crate) fn windows() -> anyhow::Result<Vec<WindowInfo>> {
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

pub(crate) struct DeferredSource {
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
    pub(crate) fn new(
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

pub(crate) struct MonitorSource {
    monitor: xcap::Monitor,
    name: String,
    region: Option<Rect>,
    size: (u32, u32),
    cursor: Option<(i32, i32)>,
}

impl MonitorSource {
    pub(crate) fn new(
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
        let cursor = with_cursor.then_some((info.x + rx, info.y + ry));
        Ok(Self {
            monitor,
            name,
            region,
            size,
            cursor,
        })
    }

    pub(crate) fn region(&self) -> Option<Rect> {
        self.region
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
        if let Some(origin) = self.cursor {
            draw_cursor(&mut frame, origin);
        }
        Ok(frame)
    }
}

pub(crate) struct WindowSource {
    window: xcap::Window,
    id: CaptureWindowId,
    name: String,
    size: (u32, u32),
    with_cursor: bool,
}

impl WindowSource {
    pub(crate) fn new(window_id: CaptureWindowId, with_cursor: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(
            crate::screen::window_available(window_id),
            "window is minimized or hidden"
        );
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
        crate::windows::screen::window_is_available(self.id)
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        let frame = Frame::from_image(self.window.capture_image()?);
        let mut frame = frame.fit_to(self.size.0, self.size.1, [0, 0, 0, 255]);
        if self.with_cursor {
            if let (Ok(x), Ok(y)) = (self.window.x(), self.window.y()) {
                draw_cursor(&mut frame, (x, y));
            }
        }
        Ok(frame)
    }
}

fn draw_cursor(frame: &mut Frame, origin: (i32, i32)) {
    let Some(c) = crate::cursor::current() else {
        return;
    };
    let x = c.x - origin.0;
    let y = c.y - origin.1;
    crate::cursor_overlay::draw(frame, &c.image, x, y);
}
