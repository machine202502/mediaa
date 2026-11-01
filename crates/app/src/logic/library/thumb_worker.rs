use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use mediaa_core::{Frame, MediaKind};
use mediaa_media::info::MediaInfo;
use mediaa_storage::library_index::cache_dir;
use mediaa_storage::thumb_cache::ThumbCache;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const THUMB_SIZE: u32 = 512;
const FLUSH_AFTER: Duration = Duration::from_secs(1);

pub(crate) type Key = (PathBuf, SystemTime);

pub(crate) enum Msg {
    Thumb {
        key: Key,
        size: u64,
        kind: MediaKind,
        need_details: bool,
    },
    Forget(PathBuf),
    Relink {
        old: PathBuf,
        new: PathBuf,
    },
    Retain(PathBuf, HashSet<String>),
    Clear(PathBuf),
    Flush(Sender<()>),
}

pub(crate) enum ProxyMsg {
    Forget(mediaa_core::paths::CachePath),
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

pub(crate) fn start(
    req_rx: Receiver<Msg>,
    proxy_rx: Receiver<ProxyMsg>,
    done_tx: Sender<(Key, Option<Frame>, Option<MediaInfo>)>,
    repaint: impl Fn() + Send + 'static,
) {
    if let Err(error) = std::thread::Builder::new()
        .name("preview-proxies".into())
        .spawn(move || {
            while let Ok(msg) = proxy_rx.recv() {
                match msg {
                    ProxyMsg::Forget(path) => match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                        Err(error) => {
                            log::debug!("remove preview proxy {}: {error}", path.display())
                        }
                    },
                }
            }
        })
    {
        log::warn!("preview proxy thread: {error}");
    }
    if let Err(error) = std::thread::Builder::new()
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
                        let name = file_name(&key.0);
                        let c = cache_for(&mut cache, key.0.parent().unwrap_or(Path::new(".")));
                        let frame = c.get(&name, size, key.1).or_else(|| {
                            let made = mediaa_media::thumbnail::thumbnail(&key.0, kind, THUMB_SIZE)
                                .map_err(|e| log::debug!("thumbnail {}: {e:#}", key.0.display()))
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
                        repaint();
                    }
                    Msg::Forget(path) => {
                        if let Some(dir) = path.parent() {
                            cache_for(&mut cache, dir).remove(&file_name(&path));
                        }
                    }
                    Msg::Relink { old, new } => {
                        if let Some(dir) = old.parent() {
                            cache_for(&mut cache, dir).relink(&file_name(&old), &file_name(&new));
                        }
                    }
                    Msg::Retain(dir, names) => cache_for(&mut cache, &dir).retain(&names),
                    Msg::Clear(dir) => cache_for(&mut cache, &dir).clear(),
                    Msg::Flush(ack) => {
                        cache.as_mut().map(flush);
                        let _ = ack.send(());
                    }
                }
            }
            cache.as_mut().map(flush);
        })
    {
        log::warn!("thumbnail thread: {error}");
    }
}
