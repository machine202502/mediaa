use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use eframe::egui;
use mediaa_core::library::LibraryEntry;
use mediaa_core::library_index::cache_dir;
use mediaa_core::thumb_cache::ThumbCache;
use mediaa_core::{Frame, MediaKind};
use mediaa_media::info::MediaInfo;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const THUMB_SIZE: u32 = 512;
const FLUSH_AFTER: Duration = Duration::from_secs(1);

type Key = (PathBuf, SystemTime);

#[derive(Clone, Default)]
pub struct Tile {
    pub texture: Option<egui::TextureHandle>,
    pub info: Option<MediaInfo>,
}

enum Msg {
    Thumb {
        key: Key,
        size: u64,
        kind: MediaKind,
        need_details: bool,
    },
    Forget(PathBuf),
    Retain(PathBuf, HashSet<String>),
    Flush(Sender<()>),
}

enum ProxyMsg {
    Ensure(PathBuf),
    Forget(PathBuf),
}

pub struct Thumbnails {
    tiles: HashMap<Key, Option<Tile>>,
    req_tx: Sender<Msg>,
    proxy_tx: Sender<ProxyMsg>,
    done_rx: Receiver<(Key, Option<Frame>, Option<MediaInfo>)>,
}

fn cache_for<'a>(cache: &'a mut Option<ThumbCache>, media_dir: &Path) -> &'a mut ThumbCache {
    let dir = cache_dir(media_dir);
    if cache.as_ref().is_none_or(|c| c.dir() != dir) {
        if let Some(old) = cache.as_mut() {
            flush(old);
        }
        *cache = Some(ThumbCache::open(&dir));
    }
    cache.as_mut().expect("cache just opened")
}

fn flush(cache: &mut ThumbCache) {
    if let Err(e) = cache.flush() {
        log::warn!("thumbnail cache: {e:#}");
    }
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

impl Thumbnails {
    pub fn new(ctx: &egui::Context) -> Self {
        let (req_tx, req_rx) = crossbeam_channel::unbounded::<Msg>();
        let (proxy_tx, proxy_rx) = crossbeam_channel::unbounded::<ProxyMsg>();
        let (done_tx, done_rx) = crossbeam_channel::unbounded();
        std::thread::Builder::new()
            .name("preview-proxies".into())
            .spawn(move || {
                let mut seen = HashSet::new();
                while let Ok(msg) = proxy_rx.recv() {
                    match msg {
                        ProxyMsg::Ensure(path) => {
                            let proxy = mediaa_media::preview_cache::proxy_path(&path);
                            if !seen.insert(proxy.clone()) {
                                continue;
                            }
                            let cancel = std::sync::atomic::AtomicBool::new(false);
                            if let Err(error) =
                                mediaa_media::preview_cache::ensure_proxy(&path, &cancel)
                            {
                                seen.remove(&proxy);
                                log::debug!("preview proxy {}: {error:#}", path.display());
                            }
                        }
                        ProxyMsg::Forget(path) => {
                            seen.remove(&path);
                            match std::fs::remove_file(&path) {
                                Ok(()) => {}
                                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                                Err(error) => {
                                    log::debug!("remove preview proxy {}: {error}", path.display())
                                }
                            }
                        }
                    }
                }
            })
            .expect("spawn preview proxy thread");
        let proxy_sender = proxy_tx.clone();
        let ctx = ctx.clone();
        std::thread::Builder::new()
            .name("thumbnails".into())
            .spawn(move || {
                let mut cache: Option<ThumbCache> = None;
                loop {
                    let msg = match req_rx.recv_timeout(FLUSH_AFTER) {
                        Ok(msg) => msg,
                        Err(RecvTimeoutError::Timeout) => {
                            cache.as_mut().filter(|c| c.is_dirty()).map(flush);
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => break,
                    };
                    match msg {
                        Msg::Thumb {
                            key,
                            size,
                            kind,
                            need_details,
                        } => {
                            if matches!(kind, MediaKind::Animation)
                                && mediaa_media::preview_cache::ready_proxy(&key.0).is_none()
                            {
                                let _ = proxy_sender.send(ProxyMsg::Ensure(key.0.clone()));
                            }
                            let name = file_name(&key.0);
                            let c = cache_for(&mut cache, key.0.parent().unwrap_or(Path::new(".")));
                            let frame = c.get(&name, size, key.1).or_else(|| {
                                let made =
                                    mediaa_media::thumbnail::thumbnail(&key.0, kind, THUMB_SIZE)
                                        .map_err(|e| {
                                            log::debug!("thumbnail {}: {e:#}", key.0.display())
                                        })
                                        .ok()?;
                                c.put(&name, size, key.1, &made);
                                Some(made)
                            });
                            let info = need_details
                                .then(|| mediaa_media::preview::details(&key.0, kind))
                                .and_then(|r| {
                                    r.map_err(|e| log::debug!("details {}: {e:#}", key.0.display()))
                                        .ok()
                                });
                            if done_tx.send((key, frame, info)).is_err() {
                                break;
                            }
                            ctx.request_repaint();
                        }
                        Msg::Forget(path) => {
                            if let Some(dir) = path.parent() {
                                cache_for(&mut cache, dir).remove(&file_name(&path));
                            }
                        }
                        Msg::Retain(dir, names) => cache_for(&mut cache, &dir).retain(&names),
                        Msg::Flush(ack) => {
                            cache.as_mut().map(flush);
                            let _ = ack.send(());
                        }
                    }
                }
                cache.as_mut().map(flush);
            })
            .expect("spawn thumbnail thread");
        Self {
            tiles: HashMap::new(),
            req_tx,
            proxy_tx,
            done_rx,
        }
    }

    pub fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, frame, info)) = self.done_rx.try_recv() {
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
        match self.tiles.get(&key) {
            Some(Some(t)) => t.clone(),
            Some(None) => Tile::default(),
            None => {
                self.tiles.insert(key.clone(), None);
                let _ = self.req_tx.send(Msg::Thumb {
                    key,
                    size: e.size,
                    kind: e.kind,
                    need_details,
                });
                Tile::default()
            }
        }
    }

    pub fn forget(&mut self, path: &Path, proxy: PathBuf) {
        self.tiles.retain(|(p, _), _| p != path);
        let _ = self.req_tx.send(Msg::Forget(path.to_path_buf()));
        let _ = self.proxy_tx.send(ProxyMsg::Forget(proxy));
    }

    pub fn retain(&mut self, dir: &Path, names: HashSet<String>) {
        let _ = self.req_tx.send(Msg::Retain(dir.to_path_buf(), names));
    }

    pub fn flush(&self) {
        let (tx, rx) = crossbeam_channel::bounded(1);
        if self.req_tx.send(Msg::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Thumbnails;
    use std::time::Duration;

    #[test]
    fn forgetting_media_removes_its_preview_proxy() {
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
}
