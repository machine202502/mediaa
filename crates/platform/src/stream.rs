use anyhow::Context as _;
use mediaa_core::source::FrameSource;
use mediaa_core::{Frame, Rect};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const FIRST_FRAME_TIMEOUT: Duration =
    Duration::from_secs(if cfg!(target_os = "macos") { 5 } else { 60 });

#[derive(Default)]
struct Latest {
    frame: Mutex<Option<Frame>>,
    arrived: Condvar,
}

pub struct StreamSource {
    name: String,
    size: (u32, u32),
    region: Option<Rect>,
    monitor_size: (u32, u32),
    recorder: xcap::VideoRecorder,
    latest: Arc<Latest>,
    stop: Arc<AtomicBool>,
    last: Option<Frame>,
}

impl StreamSource {
    pub fn monitor(
        monitor: &xcap::Monitor,
        name: String,
        region: Option<Rect>,
    ) -> anyhow::Result<Self> {
        let monitor_size = (monitor.width()?, monitor.height()?);
        let (recorder, rx) = monitor.video_recorder().context("screen recorder")?;
        recorder.start().context("start screen recorder")?;
        let latest = Arc::new(Latest::default());
        let stop = Arc::new(AtomicBool::new(false));
        let (l, s) = (latest.clone(), stop.clone());
        if let Err(error) = std::thread::Builder::new()
            .name("capture-stream".into())
            .spawn(move || {
                while !s.load(Ordering::Relaxed) {
                    if let Ok(f) = rx.recv_timeout(Duration::from_millis(200)) {
                        let mut raw = f.raw;
                        if cfg!(target_os = "linux") {
                            raw.as_chunks_mut::<4>()
                                .0
                                .iter_mut()
                                .for_each(|p| p[3] = 255);
                        }
                        *l.frame.lock() = Some(Frame::new(f.width, f.height, raw));
                        l.arrived.notify_all();
                    }
                }
            })
        {
            let _ = recorder.stop();
            return Err(error.into());
        }
        let mut s = Self {
            name,
            size: (0, 0),
            region,
            monitor_size,
            recorder,
            latest,
            stop,
            last: None,
        };
        let mut guard = s.latest.frame.lock();
        s.latest
            .arrived
            .wait_while_for(&mut guard, |f| f.is_none(), FIRST_FRAME_TIMEOUT);
        let first = guard.clone().context("no frame from the screen recorder")?;
        drop(guard);
        let first = s.cut(first);
        s.size = (first.width & !1, first.height & !1);
        anyhow::ensure!(
            s.size.0 >= 16 && s.size.1 >= 16,
            "selected area is too small"
        );
        s.last = Some(s.cut_to_size(first));
        Ok(s)
    }

    fn cut(&self, frame: Frame) -> Frame {
        match self.region {
            Some(r) => {
                let (mw, mh) = self.monitor_size;
                let (sx, sy) = (
                    frame.width as f32 / mw.max(1) as f32,
                    frame.height as f32 / mh.max(1) as f32,
                );
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
        if let Some(f) = self.latest.frame.lock().take() {
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
