mod webp_parts;

use anyhow::Context;
use crossbeam_channel::{unbounded, RecvTimeoutError};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

pub const DEFAULT_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
pub const DEFAULT_ENCODER_BYTES: u64 = 48 * 1024 * 1024;
pub const DEFAULT_MAX_WORKERS: usize = 8;
pub const DEFAULT_MAX_ATTEMPTS: usize = 6;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineTuning {
    pub memory_bytes: u64,
    pub max_workers: usize,
    pub max_attempts: usize,
    pub min_segment_frames: usize,
    pub max_segment_frames: Option<usize>,
    pub fixed_encoder_bytes: u64,
    pub retry_backoff: Duration,
}

impl Default for PipelineTuning {
    fn default() -> Self {
        Self {
            memory_bytes: DEFAULT_MEMORY_BYTES,
            max_workers: DEFAULT_MAX_WORKERS,
            max_attempts: DEFAULT_MAX_ATTEMPTS,
            min_segment_frames: 1,
            max_segment_frames: None,
            fixed_encoder_bytes: DEFAULT_ENCODER_BYTES,
            retry_backoff: Duration::from_millis(100),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipelineConfig {
    pub total_us: i64,
    pub fps: u32,
    pub width: u32,
    pub height: u32,
    pub quality: u8,
    pub memory_bytes: u64,
    pub max_workers: usize,
    pub max_attempts: usize,
    pub min_segment_frames: usize,
    pub max_segment_frames: Option<usize>,
    pub fixed_encoder_bytes: u64,
    pub retry_backoff: Duration,
    pub trace_id: Option<String>,
}

impl PipelineConfig {
    pub fn new(total_us: i64, fps: u32, width: u32, height: u32, quality: u8) -> Self {
        let defaults = PipelineTuning::default();
        Self {
            total_us,
            fps,
            width,
            height,
            quality,
            memory_bytes: defaults.memory_bytes,
            max_workers: defaults.max_workers,
            max_attempts: defaults.max_attempts,
            min_segment_frames: defaults.min_segment_frames,
            max_segment_frames: defaults.max_segment_frames,
            fixed_encoder_bytes: defaults.fixed_encoder_bytes,
            retry_backoff: defaults.retry_backoff,
            trace_id: None,
        }
    }

    pub fn with_tuning(mut self, tuning: &PipelineTuning) -> Self {
        self.memory_bytes = tuning.memory_bytes;
        self.max_workers = tuning.max_workers;
        self.max_attempts = tuning.max_attempts;
        self.min_segment_frames = tuning.min_segment_frames;
        self.max_segment_frames = tuning.max_segment_frames;
        self.fixed_encoder_bytes = tuning.fixed_encoder_bytes;
        self.retry_backoff = tuning.retry_backoff;
        self
    }

    pub fn with_memory_bytes(mut self, memory_bytes: u64) -> Self {
        self.memory_bytes = memory_bytes;
        self
    }

    pub fn with_workers(mut self, max_workers: usize) -> Self {
        self.max_workers = max_workers;
        self
    }

    pub fn with_attempts(mut self, max_attempts: usize) -> Self {
        self.max_attempts = max_attempts;
        self
    }

    pub fn with_segment_frames(
        mut self,
        min_segment_frames: usize,
        max_segment_frames: Option<usize>,
    ) -> Self {
        self.min_segment_frames = min_segment_frames;
        self.max_segment_frames = max_segment_frames;
        self
    }

    pub fn with_retry_backoff(mut self, retry_backoff: Duration) -> Self {
        self.retry_backoff = retry_backoff;
        self
    }

    pub fn with_trace_id(mut self, trace_id: Option<&str>) -> Self {
        self.trace_id = trace_id.map(str::to_string);
        self
    }

    fn normalized(&self) -> Self {
        let fps = self.fps.clamp(1, 60);
        let width = self.width.max(1);
        let height = self.height.max(1);
        let quality = self.quality.clamp(1, 100);
        let frame_bytes = frame_memory(width, height, quality);
        let fixed_encoder_bytes = self.fixed_encoder_bytes;
        let memory_bytes = self
            .memory_bytes
            .max(fixed_encoder_bytes.saturating_add(frame_bytes));
        Self {
            total_us: self.total_us,
            fps,
            width,
            height,
            quality,
            memory_bytes,
            max_workers: self.max_workers.max(1),
            max_attempts: self.max_attempts.max(1),
            min_segment_frames: self.min_segment_frames.max(1),
            max_segment_frames: self.max_segment_frames.map(|frames| frames.max(1)),
            fixed_encoder_bytes,
            retry_backoff: self.retry_backoff,
            trace_id: self.trace_id.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentPlan {
    pub index: usize,
    pub start_us: i64,
    pub len_us: i64,
    pub frames: usize,
    pub estimated_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub id: usize,
    pub start_us: i64,
    pub len_us: i64,
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Degradation {
    pub attempt: usize,
    pub workers: usize,
    pub max_frames: usize,
    pub error: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunReport {
    pub planned_segments: usize,
    pub completed_segments: usize,
    pub attempts: usize,
    pub peak_reserved_bytes: u64,
    pub workers: usize,
    pub max_frames: usize,
    pub degradations: Vec<Degradation>,
}

pub trait SegmentBackend: Sync {
    fn encode(&self, segment: &Segment, cancel: &AtomicBool) -> anyhow::Result<()>;
}

pub fn merge_parts(parts: &[PathBuf], output: &Path) -> anyhow::Result<()> {
    webp_parts::merge_animation(parts, output)
}

pub fn validate_part(path: &Path) -> anyhow::Result<()> {
    webp_parts::validate_animation(path)
}

pub fn normalize_part(path: &Path, duration_us: i64) -> anyhow::Result<()> {
    webp_parts::normalize_animation(path, duration_us)
}

#[derive(Debug)]
pub struct Cancelled;

impl fmt::Display for Cancelled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("cancelled")
    }
}

impl std::error::Error for Cancelled {}

#[derive(Debug)]
pub struct PipelineError {
    pub report: RunReport,
    pub source: anyhow::Error,
}

impl PipelineError {
    pub fn is_cancelled(&self) -> bool {
        self.source.downcast_ref::<Cancelled>().is_some()
    }
}

impl fmt::Display for PipelineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:#}", self.source)
    }
}

impl std::error::Error for PipelineError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.source.as_ref())
    }
}

pub fn plan(config: &PipelineConfig) -> Vec<SegmentPlan> {
    let config = config.normalized();
    if config.total_us <= 0 {
        return Vec::new();
    }
    let frame_cost = frame_memory(config.width, config.height, config.quality);
    let frames = initial_frames(&config, frame_cost);
    let total_frames =
        ((config.total_us as u128 * u128::from(config.fps)).div_ceil(1_000_000)).max(1);
    let mut segments = Vec::new();
    let mut first = 0u128;
    let mut index = 0usize;
    while first < total_frames {
        let end = (first + frames as u128).min(total_frames);
        let start_us = (first * 1_000_000 / u128::from(config.fps)) as i64;
        let end_us = (end * 1_000_000 / u128::from(config.fps)).min(config.total_us as u128) as i64;
        if end_us > start_us {
            let frames = usize::try_from(end - first).unwrap_or(usize::MAX);
            segments.push(SegmentPlan {
                index,
                start_us,
                len_us: end_us - start_us,
                frames,
                estimated_bytes: segment_memory(&config, frame_cost, frames),
            });
            index += 1;
        }
        first = end;
    }
    segments
}

pub fn recommended_workers(config: &PipelineConfig) -> usize {
    let config = config.normalized();
    let frame_cost = frame_memory(config.width, config.height, config.quality);
    worker_limit(
        &config,
        initial_frames(&config, frame_cost),
        config.max_workers,
    )
}

pub fn run(
    config: &PipelineConfig,
    backend: &dyn SegmentBackend,
    output: &Path,
    on_progress: &mut dyn FnMut(Option<f32>),
    cancel: &AtomicBool,
) -> Result<RunReport, PipelineError> {
    let config = config.normalized();
    let initial = plan(&config);
    let mut report = RunReport {
        planned_segments: initial.len(),
        workers: worker_limit(
            &config,
            initial.first().map_or(1, |segment| segment.frames),
            initial.len(),
        ),
        max_frames: initial.first().map_or(1, |segment| segment.frames),
        ..RunReport::default()
    };
    if initial.is_empty() {
        return Err(PipelineError {
            report,
            source: anyhow::anyhow!("WebP source has no duration"),
        });
    }
    let dir = tempfile::tempdir().map_err(|error| PipelineError {
        report: report.clone(),
        source: error.into(),
    })?;
    let mut schedule = Schedule::new(config, initial, dir.path(), report.workers);
    let trace = schedule
        .config
        .trace_id
        .clone()
        .unwrap_or_else(|| "webp".to_string());
    log::info!(
        "[{trace}] stage=pipeline start segments={} workers={} frames={}",
        report.planned_segments,
        report.workers,
        report.max_frames
    );
    on_progress(None);
    if let Err(error) = schedule.run(on_progress, cancel, backend) {
        report.completed_segments = schedule.completed.len();
        report.attempts = schedule.attempts;
        report.peak_reserved_bytes = schedule.peak_reserved_bytes;
        report.workers = schedule.workers;
        report.max_frames = schedule.max_frames;
        report.degradations = schedule.degradations;
        return Err(PipelineError {
            report,
            source: error,
        });
    }
    schedule.completed.sort_by_key(|segment| segment.start_us);
    on_progress(Some(0.97));
    webp_parts::merge_animation(
        &schedule
            .completed
            .iter()
            .map(|segment| segment.path.clone())
            .collect::<Vec<_>>(),
        output,
    )
    .map_err(|source| PipelineError {
        report: report.clone(),
        source,
    })?;
    log::info!(
        "[{trace}] stage=pipeline complete segments={} attempts={}",
        schedule.completed.len(),
        schedule.attempts
    );
    on_progress(Some(1.0));
    report.completed_segments = schedule.completed.len();
    report.attempts = schedule.attempts;
    report.peak_reserved_bytes = schedule.peak_reserved_bytes;
    report.workers = schedule.worker_limit();
    report.max_frames = schedule.max_frames;
    report.degradations = schedule.degradations;
    Ok(report)
}

fn frame_memory(width: u32, height: u32, quality: u8) -> u64 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    pixels
        .saturating_mul(u64::from(quality))
        .div_ceil(100)
        .max(1)
}

