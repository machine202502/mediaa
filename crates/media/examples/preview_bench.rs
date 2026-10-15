//! Dev helper: `cargo run --release -p mediaa-media --example preview_bench -- <file> [width] [interval]`
//! times the preview frame decoder (frames delivered and how far ahead of real time it runs).
use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("file"));
    let width: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(480);
    let interval: f64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(1.0 / 15.0);
    let t = Instant::now();
    let mut n = 0;
    let mut last = 0.0;
    let mut worst_lag: f64 = 0.0;
    mediaa_media::preview::preview_frames(&path, width, interval, 0.0, &AtomicBool::new(false), &mut |f| {
        n += 1;
        last = f.at;
        // Lag: frame decoded later than its play time (would stall the preview).
        worst_lag = worst_lag.max(t.elapsed().as_secs_f64() - f.at);
        true
    })?;
    let secs = t.elapsed().as_secs_f64();
    println!("{n} frames up to {last:.2}s in {secs:.2}s ({:.0} ms/frame), worst lag {worst_lag:.2}s", secs * 1000.0 / f64::from(n.max(1)));
    Ok(())
}
