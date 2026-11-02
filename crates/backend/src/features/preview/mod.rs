pub mod store;

use crate::features::preview::store::{now_ms, PreviewDecision, PreviewStore};
use mediaa_core::formats::MediaKind;
use mediaa_protocol::backend::{
    PreviewChangedWire, PreviewKindWire, PreviewRequestWire, PreviewStateWire,
};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

const THUMBNAIL_SIZE: u32 = 320;
const JOB_TIMEOUT: Duration = Duration::from_secs(120);
pub const PREVIEW_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const PREVIEW_CLEANUP_INTERVAL: Duration = Duration::from_secs(15 * 60);
pub const PREVIEW_WORKERS: usize = 1;

#[derive(Debug, Clone)]
struct Job {
    path: String,
    kind: PreviewKindWire,
}

pub struct PreviewService {
    tx: mpsc::UnboundedSender<Job>,
    events: broadcast::Sender<PreviewChangedWire>,
    store: Arc<Mutex<PreviewStore>>,
}

impl PreviewService {
    pub fn start(store: Arc<Mutex<PreviewStore>>, media_dir: PathBuf) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<Job>();
        let (events, _) = broadcast::channel(128);
        let cleanup_store = store.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(PREVIEW_CLEANUP_INTERVAL);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tick.tick().await;
                let now = now_ms();
                let cutoff = now.saturating_sub(PREVIEW_TTL.as_millis() as u64);
                let _ = cleanup_store.lock().cleanup(cutoff);
            }
        });
        let worker_events = events.clone();
        let worker_store = store.clone();
        tokio::spawn(async move {
            while let Some(job) = rx.recv().await {
                let started = {
                    let mut store = worker_store.lock();
                    store.started(&job.path, job.kind).is_ok()
                };
                if !started {
                    continue;
                }
                let kind = MediaKind::detect(Path::new(&job.path)).unwrap_or(MediaKind::Image);
                let path = job.path.clone();
                let kind_request = job.kind;
                let media = media_dir.clone();
                let result = tokio::time::timeout(
                    JOB_TIMEOUT,
                    tokio::task::spawn_blocking(move || {
                        generate(&media, &path, kind, kind_request)
                    }),
                )
                .await;
                let outcome = match result {
                    Ok(Ok(Ok(path))) => Ok(path),
                    Ok(Ok(Err(error))) => Err(error),
                    Ok(Err(error)) => Err(error.to_string()),
                    Err(_) => Err("preview generation timed out".to_string()),
                };
                let state = {
                    let mut store = worker_store.lock();
                    match &outcome {
                        Ok(path) => {
                            let _ = store.finished(&job.path, job.kind, Ok(path));
                            PreviewStateWire::Ready { path: path.clone() }
                        }
                        Err(error) => {
                            let _ = store.finished(&job.path, job.kind, Err(error));
                            PreviewStateWire::Failed {
                                error: error.clone(),
                            }
                        }
                    }
                };
                let _ = worker_events.send(PreviewChangedWire {
                    path: job.path,
                    kind: job.kind,
                    state,
                });
            }
        });
        Self { tx, events, store }
    }

    pub fn request(&self, request: &PreviewRequestWire) -> anyhow::Result<PreviewDecision> {
        let decision = {
            let mut store = self.store.lock();
            store.request(request)?
        };
        if let PreviewDecision::Pending { enqueue: true } = decision {
            let _ = self.tx.send(Job {
                path: request.path.clone(),
                kind: request.kind,
            });
        }
        Ok(decision)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<PreviewChangedWire> {
        self.events.subscribe()
    }

    pub fn cleanup_stale(&self) -> anyhow::Result<usize> {
        let now = now_ms();
        let cutoff = now.saturating_sub(PREVIEW_TTL.as_millis() as u64);
        self.store.lock().cleanup(cutoff)
    }

    pub fn store(&self) -> &Arc<Mutex<PreviewStore>> {
        &self.store
    }
}

