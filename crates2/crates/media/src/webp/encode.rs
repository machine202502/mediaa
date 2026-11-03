use crate::ffmpeg::output::Output;
use crate::ffmpeg::{job, Input, JobBuilder};
use crate::webp::graph::graph as webp_graph;
use crate::webp::stream::frame_stream;
use mediaa2_core::formats;
use mediaa2_core::settings::AnimSettings;
use std::path::Path;
use std::sync::atomic::AtomicBool;

#[derive(Debug, Clone, Copy)]
pub struct EncodeOptions {
    pub quality: u8,
    pub compression_level: u8,
    pub threads: Option<u32>,
}

pub fn encode(
    input: &Path,
    output: &Path,
    window: Option<(i64, i64)>,
    settings: &AnimSettings,
    options: EncodeOptions,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if formats::is_webp_path(input) && formats::webp_is_animated(input)? {
        let stream = frame_stream(input)?;
        let context = JobBuilder::new()
            .push_input(stream.source)
            .filter_desc(&webp_graph(settings))
            .output(webp_output(output, options))
            .build()?;
        job::run_untracked(context, cancel)?;
        return stream.producer.wait();
    }
    let mut source = Input::new(input);
    if let Some(threads) = options.threads {
        source = source.video_codec_opt("threads", threads.to_string());
    }
    if let Some((start, len)) = window {
        source = source.start_us(start).duration_us(len);
    }
    let context = JobBuilder::new()
        .input(source)
        .filter_desc(&webp_graph(settings))
        .output(webp_output(output, options))
        .build()?;
    job::run_untracked(context, cancel)
}

fn webp_output(output: &Path, options: EncodeOptions) -> Output {
    let output = Output::new(output)
        .format("webp")
        .video_codec("libwebp_anim")
        .video_opt("lossless", "0")
        .video_opt("quality", options.quality.min(100).to_string())
        .video_opt("compression_level", options.compression_level.to_string())
        .format_opt("loop", "0")
        .stream_map("[v]");
    match options.threads {
        Some(threads) => output.video_opt("threads", threads.to_string()),
        None => output,
    }
}
