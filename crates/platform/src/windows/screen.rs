use super::wgc::WgcSource;
use crate::screen::{CaptureWindowId, MonitorId, WindowState};
use mediaa_core::source::FrameSource;
use mediaa_core::{Frame, Rect};

pub(crate) fn window_state(id: CaptureWindowId) -> WindowState {
    use windows_sys::Win32::Foundation::HWND;
    use windows_sys::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindowVisible};
    let hwnd = id.get() as HWND;
    // SAFETY: querying visibility and minimization of a window id owned by this process.
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return WindowState::Hidden;
        }
        if IsIconic(hwnd) != 0 {
            return WindowState::Minimized;
        }
    }
    WindowState::Available
}

pub(crate) fn window_is_available(id: CaptureWindowId) -> bool {
    window_state(id).is_available()
}

pub(crate) fn capture_window(id: CaptureWindowId) -> anyhow::Result<Frame> {
    WgcSource::window_frame(id)
}

pub(crate) fn monitor_source(
    monitor_id: MonitorId,
    name: String,
    size: (u32, u32),
    region: Option<Rect>,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    WgcSource::monitor(monitor_id, name, size, region, with_cursor)
        .map(|source| Box::new(source) as Box<dyn FrameSource>)
}

pub(crate) fn window_source(
    window_id: CaptureWindowId,
    name: String,
    with_cursor: bool,
) -> anyhow::Result<Box<dyn FrameSource>> {
    WgcSource::window(window_id, name, with_cursor)
        .map(|source| Box::new(source) as Box<dyn FrameSource>)
}
