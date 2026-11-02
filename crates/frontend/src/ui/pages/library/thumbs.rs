use crossbeam_channel::Sender;
use eframe::egui;
use mediaa_core::library::LibraryEntry;
use mediaa_core::Frame;
use mediaa_media::info::MediaInfo;
use mediaa_protocol::backend::{PreviewKindWire, PreviewRequestWire};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub type Key = (PathBuf, SystemTime);

#[derive(Clone, Default)]
pub struct Tile {
    pub texture: Option<egui::TextureHandle>,
    pub info: Option<MediaInfo>,
}

pub struct Thumbnails {
    tiles: HashMap<Key, Option<Tile>>,
    backend: Option<Sender<PreviewRequestWire>>,
    backend_pending: HashSet<PathBuf>,
    backend_details: HashMap<PathBuf, Option<MediaInfo>>,
    ctx: egui::Context,
}

impl Thumbnails {
    pub fn new(ctx: &egui::Context, backend: Option<Sender<PreviewRequestWire>>) -> Self {
        Self {
            tiles: HashMap::new(),
            backend,
            backend_pending: HashSet::new(),
            backend_details: HashMap::new(),
            ctx: ctx.clone(),
        }
    }

    pub fn poll(&mut self, _ctx: &egui::Context) {}

    pub fn get(&mut self, entry: &LibraryEntry, need_details: bool) -> Tile {
        if let Some(backend) = self.backend.clone() {
            return self.get_backend(entry, need_details, &backend);
        }
        self.tiles
            .get(&(entry.path.clone(), entry.modified))
            .and_then(Clone::clone)
            .unwrap_or_default()
    }

    fn get_backend(
        &mut self,
        entry: &LibraryEntry,
        need_details: bool,
        backend: &Sender<PreviewRequestWire>,
    ) -> Tile {
        let key = (entry.path.clone(), entry.modified);
        if let Some(Some(tile)) = self.tiles.get(&key) {
            return tile.clone();
        }
        if self.backend_pending.contains(&entry.path) {
            return Tile::default();
        }
        let info = entry.details.map(|details| MediaInfo {
            duration_us: details
                .duration_secs
                .map(|seconds| (seconds * 1_000_000.0) as i64),
            width: details.width,
            height: details.height,
            fps: 0.0,
            video_codec: None,
            audio_codec: None,
            audio_streams: usize::from(details.audio),
        });
        if need_details {
            self.backend_details.insert(entry.path.clone(), info);
        }
        self.tiles.insert(key, None);
        self.backend_pending.insert(entry.path.clone());
        let request = PreviewRequestWire {
            path: entry.path.to_string_lossy().into_owned(),
            kind: PreviewKindWire::Thumbnail,
        };
        if backend.try_send(request).is_err() {
            self.backend_pending.remove(&entry.path);
            self.tiles.remove(&(entry.path.clone(), entry.modified));
        }
        Tile::default()
    }

    pub fn backend_ready(&mut self, path: &Path, frame: Frame) {
        self.backend_pending.remove(path);
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [frame.width as usize, frame.height as usize],
            &frame.data,
        );
        let texture =
            self.ctx
                .load_texture(path.to_string_lossy(), img, egui::TextureOptions::LINEAR);
        let keys: Vec<Key> = self
            .tiles
            .keys()
            .filter(|(tile_path, _)| tile_path == path)
            .cloned()
            .collect();
        for key in keys {
            let info = self.backend_details.remove(path).flatten();
            self.tiles.insert(
                key,
                Some(Tile {
                    texture: Some(texture.clone()),
                    info,
                }),
            );
        }
    }

    pub fn backend_failed(&mut self, path: &Path) {
        self.backend_pending.remove(path);
        self.tiles.retain(|(tile_path, _), _| tile_path != path);
    }

    pub fn prewarm(&mut self, entries: &[LibraryEntry]) {
        for entry in entries.iter().take(120) {
            let _ = self.get(entry, false);
        }
    }

    pub fn clear_cache(&mut self, dir: &Path) {
        self.tiles.retain(|(path, _), _| path.parent() != Some(dir));
    }

    pub fn forget_cached(&mut self, path: &Path) {
        self.tiles.retain(|(tile_path, _), _| tile_path != path);
    }

    pub fn forget(&mut self, path: &Path, _proxy: mediaa_core::paths::CachePath) {
        self.forget_cached(path);
    }

    pub fn relink(&mut self, old: &Path, new: &Path) {
        let keys: Vec<Key> = self
            .tiles
            .keys()
            .filter(|(path, _)| path == old)
            .cloned()
            .collect();
        for key in keys {
            if let Some(tile) = self.tiles.remove(&key) {
                self.tiles.insert((new.to_path_buf(), key.1), tile);
            }
        }
    }

    pub fn retain(&mut self, dir: &Path, entries: HashMap<String, SystemTime>) {
        self.tiles
            .retain(|(path, modified), _| keep_tile(path, *modified, dir, &entries));
    }

    pub fn flush(&self) {}

    #[cfg(test)]
    pub(crate) fn is_pending(&self, path: &Path) -> bool {
        self.backend_pending.contains(path)
    }
}

fn keep_tile(
    path: &Path,
    modified: SystemTime,
    dir: &Path,
    entries: &HashMap<String, SystemTime>,
) -> bool {
    path.parent() == Some(dir)
        && path
            .file_name()
            .is_some_and(|name| entries.get(name.to_string_lossy().as_ref()) == Some(&modified))
}

#[cfg(test)]
mod tests {
    use super::{keep_tile, Thumbnails};
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::{Duration, SystemTime};

    #[test]
    fn retention_removes_tiles_from_old_and_missing_files() {
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let entries = HashMap::from([("keep.mp4".to_string(), modified)]);
        assert!(keep_tile(
            Path::new("/library/keep.mp4"),
            modified,
            Path::new("/library"),
            &entries
        ));
        assert!(!keep_tile(
            Path::new("/library/gone.mp4"),
            modified,
            Path::new("/library"),
            &entries
        ));
        assert!(!keep_tile(
            Path::new("/other/keep.mp4"),
            modified,
            Path::new("/library"),
            &entries
        ));
    }

    #[test]
    fn backend_requests_are_lazy() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let mut thumbs = Thumbnails::new(&eframe::egui::Context::default(), Some(tx));
        let entry = mediaa_core::library::LibraryEntry {
            path: std::path::PathBuf::from("/library/clip.mp4"),
            name: "clip.mp4".into(),
            title: "Clip".into(),
            kind: mediaa_core::MediaKind::Video,
            size: 10,
            modified: SystemTime::UNIX_EPOCH,
            taken: chrono::NaiveDateTime::default(),
            details: None,
        };
        thumbs.get(&entry, true);
        assert!(thumbs.is_pending(&entry.path));
        let request = rx.try_recv().expect("request");
        assert_eq!(
            request.kind,
            mediaa_protocol::backend::PreviewKindWire::Thumbnail
        );
    }
}
