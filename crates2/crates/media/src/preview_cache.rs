use crate::ffmpeg::{job, push_source, Input, JobBuilder, Output, VSync};
use crate::info::probe;
use crate::webp::Reader;
use mediaa2_core::atomic_file::AtomicFile;
use mediaa2_core::paths::CachePath;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

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
    cache_path_at(
        source,
        tag,
        version,
        metadata.as_ref().map_or(0, |m| m.len()),
        metadata.and_then(|m| m.modified().ok()),
    )
}

fn cache_path_at(
    source: &Path,
    tag: &[u8],
    version: u32,
    size: u64,
    modified: Option<SystemTime>,
) -> CachePath {
    let mut hasher = mediaa2_core::hash::StableHasher::default();
    hasher.write(&version.to_le_bytes());
    hasher.write(tag);
    hasher.write(
        std::fs::canonicalize(source)
            .unwrap_or_else(|_| source.to_path_buf())
            .to_string_lossy()
            .as_bytes(),
    );
    hasher.write(&size.to_le_bytes());
    hasher.write(
        &modified
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

pub fn proxy_path_at(source: &Path, size: u64, modified: Option<SystemTime>) -> CachePath {
    cache_path_at(source, b"preview", PREVIEW_VERSION, size, modified)
}

pub fn hover_proxy_path_at(source: &Path, size: u64, modified: Option<SystemTime>) -> CachePath {
    cache_path_at(source, b"hover", HOVER_VERSION, size, modified)
}
pub fn hover_proxy_path(source: &Path) -> CachePath {
    cache_path(source, b"hover", HOVER_VERSION)
}

pub fn relink_proxies(old: &Path, new: &Path) -> anyhow::Result<()> {
    for (from, to) in [
        (proxy_path(old), proxy_path(new)),
        (hover_proxy_path(old), hover_proxy_path(new)),
    ] {
        if from == to || !from.is_file() {
            continue;
        }
        if let Some(parent) = to.as_ref().parent() {
            std::fs::create_dir_all(parent)?;
        }
        match std::fs::rename(from.as_ref(), to.as_ref()) {
            Ok(()) => {}
            Err(error) if to.is_file() => {
                log::debug!("preview proxy already relinked: {error:#}");
            }
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
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
    let result = if mediaa2_core::formats::is_webp_path(source) {
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
    let context = JobBuilder::new()
        .input(Input::new(source))
        .filter_desc(&graph)
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
    let mut reader = Reader::open(source)?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    let delay = info.duration_ms;
    let fps = (u64::from(info.frames) * 1000)
        .checked_div(delay)
        .unwrap_or(u64::from(max_fps))
        .min(u64::from(max_fps))
        .max(1) as u32;
    let (source, mut frames) = push_source(
        width,
        height,
        if info.has_alpha { "rgba" } else { "rgb24" },
        (1, 1000),
    )?;
    let context = JobBuilder::new()
        .push_input(source)
        .filter_desc(&preview_graph(fps, max_height))
        .output(preview_output(target))
        .build()?;
    let scheduler = context.start()?;
    let mut pts = 0i64;
    let animated = info.animated;
    let count = info.frames.max(1);
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
    scheduler.wait()
}

fn preview_graph(fps: u32, max_height: u32) -> String {
    format!(
        "[0:v]fps={fps},scale=w=-2:h='min({max_height},ih)':flags=fast_bilinear,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p[v]"
    )
}

fn preview_output(target: &Path) -> Output {
    Output::new(target)
        .format("mp4")
        .format_opt("movflags", "+faststart")
        .video_codec("libx264")
        .video_opt("preset", "ultrafast")
        .video_opt("crf", "26")
        .vsync(VSync::Cfr)
        .stream_map("[v]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preview::preview_frames;
    use mediaa2_core::formats::MediaKind;

    fn asset(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(name)
    }

    fn decoded_frames(path: &std::path::Path) -> usize {
        let mut count = 0;
        let cancel = AtomicBool::new(false);
        preview_frames(path, 160, 0.25, 0.0, &cancel, &mut |_frame| {
            count += 1;
            count < 16
        })
        .expect("hover proxy decodes");
        count
    }

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

    #[test]
    fn renaming_moves_both_proxy_namespaces_to_the_new_source() {
        let dir = tempfile::tempdir().unwrap();
        let previews = previews_dir(dir.path());
        std::fs::create_dir_all(&previews).unwrap();
        let old = dir.path().join("old_2026-01-01_10-00-00.png");
        let new = dir.path().join("new_2026-01-01_10-00-00.png");
        std::fs::write(&old, b"same").unwrap();
        std::fs::write(&new, b"same").unwrap();
        let old_full = proxy_path(&old);
        let old_hover = hover_proxy_path(&old);
        std::fs::write(old_full.as_ref(), b"full").unwrap();
        std::fs::write(old_hover.as_ref(), b"hover").unwrap();
        relink_proxies(&old, &new).unwrap();
        assert!(!old_full.exists());
        assert!(!old_hover.exists());
        assert_eq!(std::fs::read(proxy_path(&new).as_ref()).unwrap(), b"full");
        assert_eq!(
            std::fs::read(hover_proxy_path(&new).as_ref()).unwrap(),
            b"hover"
        );
    }

    #[test]
    fn hover_proxy_plays_back_frames_for_video() {
        let source = asset("window-a.mp4");
        let hover = ensure_hover_proxy(&source, &AtomicBool::new(false)).unwrap();
        assert!(hover.is_file());
        assert!(decoded_frames(hover.as_ref()) > 0);
    }

    #[test]
    fn hover_proxy_plays_back_frames_for_animated_webp() {
        let source = asset("fake-window.webp");
        assert_eq!(MediaKind::detect(&source), Some(MediaKind::Animation));
        let hover = ensure_hover_proxy(&source, &AtomicBool::new(false)).unwrap();
        assert!(hover.is_file());
        assert!(decoded_frames(hover.as_ref()) > 0);
    }
}
