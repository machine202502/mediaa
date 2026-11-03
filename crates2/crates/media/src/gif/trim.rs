use crate::ffmpeg::{job, Input, JobBuilder, Output};
use mediaa2_core::atomic_file::AtomicFile;
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub fn trim(path: &Path, max_duration_us: i64, cancel: &AtomicBool) -> anyhow::Result<()> {
    let pending = AtomicFile::new(path)?;
    let seconds = max_duration_us as f64 / 1_000_000.0;
    let context = JobBuilder::new()
        .input(Input::new(path))
        .output(
            Output::new(pending.path())
                .format("gif")
                .format_opt("t", format!("{seconds:.6}"))
                .stream_map("0:v"),
        )
        .build()?;
    job::run_untracked(context, cancel)?;
    pending.commit_replace()?;
    Ok(())
}
