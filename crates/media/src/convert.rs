use crate::animation_backends::{self, AnimationEffort, AnimationEncodeRequest, AnimationOrigin};
use crate::filters;
use crate::info::{probe, MediaInfo};
use crate::job;
use ez_ffmpeg::{FfmpegContext, Input, Output};
use mediaa_core::formats::{self, AnimFormat, AudioFormat, MediaKind, VideoContainer};
use mediaa_core::naming;
use mediaa_core::settings::AnimSettings;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Effort {
    Fast,
    #[default]
    Balanced,
    Max,
}

impl Effort {
    pub const ALL: [Effort; 3] = [Effort::Fast, Effort::Balanced, Effort::Max];

    fn x264_preset(self) -> &'static str {
        match self {
            Effort::Fast => "veryfast",
            Effort::Balanced => "medium",
            Effort::Max => "veryslow",
        }
    }

    fn vp9_cpu_used(self) -> &'static str {
        match self {
            Effort::Fast => "6",
            Effort::Balanced => "4",
            Effort::Max => "1",
        }
    }

    fn webp_level(self) -> &'static str {
        match self {
            Effort::Fast => "2",
            Effort::Balanced => "4",
            Effort::Max => "6",
        }
    }

    fn animation_effort(self) -> AnimationEffort {
        match self {
            Self::Fast => AnimationEffort::Fast,
            Self::Balanced => AnimationEffort::Balanced,
            Self::Max => AnimationEffort::Max,
        }
    }

    fn flac_level(self) -> &'static str {
        match self {
            Effort::Fast => "2",
            Effort::Balanced => "5",
            Effort::Max => "8",
        }
    }

    fn png_preset(self) -> u8 {
        match self {
            Effort::Fast => 1,
            Effort::Balanced => 3,
            Effort::Max => 6,
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
            audio_kbps: 256,
            mono: false,
            png_colors: None,
            webp_lossless: false,
        }
    }
}

