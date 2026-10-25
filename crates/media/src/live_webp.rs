use crate::animation_backends::{self, AnimationEncodeRequest, AnimationOrigin, WebpSegmentation};
use crate::filters::Effort;
use anyhow::Context;
use crossbeam_channel::{bounded, Receiver, Sender};
use mediaa_core::settings::AnimSettings;
use mediaa_webp_pipeline::{self, PipelineConfig, PipelineTuning};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug, Clone)]
struct LiveChunk {
    source: usize,
    input: PathBuf,
    output: PathBuf,
}

pub(crate) struct LiveResult {
    pub parts: Vec<Vec<PathBuf>>,
    pub error: Option<String>,
}

pub(crate) struct LiveWebp {
    control: Sender<bool>,
    cancel: Arc<AtomicBool>,
    join: JoinHandle<LiveResult>,
}

impl LiveWebp {
    pub(crate) fn start(
        lists: Vec<PathBuf>,
        root: PathBuf,
        settings: AnimSettings,
        scheduling: PipelineTuning,
        trace_id: String,
        size: (u32, u32),
    ) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&root)?;
        let config = PipelineConfig::new(
            1_000_000,
            settings.fps,
            size.0,
            size.1,
            settings.webp_quality,
        )
        .with_tuning(&scheduling);
        let workers = mediaa_webp_pipeline::recommended_workers(&config).max(1);
        let (control, control_rx) = bounded(8);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        log::info!("[{trace_id}] stage=live start size={}x{}", size.0, size.1);
        let join = std::thread::Builder::new()
            .name("live-webp".into())
            .spawn(move || {
                run_manager(
                    lists,
                    root,
                    settings,
                    scheduling,
                    workers,
                    control_rx,
                    worker_cancel,
                    trace_id,
                )
            })?;
        Ok(Self {
            control,
            cancel,
            join,
        })
    }

    pub(crate) fn finish(self, external_cancel: Arc<AtomicBool>) -> LiveResult {
        let Self {
            control,
            cancel,
            join,
        } = self;
        let done = Arc::new(AtomicBool::new(false));
        let watcher_done = done.clone();
        let watcher_cancel = cancel.clone();
        let watcher = std::thread::Builder::new()
            .name("live-webp-cancel".into())
            .spawn(move || {
                while !watcher_done.load(Ordering::Relaxed) {
                    if external_cancel.load(Ordering::Relaxed) {
                        watcher_cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            })
            .ok();
        let _ = control.send(true);
        let result = join.join().unwrap_or_else(|_| LiveResult {
            parts: Vec::new(),
            error: Some("live WebP manager panicked".into()),
        });
        done.store(true, Ordering::Relaxed);
        if let Some(watcher) = watcher {
            let _ = watcher.join();
        }
        result
    }

    pub(crate) fn cancel(self) -> LiveResult {
        self.cancel.store(true, Ordering::Relaxed);
        let _ = self.control.send(false);
        self.join.join().unwrap_or_else(|_| LiveResult {
            parts: Vec::new(),
            error: Some("live WebP manager panicked".into()),
        })
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "manager state collected for one recording"
)]
fn run_manager(
    lists: Vec<PathBuf>,
    root: PathBuf,
    settings: AnimSettings,
    scheduling: PipelineTuning,
    workers: usize,
    control: crossbeam_channel::Receiver<bool>,
    cancel: Arc<AtomicBool>,
    trace_id: String,
) -> LiveResult {
    let (jobs_tx, jobs_rx): (Sender<LiveChunk>, Receiver<LiveChunk>) =
        bounded(workers.saturating_mul(2).max(8));
    let (results_tx, results_rx) = bounded(workers.max(1));
    let failed = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::new();
    let mut error = None;
    for worker in 0..workers {
        let jobs_rx = jobs_rx.clone();
        let results_tx = results_tx.clone();
        let worker_failed = failed.clone();
        let cancel = cancel.clone();
        let settings = settings.clone();
        let scheduling = scheduling.clone();
        let worker_trace = trace_id.clone();
        match std::thread::Builder::new()
            .name(format!("live-webp-{worker}"))
            .spawn(move || {
                while let Ok(chunk) = jobs_rx.recv() {
                    if cancel.load(Ordering::Relaxed) || worker_failed.load(Ordering::Relaxed) {
                        break;
                    }
                    let request = AnimationEncodeRequest {
                        input: &chunk.input,
                        output: &chunk.output,
                        window: None,
                        format: mediaa_core::AnimFormat::Webp,
                        settings: &settings,
                        effort: Effort::Balanced,
                        origin: AnimationOrigin::Recording,
                        segmentation: WebpSegmentation::Direct,
                        scheduling: scheduling.clone(),
                        threads: Some(1),
                        trace_id: Some(&worker_trace),
                    };
                    let result = animation_backends::encode(&request, &mut |_| {}, &cancel)
                        .and_then(|()| {
                            let duration_us = crate::info::probe(&chunk.input)?
                                .duration_us
                                .unwrap_or(1_000_000);
                            mediaa_webp_pipeline::normalize_part(&chunk.output, duration_us)
                        })
                        .and_then(|()| mediaa_webp_pipeline::validate_part(&chunk.output))
                        .with_context(|| {
                            format!("encode live WebP chunk {}", chunk.input.display())
                        });
                    if let Err(error) = &result {
                        worker_failed.store(true, Ordering::Relaxed);
                        log::warn!("[{}] stage=live chunk-failed: {error:#}", worker_trace);
                    }
                    let _ = results_tx.send((chunk, result));
                    if worker_failed.load(Ordering::Relaxed) {
                        break;
                    }
                }
            }) {
            Ok(handle) => handles.push(handle),
            Err(failure) => {
                failed.store(true, Ordering::Relaxed);
                error = Some(format!("could not start live WebP worker: {failure}"));
                break;
            }
        }
    }
    drop(results_tx);

    let mut seen = vec![0usize; lists.len()];
    let mut parts = vec![Vec::new(); lists.len()];
    loop {
        for (chunk, result) in results_rx.try_iter() {
            match result {
                Ok(()) => parts[chunk.source].push(chunk.output),
                Err(failure) => {
                    failed.store(true, Ordering::Relaxed);
                    error = Some(format!("{failure:#}"));
                }
            }
        }
        if failed.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
            break;
        }
        scan_lists(&lists, &mut seen, &root, &jobs_tx, false);
        match control.recv_timeout(Duration::from_millis(100)) {
            Ok(true) => {
                scan_lists(&lists, &mut seen, &root, &jobs_tx, true);
                break;
            }
            Ok(false) => break,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(jobs_tx);
    for handle in handles {
        let _ = handle.join();
    }
    for (chunk, result) in results_rx.try_iter() {
        match result {
            Ok(()) => parts[chunk.source].push(chunk.output),
            Err(failure) => {
                failed.store(true, Ordering::Relaxed);
                error = Some(format!("{failure:#}"));
            }
        }
    }
    for source_parts in &mut parts {
        source_parts.sort();
    }
    if cancel.load(Ordering::Relaxed) {
        error = Some("cancelled".into());
    } else if error.is_none() {
        let expected: usize = seen.iter().sum();
        let completed: usize = parts.iter().map(Vec::len).sum();
        if completed != expected {
            error = Some(format!(
                "live WebP encoded {completed} of {expected} chunks"
            ));
        }
    }
    match &error {
        Some(error) => log::warn!(
            "[{trace_id}] stage=live stopped completed={} expected={} error={error}",
            parts.iter().map(Vec::len).sum::<usize>(),
            seen.iter().sum::<usize>()
        ),
        None => log::info!(
            "[{trace_id}] stage=live complete chunks={}",
            parts.iter().map(Vec::len).sum::<usize>()
        ),
    }
    LiveResult { parts, error }
}

fn scan_lists(
    lists: &[PathBuf],
    seen: &mut [usize],
    root: &Path,
    jobs: &Sender<LiveChunk>,
    final_scan: bool,
) {
    for (source, list) in lists.iter().enumerate() {
        let Ok(text) = std::fs::read_to_string(list) else {
            continue;
        };
        let complete = if final_scan || text.ends_with('\n') {
            text.as_str()
        } else {
            text.rsplit_once('\n').map_or("", |(complete, _)| complete)
        };
        let lines: Vec<_> = complete
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect();
        let ready = if final_scan {
            lines.len()
        } else {
            lines.len().saturating_sub(1)
        };
        for (index, line) in lines.iter().take(ready).enumerate().skip(seen[source]) {
            let listed = PathBuf::from(line.trim());
            let input = if listed.is_absolute() {
                listed
            } else {
                list.parent().unwrap_or(root).join(listed)
            };
            let dir = root.join(format!("source-{source}"));
            let _ = std::fs::create_dir_all(&dir);
            let output = dir.join(format!("part-{index:06}.webp"));
            if jobs
                .try_send(LiveChunk {
                    source,
                    input,
                    output,
                })
                .is_ok()
            {
                seen[source] += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{scan_lists, LiveWebp};
    use crossbeam_channel::bounded;
    use mediaa_core::settings::AnimSettings;
    use mediaa_webp_pipeline::PipelineTuning;
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn live_scan_leaves_the_open_segment_for_the_final_scan() {
        let dir = tempfile::tempdir().unwrap();
        let chunks = dir.path().join("chunks");
        std::fs::create_dir_all(&chunks).unwrap();
        let list = chunks.join("chunks.txt");
        std::fs::write(
            &list,
            "chunk-000000.mkv\nchunk-000001.mkv\nchunk-000002.mkv\n",
        )
        .unwrap();
        let (jobs, queued) = bounded(64);
        let mut seen = [0];
        scan_lists(
            std::slice::from_ref(&list),
            &mut seen,
            dir.path(),
            &jobs,
            false,
        );
        assert_eq!(seen, [2]);
        let first = queued.try_recv().unwrap();
        let second = queued.try_recv().unwrap();
        assert_eq!(first.input, chunks.join("chunk-000000.mkv"));
        assert_eq!(second.input, chunks.join("chunk-000001.mkv"));
        assert!(queued.try_recv().is_err());

        scan_lists(
            std::slice::from_ref(&list),
            &mut seen,
            dir.path(),
            &jobs,
            true,
        );
        assert_eq!(seen, [3]);
        assert_eq!(
            queued.try_recv().unwrap().input,
            chunks.join("chunk-000002.mkv")
        );
        assert!(Path::new(&list).is_file());
    }

    #[test]
    fn missing_live_chunk_returns_an_error_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let list = dir.path().join("chunks.txt");
        std::fs::write(&list, "missing.mkv\n").unwrap();
        let live = LiveWebp::start(
            vec![list],
            dir.path().join("webp"),
            AnimSettings {
                fps: 10,
                max_height: 48,
                ..AnimSettings::default()
            },
            PipelineTuning {
                max_workers: 1,
                max_attempts: 1,
                retry_backoff: std::time::Duration::from_millis(1),
                ..PipelineTuning::default()
            },
            "webp-test".into(),
            (64, 48),
        )
        .unwrap();
        let result = live.finish(Arc::new(AtomicBool::new(false)));
        let error = result.error.expect("missing chunk must fail");
        assert!(error.contains("missing.mkv"), "{error}");
    }
}