fn segment_memory(config: &PipelineConfig, frame_cost: u64, frames: usize) -> u64 {
    config
        .fixed_encoder_bytes
        .saturating_add(frame_cost.saturating_mul(frames as u64))
}

fn initial_frames(config: &PipelineConfig, frame_cost: u64) -> usize {
    let available = config
        .memory_bytes
        .saturating_sub(config.fixed_encoder_bytes);
    let by_memory = usize::try_from((available / frame_cost).max(1)).unwrap_or(usize::MAX);
    let requested = config.max_segment_frames.unwrap_or(config.fps as usize);
    requested
        .min(by_memory)
        .max(config.min_segment_frames)
        .max(1)
}

fn worker_limit(config: &PipelineConfig, frames: usize, segments: usize) -> usize {
    let cost = segment_memory(
        config,
        frame_memory(config.width, config.height, config.quality),
        frames,
    );
    let by_memory = (config.memory_bytes / cost).max(1) as usize;
    let cpus = std::thread::available_parallelism().map_or(1, |cpus| cpus.get());
    let by_cpu = cpus.saturating_sub(cpus / 4);
    by_cpu
        .min(by_memory)
        .min(config.max_workers)
        .min(segments.max(1))
        .max(1)
}

struct Schedule {
    config: PipelineConfig,
    frame_cost: u64,
    pending: Vec<Segment>,
    completed: Vec<Segment>,
    workers: usize,
    max_frames: usize,
    next_id: usize,
    attempts: usize,
    peak_reserved_bytes: u64,
    degradations: Vec<Degradation>,
}

