use crate::context::JobContext;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

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

pub fn run(
    ctx: JobContext,
    total_us: Option<i64>,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let job = ctx.start()?;
    while !job.is_ended() {
        if cancel.load(Ordering::Relaxed) {
            job.abort();
            return Err(Cancelled.into());
        }
        if let Some(total) = total_us.filter(|t| *t > 0) {
            let done = job.output_time_us().unwrap_or(0);
            on_progress((done as f64 / total as f64).clamp(0.0, 1.0) as f32);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    job.wait()?;
    on_progress(1.0);
    Ok(())
}

pub fn run_untracked(ctx: JobContext, cancel: &AtomicBool) -> anyhow::Result<()> {
    run(ctx, None, &mut |_| {}, cancel)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_typed_and_detected() {
        let error: anyhow::Error = Cancelled.into();
        assert!(is_cancelled(&error));
        assert_eq!(error.to_string(), "cancelled");
    }
}
