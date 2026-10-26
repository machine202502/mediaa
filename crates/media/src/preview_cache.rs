use crate::info::probe;
use crate::job;
use crate::webp_decode::WebpReader;
use ez_ffmpeg::core::context::output::VSyncMethod;
use ez_ffmpeg::{FfmpegContext, FramePushSource, Input, Output};
use mediaa_core::atomic_file::AtomicFile;
use mediaa_core::paths::CachePath;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::UNIX_EPOCH;

const PREVIEW_MAX_HEIGHT: u32 = 1080;
const PREVIEW_MAX_FPS: u32 = 30;

pub fn previews_dir(media_dir: &Path) -> std::path::PathBuf {
    media_dir.join(".cache").join("previews")
}

pub fn proxy_path(source: &Path) -> CachePath {
    let metadata = std::fs::metadata(source).ok();
    let mut hasher = mediaa_core::hash::StableHasher::default();
    hasher.write(
        std::fs::canonicalize(source)
            .unwrap_or_else(|_| source.to_path_buf())
            .to_string_lossy()
            .as_bytes(),
    );
    hasher.write(
        &metadata
            .as_ref()
            .map(|m| m.len())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    hasher.write(
        &metadata
            .and_then(|m| m.modified().ok())
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0)
            .to_le_bytes(),
    );
    let hash = hasher.finish();
    CachePath::new(
        previews_dir(source.parent().unwrap_or(Path::new("."))).join(format!("{hash:016x}.mp4")),
    )
}

pub fn ready_proxy(source: &Path) -> Option<CachePath> {
    let path = proxy_path(source);
    path.is_file().then_some(path)
}

pub fn ensure_proxy(source: &Path, cancel: &AtomicBool) -> anyhow::Result<CachePath> {
    ensure_proxy_for(source, source, cancel)
}

pub fn ensure_proxy_for(
    source: &Path,
    media: &Path,
    cancel: &AtomicBool,
) -> anyhow::Result<CachePath> {
    let target = proxy_path(media);
    if target.is_file() {
        return Ok(target);
    }
    let pending = AtomicFile::new(target.as_ref())?;
    let result = if mediaa_core::formats::is_webp_path(source) {
        encode_webp(source, pending.path(), cancel)
    } else {
        encode_video(source, pending.path(), cancel)
    };
    result?;
    match pending.commit() {
        Ok(path) => Ok(CachePath::new(path)),
        Err(error) if target.is_file() => {
            log::debug!("preview proxy already completed: {error:#}");
            Ok(target)
        }
        Err(error) => Err(error),
    }
}

fn encode_video(source: &Path, target: &Path, cancel: &AtomicBool) -> anyhow::Result<()> {
    let info = probe(source)?;
    let fps = if info.fps > 0.0 {
        info.fps.min(f64::from(PREVIEW_MAX_FPS)).round().max(1.0) as u32
    } else {
        PREVIEW_MAX_FPS
    };
    let graph = preview_graph(fps, PREVIEW_MAX_HEIGHT);
    let output = preview_output(target);
    let context = FfmpegContext::builder()
        .input(Input::from(source.to_string_lossy().into_owned()))
        .filter_desc(graph)
        .output(output)
        .build()?;
    job::run(context, info.duration_us, &mut |_| {}, cancel)
}

fn encode_webp(source: &Path, target: &Path, cancel: &AtomicBool) -> anyhow::Result<()> {
    let mut reader = WebpReader::open(source)?;
    let (width, height) = reader.dimensions();
    let alpha = reader.has_alpha();
    let delay = reader.duration_ms();
    let fps = (u64::from(reader.frame_count()) * 1000)
        .checked_div(delay)
        .unwrap_or(u64::from(PREVIEW_MAX_FPS))
        .min(u64::from(PREVIEW_MAX_FPS))
        .max(1) as u32;
    let (source, mut frames) = FramePushSource::builder(width, height)
        .pixel_format(if alpha { "rgba" } else { "rgb24" })
        .time_base(1, 1000)
        .build()?;
    let context = FfmpegContext::builder()
        .input(source)
        .filter_desc(preview_graph(fps, PREVIEW_MAX_HEIGHT))
        .output(preview_output(target))
        .build()?;
    let scheduler = context.start()?;
    let mut pts = 0i64;
    let animated = reader.is_animated();
    let count = reader.frame_count().max(1);
    for _ in 0..count {
        if cancel.load(Ordering::Relaxed) {
            scheduler.abort();
            return Err(job::Cancelled.into());
        }
        let frame = reader.read_frame()?;
        let duration = if animated { frame.delay_ms } else { 1_000 };
        if frames
            .push(frame.data, pts, Some(i64::from(duration)))
            .is_err()
        {
            break;
        }
        pts += i64::from(duration);
    }
    frames.finish();
    scheduler.wait().map_err(Into::into)
}

fn preview_graph(fps: u32, max_height: u32) -> String {
    format!(
        "[0:v]fps={fps},scale=w=-2:h='min({max_height},ih)':flags=fast_bilinear,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p[v]"
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
    fn odd_sized_animation_gets_a_valid_even_proxy() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("odd.webp");
        image::RgbaImage::new(301, 201)
            .save_with_format(&source, image::ImageFormat::WebP)
            .unwrap();
        let proxy = ensure_proxy(&source, &AtomicBool::new(false)).unwrap();
        let info = crate::info::probe(&proxy).unwrap();
        assert_eq!((info.width % 2, info.height % 2), (0, 0));
    }

    #[test]
    fn preview_contract_is_video_with_a_rate_cap() {
        assert_eq!(
            preview_graph(PREVIEW_MAX_FPS, PREVIEW_MAX_HEIGHT),
            "[0:v]fps=30,scale=w=-2:h='min(1080,ih)':flags=fast_bilinear,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p[v]"
        );
    }

    #[test]
    fn preview_graph_uses_the_requested_rate_and_height() {
        assert!(preview_graph(15, 480).starts_with("[0:v]fps=15,"));
        assert!(preview_graph(15, 480).contains("min(480,ih)"));
    }
}
