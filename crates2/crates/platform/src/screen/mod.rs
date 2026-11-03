mod capture;
pub mod types;

pub use types::{CaptureWindowId, MonitorId, MonitorInfo, MonitorState, WindowInfo, WindowState};

use capture::{DeferredSource, MonitorSource, WindowSource};
use mediaa2_core::frame::Frame;
use mediaa2_core::geometry::Rect;
use mediaa2_core::source::FrameSource;
use std::sync::Arc;

pub fn monitors() -> anyhow::Result<Vec<MonitorInfo>> {
    capture::monitors()
}

pub fn monitor_at(x: i32, y: i32) -> Option<MonitorId> {
    capture::monitor_at(x, y)
}

pub fn capture_monitor(id: MonitorId) -> anyhow::Result<Frame> {
    capture::capture_monitor(id)
}

pub fn windows() -> anyhow::Result<Vec<WindowInfo>> {
    capture::windows()
}

pub fn window_available(id: CaptureWindowId) -> bool {
    window_status(id, 0, "").is_available()
}

pub fn window_status(id: CaptureWindowId, pid: u32, app: &str) -> WindowState {
    let Ok(window) = capture::find_window(id) else {
        return WindowState::Missing;
    };
    if pid != 0 && window.pid().ok() != Some(pid) {
        return WindowState::Mismatch;
    }
    if !app.is_empty() && window.app_name().ok().is_some_and(|name| name != app) {
        return WindowState::Mismatch;
    }
    crate::windows::screen::window_state(id)
}

pub fn monitor_state(id: MonitorId) -> MonitorState {
    if capture::find_monitor(id).is_ok() {
        MonitorState::Available
    } else {
        MonitorState::Disconnected
    }
}

pub fn monitor_available(id: MonitorId) -> bool {
    monitor_state(id) == MonitorState::Available
}

pub fn capture_window(id: CaptureWindowId) -> anyhow::Result<Frame> {
    match crate::windows::screen::capture_window(id) {
        Ok(frame) => Ok(frame),
        Err(error) => {
            log::warn!("{error:#}; window screenshot through GDI instead");
            capture::capture_window(id)
        }
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

pub fn monitor_source(
    monitor_id: MonitorId,
    region: Option<Rect>,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    let src = MonitorSource::new(monitor_id, region, with_cursor)?;
    match crate::windows::screen::monitor_source(
        monitor_id,
        src.name(),
        src.size(),
        src.region(),
        with_cursor,
    ) {
        Ok(source) => Ok(source),
        Err(error) => {
            log::warn!("{error:#}; recording with screenshots instead");
            Ok(Box::new(src))
        }
    }
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
    let name = capture::find_window(window_id)?
        .title()
        .unwrap_or_else(|_| "Window".into());
    match crate::windows::screen::window_source(window_id, name, with_cursor) {
        Ok(source) => Ok(source),
        Err(error) => {
            log::warn!("{error:#}; recording with screenshots instead");
            Ok(Box::new(WindowSource::new(window_id, with_cursor)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::capture::DeferredSource;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::time::Duration;

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
