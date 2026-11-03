use crate::animation_backends::{self, AnimationEncodeRequest};
use crate::error::invalid_input;
use crate::ffmpeg::{job, Input, JobBuilder, JobContext, Output, PushSource};
use crate::filters;
use crate::info::{probe, MediaInfo};
use mediaa2_core::atomic_file::AtomicFile;
use mediaa2_core::convert::{ConversionOptions, ConvertTarget, Effort, StillFormat, VideoFormat};
use mediaa2_core::formats::{self, AudioFormat, VideoContainer};
use mediaa2_core::limits::AUDIO_KBPS;
use mediaa2_core::naming;
use mediaa2_core::paths::{MediaDir, OutputPath, SourcePath};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

fn muxer(format: VideoFormat) -> &'static str {
    match format {
        VideoFormat::Mkv => "matroska",
        other => other.extension(),
    }
}

pub fn output_path(
    input: &SourcePath,
    target: ConvertTarget,
    title: Option<&str>,
    out_dir: &MediaDir,
) -> OutputPath {
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| naming::parse_path(input).map(|n| n.title))
        .or_else(|| input.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "output".into());
    OutputPath::new(naming::media_path(
        out_dir,
        &title,
        chrono::Local::now(),
        target.extension(),
    ))
}

fn run_tracked(
    ctx: JobContext,
    total_us: Option<i64>,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    match total_us.filter(|total| *total > 0) {
        Some(total) => job::run(
            ctx,
            Some(total),
            &mut |progress| on_progress(Some(progress)),
            cancel,
        ),
        None => job::run(ctx, None, &mut |_| {}, cancel),
    }
}

fn bitrate(kbps: u32) -> String {
    format!("{}k", AUDIO_KBPS.clamp(kbps))
}

fn video_output(out: Output, format: VideoFormat, s: &ConversionOptions, audio: bool) -> Output {
    let out = out.format(muxer(format)).stream_map("[v]");
    let out = match s.max_fps {
        Some(fps) => out.max_fps(fps as i32, 1),
        None => out,
    };
    let out = match format {
        VideoFormat::Mp4 | VideoFormat::Mov | VideoFormat::Mkv => out
            .video_codec("libx264")
            .video_opt("preset", filters::x264_preset(s.effort))
            .video_opt("crf", filters::crf_for_quality(s.quality).to_string()),
        VideoFormat::Webm => out
            .video_codec("libvpx-vp9")
            .video_opt("crf", filters::vp9_crf(s.quality).to_string())
            .video_opt("b", "0")
            .video_opt("deadline", "good")
            .video_opt("cpu-used", filters::vp9_cpu_used(s.effort))
            .video_opt("row-mt", "1"),
        VideoFormat::Avi => {
            let out = out
                .video_codec("mpeg4")
                .video_qscale(filters::qscale(s.quality));
            match s.effort {
                Effort::Fastest | Effort::Fast => out,
                Effort::Balanced => out.video_opt("mbd", "rd"),
                Effort::Max => out.video_opt("mbd", "rd").video_opt("trellis", "1"),
            }
        }
    };
    let out = match format {
        VideoFormat::Mp4 | VideoFormat::Mov => out.format_opt("movflags", "+faststart"),
        _ => out,
    };
    if !audio {
        return out;
    }
    let out = out.stream_map("0:a").audio_bitrate(bitrate(s.audio_kbps));
    let out = if s.mono { out.audio_channels(1) } else { out };
    match format {
        VideoFormat::Mp4 | VideoFormat::Mov | VideoFormat::Mkv => out.audio_codec("aac"),
        VideoFormat::Webm => out.audio_codec("libopus"),
        VideoFormat::Avi => out.audio_codec("libmp3lame"),
    }
}

pub(crate) fn audio_output(out: Output, map: &str, format: AudioFormat, kbps: u32) -> Output {
    let out = out.stream_map(map);
    match format {
        AudioFormat::Mp3 => out
            .format("mp3")
            .audio_codec("libmp3lame")
            .audio_bitrate(bitrate(kbps)),
        AudioFormat::M4a => out
            .format("ipod")
            .audio_codec("aac")
            .audio_bitrate(bitrate(kbps))
            .format_opt("movflags", "+faststart"),
        AudioFormat::Wav => out.format("wav").audio_codec("pcm_s16le"),
        AudioFormat::Flac => out.format("flac").audio_codec("flac"),
        AudioFormat::Ogg => out
            .format("ogg")
            .audio_codec("libopus")
            .audio_bitrate(bitrate(kbps)),
    }
}

