//! Windows recording sources on Windows Graphics Capture (`windows-capture`): the GPU delivers
//! a frame whenever the screen / window changes (up to the display refresh rate), the cursor
//! is composited by Windows itself. `grab` returns the newest frame without waiting, so the
//! recorder keeps its frame rate even for large displays (GDI screenshots take ~40 ms at 4K).

use anyhow::{anyhow, Context as _};
use mediaa_core::source::FrameSource;
use mediaa_core::{Frame, Rect};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;
use windows_capture::capture::{CaptureControl, Context, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame as WgcFrame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::monitor::Monitor;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings, GraphicsCaptureItemType,
    MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};
use windows_capture::window::Window;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Newest captured frame, shared between the capture thread and `grab`.
#[derive(Default)]
struct Latest {
    frame: Mutex<Option<Frame>>,
    arrived: Condvar,
}

struct Handler {
    latest: Arc<Latest>,
    region: Option<Rect>,
    scratch: Vec<u8>,
}

impl GraphicsCaptureApiHandler for Handler {
    type Flags = (Arc<Latest>, Option<Rect>);
    type Error = BoxError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        let (latest, region) = ctx.flags;
        Ok(Self { latest, region, scratch: Vec::new() })
    }

    fn on_frame_arrived(&mut self, frame: &mut WgcFrame, _control: InternalCaptureControl) -> Result<(), Self::Error> {
        let buffer = match self.region {
            Some(r) => frame.buffer_crop(r.x as u32, r.y as u32, (r.x + r.w) as u32, (r.y + r.h) as u32)?,
            None => frame.buffer()?,
        };
        let (w, h) = (buffer.width(), buffer.height());
        let data = buffer.as_nopadding_buffer(&mut self.scratch).to_vec();
        *self.latest.frame.lock().expect("frame lock") = Some(Frame::new(w, h, data));
        self.latest.arrived.notify_all();
        Ok(())
    }
}

/// A display (optionally a region of it) or a window captured with WGC.
pub struct WgcSource {
    name: String,
    size: (u32, u32),
    latest: Arc<Latest>,
    last: Option<Frame>,
    /// Grabs that got a new picture vs. all grabs (logged at the end, to diagnose frame rates).
    fresh: u64,
    grabs: u64,
    control: Option<CaptureControl<Handler, BoxError>>,
}

impl WgcSource {
    /// `monitor_id` / `region` as for `screen::MonitorSource` (`region` already clamped, even).
    pub fn monitor(monitor_id: u32, name: String, size: (u32, u32), region: Option<Rect>, cursor: bool) -> anyhow::Result<Self> {
        let monitor = Monitor::from_raw_hmonitor(monitor_id as i32 as isize as *mut std::ffi::c_void);
        Self::start(monitor, name, size, region, cursor)
    }

    /// Window by its id (HWND); the frame size is fixed to the first frame's.
    pub fn window(window_id: u32, name: String, cursor: bool) -> anyhow::Result<Self> {
        let window = Window::from_raw_hwnd(window_id as i32 as isize as *mut std::ffi::c_void);
        let mut s = Self::start(window, name, (0, 0), None, cursor)?;
        let first = s.wait_first()?;
        s.size = (first.width & !1, first.height & !1);
        anyhow::ensure!(s.size.0 >= 16 && s.size.1 >= 16, "window is too small to record");
        Ok(s)
    }

    /// One picture of a window (screenshots): exactly its visible frame, without the
    /// invisible resize borders or the drop shadow.
    pub fn window_frame(window_id: u32) -> anyhow::Result<Frame> {
        let window = Window::from_raw_hwnd(window_id as i32 as isize as *mut std::ffi::c_void);
        Self::start(window, String::new(), (0, 0), None, false)?.wait_first()
    }

    fn start<T>(item: T, name: String, size: (u32, u32), region: Option<Rect>, cursor: bool) -> anyhow::Result<Self>
    where
        T: TryInto<GraphicsCaptureItemType> + Clone + Send + 'static,
    {
        let latest = Arc::new(Latest::default());
        let cursor = if cursor { CursorCaptureSettings::WithCursor } else { CursorCaptureSettings::WithoutCursor };
        let settings = |border| {
            Settings::new(
                item.clone(),
                cursor,
                border,
                SecondaryWindowSettings::Default,
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Rgba8,
                (latest.clone(), region),
            )
        };
        // Hiding the yellow capture border needs Windows 11; older systems keep it.
        let control = Handler::start_free_threaded(settings(DrawBorderSettings::WithoutBorder))
            .or_else(|_| Handler::start_free_threaded(settings(DrawBorderSettings::Default)))
            .map_err(|e| anyhow!("Windows Graphics Capture: {e}"))?;
        let mut s = Self { name, size, latest, last: None, fresh: 0, grabs: 0, control: Some(control) };
        if size != (0, 0) {
            s.wait_first()?;
        }
        Ok(s)
    }

    /// Waits for the first frame (WGC delivers it within a few milliseconds).
    fn wait_first(&mut self) -> anyhow::Result<Frame> {
        let guard = self.latest.frame.lock().expect("frame lock");
        let (guard, _) = self
            .latest
            .arrived
            .wait_timeout_while(guard, Duration::from_secs(3), |f| f.is_none())
            .expect("frame lock");
        let frame = guard.clone().context("no frame from Windows Graphics Capture")?;
        drop(guard);
        self.last = Some(frame.clone());
        Ok(frame)
    }
}

impl FrameSource for WgcSource {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The newest frame; the previous one again if the screen has not changed.
    fn grab(&mut self) -> anyhow::Result<Frame> {
        if self.control.as_ref().is_some_and(|c| c.is_finished()) {
            anyhow::bail!("capture ended (window closed or display disconnected)");
        }
        self.grabs += 1;
        // Never wait here: the recorder's frame timer must keep its pace (waiting for a fresh
        // picture costs whole frames when the display refresh drifts against the timer).
        if let Some(f) = self.latest.frame.lock().expect("frame lock").take() {
            self.last = Some(f);
            self.fresh += 1;
        }
        let frame = self.last.clone().context("no frame yet")?;
        let (w, h) = self.size;
        Ok(if (frame.width, frame.height) == (w, h) { frame } else { frame.fit_to(w, h, [0, 0, 0, 255]) })
    }
}

impl Drop for WgcSource {
    fn drop(&mut self) {
        log::info!("capture \"{}\": {} of {} frames had new content", self.name, self.fresh, self.grabs);
        if let Some(c) = self.control.take() {
            let _ = c.stop();
        }
    }
}
