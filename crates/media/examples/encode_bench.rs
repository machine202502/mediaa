//! Dev helper: `cargo run --release -p mediaa-media --example encode_bench -- 3200 2000 [1080]`
//! pushes frames as fast as possible through the recording encoder (as in `recorder.rs`) and
//! prints the achievable frame rate.
use ez_ffmpeg::{FfmpegContext, FramePushSource, Output};
use std::time::Instant;

fn main() -> anyhow::Result<()> {
    let args: Vec<u32> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    let (w, h) = (args.first().copied().unwrap_or(3200), args.get(1).copied().unwrap_or(2000));
    let max_h = args.get(2).copied();
    let preset = std::env::var("PRESET").unwrap_or_else(|_| "veryfast".into());
    let out = std::env::temp_dir().join("mediaa-encode-bench.mkv");
    let (src, mut handle) = FramePushSource::builder(w, h).pixel_format("rgba").time_base(1, 60).build()?;
    let graph = mediaa_media::filters::scratch_graph("yuv420p", max_h);
    let output = Output::from(out.to_string_lossy().into_owned())
        .set_format("matroska")
        .add_stream_map("[v]")
        .set_video_codec("libx264")
        .set_video_codec_opt("preset", preset.clone())
        .set_video_codec_opt("crf", "16");
    let job = FfmpegContext::builder().input(src).filter_desc(graph).output(output).build()?.start()?;
    let frames: Vec<Vec<u8>> = (0..8u8)
        .map(|i| (0..w * h).flat_map(|p| [(p % 251) as u8 ^ i, (p / w) as u8, i * 30, 255]).collect())
        .collect();
    let n = 180;
    let t = Instant::now();
    for i in 0..n {
        handle.push_owned(frames[i % frames.len()].clone(), i as i64, Some(1))?;
    }
    handle.finish();
    job.wait()?;
    let secs = t.elapsed().as_secs_f64();
    println!("{w}x{h} -> {:?}, preset {preset}: {:.1} fps", max_h, n as f64 / secs);
    let _ = std::fs::remove_file(out);
    Ok(())
}
