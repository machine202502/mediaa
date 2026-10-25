mod webp_parts;

use anyhow::Context;
use crossbeam_channel::{bounded, RecvTimeoutError};
use mediaa_core::limits::{FPS, VIDEO_QUALITY};
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
        let fps = FPS.clamp(self.fps);
        let width = self.width.max(1);
        let height = self.height.max(1);
        let quality = VIDEO_QUALITY.clamp(u32::from(self.quality)) as u8;
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

mod memory;
use memory::{frame_memory, initial_frames, segment_memory, worker_limit};
mod scheduler;
use scheduler::Schedule;

#[cfg(test)]
mod tests;