impl Schedule {
    fn new(config: PipelineConfig, plan: Vec<SegmentPlan>, dir: &Path, workers: usize) -> Self {
        let frame_cost = frame_memory(config.width, config.height, config.quality);
        let max_frames = plan.first().map_or(1, |segment| segment.frames);
        let next_id = plan.len();
        let pending = plan
            .into_iter()
            .map(|segment| Segment {
                id: segment.index,
                start_us: segment.start_us,
                len_us: segment.len_us,
                path: dir.join(format!("part-{:06}.webp", segment.index)),
            })
            .collect();
        Self {
            config,
            frame_cost,
            pending,
            completed: Vec::new(),
            workers,
            max_frames,
            next_id,
            attempts: 0,
            peak_reserved_bytes: 0,
            degradations: Vec::new(),
        }
    }

    fn worker_limit(&self) -> usize {
        worker_limit(
            &self.config,
            self.max_frames,
            self.pending.len().max(self.completed.len()),
        )
    }

    fn frames(&self, segment: &Segment) -> usize {
        ((segment.len_us.max(1) as u128 * u128::from(self.config.fps)).div_ceil(1_000_000)).max(1)
            as usize
    }

    fn split_pending(&mut self) {
        let mut split = Vec::new();
        for segment in std::mem::take(&mut self.pending) {
            let frames = self.frames(&segment);
            if frames <= self.max_frames || frames <= 1 {
                split.push(segment);
                continue;
            }
            let _ = std::fs::remove_file(&segment.path);
            let left_frames = (frames / 2).max(1);
            let mid_us = (left_frames as u128 * 1_000_000 / u128::from(self.config.fps)) as i64;
            for (start_us, len_us) in [
                (segment.start_us, mid_us),
                (segment.start_us + mid_us, segment.len_us - mid_us),
            ] {
                if len_us <= 0 {
                    continue;
                }
                let id = self.next_id;
                self.next_id += 1;
                split.push(Segment {
                    id,
                    start_us,
                    len_us,
                    path: segment.path.with_file_name(format!("part-{id:06}.webp")),
                });
            }
        }
        self.pending = split;
    }

