use crate::graph::graph as gif_graph;
use mediaa_core::settings::AnimSettings;
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
    let mut source = Input::new(input);
    if let Some(threads) = threads {
        source = source.video_codec_opt("threads", threads.to_string());
    }
    if let Some((start, len)) = window {
        source = source.start_us(start).duration_us(len);
    }
    let context = JobBuilder::new()
        .input(source)
        .filter_desc(&gif_graph(settings))
        .output(gif_output(output))
        .build()?;
    job::run_untracked(context, cancel)
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