fn still_output(out: Output, format: StillFormat, s: &ConversionOptions) -> Output {
    let out = out
        .format("image2")
        .format_opt("update", "1")
        .stream_map("[v]")
        .max_video_frames(1);
    match format {
        StillFormat::Jpeg => out
            .video_codec("mjpeg")
            .video_qscale(filters::qscale(s.quality)),
        StillFormat::Png => out.video_codec("png").video_opt("compression_level", "1"),
        StillFormat::Webp => out
            .video_codec("libwebp")
            .video_opt("lossless", if s.webp_lossless { "1" } else { "0" })
            .video_opt(
                "quality",
                if s.webp_lossless {
                    "100".to_string()
                } else {
                    s.quality.min(100).to_string()
                },
            )
            .video_opt(
                "compression_level",
                filters::webp_level(s.effort).to_string(),
            ),
        StillFormat::Bmp => out.video_codec("bmp"),
        StillFormat::Tiff => out
            .video_codec("tiff")
            .video_opt("compression_algo", "deflate"),
    }
}

fn still_pix_fmt(format: StillFormat, s: &ConversionOptions) -> &'static str {
    match format {
        StillFormat::Jpeg => "yuvj420p",
        StillFormat::Png | StillFormat::Tiff => "rgba",
        StillFormat::Webp if s.webp_lossless => "bgra",
        StillFormat::Webp => "yuva420p",
        StillFormat::Bmp => "bgra",
    }
}

enum InputSource {
    File(Box<Input>),
    Push(PushSource),
}

fn build_context(
    input: InputSource,
    output: &Path,
    target: ConvertTarget,
    s: &ConversionOptions,
    info: &MediaInfo,
) -> anyhow::Result<JobContext> {
    let builder = match input {
        InputSource::File(input) => JobBuilder::new().input(*input),
        InputSource::Push(source) => JobBuilder::new().push_input(source),
    };
    let out = Output::new(output);
    let ctx = match target {
        ConvertTarget::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
        ConvertTarget::Video(format) => {
            let audio = info.has_audio() && !s.remove_audio;
            builder
                .filter_desc(&filters::convert_video_graph(s.max_height))
                .output(video_output(out, format, s, audio))
                .build()?
        }
        ConvertTarget::Still(StillFormat::Png) if s.png_colors.is_some() => builder
            .filter_desc(&filters::palette_graph(
                s.max_height,
                s.png_colors.unwrap_or(256),
            ))
            .output(still_output(out, StillFormat::Png, s))
            .build()?,
        ConvertTarget::Still(format) => builder
            .filter_desc(&filters::still_graph(
                s.max_height,
                still_pix_fmt(format, s),
            ))
            .output(still_output(out, format, s))
            .build()?,
        ConvertTarget::Audio(format) => {
            let out = audio_output(out, "0:a", format, s.audio_kbps);
            let out = if format == AudioFormat::Flac {
                out.audio_opt("compression_level", filters::flac_level(s.effort))
            } else {
                out
            };
            builder
                .output(if s.mono { out.audio_channels(1) } else { out })
                .build()?
        }
    };
    Ok(ctx)
}

fn optimize_png(path: &Path, effort: Effort) -> anyhow::Result<()> {
    let mut opts = oxipng::Options::from_preset(filters::png_preset(effort));
    opts.strip = oxipng::StripChunks::Safe;
    oxipng::optimize(
        &oxipng::InFile::Path(path.to_path_buf()),
        &oxipng::OutFile::from_path(path.to_path_buf()),
        &opts,
    )
    .map_err(|e| anyhow::anyhow!("png optimisation: {e}"))?;
    Ok(())
}

pub fn convert(
    input: &SourcePath,
    output: &OutputPath,
    options: &ConversionOptions,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let pending = AtomicFile::new(output)?;
    convert_part(input, pending.path(), options, None, on_progress, cancel)?;
    pending.commit()?;
    Ok(())
}

