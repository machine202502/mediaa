use crate::logic::jobs::Job;
use crate::wake::Wake;
use mediaa_core::paths::OutputPath;
use mediaa_core::MediaKind;
use mediaa_media::convert::{self, ConvertSettings, Target};
use mediaa_media::error::{classify_media_error, MediaErrorKind};
use mediaa_media::info::MediaInfo;
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub(crate) struct ConversionJob {
    output: OutputPath,
    progress: Arc<AtomicI32>,
    started: Instant,
    pub(crate) job: Job<ConversionTaskResult>,
}

pub(crate) enum ConversionTaskResult {
    Completed,
    Failed {
        kind: MediaErrorKind,
        message: String,
    },
    Cancelled,
}

pub enum ConversionOutcome {
    Finished(OutputPath),
    Failed {
        kind: MediaErrorKind,
        message: String,
    },
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConversionUiAction {
    Error(String),
}

#[derive(Clone, Copy)]
pub(crate) struct ConversionStatus {
    pub progress: Option<f32>,
    pub elapsed: Duration,
    pub done: usize,
    pub total: usize,
}

#[derive(Clone)]
pub(crate) struct ProbeInfo {
    pub(crate) kind: MediaKind,
    pub(crate) info: MediaInfo,
    pub(crate) size: Option<u64>,
}

pub(crate) type ProbeResult = Result<ProbeInfo, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchStatus {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone)]
pub(crate) struct BatchItem {
    pub(crate) path: PathBuf,
    pub(crate) kind: Option<MediaKind>,
    pub(crate) size: Option<u64>,
    pub(crate) duration: Option<f64>,
    pub(crate) status: BatchStatus,
    pub(crate) error: Option<String>,
}

#[derive(Default)]
pub struct ConversionController {
    pub(crate) input: Option<PathBuf>,
    pub(crate) queue: Vec<PathBuf>,
    pub(crate) batch_total: usize,
    pub(crate) batch_done: usize,
    pub(crate) batch_auto: bool,
    pub(crate) failed: Vec<(PathBuf, String)>,
    pub(crate) batch: Vec<BatchItem>,
    pub(crate) conflict_notified: bool,
    pub(crate) probed: Option<ProbeResult>,
    pub(crate) probe_job: Option<Job<ProbeResult>>,
    pub(crate) probe_cancel: Arc<AtomicBool>,
    pub(crate) probe_loading: bool,
    pub(crate) target: Option<Target>,
    pub(crate) settings: ConvertSettings,
    pub(crate) max_height: u32,
    pub(crate) folder: Option<PathBuf>,
    pub(crate) job: Option<ConversionJob>,
    pub(crate) run_started: Option<Instant>,
    #[cfg(test)]
    pub(crate) test_elapsed: Option<Duration>,
}

impl ConversionController {
    pub fn set_input(&mut self, path: PathBuf) {
        self.set_inputs(vec![path]);
    }

    pub fn set_inputs(&mut self, paths: Vec<PathBuf>) {
        let mut unique = Vec::new();
        for path in paths {
            if !unique.contains(&path) {
                unique.push(path);
            }
        }
        let Some(first) = unique.first().cloned() else {
            return;
        };
        self.batch_total = unique.len();
        self.batch_done = 0;
        self.batch_auto = false;
        self.failed.clear();
        self.conflict_notified = false;
        self.queue = unique.iter().skip(1).cloned().collect();
        self.batch = unique
            .iter()
            .map(|path| BatchItem {
                path: path.clone(),
                kind: MediaKind::detect(std::path::Path::new(path)),
                size: std::fs::metadata(path).ok().map(|metadata| metadata.len()),
                duration: None,
                status: BatchStatus::Pending,
                error: None,
            })
            .collect();
        self.target = None;
        self.set_current(first);
    }

    pub fn add_inputs(&mut self, paths: Vec<PathBuf>) {
        let mut unique = Vec::new();
        for path in paths {
            if !unique.contains(&path) {
                unique.push(path);
            }
        }
        if unique.is_empty() {
            return;
        }
        if self.batch.is_empty() {
            self.set_inputs(unique);
            return;
        }
        let first = self.input.is_none().then(|| unique[0].clone());
        for path in unique {
            if self.batch.iter().any(|item| item.path == path) {
                continue;
            }
            self.batch.push(BatchItem {
                kind: MediaKind::detect(&path),
                size: std::fs::metadata(&path).ok().map(|metadata| metadata.len()),
                duration: None,
                status: BatchStatus::Pending,
                error: None,
                path,
            });
        }
        self.batch_total = self.batch.len();
        self.conflict_notified = false;
        if let Some(path) = first {
            self.set_current(path);
        }
    }
    pub(crate) fn set_current(&mut self, path: PathBuf) {
        if let Some(job) = self.probe_job.take() {
            job.cancel();
        }
        self.probe_cancel = Arc::default();
        self.input = Some(path.clone());
        if self.batch_auto {
            self.probe_loading = true;
        } else {
            self.probe_loading = false;
            self.probed = None;
        }
        if MediaKind::detect(&path).is_none() {
            self.probed = Some(Err(t!("convert.unsupported").into()));
            self.probe_loading = false;
        }
    }

