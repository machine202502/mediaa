use mediaa_webp_pipeline::{run, PipelineConfig, PipelineTuning, Segment, SegmentBackend};
use std::sync::atomic::AtomicBool;

struct TestBackend;

impl SegmentBackend for TestBackend {
    fn encode(&self, segment: &Segment, _cancel: &AtomicBool) -> anyhow::Result<()> {
        std::fs::write(&segment.path, animation(segment.id as u8))?;
        Ok(())
    }
}

fn animation(frame: u8) -> Vec<u8> {
    fn chunk(id: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(id);
        bytes.extend_from_slice(&(data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(data);
        if data.len() & 1 != 0 {
            bytes.push(0);
        }
        bytes
    }
    let mut chunks = Vec::new();
    chunks.extend(chunk(b"VP8X", &[0x02, 0, 0, 0, 9, 0, 0, 10, 0, 0]));
    chunks.extend(chunk(b"ANIM", &[0, 0, 0, 0, 0, 0]));
    chunks.extend(chunk(b"ANMF", &[frame; 17]));
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&((chunks.len() + 4) as u32).to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(&chunks);
    bytes
}

#[test]
fn public_api_runs_independently_and_merges_segments() {
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("out.webp");
    let tuning = PipelineTuning {
        memory_bytes: 64 * 1024 * 1024,
        max_workers: 3,
        max_attempts: 2,
        min_segment_frames: 1,
        max_segment_frames: Some(5),
        fixed_encoder_bytes: 16 * 1024 * 1024,
        retry_backoff: std::time::Duration::from_millis(1),
    };
    let config = PipelineConfig::new(1_000_000, 10, 640, 480, 80).with_tuning(&tuning);
    let mut progress = Vec::new();
    run(
        &config,
        &TestBackend,
        &output,
        &mut |value| progress.push(value),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert!(output.is_file());
    assert_eq!(progress.last(), Some(&Some(1.0)));
}
