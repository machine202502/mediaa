use crate::filters;
use crate::info::{probe, MediaInfo};
use crate::job;
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use mediaa_core::formats::{self, AnimFormat};
use mediaa_core::settings::AnimSettings;
use mediaa_webp_pipeline::{self, PipelineConfig, PipelineTuning, Segment, SegmentBackend};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationEffort {
    Fast,
    Balanced,
    Max,
}

impl AnimationEffort {
    fn webp_level(self) -> &'static str {
        match self {
            Self::Fast => "2",
            Self::Balanced => "4",
            Self::Max => "6",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationOrigin {
    Conversion,
    Recording,
}

pub struct AnimationEncodeRequest<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub window: Option<(i64, i64)>,
    pub format: AnimFormat,
    pub settings: &'a AnimSettings,
    pub effort: AnimationEffort,
    pub origin: AnimationOrigin,
    pub segmented_webp: bool,
    pub scheduling: PipelineTuning,
    pub threads: Option<u32>,
    pub trace_id: Option<&'a str>,
}

pub trait AnimationEncoder: Send + Sync {
    fn name(&self) -> &'static str;

    fn encode(
        &self,
        request: &AnimationEncodeRequest<'_>,
        on_progress: &mut dyn FnMut(Option<f32>),
        cancel: &AtomicBool,
    ) -> anyhow::Result<()>;
}

pub struct FfmpegAnimationEncoder;
pub struct SegmentedWebpEncoder;

static FFMPEG: FfmpegAnimationEncoder = FfmpegAnimationEncoder;
static SEGMENTED_WEBP: SegmentedWebpEncoder = SegmentedWebpEncoder;

pub fn backend(
    format: AnimFormat,
    origin: AnimationOrigin,
    segmented_webp: bool,
) -> &'static dyn AnimationEncoder {
    if format == AnimFormat::Webp && origin == AnimationOrigin::Recording && segmented_webp {
        &SEGMENTED_WEBP
    } else {
        &FFMPEG
    }
}

pub fn encode(
    request: &AnimationEncodeRequest<'_>,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    encode_with(
        backend(request.format, request.origin, request.segmented_webp),
        request,
        on_progress,
        cancel,
    )
}

pub fn encode_with(
    encoder: &dyn AnimationEncoder,
    request: &AnimationEncodeRequest<'_>,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    on_progress(None);
    let result = encoder.encode(request, on_progress, cancel);
    if result.is_err() {
        let _ = std::fs::remove_file(request.output);
    }
    result
}

struct FfmpegSegmentBackend<'a> {
    request: &'a AnimationEncodeRequest<'a>,
    base_start_us: i64,
}

impl SegmentBackend for FfmpegSegmentBackend<'_> {
    fn encode(&self, segment: &Segment, cancel: &AtomicBool) -> anyhow::Result<()> {
        let source = Input::from(url(self.request.input))
            .set_video_codec_opt("threads", "1")
            .set_start_time_us(self.base_start_us + segment.start_us)
            .set_recording_time_us(segment.len_us);
        let out = webp_output(
            Output::from(url(&segment.path)).add_stream_map("[v]"),
            self.request.settings.webp_quality,
            self.request.effort,
            None,
        );
        let ctx = FfmpegContext::builder()
            .input(source)
            .filter_desc(filters::webp_segment_graph(self.request.settings))
            .output(out)
            .build()?;
        job::run(ctx, None, &mut |_| {}, cancel)
    }
}

