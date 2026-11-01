use crate::logic::library::thumb_worker::{Key, Msg, ProxyMsg};
use crossbeam_channel::{Receiver, Sender};
use eframe::egui;
use mediaa_core::library::LibraryEntry;
use mediaa_core::Frame;
use mediaa_media::info::MediaInfo;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, SystemTime};

const REQUEST_QUEUE: usize = 4096;

#[derive(Clone, Default)]
pub struct Tile {
    pub texture: Option<egui::TextureHandle>,
    pub info: Option<MediaInfo>,
}

pub struct Thumbnails {
    tiles: HashMap<Key, Option<Tile>>,
    details_requested: HashSet<Key>,
    req_tx: Sender<Msg>,
    proxy_tx: Sender<ProxyMsg>,
    done_rx: Receiver<(Key, Option<Frame>, Option<MediaInfo>)>,
}

impl Thumbnails {
    pub fn new(ctx: &egui::Context) -> Self {
        let (req_tx, req_rx) = crossbeam_channel::bounded::<Msg>(REQUEST_QUEUE);
        let (proxy_tx, proxy_rx) = crossbeam_channel::bounded::<ProxyMsg>(REQUEST_QUEUE);
        let (done_tx, done_rx) = crossbeam_channel::bounded(64);
        let repaint_ctx = ctx.clone();
        crate::logic::library::thumb_worker::start(req_rx, proxy_rx, done_tx, move || {
            repaint_ctx.request_repaint()
        });
        Self {
            tiles: HashMap::new(),
            details_requested: HashSet::new(),
            req_tx,
            proxy_tx,
            done_rx,
        }
    }
    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, frame, info)) = self.done_rx.try_recv() {
            if !self.tiles.contains_key(&key) {
                continue;
            }
            let texture = frame.map(|f| {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [f.width as usize, f.height as usize],
                    &f.data,
                );
                ctx.load_texture(key.0.to_string_lossy(), img, egui::TextureOptions::LINEAR)
            });
            self.tiles.insert(key, Some(Tile { texture, info }));
        }
    }

    pub fn get(&mut self, e: &LibraryEntry, need_details: bool) -> Tile {
        let key = (e.path.clone(), e.modified);
        if let Some(Some(tile)) = self.tiles.get(&key) {
            let tile = tile.clone();
            if need_details && tile.info.is_none() && self.details_requested.insert(key.clone()) {
                let request = Msg::Thumb {
                    key: key.clone(),
                    size: e.size,
                    kind: e.kind,
                    need_details: true,
                };
                if self.req_tx.try_send(request).is_err() {
                    self.details_requested.remove(&key);
                }
            }
            return tile;
        }
        if matches!(self.tiles.get(&key), Some(None)) {
            return Tile::default();
        }
        self.tiles.retain(|(path, _), _| path != &e.path);
        self.tiles.insert(key.clone(), None);
        if self
            .req_tx
            .try_send(Msg::Thumb {
                key: key.clone(),
                size: e.size,
                kind: e.kind,
                need_details,
            })
            .is_err()
        {
            self.tiles.remove(&key);
        }
        Tile::default()
    }

    pub fn prewarm(&mut self, entries: &[LibraryEntry]) {
        for entry in entries.iter().take(120) {
            let _ = self.get(entry, false);
        }
    }
    #[cfg(test)]
    pub(crate) fn details_requested_for(&self, path: &Path) -> bool {
        self.details_requested
            .iter()
            .any(|(requested, _)| requested == path)
    }

    #[cfg(test)]
    pub(crate) fn is_pending(&self, path: &Path) -> bool {
        self.tiles
            .iter()
            .any(|((p, _), tile)| p == path && tile.is_none())
    }
    pub fn clear_cache(&mut self, dir: &Path) {
        self.tiles.retain(|(path, _), _| path.parent() != Some(dir));
        let _ = self.req_tx.try_send(Msg::Clear(dir.to_path_buf()));
    }
    pub fn forget_cached(&mut self, path: &Path) {
        self.tiles.retain(|(p, _), _| p != path);
        let _ = self.req_tx.try_send(Msg::Forget(path.to_path_buf()));
    }

    pub fn forget(&mut self, path: &Path, proxy: mediaa_core::paths::CachePath) {
        self.forget_cached(path);
        let _ = self.proxy_tx.try_send(ProxyMsg::Forget(proxy));
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
        let _ = self.req_tx.try_send(Msg::Relink {
            old: old.to_path_buf(),
            new: new.to_path_buf(),
        });
    }
    pub fn retain(&mut self, dir: &Path, entries: HashMap<String, SystemTime>) {
        self.tiles
            .retain(|(path, modified), _| keep_tile(path, *modified, dir, &entries));
        let names = entries.into_keys().collect();
        let _ = self.req_tx.try_send(Msg::Retain(dir.to_path_buf(), names));
    }

    pub fn flush(&self) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.req_tx.try_send(Msg::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
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
    fn forgetting_media_removes_its_preview_proxy() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("capture.webp");
        std::fs::write(&source, b"media").unwrap();
        let proxy = mediaa_media::preview_cache::proxy_path(&source);
        std::fs::create_dir_all(proxy.parent().unwrap()).unwrap();
        std::fs::write(&proxy, b"proxy").unwrap();
        let mut thumbnails = Thumbnails::new(&eframe::egui::Context::default());
        thumbnails.forget(&source, proxy.clone());
        for _ in 0..100 {
            if !proxy.exists() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(!proxy.exists());
    }

    #[test]
    fn retention_removes_tiles_from_old_and_missing_files() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
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
        assert!(!keep_tile(
            Path::new("/library/keep.mp4"),
            modified + Duration::from_secs(1),
            Path::new("/library"),
            &entries
        ));
    }

    #[test]
    fn a_prewarmed_tile_upgrades_to_details_on_demand() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        use super::Tile;
        use mediaa_core::library::LibraryEntry;
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1);
        let entry = LibraryEntry {
            path: std::path::PathBuf::from("/library/clip.mp4"),
            name: "clip.mp4".into(),
            title: "Clip".into(),
            kind: mediaa_core::MediaKind::Video,
            size: 10,
            modified,
            taken: chrono::NaiveDateTime::default(),
            details: None,
        };
        let mut thumbnails = Thumbnails::new(&eframe::egui::Context::default());
        thumbnails
            .tiles
            .insert((entry.path.clone(), modified), Some(Tile::default()));
        thumbnails.get(&entry, true);
        assert!(thumbnails.details_requested_for(&entry.path));
    }

    #[test]
    fn startup_prewarms_thumbnails_without_opening_the_library() {
        let _ui = crate::test_support::ui_test_guard();
        let _locale = crate::test_support::english_locale();
        use crate::logic::library::LibraryController;
        use crate::wake::Wake;
        use mediaa_core::Settings;
        use mediaa_storage::library_index::LibraryIndex;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Recording_2026-01-01_10-00-00.mkv");
        let entry = mediaa_core::library::LibraryEntry {
            name: "Recording_2026-01-01_10-00-00.mkv".into(),
            title: "Recording".into(),
            path: path.clone(),
            kind: mediaa_core::MediaKind::Video,
            size: 1024,
            modified: SystemTime::UNIX_EPOCH,
            taken: chrono::NaiveDateTime::default(),
            details: None,
        };
        let mut index = LibraryIndex::load(dir.path());
        index.apply_scan(vec![entry]);
        let mut controller = LibraryController::with_index(index);
        let ctx = eframe::egui::Context::default();
        let mut thumbs = Thumbnails::new(&ctx);
        let settings = Settings {
            media_dir: dir.path().to_path_buf(),
            ..Settings::default()
        };
        if let Some((dir, names)) = controller.poll_background(&settings, &Wake::default()) {
            thumbs.retain(&dir, names);
        }
        if let Some(entries) = controller.prewarm() {
            thumbs.prewarm(&entries);
        }
        assert!(thumbs.is_pending(&path));
    }
}
