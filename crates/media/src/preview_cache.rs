use crate::info::probe;
use crate::job;
use ez_ffmpeg::core::context::output::VSyncMethod;
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

const PREVIEW_MAX_HEIGHT: u32 = 1080;
const PREVIEW_MAX_FPS: u32 = 30;
static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

pub fn proxy_path(source: &Path) -> PathBuf {
    let metadata = std::fs::metadata(source).ok();
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in std::fs::canonicalize(source)
        .unwrap_or_else(|_| source.to_path_buf())
        .to_string_lossy()
        .bytes()
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    for byte in metadata
        .as_ref()
        .map(|m| m.len())
        .unwrap_or(0)
        .to_le_bytes()
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    for byte in metadata
        .and_then(|m| m.modified().ok())
        .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos())
        .unwrap_or(0)
        .to_le_bytes()
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    source
        .parent()
        .unwrap_or(Path::new("."))
        .join(".cache")
        .join("previews")
        .join(format!("{hash:016x}.mp4"))
}

pub fn ready_proxy(source: &Path) -> Option<PathBuf> {
    let path = proxy_path(source);
    path.is_file().then_some(path)
}

pub fn ensure_proxy(source: &Path, cancel: &AtomicBool) -> anyhow::Result<PathBuf> {
    ensure_proxy_for(source, source, cancel)
}

pub fn ensure_proxy_for(
    source: &Path,
    media: &Path,
    cancel: &AtomicBool,
) -> anyhow::Result<PathBuf> {
    let target = proxy_path(media);
    if target.is_file() {
        return Ok(target);
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = target.with_extension(format!(
        "{}-{}.tmp.mp4",
        std::process::id(),
        NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
    ));
    let result = if mediaa_core::formats::is_webp_path(source) {
        encode_webp(source, &temp, cancel)
    } else {
        encode_video(source, &temp, cancel)
    };
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temp);
        return Err(error);
    }
    if target.exists() {
        let _ = std::fs::remove_file(&temp);
        return Ok(target);
    }
    std::fs::rename(&temp, &target)?;
    Ok(target)
}

fn encode_video(source: &Path, target: &Path, cancel: &AtomicBool) -> anyhow::Result<()> {
    let info = probe(source)?;
    let fps = if info.fps > 0.0 {
        info.fps.min(f64::from(PREVIEW_MAX_FPS)).round().max(1.0) as u32
    } else {
        PREVIEW_MAX_FPS
    };
    let graph = preview_graph(fps);
    let output = preview_output(target);
    let context = FfmpegContext::builder()
        .input(Input::from(source.to_string_lossy().into_owned()))
        .filter_desc(graph)
        .output(output)
        .build()?;
    job::run(context, info.duration_us, &mut |_| {}, cancel)
}

fn encode_webp(source: &Path, target: &Path, cancel: &AtomicBool) -> anyhow::Result<()> {
    let mut decoder =
        image_webp::WebPDecoder::new(std::io::BufReader::new(std::fs::File::open(source)?))?;
    let (width, height) = decoder.dimensions();
    let alpha = decoder.has_alpha();
    let delay = decoder.loop_duration();
    let fps = if delay == 0 {
        PREVIEW_MAX_FPS
    } else {
        (decoder.num_frames() * 1000 / delay as u32).clamp(1, PREVIEW_MAX_FPS)
    };
    let (source, mut frames) = FramePushSource::builder(width, height)
        .pixel_format(if alpha { "rgba" } else { "rgb24" })
        .time_base(1, 1000)
        .build()?;
    let context = FfmpegContext::builder()
        .input(source)
        .filter_desc(preview_graph(fps))
        .output(preview_output(target))
        .build()?;
    let scheduler = context.start()?;
    let mut buffer = vec![
        0u8;
        decoder
            .output_buffer_size()
            .ok_or_else(|| anyhow::anyhow!("WebP is too large"))?
    ];
    let mut pts = 0i64;
    let animated = decoder.is_animated();
    for _ in 0..decoder.num_frames().max(1) {
        if cancel.load(Ordering::Relaxed) {
            scheduler.abort();
            return Err(job::Cancelled.into());
        }
        let duration = if animated {
            decoder.read_frame(&mut buffer)?.max(1)
        } else {
            decoder.read_image(&mut buffer)?;
            1_000
        };
        if frames
            .push(&buffer, pts, Some(i64::from(duration)))
            .is_err()
        {
            break;
        }
        pts += i64::from(duration);
    }
    frames.finish();
    scheduler.wait().map_err(Into::into)
}

fn preview_graph(fps: u32) -> String {
    format!(
        "[0:v]fps={fps},scale=w=-2:h='min({PREVIEW_MAX_HEIGHT},ih)':flags=fast_bilinear,format=yuv420p[v]"
    )
}

fn preview_output(target: &Path) -> Output {
    Output::from(target.to_string_lossy().into_owned())
        .set_format("mp4")
        .set_format_opt("movflags", "+faststart")
        .set_video_codec("libx264")
        .set_video_codec_opt("preset", "ultrafast")
        .set_video_codec_opt("crf", "26")
        .set_vsync_method(VSyncMethod::VsyncCfr)
        .add_stream_map("[v]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_path_changes_with_the_source_fingerprint() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.webp");
        std::fs::write(&source, b"first").unwrap();
        let first = proxy_path(&source);
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(&source, b"second").unwrap();
        let second = proxy_path(&source);
        assert_ne!(first, second);
        assert!(first.starts_with(dir.path().join(".cache").join("previews")));
    }

    #[test]
    fn preview_contract_is_video_with_a_rate_cap() {
        assert_eq!(
            preview_graph(30),
            "[0:v]fps=30,scale=w=-2:h='min(1080,ih)':flags=fast_bilinear,format=yuv420p[v]"
        );
    }
}