fn convert_part(
    input: &Path,
    output: &Path,
    options: &ConversionOptions,
    window: Option<(i64, i64)>,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let target = options.target;
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let ConvertTarget::Anim(format) = target {
        let result = animation_backends::encode(
            &AnimationEncodeRequest {
                input,
                output,
                window,
                format,
                settings: &options.animation,
                effort: options.effort,
                threads: None,
            },
            on_progress,
            cancel,
        );
        if result.is_ok() {
            if let Err(error) = crate::preview_cache::ensure_proxy_for(input, output, cancel) {
                log::warn!("preview proxy {}: {error:#}", input.display());
            }
        }
        return result;
    }
    on_progress(None);
    let animated_webp = formats::is_webp_path(input) && formats::webp_is_animated(input)?;
    let encoded = if animated_webp {
        crate::webp::frame_stream(input).and_then(|stream| {
            let info = crate::info::from_webp(stream.info);
            let ctx = build_context(
                InputSource::Push(stream.source),
                output,
                target,
                options,
                &info,
            )?;
            match target {
                ConvertTarget::Video(_) | ConvertTarget::Audio(_) => {
                    run_tracked(ctx, info.duration_us, on_progress, cancel)?
                }
                ConvertTarget::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
                ConvertTarget::Still(_) => job::run(ctx, None, &mut |_| {}, cancel)?,
            }
            stream.producer.wait()
        })
    } else {
        probe(input).and_then(|info| {
            match target {
                ConvertTarget::Audio(_) => {
                    if !info.has_audio() {
                        return Err(invalid_input(format!("{} has no sound", input.display())));
                    }
                }
                _ => {
                    if !info.has_video() {
                        return Err(invalid_input(format!("{} has no picture", input.display())));
                    }
                }
            }
            let (start, total) = match window {
                Some((start, len)) => (start, Some(len)),
                None => (0, info.duration_us),
            };
            let mut source = Input::new(input);
            if start > 0 {
                source = source.start_us(start);
            }
            if let Some((_, len)) = window {
                source = source.duration_us(len);
            }
            let ctx = build_context(
                InputSource::File(Box::new(source)),
                output,
                target,
                options,
                &info,
            )?;
            match target {
                ConvertTarget::Video(_) | ConvertTarget::Audio(_) => {
                    run_tracked(ctx, total, on_progress, cancel)
                }
                ConvertTarget::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
                ConvertTarget::Still(_) => job::run(ctx, None, &mut |_| {}, cancel),
            }
        })
    };
    let result = encoded.and_then(|()| match target {
        ConvertTarget::Still(StillFormat::Png) => optimize_png(output, options.effort),
        _ => Ok(()),
    });
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

const SAMPLE_US: i64 = 5_000_000;

pub fn estimate_size(
    input: &SourcePath,
    options: &ConversionOptions,
    cancel: &AtomicBool,
) -> anyhow::Result<u64> {
    let dir = tempfile::tempdir()?;
    let output = dir
        .path()
        .join(format!("estimate.{}", options.target.extension()));
    let animated_webp = formats::is_webp_path(input) && formats::webp_is_animated(input)?;
    let duration = if animated_webp || matches!(options.target, ConvertTarget::Still(_)) {
        None
    } else {
        probe(input)?.duration_us
    };
    let window = duration
        .filter(|d| *d > 3 * SAMPLE_US)
        .map(|d| ((d - SAMPLE_US) / 3, SAMPLE_US));
    convert_part(input, &output, options, window, &mut |_| {}, cancel)?;
    let size = std::fs::metadata(&output)?.len();
    Ok(match (window, duration) {
        (Some((_, len)), Some(total)) => (size as f64 * total as f64 / len as f64) as u64,
        _ => size,
    })
}

pub(crate) fn encode_audio_recording(
    tracks: &[PathBuf],
    output: &Path,
    format: AudioFormat,
    kbps: u32,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    anyhow::ensure!(!tracks.is_empty(), "no audio tracks");
    let mut builder = JobBuilder::new();
    for t in tracks {
        builder = builder.input(Input::new(t));
    }
    let out = audio_output(Output::new(output), "[a]", format, kbps);
    let total = crate::ffmpeg::probe::duration_us(&tracks[0]);
    let result = builder
        .filter_desc(&filters::audio_only_mix_graph(tracks.len()))
        .output(out)
        .build()
        .and_then(|ctx| job::run(ctx, total, on_progress, cancel));
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

pub(crate) fn mux_recording(
    video: &Path,
    audio_tracks: &[PathBuf],
    output: &Path,
    container: VideoContainer,
    audio_kbps: u32,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let mut builder = JobBuilder::new().input(Input::new(video));
    for track in audio_tracks {
        builder = builder.input(Input::new(track));
    }
    let mut out = Output::new(output)
        .format(container.muxer())
        .copy_stream_map("0:v");
    if container == VideoContainer::Mp4 {
        out = out.format_opt("movflags", "+faststart");
    }
    if let Some(graph) = filters::audio_mix_graph(audio_tracks.len()) {
        builder = builder.filter_desc(&graph);
        out = out
            .stream_map("[a]")
            .audio_codec("aac")
            .audio_bitrate(bitrate(audio_kbps));
    }
    let total = crate::ffmpeg::probe::duration_us(video);
    let result = builder
        .output(out)
        .build()
        .and_then(|ctx| job::run(ctx, total, on_progress, cancel));
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::formats::{AnimFormat, MediaKind};

    #[test]
    fn targets_by_kind() {
        let defaults = ConversionOptions::default();
        assert_eq!((defaults.quality, defaults.audio_kbps), (100, 256));
        let video = ConvertTarget::for_kind(MediaKind::Video);
        assert_eq!(video.len(), 17);
        assert!(
            video.contains(&ConvertTarget::Video(VideoFormat::Webm))
                && video.contains(&ConvertTarget::Anim(AnimFormat::Gif))
        );
        assert!(
            video.contains(&ConvertTarget::Audio(AudioFormat::Mp3)),
            "sound out of a video"
        );
        assert_eq!(
            ConvertTarget::for_kind(MediaKind::Animation),
            video[..12],
            "animations: no sound"
        );
        assert!(
            video.contains(&ConvertTarget::Still(StillFormat::Png)),
            "first frame as an image"
        );
        assert_eq!(
            ConvertTarget::for_kind(MediaKind::Audio),
            AudioFormat::ALL.map(ConvertTarget::Audio).to_vec()
        );
        let images = ConvertTarget::for_kind(MediaKind::Image);
        assert_eq!(images, StillFormat::ALL.map(ConvertTarget::Still).to_vec());
        assert!(StillFormat::Jpeg.has_quality() && !StillFormat::Png.has_quality());
        assert_eq!(muxer(VideoFormat::Mkv), "matroska");
        assert_eq!(ConvertTarget::Still(StillFormat::Jpeg).extension(), "jpg");
        for kind in [
            MediaKind::Image,
            MediaKind::Video,
            MediaKind::Animation,
            MediaKind::Audio,
        ] {
            for target in ConvertTarget::for_kind(kind) {
                assert!(
                    MediaKind::from_extension(target.extension()).is_some(),
                    "{kind:?}: {} is not a known media extension",
                    target.extension()
                );
            }
        }
    }

    #[test]
    fn audio_bitrate_uses_the_shared_limits() {
        assert_eq!(bitrate(0), "32k");
        assert_eq!(bitrate(192), "192k");
        assert_eq!(bitrate(1000), "320k");
    }

    #[test]
    fn output_names() {
        let dir = tempfile::tempdir().unwrap();
        let title = |input: &str, target, name: Option<&str>| {
            let p = output_path(
                &SourcePath::new(input),
                target,
                name,
                &MediaDir::new(dir.path()),
            );
            (
                naming::parse_path(&p).unwrap().title,
                p.extension().unwrap().to_string_lossy().into_owned(),
            )
        };
        let clip = "/x/My clip_2026-10-13_09-05-07.mp4";
        assert_eq!(
            title(clip, ConvertTarget::Anim(AnimFormat::Gif), None),
            ("My clip".into(), "gif".into())
        );
        assert_eq!(
            title(
                clip,
                ConvertTarget::Video(VideoFormat::Webm),
                Some("  Short ")
            ),
            ("Short".into(), "webm".into())
        );
        assert_eq!(
            title(clip, ConvertTarget::Video(VideoFormat::Mp4), Some("")),
            ("My clip".into(), "mp4".into()),
            "empty name: default"
        );
        assert_eq!(
            title(
                "/x/holiday.MOV",
                ConvertTarget::Video(VideoFormat::Mp4),
                None
            )
            .0,
            "holiday",
            "files from elsewhere: their stem"
        );
    }

    #[test]
    fn failed_conversion_preserves_an_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("broken.mp4");
        let output = dir.path().join("result.mp4");
        std::fs::write(&input, b"not media").unwrap();
        std::fs::write(&output, b"keep me").unwrap();
        let error = convert(
            &SourcePath::new(&input),
            &OutputPath::new(&output),
            &ConversionOptions::for_target(ConvertTarget::Video(VideoFormat::Mp4)),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(std::fs::read(output).unwrap(), b"keep me");
    }
}
