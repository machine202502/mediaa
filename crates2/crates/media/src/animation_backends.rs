use crate::error::invalid_input;
use crate::filters;
use crate::info::probe;
use crate::webp::EncodeOptions;
use mediaa2_core::convert::Effort;
use mediaa2_core::formats::{self, AnimFormat};
use mediaa2_core::settings::AnimSettings;
use std::path::Path;
use std::sync::atomic::AtomicBool;

pub struct AnimationEncodeRequest<'a> {
    pub input: &'a Path,
    pub output: &'a Path,
    pub window: Option<(i64, i64)>,
    pub format: AnimFormat,
    pub settings: &'a AnimSettings,
    pub effort: Effort,
    pub threads: Option<u32>,
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

static FFMPEG: FfmpegAnimationEncoder = FfmpegAnimationEncoder;

pub fn encode(
    request: &AnimationEncodeRequest<'_>,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    encode_with(&FFMPEG, request, on_progress, cancel)
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
        if !probe(request.input)?.has_video() {
            return Err(invalid_input(format!(
                "{} has no picture",
                request.input.display()
            )));
        }
        match request.format {
            AnimFormat::Gif => {
                if formats::is_webp_path(request.input) && formats::webp_is_animated(request.input)?
                {
                    let stream = crate::webp::frame_stream(request.input)?;
                    crate::gif::encode_frames(
                        stream.source,
                        request.output,
                        request.settings,
                        cancel,
                    )?;
                    return stream.producer.wait();
                }
                crate::gif::encode(
                    request.input,
                    request.output,
                    request.window,
                    request.settings,
                    request.threads,
                    cancel,
                )
            }
            AnimFormat::Webp => crate::webp::encode(
                request.input,
                request.output,
                request.window,
                request.settings,
                EncodeOptions {
                    quality: request.settings.webp_quality,
                    compression_level: filters::webp_level(request.effort),
                    threads: request.threads,
                },
                cancel,
            ),
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
            Ok(std::fs::write(request.output, [1u8])?)
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
            threads: None,
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
            threads: None,
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