    fn round(
        &self,
        base_completed_us: i64,
        on_progress: &mut dyn FnMut(Option<f32>),
        cancel: &AtomicBool,
        backend: &dyn SegmentBackend,
    ) -> Round {
        let (work_tx, work_rx) = unbounded();
        let (result_tx, result_rx) = unbounded();
        for segment in &self.pending {
            let _ = work_tx.send(segment.clone());
        }
        drop(work_tx);
        let budget = MemoryBudget::new(self.config.memory_bytes);
        let stop = AtomicBool::new(false);
        let workers = self.workers.clamp(1, self.pending.len().max(1));
        std::thread::scope(|scope| {
            for _ in 0..workers {
                let work_rx = work_rx.clone();
                let result_tx = result_tx.clone();
                let budget = &budget;
                let stop = &stop;
                scope.spawn(move || {
                    while let Ok(segment) = work_rx.recv() {
                        if stop.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let frames = ((segment.len_us.max(1) as u128
                            * u128::from(self.config.fps))
                        .div_ceil(1_000_000))
                        .max(1);
                        let bytes = self.config.fixed_encoder_bytes.saturating_add(
                            self.frame_cost
                                .saturating_mul(frames.min(u128::from(u64::MAX)) as u64),
                        );
                        let Some(_reservation) = budget.reserve(bytes) else {
                            stop.store(true, Ordering::Relaxed);
                            let _ = result_tx.send((
                                segment,
                                Err(anyhow::anyhow!("WebP memory budget exhausted")),
                            ));
                            break;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            backend.encode(&segment, stop)
                        }))
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("WebP segment worker panicked")))
                        .and_then(|()| {
                            webp_parts::normalize_animation(&segment.path, segment.len_us)
                        })
                        .and_then(|()| webp_parts::validate_animation(&segment.path));
                        if result.is_err() {
                            stop.store(true, Ordering::Relaxed);
                        }
                        let failed = result.is_err();
                        if result_tx.send((segment, result)).is_err() || failed {
                            break;
                        }
                    }
                });
            }
            drop(result_tx);

            let mut round = Round::default();
            let mut completed_us = base_completed_us;
            loop {
                if cancel.load(Ordering::Relaxed) {
                    stop.store(true, Ordering::Relaxed);
                    round.failure = Some(Cancelled.into());
                    break;
                }
                match result_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok((segment, Ok(()))) => {
                        completed_us += segment.len_us;
                        on_progress(Some(
                            0.95 * completed_us as f32 / self.config.total_us as f32,
                        ));
                        round.completed.push(segment);
                    }
                    Ok((_, Err(error)))
                        if error.downcast_ref::<Cancelled>().is_some()
                            && !cancel.load(Ordering::Relaxed) => {}
                    Ok((_, Err(error))) => {
                        stop.store(true, Ordering::Relaxed);
                        round.failure = Some(error);
                        break;
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
            stop.store(true, Ordering::Relaxed);
            round.peak_reserved = budget.peak();
            round
        })
    }

    fn run(
        &mut self,
        on_progress: &mut dyn FnMut(Option<f32>),
        cancel: &AtomicBool,
        backend: &dyn SegmentBackend,
    ) -> anyhow::Result<()> {
        let mut last_error = None;
        for attempt in 0..self.config.max_attempts {
            if self.pending.is_empty() {
                return Ok(());
            }
            self.attempts += 1;
            let completed_us = self.completed.iter().map(|segment| segment.len_us).sum();
            let round = self.round(completed_us, on_progress, cancel, backend);
            self.peak_reserved_bytes = self.peak_reserved_bytes.max(round.peak_reserved);
            let completed_ids: Vec<_> = round.completed.iter().map(|segment| segment.id).collect();
            self.pending
                .retain(|segment| !completed_ids.contains(&segment.id));
            self.completed.extend(round.completed);
            if self.pending.is_empty() {
                return Ok(());
            }
            if cancel.load(Ordering::Relaxed)
                || round
                    .failure
                    .as_ref()
                    .is_some_and(|error| error.downcast_ref::<Cancelled>().is_some())
            {
                return Err(Cancelled.into());
            }
            let error = round
                .failure
                .unwrap_or_else(|| anyhow::anyhow!("WebP segment workers stopped"));
            self.workers = (self.workers / 2).max(1);
            self.max_frames = (self.max_frames / 2).max(1);
            self.split_pending();
            self.degradations.push(Degradation {
                attempt: attempt + 1,
                workers: self.workers,
                max_frames: self.max_frames,
                error: format!("{error:#}"),
            });
            let trace = self
                .config
                .trace_id
                .as_deref()
                .map_or("webp", |trace| trace);
            log::warn!(
                "[{trace}] segment failed; retry {} with {} workers and at most {} frames: {error:#}",
                attempt + 1,
                self.workers,
                self.max_frames
            );
            last_error = Some(error);
            std::thread::sleep(self.config.retry_backoff * (attempt as u32 + 1));
        }
        let trace = self
            .config
            .trace_id
            .as_deref()
            .map_or("webp", |trace| trace);
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("WebP segment schedule stopped"))).context(
            format!(
                "[{trace}] WebP encoding failed after {} attempts",
                self.config.max_attempts
            ),
        )
    }
}