pub fn output_path(input: &Path, target: Target, title: Option<&str>, out_dir: &Path) -> PathBuf {
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .or_else(|| naming::parse_path(input).map(|n| n.title))
        .or_else(|| input.file_stem().map(|s| s.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "output".into());
    naming::media_path(out_dir, &title, chrono::Local::now(), target.extension())
}

fn url(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

fn run_tracked(
    ctx: FfmpegContext,
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
    format!("{}k", kbps.clamp(32, 320))
}

fn video_output(out: Output, format: VideoFormat, s: &ConvertSettings, audio: bool) -> Output {
    let out = out.set_format(format.muxer()).add_stream_map("[v]");
    let out = match s.max_fps {
        Some(fps) => out.set_framerate_max(fps as i32, 1),
        None => out,
    };
    let out = match format {
        VideoFormat::Mp4 | VideoFormat::Mov | VideoFormat::Mkv => out
            .set_video_codec("libx264")
            .set_video_codec_opt("preset", s.effort.x264_preset())
            .set_video_codec_opt("crf", filters::crf_for_quality(s.quality).to_string()),
        VideoFormat::Webm => out
            .set_video_codec("libvpx-vp9")
            .set_video_codec_opt("crf", filters::vp9_crf(s.quality).to_string())
            .set_video_codec_opt("b", "0")
            .set_video_codec_opt("deadline", "good")
            .set_video_codec_opt("cpu-used", s.effort.vp9_cpu_used())
            .set_video_codec_opt("row-mt", "1"),
        VideoFormat::Avi => {
            let out = out
                .set_video_codec("mpeg4")
                .set_video_qscale(filters::qscale(s.quality));
            match s.effort {
                Effort::Fast => out,
                Effort::Balanced => out.set_video_codec_opt("mbd", "rd"),
                Effort::Max => out
                    .set_video_codec_opt("mbd", "rd")
                    .set_video_codec_opt("trellis", "1"),
            }
        }
    };
    let out = match format {
        VideoFormat::Mp4 | VideoFormat::Mov => out.set_format_opt("movflags", "+faststart"),
        _ => out,
    };
    if !audio {
        return out;
    }
    let out = out
        .add_stream_map("0:a")
        .set_audio_bitrate(bitrate(s.audio_kbps));
    let out = if s.mono {
        out.set_audio_channels(1)
    } else {
        out
    };
    match format {
        VideoFormat::Mp4 | VideoFormat::Mov | VideoFormat::Mkv => out.set_audio_codec("aac"),
        VideoFormat::Webm => out.set_audio_codec("libopus"),
        VideoFormat::Avi => out.set_audio_codec("libmp3lame"),
    }
}

pub(crate) fn audio_output(out: Output, map: &str, format: AudioFormat, kbps: u32) -> Output {
    let out = out.add_stream_map(map.to_string());
    match format {
        AudioFormat::Mp3 => out
            .set_format("mp3")
            .set_audio_codec("libmp3lame")
            .set_audio_bitrate(bitrate(kbps)),
        AudioFormat::M4a => out
            .set_format("ipod")
            .set_audio_codec("aac")
            .set_audio_bitrate(bitrate(kbps))
            .set_format_opt("movflags", "+faststart"),
        AudioFormat::Wav => out.set_format("wav").set_audio_codec("pcm_s16le"),
        AudioFormat::Flac => out.set_format("flac").set_audio_codec("flac"),
        AudioFormat::Ogg => out
            .set_format("ogg")
            .set_audio_codec("libopus")
            .set_audio_bitrate(bitrate(kbps)),
    }
}

fn still_output(out: Output, format: StillFormat, s: &ConvertSettings) -> Output {
    let out = out
        .set_format("image2")
        .set_format_opt("update", "1")
        .add_stream_map("[v]")
        .set_max_video_frames(1);
    match format {
        StillFormat::Jpeg => out
            .set_video_codec("mjpeg")
            .set_video_qscale(filters::qscale(s.quality)),
        StillFormat::Png => out
            .set_video_codec("png")
            .set_video_codec_opt("compression_level", "1"),
        StillFormat::Webp => out
            .set_video_codec("libwebp")
            .set_video_codec_opt("lossless", if s.webp_lossless { "1" } else { "0" })
            .set_video_codec_opt(
                "quality",
                if s.webp_lossless {
                    "100".to_string()
                } else {
                    s.quality.min(100).to_string()
                },
            )
            .set_video_codec_opt("compression_level", s.effort.webp_level()),
        StillFormat::Bmp => out.set_video_codec("bmp"),
        StillFormat::Tiff => out
            .set_video_codec("tiff")
            .set_video_codec_opt("compression_algo", "deflate"),
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

fn build_context(
    input: Input,
    output: &Path,
    target: Target,
    s: &ConvertSettings,
    info: &MediaInfo,
) -> anyhow::Result<FfmpegContext> {
    let builder = FfmpegContext::builder().input(input);
    let out = Output::from(url(output));
    let ctx = match target {
        Target::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
        Target::Video(format) => {
            let audio = info.has_audio() && !s.remove_audio;
            builder
                .filter_desc(filters::convert_video_graph(s.max_height))
                .output(video_output(out, format, s, audio))
                .build()?
        }
        Target::Still(StillFormat::Png) if s.png_colors.is_some() => builder
            .filter_desc(filters::palette_graph(
                s.max_height,
                s.png_colors.unwrap_or(256),
            ))
            .output(still_output(out, StillFormat::Png, s))
            .build()?,
        Target::Still(format) => builder
            .filter_desc(filters::still_graph(s.max_height, still_pix_fmt(format, s)))
            .output(still_output(out, format, s))
            .build()?,
        Target::Audio(format) => {
            let out = audio_output(out, "0:a", format, s.audio_kbps);
            let out = if format == AudioFormat::Flac {
                out.set_audio_codec_opt("compression_level", s.effort.flac_level())
            } else {
                out
            };
            builder
                .output(if s.mono {
                    out.set_audio_channels(1)
                } else {
                    out
                })
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
    input: &Path,
    output: &Path,
    target: Target,
    settings: &ConvertSettings,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    convert_part(input, output, target, settings, None, on_progress, cancel)
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
                effort: settings.effort.animation_effort(),
                origin: AnimationOrigin::Conversion,
                segmented_webp: false,
                scheduling: Default::default(),
                threads: None,
                trace_id: None,
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
        animation_backends::webp_frames_input(input).and_then(|(source, info, producer)| {
            let r = build_context(source.into(), output, target, settings, &info).and_then(|ctx| {
                match target {
                    Target::Video(_) | Target::Audio(_) => {
                        run_tracked(ctx, info.duration_us, on_progress, cancel)
                    }
                    Target::Anim(_) => unreachable!("animation targets use AnimationEncoder"),
                    Target::Still(_) => job::run(ctx, None, &mut |_| {}, cancel),
                }
            });
            let produced = producer
                .join()
                .map_err(|_| anyhow::anyhow!("webp decoder panicked"))?;
            r.and(produced)
        })
    } else {
        probe(input).and_then(|info| {
            match target {
                Target::Audio(_) => {
                    anyhow::ensure!(info.has_audio(), "{} has no sound", input.display())
                }
                _ => anyhow::ensure!(info.has_video(), "{} has no picture", input.display()),
            }
            let (start, total) = match window {
                Some((start, len)) => (start, Some(len)),
                None => (0, info.duration_us),
            };
            let mut source = Input::from(url(input));
            if start > 0 {
                source = source.set_start_time_us(start);
            }
            if let Some((_, len)) = window {
                source = source.set_recording_time_us(len);
            }
            let ctx = build_context(source, output, target, settings, &info)?;
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
    input: &Path,
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

pub fn encode_audio_recording(
    tracks: &[PathBuf],
    output: &Path,
    format: AudioFormat,
    kbps: u32,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    anyhow::ensure!(!tracks.is_empty(), "no audio tracks");
    let mut builder = FfmpegContext::builder();
    for t in tracks {
        builder = builder.input(Input::from(url(t)));
    }
    let out = audio_output(Output::from(url(output)), "[a]", format, kbps);
    let total = ez_ffmpeg::container_info::get_duration_us(url(&tracks[0])).ok();
    let result = builder
        .filter_desc(filters::audio_only_mix_graph(tracks.len()))
        .output(out)
        .build()
        .map_err(anyhow::Error::from)
        .and_then(|ctx| job::run(ctx, total, on_progress, cancel));
    if result.is_err() {
        let _ = std::fs::remove_file(output);
    }
    result
}

pub fn mux_recording(
    video: &Path,
    audio_tracks: &[PathBuf],
    output: &Path,
    container: VideoContainer,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let mut builder = FfmpegContext::builder().input(Input::from(url(video)));
    for track in audio_tracks {
        builder = builder.input(Input::from(url(track)));
    }
    let mut out = Output::from(url(output))
        .set_format(container.muxer())
        .add_stream_map_with_copy("0:v");
    if container == VideoContainer::Mp4 {
        out = out.set_format_opt("movflags", "+faststart");
    }
    if let Some(graph) = filters::audio_mix_graph(audio_tracks.len()) {
        builder = builder.filter_desc(graph);
        out = out
            .add_stream_map("[a]")
            .set_audio_codec("aac")
            .set_audio_bitrate("192k");
    }
    let total = ez_ffmpeg::container_info::get_duration_us(url(video)).ok();
    let result = builder
        .output(out)
        .build()
        .map_err(anyhow::Error::from)
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
    }

    #[test]
    fn output_names() {
        let dir = tempfile::tempdir().unwrap();
        let title = |input: &str, target, name: Option<&str>| {
            let p = output_path(Path::new(input), target, name, dir.path());
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
}