impl AnimationEncoder for SegmentedWebpEncoder {
    fn name(&self) -> &'static str {
        "segmented-webp"
    }

    fn encode(
        &self,
        request: &AnimationEncodeRequest<'_>,
        on_progress: &mut dyn FnMut(Option<f32>),
        cancel: &AtomicBool,
    ) -> anyhow::Result<()> {
        let info = probe(request.input)?;
        anyhow::ensure!(
            info.has_video(),
            "{} has no picture",
            request.input.display()
        );
        let (base_start_us, total_us) = match request.window {
            Some((start, len)) => (start, len),
            None => (0, info.duration_us.unwrap_or(0)),
        };
        anyhow::ensure!(total_us > 0, "recorded WebP source has no duration");
        let (width, height) = filters::scaled_size(
            info.width.max(1),
            info.height.max(1),
            request.settings.height_limit(),
        );
        let config = PipelineConfig::new(
            total_us,
            request.settings.fps,
            width,
            height,
            request.settings.webp_quality,
        )
        .with_tuning(&request.scheduling)
        .with_trace_id(request.trace_id);
        let backend = FfmpegSegmentBackend {
            request,
            base_start_us,
        };
        match mediaa_webp_pipeline::run(&config, &backend, request.output, on_progress, cancel) {
            Ok(_) => Ok(()),
            Err(error) if error.is_cancelled() => Err(job::Cancelled.into()),
            Err(error) => Err(error.source),
        }
    }
}

impl AnimationEncoder for FfmpegAnimationEncoder {
    fn name(&self) -> &'static str {
        "ffmpeg"
    }

    fn encode(
        &self,
        request: &AnimationEncodeRequest<'_>,
        _on_progress: &mut dyn FnMut(Option<f32>),
        cancel: &AtomicBool,
    ) -> anyhow::Result<()> {
        let animated_webp =
            formats::is_webp_path(request.input) && formats::webp_is_animated(request.input)?;
        if animated_webp {
            let (source, info, producer) = webp_frames_input(request.input)?;
            anyhow::ensure!(
                info.has_video(),
                "{} has no picture",
                request.input.display()
            );
            let result = animation_context(source.into(), request.output, request)
                .and_then(|ctx| job::run(ctx, None, &mut |_| {}, cancel));
            let produced = producer
                .join()
                .map_err(|_| anyhow::anyhow!("webp decoder panicked"))?;
            return result.and(produced);
        }

        let info = probe(request.input)?;
        anyhow::ensure!(
            info.has_video(),
            "{} has no picture",
            request.input.display()
        );
        let mut source = Input::from(url(request.input));
        if let Some(threads) = request.threads {
            source = source.set_video_codec_opt("threads", threads.to_string());
        }
        if let Some((start, len)) = request.window {
            source = source.set_start_time_us(start).set_recording_time_us(len);
        }
        let ctx = animation_context(source, request.output, request)?;
        job::run(ctx, None, &mut |_| {}, cancel)
    }
}

fn animation_context(
    source: Input,
    output: &Path,
    request: &AnimationEncodeRequest<'_>,
) -> anyhow::Result<FfmpegContext> {
    let builder = FfmpegContext::builder().input(source);
    let out = Output::from(url(output));
    match request.format {
        AnimFormat::Gif => builder
            .filter_desc(filters::gif_graph(request.settings))
            .output(
                out.set_format("gif")
                    .add_stream_map("[v]")
                    .set_format_opt("loop", "0"),
            )
            .build()
            .map_err(Into::into),
        AnimFormat::Webp => builder
            .filter_desc(filters::webp_graph(request.settings))
            .output(webp_output(
                out.add_stream_map("[v]"),
                request.settings.webp_quality,
                request.effort,
                request.threads,
            ))
            .build()
            .map_err(Into::into),
    }
}

fn webp_output(
    output: Output,
    quality: u8,
    effort: AnimationEffort,
    threads: Option<u32>,
) -> Output {
    let output = output
        .set_format("webp")
        .set_video_codec("libwebp_anim")
        .set_video_codec_opt("lossless", "0")
        .set_video_codec_opt("quality", quality.min(100).to_string())
        .set_video_codec_opt("compression_level", effort.webp_level())
        .set_format_opt("loop", "0");
    match threads {
        Some(threads) => output.set_video_codec_opt("threads", threads.to_string()),
        None => output,
    }
}

