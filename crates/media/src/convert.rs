//! Conversions between popular formats (video ⇄ video, video ⇄ animation, image → image) with
//! quality / size settings and optional audio removal, plus muxing of recordings.

use crate::filters;
use crate::info::{probe, MediaInfo};
use crate::job;
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use mediaa_core::formats::{self, AnimFormat, AudioFormat, MediaKind, VideoContainer};
use mediaa_core::naming;
use mediaa_core::settings::AnimSettings;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::thread::JoinHandle;

/// Video files people use most: MP4 (everywhere), MOV (Apple), MKV, WebM (web), AVI (legacy).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VideoFormat {
    Mp4,
    Mov,
    Mkv,
    Webm,
    Avi,
}

impl VideoFormat {
    pub const ALL: [VideoFormat; 5] = [VideoFormat::Mp4, VideoFormat::Mov, VideoFormat::Mkv, VideoFormat::Webm, VideoFormat::Avi];

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

/// Still image formats: JPEG and PNG (everywhere), WebP (web), BMP and TIFF.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StillFormat {
    Jpeg,
    Png,
    Webp,
    Bmp,
    Tiff,
}

impl StillFormat {
    pub const ALL: [StillFormat; 5] = [StillFormat::Jpeg, StillFormat::Png, StillFormat::Webp, StillFormat::Bmp, StillFormat::Tiff];

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

    /// Lossy formats take a quality setting.
    pub fn has_quality(self) -> bool {
        matches!(self, StillFormat::Jpeg | StillFormat::Webp)
    }
}

/// What a file is converted into.
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

    /// Targets for a file of `kind`: videos and animations become videos or animations (the
    /// same format too — to compress, resize or drop the sound) or an image of their first
    /// frame, videos also sound only; images become images, audio becomes audio.
    pub fn for_kind(kind: MediaKind) -> Vec<Target> {
        let moving = VideoFormat::ALL.map(Target::Video).into_iter().chain(AnimFormat::ALL.map(Target::Anim));
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

/// How hard the encoder works: a faster conversion or a smaller file at the same quality.
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

    /// libwebp `compression_level` (0–6).
    fn webp_level(self) -> &'static str {
        match self {
            Effort::Fast => "2",
            Effort::Balanced => "4",
            Effort::Max => "6",
        }
    }

    /// FLAC `compression_level` (0–12).
    fn flac_level(self) -> &'static str {
        match self {
            Effort::Fast => "2",
            Effort::Balanced => "5",
            Effort::Max => "8",
        }
    }

    /// oxipng preset (0–6).
    fn png_preset(self) -> u8 {
        match self {
            Effort::Fast => 1,
            Effort::Balanced => 3,
            Effort::Max => 6,
        }
    }
}

/// Palette sizes offered for PNG (fewer colours = smaller file; lossy).
pub const PNG_COLORS: [u16; 5] = [256, 128, 64, 32, 16];

/// Frame-rate caps offered for video.
pub const VIDEO_FPS: [u32; 4] = [60, 30, 24, 15];

/// How the output is made.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertSettings {
    /// 1–100: video / JPEG / WebP image quality (lower = smaller file).
    pub quality: u8,
    /// Encoder effort: video, WebP, PNG, FLAC.
    pub effort: Effort,
    /// Downscale to this height (aspect kept, never upscaled); videos and images.
    pub max_height: Option<u32>,
    /// Video targets: frame-rate cap (never raised).
    pub max_fps: Option<u32>,
    /// Video targets: leave the sound out.
    pub remove_audio: bool,
    /// Animation targets.
    pub anim: AnimSettings,
    /// Sound bitrate in kbit/s: lossy audio targets and the sound of videos.
    pub audio_kbps: u32,
    /// Mix the sound down to one channel (about half the size of lossless sound).
    pub mono: bool,
    /// PNG: reduce to a palette of this many colours (lossy); `None` keeps every colour.
    pub png_colors: Option<u16>,
    /// WebP image: lossless instead of `quality`.
    pub webp_lossless: bool,
}

