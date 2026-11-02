use crate::graph::{graph as gif_graph, palette_graph, use_graph};
use mediaa_core::limits::FPS;
use mediaa_core::settings::AnimSettings;
use mediaa_ffmpeg::probe::probe;
use mediaa_ffmpeg::{job, Input, JobBuilder, Output, PushSource};
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub fn encode(
    input: &Path,
    output: &Path,
    window: Option<(i64, i64)>,
    settings: &AnimSettings,
    threads: Option<u32>,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if !needs_two_pass(input, window, settings) {
        return encode_single(input, output, window, settings, threads, cancel);
    }
    let palette = output.with_file_name(format!(
        ".{}.palette.png",
        output
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("gif")
    ));
    let _ = std::fs::remove_file(&palette);
    let result = palette_pass(input, &palette, window, settings, threads, cancel)
        .and_then(|()| encode_pass(input, &palette, output, window, settings, threads, cancel));
    let _ = std::fs::remove_file(&palette);
    result
}

fn encode_single(
    input: &Path,
    output: &Path,
    window: Option<(i64, i64)>,
    settings: &AnimSettings,
    threads: Option<u32>,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let context = JobBuilder::new()
        .input(input_source(input, window, threads))
        .filter_desc(&gif_graph(settings))
        .output(gif_output(output))
        .build()?;
    job::run_untracked(context, cancel)
}

const TWO_PASS_BYTES: u64 = 128 * 1024 * 1024;

fn needs_two_pass(input: &Path, window: Option<(i64, i64)>, settings: &AnimSettings) -> bool {
    let Ok(info) = probe(input) else {
        return true;
    };
    let duration_us = window
        .map(|(_, len)| len.max(0) as u64)
        .or_else(|| info.duration_us.map(|duration| duration.max(0) as u64))
        .unwrap_or(0);
    buffered_bytes(
        info.width,
        info.height,
        duration_us,
        FPS.clamp(settings.fps),
    ) > TWO_PASS_BYTES
}

fn buffered_bytes(width: u32, height: u32, duration_us: u64, fps: u32) -> u64 {
    let frames = duration_us.saturating_mul(u64::from(fps)) / 1_000_000;
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(4)
        .saturating_mul(frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_short_gifs_stay_single_pass() {
        assert!(buffered_bytes(854, 480, 4_000_000, 15) <= TWO_PASS_BYTES);
    }

    #[test]
    fn large_or_long_gifs_use_two_pass() {
        assert!(buffered_bytes(854, 480, 4_000_000, 30) > TWO_PASS_BYTES);
        assert!(buffered_bytes(1920, 1080, 4_000_000, 15) > TWO_PASS_BYTES);
        assert!(buffered_bytes(3840, 2160, 2_000_000, 30) > TWO_PASS_BYTES);
    }
}

fn palette_pass(
    input: &Path,
    palette: &Path,
    window: Option<(i64, i64)>,
    settings: &AnimSettings,
    threads: Option<u32>,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let context = JobBuilder::new()
        .input(input_source(input, window, threads))
        .filter_desc(&palette_graph(settings))
        .output(
            Output::new(palette)
                .format("image2")
                .video_codec("png")
                .stream_map("[p]"),
        )
        .build()?;
    job::run_untracked(context, cancel)
}

fn encode_pass(
    input: &Path,
    palette: &Path,
    output: &Path,
    window: Option<(i64, i64)>,
    settings: &AnimSettings,
    threads: Option<u32>,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let context = JobBuilder::new()
        .input(input_source(input, window, threads))
        .input(Input::new(palette))
        .filter_desc(&use_graph(settings))
        .output(gif_output(output))
        .build()?;
    job::run_untracked(context, cancel)
}

fn input_source(input: &Path, window: Option<(i64, i64)>, threads: Option<u32>) -> Input {
    let mut source = Input::new(input);
    if let Some(threads) = threads {
        source = source.video_codec_opt("threads", threads.to_string());
    }
    if let Some((start, len)) = window {
        source = source.start_us(start).duration_us(len);
    }
    source
}

pub fn encode_frames(
    source: PushSource,
    output: &Path,
    settings: &AnimSettings,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let context = JobBuilder::new()
        .push_input(source)
        .filter_desc(&gif_graph(settings))
        .output(gif_output(output))
        .build()?;
    job::run_untracked(context, cancel)
}

fn gif_output(output: &Path) -> Output {
    Output::new(output)
        .format("gif")
        .stream_map("[v]")
        .format_opt("loop", "0")
}