fn url(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

pub(crate) fn webp_frames_input(
    path: &Path,
) -> anyhow::Result<(FramePushSource, MediaInfo, JoinHandle<anyhow::Result<()>>)> {
    let file = std::io::BufReader::new(std::fs::File::open(path)?);
    let mut decoder = image_webp::WebPDecoder::new(file)?;
    let (w, h) = decoder.dimensions();
    let pix = if decoder.has_alpha() { "rgba" } else { "rgb24" };
    let info = crate::info::probe_webp(&decoder);
    let (source, mut handle) = FramePushSource::builder(w, h)
        .pixel_format(pix)
        .time_base(1, 1000)
        .build()?;
    let producer = std::thread::Builder::new()
        .name("webp-decode".into())
        .spawn(move || -> anyhow::Result<()> {
            let size = decoder
                .output_buffer_size()
                .ok_or_else(|| anyhow::anyhow!("webp too large"))?;
            let mut buf = vec![0u8; size];
            let mut pts = 0i64;
            for _ in 0..decoder.num_frames() {
                let delay = i64::from(decoder.read_frame(&mut buf)?.max(1));
                if handle.push(&buf, pts, Some(delay)).is_err() {
                    break;
                }
                pts += delay;
            }
            handle.finish();
            Ok(())
        })?;
    Ok((source, info, producer))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct CaptureEncoder;

    impl AnimationEncoder for CaptureEncoder {
        fn name(&self) -> &'static str {
            "capture"
        }

        fn encode(
            &self,
            request: &AnimationEncodeRequest<'_>,
            on_progress: &mut dyn FnMut(Option<f32>),
            _cancel: &AtomicBool,
        ) -> anyhow::Result<()> {
            on_progress(Some(0.5));
            let marker = if request.origin == AnimationOrigin::Recording {
                1u8
            } else {
                0u8
            };
            Ok(std::fs::write(request.output, [marker])?)
        }
    }

    struct FailingEncoder;

    impl AnimationEncoder for FailingEncoder {
        fn name(&self) -> &'static str {
            "failing"
        }

        fn encode(
            &self,
            request: &AnimationEncodeRequest<'_>,
            _on_progress: &mut dyn FnMut(Option<f32>),
            _cancel: &AtomicBool,
        ) -> anyhow::Result<()> {
            std::fs::write(request.output, b"partial")?;
            anyhow::bail!("encoder failed")
        }
    }

    #[test]
    fn webp_recording_uses_budgeted_segments_and_other_paths_stay_direct() {
        assert_eq!(
            backend(AnimFormat::Webp, AnimationOrigin::Recording, true).name(),
            "segmented-webp"
        );
        assert_eq!(
            backend(AnimFormat::Webp, AnimationOrigin::Recording, false).name(),
            "ffmpeg"
        );
        assert_eq!(
            backend(AnimFormat::Webp, AnimationOrigin::Conversion, true).name(),
            "ffmpeg"
        );
        assert_eq!(
            backend(AnimFormat::Gif, AnimationOrigin::Recording, true).name(),
            "ffmpeg"
        );
        assert_eq!(
            backend(AnimFormat::Gif, AnimationOrigin::Conversion, true).name(),
            "ffmpeg"
        );
    }

    #[test]
    fn custom_backend_receives_the_same_request_shape() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.gif");
        let settings = AnimSettings::default();
        let request = AnimationEncodeRequest {
            input: Path::new("input.mkv"),
            output: &output,
            window: None,
            format: AnimFormat::Gif,
            settings: &settings,
            effort: AnimationEffort::Balanced,
            origin: AnimationOrigin::Recording,
            segmented_webp: false,
            scheduling: PipelineTuning::default(),
            threads: None,
            trace_id: None,
        };
        let mut progress = Vec::new();
        encode_with(
            &CaptureEncoder,
            &request,
            &mut |value| progress.push(value),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(progress, [None, Some(0.5)]);
        assert_eq!(std::fs::read(output).unwrap(), [1]);
    }

    #[test]
    fn failed_backend_removes_partial_output() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.webp");
        let settings = AnimSettings::default();
        let request = AnimationEncodeRequest {
            input: Path::new("input.mkv"),
            output: &output,
            window: None,
            format: AnimFormat::Webp,
            settings: &settings,
            effort: AnimationEffort::Balanced,
            origin: AnimationOrigin::Conversion,
            segmented_webp: false,
            scheduling: PipelineTuning::default(),
            threads: None,
            trace_id: None,
        };
        let error = encode_with(
            &FailingEncoder,
            &request,
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("encoder failed"));
        assert!(!output.exists());
    }
}
