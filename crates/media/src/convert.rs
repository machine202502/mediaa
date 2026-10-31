use crate::animation_backends::{self, AnimationEncodeRequest};
use crate::error::invalid_input;
use crate::filters;
use crate::info::{probe, MediaInfo};
use mediaa_core::atomic_file::AtomicFile;
use mediaa_core::formats::{self, AnimFormat, AudioFormat, MediaKind, VideoContainer};
use mediaa_core::limits::{AUDIO_KBPS, DEFAULT_MUSIC_AUDIO_KBPS};
use mediaa_core::naming;
use mediaa_core::paths::{MediaDir, OutputPath, SourcePath};
use mediaa_core::settings::AnimSettings;
use mediaa_ffmpeg::{job, Input, JobBuilder, JobContext, Output, PushSource};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

pub use crate::filters::Effort;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoFormat {
    Mp4,
    Mov,
    Mkv,
    Webm,
    Avi,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 5] = [
        VideoFormat::Mp4,
        VideoFormat::Mov,
        VideoFormat::Mkv,
        VideoFormat::Webm,
        VideoFormat::Avi,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "mp4",
            VideoFormat::Mov => "mov",
            VideoFormat::Mkv => "mkv",
            VideoFormat::Webm => "webm",
            VideoFormat::Avi => "avi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoFormat::Mp4 => "MP4",
            VideoFormat::Mov => "MOV",
            VideoFormat::Mkv => "MKV",
            VideoFormat::Webm => "WebM",
            VideoFormat::Avi => "AVI",
        }
    }

    fn muxer(self) -> &'static str {
        match self {
            VideoFormat::Mkv => "matroska",
            other => other.extension(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StillFormat {
    Jpeg,
    Png,
    Webp,
    Bmp,
    Tiff,
}

impl StillFormat {
    pub const ALL: [StillFormat; 5] = [
        StillFormat::Jpeg,
        StillFormat::Png,
        StillFormat::Webp,
        StillFormat::Bmp,
        StillFormat::Tiff,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            StillFormat::Jpeg => "jpg",
            StillFormat::Png => "png",
            StillFormat::Webp => "webp",
            StillFormat::Bmp => "bmp",
            StillFormat::Tiff => "tiff",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StillFormat::Jpeg => "JPEG",
            StillFormat::Png => "PNG",
            StillFormat::Webp => "WebP",
            StillFormat::Bmp => "BMP",
            StillFormat::Tiff => "TIFF",
        }
    }

    pub fn has_quality(self) -> bool {
        matches!(self, StillFormat::Jpeg | StillFormat::Webp)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Video(VideoFormat),
    Anim(AnimFormat),
    Still(StillFormat),
    Audio(AudioFormat),
}

impl Target {
    pub fn extension(self) -> &'static str {
        match self {
            Target::Video(v) => v.extension(),
            Target::Anim(a) => a.extension(),
            Target::Still(s) => s.extension(),
            Target::Audio(a) => a.extension(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Target::Video(v) => v.label(),
            Target::Anim(a) => a.label(),
            Target::Still(s) => s.label(),
            Target::Audio(a) => a.label(),
        }
    }

    pub fn for_kind(kind: MediaKind) -> Vec<Target> {
        let moving = VideoFormat::ALL
            .map(Target::Video)
            .into_iter()
            .chain(AnimFormat::ALL.map(Target::Anim));
        let still = StillFormat::ALL.map(Target::Still);
        let sound = AudioFormat::ALL.map(Target::Audio);
        match kind {
            MediaKind::Video => moving.chain(still).chain(sound).collect(),
            MediaKind::Animation => moving.chain(still).collect(),
            MediaKind::Image => StillFormat::ALL.map(Target::Still).to_vec(),
            MediaKind::Audio => sound.to_vec(),
        }
    }
}

pub const PNG_COLORS: [u16; 5] = [256, 128, 64, 32, 16];

pub const VIDEO_FPS: [u32; 4] = [60, 30, 24, 15];

#[derive(Debug, Clone, PartialEq)]
pub struct ConvertSettings {
    pub quality: u8,
    pub effort: Effort,
    pub max_height: Option<u32>,
    pub max_fps: Option<u32>,
    pub remove_audio: bool,
    pub anim: AnimSettings,
    pub audio_kbps: u32,
    pub mono: bool,
    pub png_colors: Option<u16>,
    pub webp_lossless: bool,
}

impl Default for ConvertSettings {
    fn default() -> Self {
        Self {
            quality: 100,
            effort: Effort::Balanced,
            max_height: None,
            max_fps: None,
            remove_audio: false,
            anim: AnimSettings::default(),
            audio_kbps: DEFAULT_MUSIC_AUDIO_KBPS,
            mono: false,
            png_colors: None,
            webp_lossless: false,
        }
    }
}

pub fn output_path(
    input: &SourcePath,
    target: Target,
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

fn video_output(out: Output, format: VideoFormat, s: &ConvertSettings, audio: bool) -> Output {
    let out = out.format(format.muxer()).stream_map("[v]");
    let out = match s.max_fps {
        Some(fps) => out.max_fps(fps as i32, 1),
        None => out,
    };
    let out = match format {
        VideoFormat::Mp4 | VideoFormat::Mov | VideoFormat::Mkv => out
            .video_codec("libx264")
            .video_opt("preset", s.effort.x264_preset())
            .video_opt("crf", filters::crf_for_quality(s.quality).to_string()),
        VideoFormat::Webm => out
            .video_codec("libvpx-vp9")
            .video_opt("crf", filters::vp9_crf(s.quality).to_string())
            .video_opt("b", "0")
            .video_opt("deadline", "good")
            .video_opt("cpu-used", s.effort.vp9_cpu_used())
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

fn still_output(out: Output, format: StillFormat, s: &ConvertSettings) -> Output {
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
            .video_opt("compression_level", s.effort.webp_level().to_string()),
        StillFormat::Bmp => out.video_codec("bmp"),
        StillFormat::Tiff => out
            .video_codec("tiff")
            .video_opt("compression_algo", "deflate"),
    }
}

fn still_pix_fmt(format: StillFormat, s: &ConvertSettings) -> &'static str {
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
    target: Target,
    s: &ConvertSettings,
    info: &MediaInfo,
) -> anyhow::Result<JobContext> {
    let builder = match input {
        InputSource::File(input) => JobBuilder::new().input(*input),
        InputSource::Push(source) => JobBuilder::new().push_input(source),
    };
    let out = Output::new(output);
    let ctx = match target {
        Target::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
        Target::Video(format) => {
            let audio = info.has_audio() && !s.remove_audio;
            builder
                .filter_desc(&filters::convert_video_graph(s.max_height))
                .output(video_output(out, format, s, audio))
                .build()?
        }
        Target::Still(StillFormat::Png) if s.png_colors.is_some() => builder
            .filter_desc(&filters::palette_graph(
                s.max_height,
                s.png_colors.unwrap_or(256),
            ))
            .output(still_output(out, StillFormat::Png, s))
            .build()?,
        Target::Still(format) => builder
            .filter_desc(&filters::still_graph(
                s.max_height,
                still_pix_fmt(format, s),
            ))
            .output(still_output(out, format, s))
            .build()?,
        Target::Audio(format) => {
            let out = audio_output(out, "0:a", format, s.audio_kbps);
            let out = if format == AudioFormat::Flac {
                out.audio_opt("compression_level", s.effort.flac_level())
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
    let mut opts = oxipng::Options::from_preset(effort.png_preset());
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
    target: Target,
    settings: &ConvertSettings,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let pending = AtomicFile::new(output)?;
    convert_part(
        input,
        pending.path(),
        target,
        settings,
        None,
        on_progress,
        cancel,
    )?;
    pending.commit()?;
    Ok(())
}

fn convert_part(
    input: &Path,
    output: &Path,
    target: Target,
    settings: &ConvertSettings,
    window: Option<(i64, i64)>,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if let Target::Anim(format) = target {
        let result = animation_backends::encode(
            &AnimationEncodeRequest {
                input,
                output,
                window,
                format,
                settings: &settings.anim,
                effort: settings.effort,
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
        mediaa_webp::frame_stream(input).and_then(|stream| {
            let info = crate::info::from_webp(stream.info);
            let ctx = build_context(
                InputSource::Push(stream.source),
                output,
                target,
                settings,
                &info,
            )?;
            match target {
                Target::Video(_) | Target::Audio(_) => {
                    run_tracked(ctx, info.duration_us, on_progress, cancel)?
                }
                Target::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
                Target::Still(_) => job::run(ctx, None, &mut |_| {}, cancel)?,
            }
            stream.producer.wait()
        })
    } else {
        probe(input).and_then(|info| {
            match target {
                Target::Audio(_) => {
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
                settings,
                &info,
            )?;
            match target {
                Target::Video(_) | Target::Audio(_) => run_tracked(ctx, total, on_progress, cancel),
                Target::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
                Target::Still(_) => job::run(ctx, None, &mut |_| {}, cancel),
            }
        })
    };
    let result = encoded.and_then(|()| match target {
        Target::Still(StillFormat::Png) => optimize_png(output, settings.effort),
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
    target: Target,
    settings: &ConvertSettings,
    cancel: &AtomicBool,
) -> anyhow::Result<u64> {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join(format!("estimate.{}", target.extension()));
    let animated_webp = formats::is_webp_path(input) && formats::webp_is_animated(input)?;
    let duration = if animated_webp || matches!(target, Target::Still(_)) {
        None
    } else {
        probe(input)?.duration_us
    };
    let window = duration
        .filter(|d| *d > 3 * SAMPLE_US)
        .map(|d| ((d - SAMPLE_US) / 3, SAMPLE_US));
    convert_part(
        input,
        &output,
        target,
        settings,
        window,
        &mut |_| {},
        cancel,
    )?;
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
    let total = mediaa_ffmpeg::probe::duration_us(&tracks[0]);
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
    let total = mediaa_ffmpeg::probe::duration_us(video);
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

    #[test]
    fn targets_by_kind() {
        let defaults = ConvertSettings::default();
        assert_eq!((defaults.quality, defaults.audio_kbps), (100, 256));
        let video = Target::for_kind(MediaKind::Video);
        assert_eq!(video.len(), 17);
        assert!(
            video.contains(&Target::Video(VideoFormat::Webm))
                && video.contains(&Target::Anim(AnimFormat::Gif))
        );
        assert!(
            video.contains(&Target::Audio(AudioFormat::Mp3)),
            "sound out of a video"
        );
        assert_eq!(
            Target::for_kind(MediaKind::Animation),
            video[..12],
            "animations: no sound"
        );
        assert!(
            video.contains(&Target::Still(StillFormat::Png)),
            "first frame as an image"
        );
        assert_eq!(
            Target::for_kind(MediaKind::Audio),
            AudioFormat::ALL.map(Target::Audio).to_vec()
        );
        let images = Target::for_kind(MediaKind::Image);
        assert_eq!(images, StillFormat::ALL.map(Target::Still).to_vec());
        assert!(StillFormat::Jpeg.has_quality() && !StillFormat::Png.has_quality());
        assert_eq!(VideoFormat::Mkv.muxer(), "matroska");
        assert_eq!(Target::Still(StillFormat::Jpeg).extension(), "jpg");
        for kind in [
            MediaKind::Image,
            MediaKind::Video,
            MediaKind::Animation,
            MediaKind::Audio,
        ] {
            for target in Target::for_kind(kind) {
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
            title(clip, Target::Anim(AnimFormat::Gif), None),
            ("My clip".into(), "gif".into())
        );
        assert_eq!(
            title(clip, Target::Video(VideoFormat::Webm), Some("  Short ")),
            ("Short".into(), "webm".into())
        );
        assert_eq!(
            title(clip, Target::Video(VideoFormat::Mp4), Some("")),
            ("My clip".into(), "mp4".into()),
            "empty name: default"
        );
        assert_eq!(
            title("/x/holiday.MOV", Target::Video(VideoFormat::Mp4), None).0,
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
            Target::Video(VideoFormat::Mp4),
            &ConvertSettings::default(),
            &mut |_| {},
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(!error.to_string().is_empty());
        assert_eq!(std::fs::read(output).unwrap(), b"keep me");
    }
}