fn generate(
    media_dir: &Path,
    path: &str,
    kind: MediaKind,
    request: PreviewKindWire,
) -> Result<String, String> {
    match request {
        PreviewKindWire::Hover => {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let proxy = mediaa_media::preview_cache::ensure_hover_proxy(Path::new(path), &cancel)
                .map_err(|error| format!("{error:#}"))?;
            Ok(proxy.to_string_lossy().into_owned())
        }
        PreviewKindWire::Player => {
            let cancel = std::sync::atomic::AtomicBool::new(false);
            let proxy = mediaa_media::preview_cache::ensure_proxy(Path::new(path), &cancel)
                .map_err(|error| format!("{error:#}"))?;
            Ok(proxy.to_string_lossy().into_owned())
        }
        PreviewKindWire::Thumbnail => {
            let frame = mediaa_media::thumbnail::thumbnail(Path::new(path), kind, THUMBNAIL_SIZE)
                .map_err(|error| format!("{error:#}"))?;
            let dir = mediaa_media::preview_cache::previews_dir(media_dir);
            std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
            let digest = mediaa_core::hash::stable_hash(path.as_bytes());
            let target = dir.join(format!("thumb-{digest:016x}.png"));
            let image = image::RgbaImage::from_raw(frame.width, frame.height, frame.data)
                .ok_or_else(|| "bad thumbnail buffer".to_string())?;
            image
                .save_with_format(&target, image::ImageFormat::Png)
                .map_err(|error| error.to_string())?;
            Ok(target.to_string_lossy().into_owned())
        }
    }
}

pub struct PreviewHandle {
    runtime: tokio::runtime::Runtime,
    service: PreviewService,
    events: parking_lot::Mutex<broadcast::Receiver<PreviewChangedWire>>,
}

impl PreviewHandle {
    pub fn start(media_dir: &Path) -> anyhow::Result<Self> {
        let runtime = crate::runtime::runtime()?;
        let _guard = runtime.enter();
        let store = Arc::new(Mutex::new(PreviewStore::open(media_dir)?));
        let service = PreviewService::start(store, media_dir.to_path_buf());
        let events = parking_lot::Mutex::new(service.subscribe());
        Ok(Self {
            runtime,
            service,
            events,
        })
    }

    pub fn request(&self, request: &PreviewRequestWire) -> anyhow::Result<PreviewDecision> {
        self.service.request(request)
    }

    pub fn try_event(&self) -> Option<PreviewChangedWire> {
        self.events.lock().try_recv().ok()
    }

    pub fn cleanup_stale(&self) -> anyhow::Result<usize> {
        self.service.cleanup_stale()
    }

    pub fn runtime(&self) -> &tokio::runtime::Runtime {
        &self.runtime
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_protocol::backend::{PreviewKindWire, PreviewRequestWire, PreviewStateWire};

    #[test]
    fn preview_handle_starts_a_backend_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let handle = PreviewHandle::start(dir.path()).expect("preview handle");
        assert_eq!(handle.cleanup_stale().expect("cleanup"), 0);
        assert!(handle.try_event().is_none());
        assert!(handle.runtime().handle().metrics().num_workers() > 0);
    }

    #[tokio::test]
    async fn request_generates_one_thumbnail_and_reuses_it() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path();
        let source = media.join("shot.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([1, 2, 3, 255]))
            .save(&source)
            .unwrap();
        let store = Arc::new(Mutex::new(PreviewStore::open(media).unwrap()));
        let previews = PreviewService::start(store, media.to_path_buf());
        let mut events = previews.subscribe();
        let request = PreviewRequestWire {
            path: source.to_string_lossy().into_owned(),
            kind: PreviewKindWire::Thumbnail,
        };
        assert_eq!(
            previews.request(&request).unwrap(),
            PreviewDecision::Pending { enqueue: true }
        );
        let changed = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("preview event")
            .expect("broadcast");
        let PreviewStateWire::Ready { path } = changed.state else {
            panic!("preview failed: {:?}", changed.state);
        };
        assert!(std::path::Path::new(&path).is_file());
        assert_eq!(
            previews.request(&request).unwrap(),
            PreviewDecision::Ready { path }
        );
    }
}
