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
const PREVIEW_VERSION: u32 = 2;
const HOVER_MAX_HEIGHT: u32 = 360;
const HOVER_MAX_FPS: u32 = 15;
const HOVER_VERSION: u32 = 1;

static ANIMATED_PREVIEWS: AtomicBool = AtomicBool::new(true);
static FULL_PREVIEWS: AtomicBool = AtomicBool::new(true);

#[derive(Clone, Copy)]
enum PreviewKind {
    Animated,
    Full,
}

impl PreviewKind {
    fn enabled(self) -> bool {
        match self {
            PreviewKind::Animated => ANIMATED_PREVIEWS.load(Ordering::Relaxed),
            PreviewKind::Full => FULL_PREVIEWS.load(Ordering::Relaxed),
        }
    }
}

pub fn set_animated_previews_enabled(enabled: bool) {
    ANIMATED_PREVIEWS.store(enabled, Ordering::Relaxed);
}

pub fn set_full_previews_enabled(enabled: bool) {
    FULL_PREVIEWS.store(enabled, Ordering::Relaxed);
}

pub fn animated_previews_enabled() -> bool {
    ANIMATED_PREVIEWS.load(Ordering::Relaxed)
}

pub fn full_previews_enabled() -> bool {
    FULL_PREVIEWS.load(Ordering::Relaxed)
}

pub fn clear_previews(media_dir: &Path, animated: bool, full: bool) -> usize {
    let dir = previews_dir(media_dir);
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let hover = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("hover-"));
        if ((hover && animated) || (!hover && full)) && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

pub fn prune_previews(media_dir: &Path, older_than: std::time::SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(previews_dir(media_dir)) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .is_ok_and(|modified| modified < older_than);
        if old && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

pub fn previews_dir(media_dir: &Path) -> std::path::PathBuf {
    media_dir.join(".cache").join("previews")
}

fn cache_path(source: &Path, tag: &[u8], version: u32) -> CachePath {
    let metadata = std::fs::metadata(source).ok();
    let mut hasher = mediaa_core::hash::StableHasher::default();
    hasher.write(&version.to_le_bytes());
    hasher.write(tag);
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
        previews_dir(source.parent().unwrap_or(Path::new("."))).join(if tag == b"hover" {
            format!("hover-{hash:016x}.mp4")
        } else {
            format!("{hash:016x}.mp4")
        }),
    )
}

pub fn proxy_path(source: &Path) -> CachePath {
    cache_path(source, b"preview", PREVIEW_VERSION)
}

pub fn hover_proxy_path(source: &Path) -> CachePath {
    cache_path(source, b"hover", HOVER_VERSION)
}

pub fn ready_proxy(source: &Path) -> Option<CachePath> {
    let path = proxy_path(source);
    path.is_file().then_some(path)
}

pub fn ready_hover_proxy(source: &Path) -> Option<CachePath> {
    let path = hover_proxy_path(source);
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
    ensure_at(
        source,
        proxy_path(media),
        PREVIEW_MAX_HEIGHT,
        PREVIEW_MAX_FPS,
        PreviewKind::Full,
        cancel,
    )
}

pub fn ensure_hover_proxy(source: &Path, cancel: &AtomicBool) -> anyhow::Result<CachePath> {
    ensure_at(
        source,
        hover_proxy_path(source),
        HOVER_MAX_HEIGHT,
        HOVER_MAX_FPS,
        PreviewKind::Animated,
        cancel,
    )
}

fn ensure_at(
    source: &Path,
    target: CachePath,
    max_height: u32,
    max_fps: u32,
    kind: PreviewKind,
    cancel: &AtomicBool,
) -> anyhow::Result<CachePath> {
    if !kind.enabled() {
        return Err(anyhow::anyhow!("preview generation is disabled"));
    }
    if target.is_file() {
        return Ok(target);
    }
    let pending = AtomicFile::new(target.as_ref())?;
    let result = if mediaa_core::formats::is_webp_path(source) {
        encode_webp(source, pending.path(), max_height, max_fps, kind, cancel)
    } else {
        encode_video(source, pending.path(), max_height, max_fps, kind, cancel)
    };
    result?;
    if !kind.enabled() {
        return Err(anyhow::anyhow!("preview generation is disabled"));
    }
    match pending.commit() {
        Ok(path) => Ok(CachePath::new(path)),
        Err(error) if target.is_file() => {
            log::debug!("preview proxy already completed: {error:#}");
            Ok(target)
        }
        Err(error) => Err(error),
    }
}

fn encode_video(
    source: &Path,
    target: &Path,
    max_height: u32,
    max_fps: u32,
    kind: PreviewKind,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    if !kind.enabled() {
        return Err(job::Cancelled.into());
    }
    let info = probe(source)?;
    let fps = if info.fps > 0.0 {
        info.fps.min(f64::from(max_fps)).round().max(1.0) as u32
    } else {
        max_fps
    };
    let graph = preview_graph(fps, max_height);
    let output = preview_output(target);
    let context = FfmpegContext::builder()
        .input(Input::from(source.to_string_lossy().into_owned()))
        .filter_desc(graph)
        .output(output)
        .build()?;
    job::run(context, info.duration_us, &mut |_| {}, cancel)
}

fn encode_webp(
    source: &Path,
    target: &Path,
    max_height: u32,
    max_fps: u32,
    kind: PreviewKind,
    cancel: &AtomicBool,
) -> anyhow::Result<()> {
    let mut reader = WebpReader::open(source)?;
    let (width, height) = reader.dimensions();
    let alpha = reader.has_alpha();
    let delay = reader.duration_ms();
    let fps = (u64::from(reader.frame_count()) * 1000)
        .checked_div(delay)
        .unwrap_or(u64::from(max_fps))
        .min(u64::from(max_fps))
        .max(1) as u32;
    let (source, mut frames) = FramePushSource::builder(width, height)
        .pixel_format(if alpha { "rgba" } else { "rgb24" })
        .time_base(1, 1000)
        .build()?;
    let context = FfmpegContext::builder()
        .input(source)
        .filter_desc(preview_graph(fps, max_height))
        .output(preview_output(target))
        .build()?;
    let scheduler = context.start()?;
    let mut pts = 0i64;
    let animated = reader.is_animated();
    let count = reader.frame_count().max(1);
    for _ in 0..count {
        if cancel.load(Ordering::Relaxed) || !kind.enabled() {
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

    #[test]
    fn hover_proxies_live_in_their_own_cache_namespace() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.webp");
        std::fs::write(&source, b"source").unwrap();
        let hover = hover_proxy_path(&source);
        assert!(hover
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("hover-")));
        assert_ne!(hover, proxy_path(&source));
    }

    #[test]
    fn clearing_previews_can_target_animated_or_full_cache_independently() {
        let dir = tempfile::tempdir().unwrap();
        let previews = previews_dir(dir.path());
        std::fs::create_dir_all(&previews).unwrap();
        std::fs::write(previews.join("aaaa.mp4"), b"full").unwrap();
        std::fs::write(previews.join("hover-bbbb.mp4"), b"hover").unwrap();

        assert_eq!(clear_previews(dir.path(), true, false), 1);
        assert!(!previews.join("hover-bbbb.mp4").exists());
        assert!(previews.join("aaaa.mp4").exists());

        std::fs::write(previews.join("hover-cccc.mp4"), b"hover").unwrap();
        assert_eq!(clear_previews(dir.path(), false, true), 1);
        assert!(!previews.join("aaaa.mp4").exists());
        assert!(previews.join("hover-cccc.mp4").exists());
    }
}