impl Default for ConvertSettings {
    fn default() -> Self {
        Self {
            quality: 80,
            effort: Effort::Balanced,
            max_height: None,
            max_fps: None,
            remove_audio: false,
            anim: AnimSettings::default(),
            audio_kbps: 192,
            mono: false,
            png_colors: None,
            webp_lossless: false,
        }
    }
}

/// Output path in `out_dir`: titled `title`, or after the input (its MediaA title or file
/// name), stamped with the current time (see `naming`).
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

fn webp_output(output: Output, quality: u8, effort: Effort) -> Output {
    output
        .set_format("webp")
        .set_video_codec("libwebp_anim")
        .set_video_codec_opt("lossless", "0")
        .set_video_codec_opt("quality", quality.min(100).to_string())
        .set_video_codec_opt("compression_level", effort.webp_level())
        .set_format_opt("loop", "0")
}

/// Sound bitrate argument, clamped to what the encoders accept.
fn bitrate(kbps: u32) -> String {
    format!("{}k", kbps.clamp(32, 320))
}

/// Video encoder and container settings for `format` (`[v]` = the filtered picture).
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
            let out = out.set_video_codec("mpeg4").set_video_qscale(filters::qscale(s.quality));
            match s.effort {
                Effort::Fast => out,
                Effort::Balanced => out.set_video_codec_opt("mbd", "rd"),
                Effort::Max => out.set_video_codec_opt("mbd", "rd").set_video_codec_opt("trellis", "1"),
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
    let out = out.add_stream_map("0:a").set_audio_bitrate(bitrate(s.audio_kbps));
    let out = if s.mono { out.set_audio_channels(1) } else { out };
    match format {
        VideoFormat::Mp4 | VideoFormat::Mov | VideoFormat::Mkv => out.set_audio_codec("aac"),
        VideoFormat::Webm => out.set_audio_codec("libopus"),
        VideoFormat::Avi => out.set_audio_codec("libmp3lame"),
    }
}

/// Sound-only output (`map`) in `format`; `kbps` for the lossy ones.
pub(crate) fn audio_output(out: Output, map: &str, format: AudioFormat, kbps: u32) -> Output {
    let out = out.add_stream_map(map.to_string());
    match format {
        AudioFormat::Mp3 => out.set_format("mp3").set_audio_codec("libmp3lame").set_audio_bitrate(bitrate(kbps)),
        AudioFormat::M4a => out.set_format("ipod").set_audio_codec("aac").set_audio_bitrate(bitrate(kbps)).set_format_opt("movflags", "+faststart"),
        AudioFormat::Wav => out.set_format("wav").set_audio_codec("pcm_s16le"),
        AudioFormat::Flac => out.set_format("flac").set_audio_codec("flac"),
        AudioFormat::Ogg => out.set_format("ogg").set_audio_codec("libopus").set_audio_bitrate(bitrate(kbps)),
    }
}

