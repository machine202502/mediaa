use std::sync::atomic::AtomicBool;
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = std::path::PathBuf::from(args.next().expect("file"));
    let start: f64 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(3.0);
    let width: u32 = args
        .next()
        .and_then(|value| value.parse().ok())
        .unwrap_or(480);
    let cancel = AtomicBool::new(false);
    let build = Instant::now();
    let proxy = mediaa_media::preview_cache::ensure_proxy(&path, &cancel)?;
    println!(
        "proxy {} in {:.3}s",
        proxy.display(),
        build.elapsed().as_secs_f64()
    );
    let preview = Instant::now();
    let mut first = None;
    let mut frames = 0;
    mediaa_media::preview::preview_frames(
        &path,
        width,
        1.0 / 15.0,
        start,
        &cancel,
        &mut |_frame| {
            first.get_or_insert_with(|| preview.elapsed().as_secs_f64());
            frames += 1;
            true
        },
    )?;
    println!(
        "preview from {start:.2}s: first frame {:.3}s, {frames} frames in {:.3}s",
        first.unwrap_or_default(),
        preview.elapsed().as_secs_f64()
    );
    Ok(())
}
