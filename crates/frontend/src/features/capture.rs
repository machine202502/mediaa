use mediaa_core::jobs::Job;
use mediaa_core::Frame;
use mediaa_protocol::backend::CaptureSourceWire;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const CAPTURE_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Captured {
    pub frame: Frame,
    pub source: Option<CaptureSourceWire>,
}

pub struct ScreenshotDone {
    pub copied: bool,
    pub saved: Option<(PathBuf, String)>,
    pub copy_error: Option<String>,
    pub save_error: Option<String>,
    pub copy_file_error: Option<String>,
    pub size: String,
}

pub struct CaptureResult<K> {
    pub kind: K,
    pub result: Result<Captured, String>,
    pub timed_out: bool,
}

pub struct ScreenshotResult {
    pub result: Result<ScreenshotDone, String>,
    pub after_completion: bool,
    pub copies_file: bool,
}

struct CaptureJob<K> {
    kind: K,
    started: Instant,
    job: Job<Result<Captured, String>>,
}

struct ScreenshotJob {
    after_completion: bool,
    copies_file: bool,
    job: Job<ScreenshotDone>,
}

struct CopyImageJob {
    job: Job<Result<String, String>>,
}

pub struct CaptureController<K> {
    capture: Option<CaptureJob<K>>,
    screenshot: Option<ScreenshotJob>,
    copy_image: Option<CopyImageJob>,
}

pub type AppCapture = CaptureController<CaptureKind>;

impl<K> Default for CaptureController<K> {
    fn default() -> Self {
        Self {
            capture: None,
            screenshot: None,
            copy_image: None,
        }
    }
}

impl<K: Clone + Send + 'static> CaptureController<K> {
    pub fn busy(&self) -> bool {
        self.capture.is_some() || self.screenshot.is_some() || self.copy_image.is_some()
    }

    #[cfg(test)]
    pub fn screenshot_busy(&self) -> bool {
        self.screenshot.is_some()
    }

    pub fn capture_busy(&self) -> bool {
        self.capture.is_some()
    }

    pub fn start_capture(
        &mut self,
        kind: K,
        worker: impl FnOnce() -> Result<Captured, String> + Send + 'static,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.capture.is_none(), "capture is already running");
        let job = Job::spawn("screen-capture", Arc::default(), worker)?;
        self.capture = Some(CaptureJob {
            kind,
            started: Instant::now(),
            job,
        });
        Ok(())
    }

    pub fn poll_capture(&mut self) -> Option<CaptureResult<K>> {
        let job = self.capture.as_mut()?;
        if let Some(result) = job.job.poll() {
            let job = self.capture.take().expect("capture job checked");
            return Some(CaptureResult {
                kind: job.kind,
                result: result.unwrap_or_else(|_| Err("capture worker stopped".into())),
                timed_out: false,
            });
        }
        if job.started.elapsed() >= CAPTURE_RESPONSE_TIMEOUT {
            let job = self.capture.take().expect("capture job checked");
            return Some(CaptureResult {
                kind: job.kind,
                result: Err("capture timed out".into()),
                timed_out: true,
            });
        }
        None
    }

    pub fn start_screenshot(
        &mut self,
        after_completion: bool,
        copies_file: bool,
        worker: impl FnOnce() -> ScreenshotDone + Send + 'static,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.screenshot.is_none(), "screenshot is already running");
        let job = Job::spawn("screenshot", Arc::default(), worker)?;
        self.screenshot = Some(ScreenshotJob {
            after_completion,
            copies_file,
            job,
        });
        Ok(())
    }

    pub fn poll_screenshot(&mut self) -> Option<ScreenshotResult> {
        let job = self.screenshot.as_mut()?;
        let result = job.job.poll()?;
        let job = self.screenshot.take().expect("screenshot job checked");
        Some(ScreenshotResult {
            result: result.map_err(|_| "screenshot worker stopped".into()),
            after_completion: job.after_completion,
            copies_file: job.copies_file,
        })
    }

    pub fn start_copy_image(
        &mut self,
        worker: impl FnOnce() -> Result<String, String> + Send + 'static,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(self.copy_image.is_none(), "image copy is already running");
        self.copy_image = Some(CopyImageJob {
            job: Job::spawn("copy-image", Arc::default(), worker)?,
        });
        Ok(())
    }

    pub fn poll_copy_image(&mut self) -> Option<Result<String, String>> {
        let job = self.copy_image.as_mut()?;
        let result = job.job.poll()?;
        self.copy_image = None;
        Some(result.unwrap_or_else(|_| Err("image worker stopped".into())))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum CaptureKind {
    Region {
        purpose: crate::features::region_editor::Purpose,
        after_completion: bool,
        restore: bool,
        start: usize,
    },
    Fullscreen {
        after_completion: bool,
    },
    Window,
    OverlayMonitor {
        index: usize,
    },
}
