use crate::error::{invalid_input, timed_out};
use crate::filters;
use crate::filters::Effort;
use crate::info::{probe, MediaInfo};
use crate::job;
use crate::webp_decode::WebpReader;
use crossbeam_channel::RecvTimeoutError;
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use mediaa_core::formats::{self, AnimFormat};
use mediaa_core::settings::AnimSettings;
use mediaa_webp_pipeline::{self, PipelineConfig, PipelineTuning, Segment, SegmentBackend};
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

const WEBP_DECODE_STOP_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnimationOrigin {
    Conversion,
    Recording,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebpSegmentation {
    Direct,
    Segmented,
}

pub struct AnimationEncodeRequest<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub window: Option<(i64, i64)>,
    pub format: AnimFormat,
    pub settings: &'a AnimSettings,
    pub effort: Effort,
    pub origin: AnimationOrigin,
    pub segmentation: WebpSegmentation,
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
        backend(
            request.format,
            request.origin,
            request.segmentation == WebpSegmentation::Segmented,
        ),
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
        if !info.has_video() {
            return Err(invalid_input(format!(
                "{} has no picture",
                request.input.display()
            )));
        }
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
            Ok(()) => Ok(()),
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
            if !info.has_video() {
                return Err(invalid_input(format!(
                    "{} has no picture",
                    request.input.display()
                )));
            }
            let result = animation_context(source.into(), request.output, request)
                .and_then(|ctx| job::run(ctx, None, &mut |_| {}, cancel));
            result?;
            return producer.wait();
        }

        let info = probe(request.input)?;
        if !info.has_video() {
            return Err(invalid_input(format!(
                "{} has no picture",
                request.input.display()
            )));
        }
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

fn webp_output(output: Output, quality: u8, effort: Effort, threads: Option<u32>) -> Output {
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
) -> anyhow::Result<(FramePushSource, MediaInfo, WebpProducer)> {
    let mut reader = WebpReader::open(path)?;
    let (w, h) = reader.dimensions();
    let pix = if reader.has_alpha() { "rgba" } else { "rgb24" };
    let info = reader.info();
    let frames = reader.frame_count();
    let (source, mut handle) = FramePushSource::builder(w, h)
        .pixel_format(pix)
        .time_base(1, 1000)
        .build()?;
    let (done_tx, done) = crossbeam_channel::bounded(1);
    std::thread::Builder::new()
        .name("webp-decode".into())
        .spawn(move || -> anyhow::Result<()> {
            let result = (|| {
                let mut pts = 0i64;
                for _ in 0..frames {
                    let frame = reader.read_frame()?;
                    let delay = i64::from(frame.delay_ms);
                    if handle.push(frame.data, pts, Some(delay)).is_err() {
                        break;
                    }
                    pts += delay;
                }
                handle.finish();
                Ok(())
            })();
            let _ = done_tx.send(result);
            Ok(())
        })?;
    Ok((
        source,
        info,
        WebpProducer {
            result: done,
            timeout: WEBP_DECODE_STOP_TIMEOUT,
        },
    ))
}

pub(crate) struct WebpProducer {
    result: crossbeam_channel::Receiver<anyhow::Result<()>>,
    timeout: Duration,
}

impl WebpProducer {
    pub(crate) fn wait(self) -> anyhow::Result<()> {
        match self.result.recv_timeout(self.timeout) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(timed_out(format!(
                "WebP decoder did not stop within {} s",
                self.timeout.as_secs()
            ))),
            Err(RecvTimeoutError::Disconnected) => {
                Err(anyhow::anyhow!("WebP decoder stopped without a result"))
            }
        }
    }
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
            effort: Effort::Balanced,
            origin: AnimationOrigin::Recording,
            segmentation: WebpSegmentation::Direct,
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
            effort: Effort::Balanced,
            origin: AnimationOrigin::Conversion,
            segmentation: WebpSegmentation::Direct,
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

    #[test]
    fn webp_producer_wait_is_bounded_and_typed() {
        let (_sender, result) = crossbeam_channel::bounded(1);
        let producer = WebpProducer {
            result,
            timeout: Duration::from_millis(10),
        };
        let error = producer.wait().unwrap_err();
        assert_eq!(
            crate::error::classify_media_error(&error),
            crate::error::MediaErrorKind::Timeout
        );
    }
}
