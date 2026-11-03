use mediaa2_core::formats::MediaKind;
use mediaa2_core::hash::stable_hash;
use mediaa2_protocol::{PreviewKind, PreviewReady};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};

const PREVIEW_WORKERS: usize = 3;

#[derive(Clone)]
pub struct PreviewHandle {
    permits: Arc<Semaphore>,
}

impl Default for PreviewHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl PreviewHandle {
    pub fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(PREVIEW_WORKERS)),
        }
    }

    pub fn request(
        &self,
        path: PathBuf,
        kind: PreviewKind,
        tx: mpsc::UnboundedSender<PreviewReady>,
    ) {
        let permits = Arc::clone(&self.permits);
        tokio::task::spawn(async move {
            let Ok(permit) = permits.acquire_owned().await else {
                return;
            };
            let generated_path = path.clone();
            let generated = tokio::task::spawn_blocking(move || generate(&path, kind))
                .await
                .ok()
                .flatten();
            drop(permit);
            if let Some(file) = generated {
                let _ = tx.send(PreviewReady {
                    path: generated_path,
                    kind,
                    file,
                });
            }
        });
    }
}

fn generate(path: &Path, kind: PreviewKind) -> Option<PathBuf> {
    if kind == PreviewKind::Thumbnail {
        return thumbnail(path);
    }
    let cancel = AtomicBool::new(false);
    let result = match kind {
        PreviewKind::Hover => mediaa2_media::preview_cache::ensure_hover_proxy(path, &cancel),
        PreviewKind::Player => mediaa2_media::preview_cache::ensure_proxy(path, &cancel),
        PreviewKind::Thumbnail => unreachable!(),
    };
    match result {
        Ok(file) => Some(file.into_path()),
        Err(error) => {
            log::debug!(
                "preview: cannot build {kind:?} for {}: {error:#}",
                path.display()
            );
            None
        }
    }
}

fn thumbnail(path: &Path) -> Option<PathBuf> {
    let kind = MediaKind::from_path(path)?;
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let dir = mediaa2_media::preview_cache::previews_dir(parent);
    std::fs::create_dir_all(&dir).ok()?;
    let name = format!(
        "thumb-{:016x}.png",
        stable_hash(path.to_string_lossy().as_bytes())
    );
    let target = dir.join(name);
    if target.is_file() {
        return Some(target);
    }
    let frame = mediaa2_media::thumbnail::thumbnail(path, kind, 320).ok()?;
    let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.data)?;
    image.save(&target).ok()?;
    Some(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_thumbnail_is_downscaled_into_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("Screenshot_2026-01-02_03-04-05.png");
        image::RgbaImage::from_pixel(1600, 900, image::Rgba([20, 60, 120, 255]))
            .save(&source)
            .unwrap();
        let file = generate(&source, PreviewKind::Thumbnail).expect("a thumbnail");
        assert_ne!(file, source);
        assert!(file.is_file());
        let (width, height) = image::image_dimensions(&file).unwrap();
        assert!(width <= 320 && height <= 320, "{width}x{height}");
    }
}
