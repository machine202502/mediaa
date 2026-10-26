use mediaa_webp_pipeline::{plan, recommended_workers, PipelineConfig, PipelineTuning};

fn frame_memory(width: u32, height: u32, quality: u8) -> u64 {
    u64::from(width)
        .saturating_mul(u64::from(height))
        .saturating_mul(u64::from(quality))
        .div_ceil(100)
        .max(1)
}

fn main() {
    let mut configs = 0usize;
    let mut oversized = 0usize;
    let mut aggregate_over_budget = 0usize;
    let mut aggregate_only_over_budget = 0usize;
    let mut rounding_mismatch = 0usize;
    let mut examples = Vec::new();

    for (width, height) in [(1280, 720), (1920, 1080), (3840, 2160)] {
        for fps in [15, 30, 60] {
            for quality in [80, 100] {
                for seconds in [1, 3, 30, 300] {
                    for budget_mb in [64, 128, 256, 512] {
                        configs += 1;
                        let tuning = PipelineTuning {
                            memory_bytes: budget_mb * 1024 * 1024,
                            max_workers: 8,
                            ..PipelineTuning::default()
                        };
                        let config =
                            PipelineConfig::new(seconds * 1_000_000, fps, width, height, quality)
                                .with_tuning(&tuning);
                        let segments = plan(&config);
                        let workers = recommended_workers(&config);
                        let cost = frame_memory(width, height, quality);
                        let costs: Vec<u64> = segments
                            .iter()
                            .map(|segment| {
                                let scheduler_frames = ((segment.len_us.max(1) as u128
                                    * u128::from(fps))
                                .div_ceil(1_000_000))
                                .max(1);
                                if scheduler_frames != segment.frames as u128 {
                                    rounding_mismatch += 1;
                                }
                                tuning
                                    .fixed_encoder_bytes
                                    .saturating_add(cost.saturating_mul(scheduler_frames as u64))
                            })
                            .collect();
                        let max = costs.iter().copied().max().unwrap_or_default();
                        let individual_over = max > tuning.memory_bytes;
                        if individual_over {
                            oversized += 1;
                        }
                        let mut highest = costs.clone();
                        highest.sort_unstable_by(|left, right| right.cmp(left));
                        let aggregate: u64 = highest
                            .into_iter()
                            .take(workers)
                            .fold(0u64, u64::saturating_add);
                        if aggregate > tuning.memory_bytes {
                            aggregate_over_budget += 1;
                            if !individual_over {
                                aggregate_only_over_budget += 1;
                            }
                            if examples.len() < 5 {
                                examples.push(format!(
                                    "{width}x{height}@{fps} q={quality} {seconds}s budget={budget_mb}MiB workers={workers} aggregate={}MiB max={}MiB",
                                    aggregate / (1024 * 1024),
                                    max / (1024 * 1024)
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    println!(
        "configs={configs} configs_with_oversized_segment={oversized} aggregate_over_budget={aggregate_over_budget} aggregate_only_over_budget={aggregate_only_over_budget} rounding_mismatch_segments={rounding_mismatch}"
    );
    for example in examples {
        println!("{example}");
    }
}
