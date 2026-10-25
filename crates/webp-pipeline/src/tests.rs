use super::memory::MemoryBudget;
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
