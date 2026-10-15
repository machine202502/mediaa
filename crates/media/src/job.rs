//! Running an FFmpeg job with progress reporting and cancellation.

use ez_ffmpeg::FfmpegContext;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Returned (inside `anyhow::Error`) when a job was cancelled by the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

pub fn is_cancelled(err: &anyhow::Error) -> bool {
    err.downcast_ref::<Cancelled>().is_some()
}

/// Starts `ctx`, reports `out_time / total` through `on_progress` (0..=1) and aborts when
/// `cancel` becomes true.
pub fn run(
    ctx: FfmpegContext,
    total_us: Option<i64>,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let scheduler = ctx.start()?;
    let progress = scheduler.progress_handle();
    while !scheduler.is_ended() {
        if cancel.load(Ordering::Relaxed) {
            scheduler.abort();
            return Err(Cancelled.into());
        }
        if let Some(total) = total_us.filter(|t| *t > 0) {
            let done = progress
                .snapshot()
                .outputs()
                .iter()
                .filter_map(|o| o.out_time_us())
                .max()
                .unwrap_or(0);
            on_progress((done as f64 / total as f64).clamp(0.0, 1.0) as f32);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    scheduler.wait()?;
    on_progress(1.0);
    Ok(())
}