#[derive(Default)]
struct Round {
    completed: Vec<Segment>,
    failure: Option<anyhow::Error>,
    peak_reserved: u64,
}

struct MemoryBudget {
    capacity: u64,
    used: AtomicU64,
    peak: AtomicU64,
}

struct MemoryReservation<'a> {
    budget: &'a MemoryBudget,
    bytes: u64,
}

impl MemoryBudget {
    fn new(capacity: u64) -> Self {
        Self {
            capacity,
            used: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        }
    }

    fn reserve(&self, bytes: u64) -> Option<MemoryReservation<'_>> {
        if bytes > self.capacity {
            return None;
        }
        let mut used = self.used.load(Ordering::Relaxed);
        loop {
            let next = used.checked_add(bytes)?;
            if next > self.capacity {
                return None;
            }
            match self
                .used
                .compare_exchange_weak(used, next, Ordering::AcqRel, Ordering::Relaxed)
            {
                Ok(_) => {
                    self.peak.fetch_max(next, Ordering::AcqRel);
                    return Some(MemoryReservation {
                        budget: self,
                        bytes,
                    });
                }
                Err(actual) => used = actual,
            }
        }
    }

    fn peak(&self) -> u64 {
        self.peak.load(Ordering::Acquire)
    }
}

impl Drop for MemoryReservation<'_> {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct FlakyBackend {
        calls: AtomicUsize,
    }

    impl SegmentBackend for FlakyBackend {
        fn encode(&self, segment: &Segment, _cancel: &AtomicBool) -> anyhow::Result<()> {
            let call = self.calls.fetch_add(1, Ordering::Relaxed);
            if call == 0 {
                anyhow::bail!("Memory allocation error during encoding");
            }
            std::fs::write(
                &segment.path,
                webp_parts::synthetic_animation(call.min(255) as u8),
            )?;
            Ok(())
        }
    }

    struct ValidBackend;

    impl SegmentBackend for ValidBackend {
        fn encode(&self, segment: &Segment, _cancel: &AtomicBool) -> anyhow::Result<()> {
            std::fs::write(
                &segment.path,
                webp_parts::synthetic_animation(segment.id as u8),
            )?;
            Ok(())
        }
    }

    struct FailingBackend;

    impl SegmentBackend for FailingBackend {
        fn encode(&self, _segment: &Segment, _cancel: &AtomicBool) -> anyhow::Result<()> {
            anyhow::bail!("persistent encoder failure")
        }
    }

    struct InvalidBackend;

    impl SegmentBackend for InvalidBackend {
        fn encode(&self, segment: &Segment, _cancel: &AtomicBool) -> anyhow::Result<()> {
            std::fs::write(&segment.path, b"not webp")?;
            Ok(())
        }
    }

    struct StaticBackend;

    impl SegmentBackend for StaticBackend {
        fn encode(&self, segment: &Segment, _cancel: &AtomicBool) -> anyhow::Result<()> {
            std::fs::write(
                &segment.path,
                webp_parts::synthetic_still(9, 10, segment.id as u8),
            )?;
            Ok(())
        }
    }

    fn config() -> PipelineConfig {
        PipelineConfig::new(1_000_000, 10, 640, 480, 80)
            .with_memory_bytes(256 * 1024 * 1024)
            .with_workers(4)
            .with_segment_frames(1, Some(10))
    }

    #[test]
    fn plan_uses_the_configured_limits() {
        let segments = plan(&config());
        assert_eq!(segments.len(), 1);
        assert_eq!(segments[0].frames, 10);
        assert_eq!(segments[0].len_us, 1_000_000);
    }

    #[test]
    fn budget_blocks_reservations_over_the_limit() {
        let budget = MemoryBudget::new(100);
        let first = budget.reserve(60).unwrap();
        assert!(budget.reserve(41).is_none());
        drop(first);
        assert_eq!(budget.peak(), 60);
        assert!(budget.reserve(80).is_some());
    }

    #[test]
    fn failed_segment_is_split_and_retried() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        let backend = FlakyBackend {
            calls: AtomicUsize::new(0),
        };
        let mut schedule = Schedule::new(config(), plan(&config()), dir.path(), 2);
        let mut progress = Vec::new();
        schedule
            .run(&mut |value| progress.push(value), &cancel, &backend)
            .unwrap();
        assert_eq!(
            schedule
                .completed
                .iter()
                .map(|segment| segment.len_us)
                .sum::<i64>(),
            1_000_000
        );
        assert!(schedule.completed.len() >= 2);
        assert!(backend.calls.load(Ordering::Relaxed) >= 3);
        assert_eq!(schedule.degradations.len(), 1);
        assert!(progress.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn successful_run_merges_parts_and_reports_usage() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.webp");
        let cancel = AtomicBool::new(false);
        let config = PipelineConfig::new(1_000_000, 10, 640, 480, 80)
            .with_memory_bytes(256 * 1024 * 1024)
            .with_workers(2)
            .with_segment_frames(1, Some(5));
        let mut progress = Vec::new();
        let report = run(
            &config,
            &ValidBackend,
            &output,
            &mut |value| progress.push(value),
            &cancel,
        )
        .unwrap();
        assert_eq!((report.planned_segments, report.completed_segments), (2, 2));
        assert_eq!(report.attempts, 1);
        assert!(report.peak_reserved_bytes > 0);
        assert_eq!(progress.last(), Some(&Some(1.0)));
        webp_parts::validate_animation(&output).unwrap();
    }

    #[test]
    fn still_segments_are_normalized_and_merged() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.webp");
        let cancel = AtomicBool::new(false);
        let config = PipelineConfig::new(1_000_000, 10, 9, 10, 80)
            .with_memory_bytes(64 * 1024 * 1024)
            .with_workers(2)
            .with_segment_frames(1, Some(5));
        let report = run(&config, &StaticBackend, &output, &mut |_| {}, &cancel).unwrap();
        assert_eq!((report.planned_segments, report.completed_segments), (2, 2));
        webp_parts::validate_animation(&output).unwrap();
    }

    #[test]
    fn permanent_failure_returns_progress_report() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.webp");
        let cancel = AtomicBool::new(false);
        let config = PipelineConfig::new(1_000_000, 10, 640, 480, 80)
            .with_attempts(3)
            .with_retry_backoff(Duration::from_millis(1));
        let error = run(&config, &FailingBackend, &output, &mut |_| {}, &cancel).unwrap_err();
        assert_eq!(error.report.attempts, 3);
        assert_eq!(error.report.degradations.len(), 3);
        assert_eq!(error.report.completed_segments, 0);
        assert!(!output.exists());
    }

    #[test]
    fn invalid_segment_output_is_rejected_before_merge() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.webp");
        let cancel = AtomicBool::new(false);
        let config = PipelineConfig::new(1_000_000, 10, 640, 480, 80)
            .with_attempts(1)
            .with_retry_backoff(Duration::from_millis(1));
        let error = run(&config, &InvalidBackend, &output, &mut |_| {}, &cancel).unwrap_err();
        assert!(format!("{:#}", error.source).contains("not a WebP"));
        assert!(!output.exists());
    }

    #[test]
    fn cancellation_is_typed_for_the_application_adapter() {
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("result.webp");
        let cancel = AtomicBool::new(true);
        let error = run(&config(), &ValidBackend, &output, &mut |_| {}, &cancel).unwrap_err();
        assert!(error.is_cancelled());
        assert!(!output.exists());
    }
}
