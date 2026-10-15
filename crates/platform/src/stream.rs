//! Recording source on `xcap`'s streaming monitor recorder: AVFoundation screen input on macOS
//! (cursor drawn by the system), PipeWire via the ScreenCast portal on Wayland. Frames arrive
//! on xcap's thread; `grab` hands out the newest one without waiting, so the recorder keeps
//! its frame rate.

use anyhow::Context as _;
use mediaa_core::source::FrameSource;
use mediaa_core::{Frame, Rect};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// macOS answers at once; on Wayland the user first confirms the screen-share prompt.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(if cfg!(target_os = "macos") { 5 } else { 60 });

#[derive(Default)]
struct Latest {
    frame: Mutex<Option<Frame>>,
    arrived: Condvar,
}

pub struct StreamSource {
    name: String,
    size: (u32, u32),
    /// Area of the monitor to keep (monitor pixels), `None` = everything.
    region: Option<Rect>,
    /// Monitor size in the same pixels as `region` (frames may come at another scale).
    monitor_size: (u32, u32),
    recorder: xcap::VideoRecorder,
    latest: Arc<Latest>,
    stop: Arc<AtomicBool>,
    last: Option<Frame>,
}

impl StreamSource {
    /// The frame size comes from the stream itself (it may differ from `xcap`'s monitor size,
    /// e.g. points vs. pixels on macOS).
    pub fn monitor(monitor: &xcap::Monitor, name: String, region: Option<Rect>) -> anyhow::Result<Self> {
        let monitor_size = (monitor.width()?, monitor.height()?);
        let (recorder, rx) = monitor.video_recorder().context("screen recorder")?;
        let latest = Arc::new(Latest::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (l, s) = (latest.clone(), stop.clone());
        std::thread::Builder::new().name("capture-stream".into()).spawn(move || {
            while !s.load(Ordering::Relaxed) {
                if let Ok(f) = rx.recv_timeout(Duration::from_millis(200)) {
                    let mut raw = f.raw;
                    // PipeWire RGBx/BGRx streams leave the fourth byte undefined.
                    if cfg!(target_os = "linux") {
                        raw.as_chunks_mut::<4>().0.iter_mut().for_each(|p| p[3] = 255);
                    }
                    *l.frame.lock().expect("frame lock") = Some(Frame::new(f.width, f.height, raw));
                    l.arrived.notify_all();
                }
            }
        })?;
        recorder.start().context("start screen recorder")?;
        let mut s = Self { name, size: (0, 0), region, monitor_size, recorder, latest, stop, last: None };
        // The first frame proves the stream works (Wayland: after the permission prompt).
        let guard = s.latest.frame.lock().expect("frame lock");
        let (guard, _) = s
            .latest
            .arrived
            .wait_timeout_while(guard, FIRST_FRAME_TIMEOUT, |f| f.is_none())
            .expect("frame lock");
        let first = guard.clone().context("no frame from the screen recorder")?;
        drop(guard);
        let first = s.cut(first);
        s.size = (first.width & !1, first.height & !1);
        anyhow::ensure!(s.size.0 >= 16 && s.size.1 >= 16, "selected area is too small");
        s.last = Some(s.cut_to_size(first));
        Ok(s)
    }

    /// Crops the region (scaled if the stream delivers another resolution than `xcap` reports).
    fn cut(&self, frame: Frame) -> Frame {
        match self.region {
            Some(r) => {
                let (mw, mh) = self.monitor_size;
                let (sx, sy) = (frame.width as f32 / mw.max(1) as f32, frame.height as f32 / mh.max(1) as f32);
                let scaled = Rect::new(
                    (r.x as f32 * sx).round() as i32,
                    (r.y as f32 * sy).round() as i32,
                    (r.w as f32 * sx).round() as i32,
                    (r.h as f32 * sy).round() as i32,
                );
                frame.crop(scaled)
            }
            None => frame,
        }
    }

    fn cut_to_size(&self, frame: Frame) -> Frame {
        let (w, h) = self.size;
        if (frame.width, frame.height) == (w, h) {
            frame
        } else {
            frame.fit_to(w, h, [0, 0, 0, 255])
        }
    }
}

impl FrameSource for StreamSource {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        if let Some(f) = self.latest.frame.lock().expect("frame lock").take() {
            self.last = Some(self.cut_to_size(self.cut(f)));
        }
        self.last.clone().context("no frame yet")
    }
}

impl Drop for StreamSource {
    fn drop(&mut self) {
        let _ = self.recorder.stop();
        self.stop.store(true, Ordering::Relaxed);
    }
}