/// Still image encoder settings for `format` (`[v]` = the filtered picture, one frame).
fn still_output(out: Output, format: StillFormat, s: &ConvertSettings) -> Output {
    let out = out.set_format("image2").set_format_opt("update", "1").add_stream_map("[v]").set_max_video_frames(1);
    match format {
        StillFormat::Jpeg => out.set_video_codec("mjpeg").set_video_qscale(filters::qscale(s.quality)),
        // Fast here: oxipng recompresses it afterwards.
        StillFormat::Png => out.set_video_codec("png").set_video_codec_opt("compression_level", "1"),
        StillFormat::Webp => out
            .set_video_codec("libwebp")
            .set_video_codec_opt("lossless", if s.webp_lossless { "1" } else { "0" })
            .set_video_codec_opt("quality", if s.webp_lossless { "100".to_string() } else { s.quality.min(100).to_string() })
            .set_video_codec_opt("compression_level", s.effort.webp_level()),
        StillFormat::Bmp => out.set_video_codec("bmp"),
        StillFormat::Tiff => out.set_video_codec("tiff").set_video_codec_opt("compression_algo", "deflate"),
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

fn build_context(input: Input, output: &Path, target: Target, s: &ConvertSettings, info: &MediaInfo) -> anyhow::Result<FfmpegContext> {
    let builder = FfmpegContext::builder().input(input);
    let out = Output::from(url(output));
    let ctx = match target {
        Target::Anim(AnimFormat::Gif) => builder
            .filter_desc(filters::gif_graph(&s.anim))
            .output(out.set_format("gif").add_stream_map("[v]").set_format_opt("loop", "0"))
            .build()?,
        Target::Anim(AnimFormat::Webp) => builder
            .filter_desc(filters::webp_graph(&s.anim))
            .output(webp_output(out.add_stream_map("[v]"), s.anim.webp_quality, s.effort))
            .build()?,
        Target::Video(format) => {
            let audio = info.has_audio() && !s.remove_audio;
            builder
                .filter_desc(filters::convert_video_graph(s.max_height))
                .output(video_output(out, format, s, audio))
                .build()?
        }
        Target::Still(StillFormat::Png) if s.png_colors.is_some() => builder
            .filter_desc(filters::palette_graph(s.max_height, s.png_colors.unwrap_or(256)))
            .output(still_output(out, StillFormat::Png, s))
            .build()?,
        Target::Still(format) => builder
            .filter_desc(filters::still_graph(s.max_height, still_pix_fmt(format, s)))
            .output(still_output(out, format, s))
            .build()?,
        Target::Audio(format) => {
            let out = audio_output(out, "0:a", format, s.audio_kbps);
            let out = if format == AudioFormat::Flac { out.set_audio_codec_opt("compression_level", s.effort.flac_level()) } else { out };
            builder.output(if s.mono { out.set_audio_channels(1) } else { out }).build()?
        }
    };
    Ok(ctx)
}

/// Lossless PNG recompression with oxipng (typically 10–40 % smaller than FFmpeg's output).
fn optimize_png(path: &Path, effort: Effort) -> anyhow::Result<()> {
    let mut opts = oxipng::Options::from_preset(effort.png_preset());
    opts.strip = oxipng::StripChunks::Safe;
    oxipng::optimize(&oxipng::InFile::Path(path.to_path_buf()), &oxipng::OutFile::from_path(path.to_path_buf()), &opts)
        .map_err(|e| anyhow::anyhow!("png optimisation: {e}"))?;
    Ok(())
}

/// Converts `input` into `output`. A partial output is removed on failure or cancellation.
pub fn convert(
    input: &Path,
    output: &Path,
    target: Target,
    settings: &ConvertSettings,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    convert_part(input, output, target, settings, None, on_progress, cancel)
}

/// Converts `input` (or, for `window = Some((start, length))` in µs, just that part of it).
fn convert_part(
    input: &Path,
    output: &Path,
    target: Target,
    settings: &ConvertSettings,
    window: Option<(i64, i64)>,
    on_progress: &mut dyn FnMut(f32),
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let animated_webp = formats::is_webp_path(input) && formats::webp_is_animated(input)?;
    let encoded = if animated_webp {
        // FFmpeg's webp decoder handles still images only; decode frames in Rust instead.
        webp_frames_input(input).and_then(|(source, info, producer)| {
            let r = build_context(source.into(), output, target, settings, &info)
                .and_then(|ctx| job::run(ctx, info.duration_us, on_progress, cancel));
            let produced = producer.join().map_err(|_| anyhow::anyhow!("webp decoder panicked"))?;
            // A producer error is the root cause when the encode failed too.
            r.and(produced)
        })
    } else {
        probe(input).and_then(|info| {
            match target {
                Target::Audio(_) => anyhow::ensure!(info.has_audio(), "{} has no sound", input.display()),
                _ => anyhow::ensure!(info.has_video(), "{} has no picture", input.display()),
            }
            let (source, total) = match window {
                Some((start, len)) => (Input::from(url(input)).set_start_time_us(start).set_recording_time_us(len), Some(len)),
                None => (Input::from(url(input)), info.duration_us),
            };
            let ctx = build_context(source, output, target, settings, &info)?;
            job::run(ctx, total, on_progress, cancel)
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

/// Length of the trial piece [`estimate_size`] encodes out of longer files.
const SAMPLE_US: i64 = 5_000_000;

/// Approximate output size in bytes: images (and short or seek-less files) are converted
/// whole into a temporary file; longer files only a [`SAMPLE_US`] piece from the first third,
/// scaled up by duration.
pub fn estimate_size(input: &Path, target: Target, settings: &ConvertSettings, cancel: &AtomicBool) -> anyhow::Result<u64> {
    let dir = tempfile::tempdir()?;
    let output = dir.path().join(format!("estimate.{}", target.extension()));
    let animated_webp = formats::is_webp_path(input) && formats::webp_is_animated(input)?;
    let duration = if animated_webp || matches!(target, Target::Still(_)) { None } else { probe(input)?.duration_us };
    let window = duration.filter(|d| *d > 3 * SAMPLE_US).map(|d| ((d - SAMPLE_US) / 3, SAMPLE_US));
    convert_part(input, &output, target, settings, window, &mut |_| {}, cancel)?;
    let size = std::fs::metadata(&output)?.len();
    Ok(match (window, duration) {
        (Some((_, len)), Some(total)) => (size as f64 * total as f64 / len as f64) as u64,
        _ => size,
    })
}

/// Decodes an animated WebP with `image-webp` and feeds its frames (with their own delays)
/// into FFmpeg through a [`FramePushSource`].
fn webp_frames_input(
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
                    break; // job stopped (cancelled or failed); its error is reported
                }
                pts += delay;
            }
            handle.finish();
            Ok(())
        })?;
    Ok((source, info, producer))
}

/// Mixes the WAV tracks of an audio recording into one file of `format`.
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

/// Combines a recorded video track with zero or more WAV tracks (mixed together) into the
/// final container. The video is stream-copied; audio is encoded to AAC.
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
        let video = Target::for_kind(MediaKind::Video);
        assert_eq!(video.len(), 17);
        assert!(video.contains(&Target::Video(VideoFormat::Webm)) && video.contains(&Target::Anim(AnimFormat::Gif)));
        assert!(video.contains(&Target::Audio(AudioFormat::Mp3)), "sound out of a video");
        assert_eq!(Target::for_kind(MediaKind::Animation), video[..12], "animations: no sound");
        assert!(video.contains(&Target::Still(StillFormat::Png)), "first frame as an image");
        assert_eq!(Target::for_kind(MediaKind::Audio), AudioFormat::ALL.map(Target::Audio).to_vec());
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
            (naming::parse_path(&p).unwrap().title, p.extension().unwrap().to_string_lossy().into_owned())
        };
        let clip = "/x/My clip_2026-10-13_09-05-07.mp4";
        assert_eq!(title(clip, Target::Anim(AnimFormat::Gif), None), ("My clip".into(), "gif".into()));
        assert_eq!(title(clip, Target::Video(VideoFormat::Webm), Some("  Short ")), ("Short".into(), "webm".into()));
        assert_eq!(title(clip, Target::Video(VideoFormat::Mp4), Some("")), ("My clip".into(), "mp4".into()), "empty name: default");
        assert_eq!(title("/x/holiday.MOV", Target::Video(VideoFormat::Mp4), None).0, "holiday", "files from elsewhere: their stem");
    }
}