    pub(crate) fn mark_batch(
        &mut self,
        path: &std::path::Path,
        status: BatchStatus,
        error: Option<String>,
    ) {
        if let Some(item) = self.batch.iter_mut().find(|item| item.path == path) {
            item.status = status;
            item.error = error;
        }
    }

    pub(crate) fn retry_batch(&mut self, path: &std::path::Path) {
        if let Some(item) = self.batch.iter_mut().find(|item| item.path == path) {
            item.status = BatchStatus::Pending;
            item.error = None;
        }
        self.failed.retain(|(failed, _)| failed != path);
        self.queue.retain(|queued| queued != path);
        self.conflict_notified = false;
        self.batch_auto = false;
        self.target = None;
        self.set_current(path.to_path_buf());
    }

    pub(crate) fn remove_batch(&mut self, path: &std::path::Path) {
        self.batch.retain(|item| item.path != path);
        self.queue.retain(|queued| queued != path);
        self.failed.retain(|(failed, _)| failed != path);
        self.batch_total = self.batch.len();
        self.conflict_notified = false;
        if self.input.as_deref() == Some(path) {
            self.input = None;
            if let Some(next) = self.queue.first().cloned() {
                self.queue.remove(0);
                self.set_current(next);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_status(&mut self, progress: i32, elapsed: Duration) {
        if let Some(job) = &mut self.job {
            job.progress.store(progress, Ordering::Relaxed);
            job.started = Instant::now() - elapsed;
        }
        self.test_elapsed = Some(elapsed);
    }
    #[cfg(test)]
    pub(crate) fn test_busy(&mut self, progress: i32, elapsed: Duration) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let job = Job::spawn("convert-test-busy", cancel.clone(), move || {
            while !worker_cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            ConversionTaskResult::Cancelled
        })
        .unwrap();
        self.run_started = Some(Instant::now() - elapsed);
        self.test_elapsed = Some(elapsed);
        self.job = Some(ConversionJob {
            output: OutputPath::new("convert-status.mp4"),
            progress: Arc::new(AtomicI32::new(progress)),
            started: Instant::now() - elapsed,
            job,
        });
        cancel
    }
    #[cfg(test)]
    pub(crate) fn test_batch_statuses(&self) -> Vec<BatchStatus> {
        self.batch.iter().map(|item| item.status).collect()
    }
    #[cfg(test)]
    pub(crate) fn test_batch_is_empty(&self) -> bool {
        self.batch.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn test_folder(&self) -> Option<&std::path::Path> {
        self.folder.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn test_batch(
        &mut self,
        items: &[(&str, BatchStatus, Option<&str>)],
        target: Option<Target>,
    ) {
        let paths: Vec<PathBuf> = items
            .iter()
            .map(|(path, _, _)| PathBuf::from(path))
            .collect();
        self.batch = items
            .iter()
            .map(|(path, status, error)| BatchItem {
                path: PathBuf::from(path),
                kind: MediaKind::detect(std::path::Path::new(path)),
                size: Some(1024),
                duration: Some(12.0),
                status: *status,
                error: error.map(str::to_string),
            })
            .collect();
        self.batch_total = self.batch.len();
        self.batch_done = self
            .batch
            .iter()
            .filter(|item| matches!(item.status, BatchStatus::Done | BatchStatus::Failed))
            .count();
        self.input = paths.first().cloned();
        self.queue = paths.into_iter().skip(1).collect();
        self.target = target;
        self.probed = self.input.as_ref().map(|_| {
            Ok(ProbeInfo {
                kind: self
                    .batch
                    .first()
                    .and_then(|item| item.kind)
                    .unwrap_or(MediaKind::Video),
                info: MediaInfo {
                    duration_us: Some(12_000_000),
                    width: 1920,
                    height: 1080,
                    fps: 30.0,
                    video_codec: Some("h264".into()),
                    audio_codec: Some("aac".into()),
                    audio_streams: 1,
                },
                size: Some(1024),
            })
        });
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some() || self.batch_auto
    }

    pub(crate) fn can_start(&self) -> bool {
        !self.is_busy()
            && self.input.is_some()
            && self.target.is_some()
            && self
                .batch
                .iter()
                .any(|item| item.status != BatchStatus::Done)
    }

    fn elapsed(&self) -> Duration {
        #[cfg(test)]
        if let Some(frozen) = self.test_elapsed {
            return frozen;
        }
        self.run_started
            .or_else(|| self.job.as_ref().map(|job| job.started))
            .map_or(Duration::ZERO, |started| started.elapsed())
    }
    pub(crate) fn status(&self) -> Option<ConversionStatus> {
        if self.job.is_none() && !self.batch_auto {
            return None;
        }
        let total = self.batch_total.max(1);
        let done = self.batch_done.min(total);
        let current = self.job.as_ref().map_or(0.0, |job| {
            let value = job.progress.load(Ordering::Relaxed);
            if value >= 0 {
                value as f32 / 1000.0
            } else {
                0.0
            }
        });
        let progress = ((done as f32 + current) / total as f32).clamp(0.0, 1.0);
        let elapsed = self.elapsed();
        Some(ConversionStatus {
            progress: Some(progress),
            elapsed,
            done,
            total,
        })
    }

    pub(crate) fn cancel(&mut self) {
        self.batch_auto = false;
        self.queue.clear();
        if let Some(job) = &self.job {
            job.job.cancel();
        }
    }

    pub(crate) fn start_from_ui(
        &mut self,
        wake: &Wake,
        convert_dir: &std::path::Path,
    ) -> Option<String> {
        if !self.can_start() {
            return None;
        }
        if !self.prepare_batch_run() {
            return None;
        }
        let target = self.target?;
        self.start_job(wake, target, convert_dir)
    }

    pub(crate) fn prepare_batch_run(&mut self) -> bool {
        let mut paths = Vec::new();
        for item in &self.batch {
            if item.status != BatchStatus::Done {
                paths.push(item.path.clone());
            }
        }
        let Some(first) = paths.first().cloned() else {
            return false;
        };
        self.input = Some(first);
        self.queue = paths.into_iter().skip(1).collect();
        self.batch_done = self
            .batch
            .iter()
            .filter(|item| item.status == BatchStatus::Done)
            .count();
        self.failed.clear();
        self.batch_auto = false;
        self.conflict_notified = false;
        self.run_started = Some(Instant::now());
        true
    }

    pub(crate) fn reset_batch_statuses(&mut self) {
        for item in &mut self.batch {
            item.status = BatchStatus::Pending;
            item.error = None;
        }
        self.batch_done = 0;
        self.failed.clear();
        self.queue.clear();
        self.batch_auto = false;
        self.conflict_notified = false;
        self.run_started = None;
    }

    pub(crate) fn clear_batch(&mut self) {
        if self.is_busy() {
            return;
        }
        if let Some(job) = self.probe_job.take() {
            job.cancel();
        }
        self.probe_cancel = Arc::default();
        self.input = None;
        self.queue.clear();
        self.batch.clear();
        self.batch_total = 0;
        self.batch_done = 0;
        self.batch_auto = false;
        self.failed.clear();
        self.conflict_notified = false;
        self.probed = None;
        self.target = None;
        self.run_started = None;
    }

    pub(crate) fn poll_probe(&mut self, wake: &Wake) {
        if self.probe_job.is_none() && (self.probed.is_none() || self.probe_loading) {
            let Some(path) = self.input.clone() else {
                return;
            };
            let Some(kind) = MediaKind::detect(&path) else {
                self.probed = Some(Err(t!("convert.unsupported").into()));
                return;
            };
            let cancel = self.probe_cancel.clone();
            let worker_cancel = cancel.clone();
            match Job::spawn("convert-probe", cancel, move || {
                mediaa_media::preview::details_cancellable(&path, kind, &worker_cancel)
                    .map(|info| ProbeInfo {
                        kind,
                        info,
                        size: std::fs::metadata(&path).ok().map(|metadata| metadata.len()),
                    })
                    .map_err(|error| format!("{error:#}"))
            }) {
                Ok(job) => self.probe_job = Some(job),
                Err(error) => {
                    self.probed = Some(Err(format!("probe: {error}")));
                    self.probe_loading = false;
                }
            }
        }
        let Some(job) = self.probe_job.as_mut() else {
            return;
        };
        let Some(result) = job.poll() else {
            wake.after(Duration::from_millis(50));
            return;
        };
        self.probe_job = None;
        self.probe_loading = false;
        self.probed = Some(result.unwrap_or_else(|_| Err(t!("convert.probe_stopped").into())));
        if let Some(Ok(probe)) = self.probed.clone() {
            if let Some(path) = self.input.clone() {
                if let Some(item) = self.batch.iter_mut().find(|item| item.path == path) {
                    item.kind = Some(probe.kind);
                    item.size = probe.size;
                    item.duration = probe.info.duration_secs();
                }
            }
        }
        let targets = self.targets();
        if self.target.is_some_and(|target| !targets.contains(&target)) {
            self.target = None;
        }
    }

    pub(crate) fn poll_job(&mut self, wake: &Wake) -> Option<ConversionOutcome> {
        let job = self.job.as_mut()?;
        let Some(result) = job.job.poll() else {
            wake.after(Duration::from_millis(100));
            return None;
        };
        let job = self.job.take().expect("job checked above");
        let outcome = match result {
            Err(_) => ConversionOutcome::Failed {
                kind: MediaErrorKind::Internal,
                message: t!("convert.worker_stopped").into(),
            },
            Ok(ConversionTaskResult::Completed) => ConversionOutcome::Finished(job.output),
            Ok(ConversionTaskResult::Cancelled) => {
                let path = job.output.clone().into_path();
                let _ = std::fs::remove_file(path);
                ConversionOutcome::Cancelled
            }
            Ok(ConversionTaskResult::Failed { kind, message }) => {
                ConversionOutcome::Failed { kind, message }
            }
        };
        if let Some(path) = self.input.clone() {
            match &outcome {
                ConversionOutcome::Finished(_) => {
                    self.mark_batch(&path, BatchStatus::Done, None);
                }
                ConversionOutcome::Failed { message, .. } => {
                    self.failed.push((path.clone(), message.clone()));
                    self.mark_batch(&path, BatchStatus::Failed, Some(message.clone()));
                }
                ConversionOutcome::Cancelled => {
                    self.mark_batch(&path, BatchStatus::Cancelled, None);
                }
            }
        }
        self.batch_done += 1;
        if !matches!(outcome, ConversionOutcome::Cancelled) {
            if let Some(next) = self.queue.first().cloned() {
                self.queue.remove(0);
                self.batch_auto = true;
                self.set_current(next);
            }
        } else {
            self.queue.clear();
            self.batch_auto = false;
        }
        Some(outcome)
    }

    pub(crate) fn start_job(
        &mut self,
        wake: &Wake,
        target: Target,
        convert_dir: &std::path::Path,
    ) -> Option<String> {
        let input = self.input.clone()?;
        self.mark_batch(&input, BatchStatus::Running, None);
        let folder = self
            .folder
            .clone()
            .unwrap_or_else(|| convert_dir.to_path_buf());
        let output = convert::output_path(
            &mediaa_core::paths::SourcePath::new(input.clone()),
            target,
            None,
            &mediaa_core::paths::MediaDir::new(folder),
        );
        let settings = self.effective();
        self.batch_auto = false;
        let progress = Arc::new(AtomicI32::new(-1));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_output = output.clone();
        let worker_cancel = cancel.clone();
        let worker_progress = progress.clone();
        let worker_wake = wake.clone();
        let job = Job::spawn("convert", cancel, move || {
            let mut report = |value: Option<f32>| {
                worker_progress.store(
                    value.map_or(-1, |value| (value * 1000.0) as i32),
                    Ordering::Relaxed,
                );
                worker_wake.now();
            };
            let source = mediaa_core::paths::SourcePath::new(input);
            match convert::convert(
                &source,
                &worker_output,
                target,
                &settings,
                &mut report,
                &worker_cancel,
            ) {
                Ok(()) => ConversionTaskResult::Completed,
                Err(error) if mediaa_ffmpeg::job::is_cancelled(&error) => {
                    ConversionTaskResult::Cancelled
                }
                Err(error) => ConversionTaskResult::Failed {
                    kind: classify_media_error(&error),
                    message: format!("{error:#}"),
                },
            }
        });
        match job {
            Ok(job) => {
                self.job = Some(ConversionJob {
                    output,
                    progress,
                    started: Instant::now(),
                    job,
                });
                None
            }
            Err(error) => Some(t!("convert.start_failed", error = error).into()),
        }
    }

    pub(crate) fn effective(&self) -> ConvertSettings {
        ConvertSettings {
            max_height: (self.max_height > 0).then_some(self.max_height),
            ..self.settings.clone()
        }
    }

    pub(crate) fn targets(&self) -> Vec<Target> {
        if !self.queue.is_empty() {
            let mut paths = self.queue.clone();
            if let Some(input) = &self.input {
                paths.insert(0, input.clone());
            }
            let mut common: Option<Vec<Target>> = None;
            for path in paths {
                let Some(kind) = MediaKind::detect(&path) else {
                    return Vec::new();
                };
                let targets = Target::for_kind(kind);
                common = Some(match common {
                    None => targets,
                    Some(previous) => previous
                        .into_iter()
                        .filter(|target| targets.contains(target))
                        .collect(),
                });
            }
            return common.unwrap_or_default();
        }
        match &self.probed {
            Some(Ok(probe)) => Target::for_kind(probe.kind)
                .into_iter()
                .filter(|t| !matches!(t, Target::Audio(_)) || probe.info.has_audio())
                .collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
