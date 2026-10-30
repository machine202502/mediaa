use super::memory::MemoryBudget;
use super::*;
use crossbeam_channel::unbounded;

pub(super) struct Schedule {
    pub(super) config: PipelineConfig,
    frame_cost: u64,
    pending: Vec<Segment>,
    pub(super) completed: Vec<Segment>,
    pub(super) workers: usize,
    pub(super) max_frames: usize,
    next_id: usize,
    pub(super) attempts: usize,
}

impl Schedule {
    pub(super) fn new(
        config: PipelineConfig,
        plan: Vec<SegmentPlan>,
        dir: &Path,
        workers: usize,
    ) -> Self {
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
        }
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
        let (work_tx, work_rx) = bounded(self.pending.len().max(1));
        let (result_tx, result_rx) = unbounded();
        for segment in &self.pending {
            let _ = work_tx.send(segment.clone());
        }
        drop(work_tx);
        let budget = MemoryBudget::new(self.config.tuning.memory_bytes);
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
                        let bytes = self.config.tuning.fixed_encoder_bytes.saturating_add(
                            self.frame_cost
                                .saturating_mul(frames.min(u128::from(u64::MAX)) as u64),
                        );
                        let Some(_reservation) = budget.reserve(bytes) else {
                            stop.store(true, Ordering::Relaxed);
                            let _ = result_tx.send((
                                segment,
                                Err(anyhow::Error::new(SegmentFailure::retryable(
                                    anyhow::anyhow!("WebP memory budget exhausted"),
                                ))),
                            ));
                            break;
                        };
                        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                            backend.encode(&segment, stop)
                        }))
                        .unwrap_or_else(|_| {
                            Err(anyhow::Error::new(SegmentFailure::permanent(
                                anyhow::anyhow!("WebP segment worker panicked"),
                            )))
                        })
                        .and_then(|()| {
                            webp_parts::normalize_animation(&segment.path, segment.len_us).map_err(
                                |error| anyhow::Error::new(SegmentFailure::permanent(error)),
                            )
                        })
                        .and_then(|()| {
                            webp_parts::validate_animation(&segment.path).map_err(|error| {
                                anyhow::Error::new(SegmentFailure::permanent(error))
                            })
                        });
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
            round
        })
    }

    pub(super) fn run(
        &mut self,
        on_progress: &mut dyn FnMut(Option<f32>),
        cancel: &AtomicBool,
        backend: &dyn SegmentBackend,
    ) -> anyhow::Result<()> {
        let mut last_error = None;
        for attempt in 0..self.config.tuning.max_attempts {
            if self.pending.is_empty() {
                return Ok(());
            }
            self.attempts += 1;
            let completed_us = self.completed.iter().map(|segment| segment.len_us).sum();
            let round = self.round(completed_us, on_progress, cancel, backend);
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
            if round.failure.as_ref().is_some_and(|error| {
                error
                    .downcast_ref::<SegmentFailure>()
                    .is_some_and(|failure| failure.kind() == SegmentFailureKind::Permanent)
            }) {
                return Err(round.failure.expect("permanent failure checked"));
            }
            let error = round
                .failure
                .unwrap_or_else(|| anyhow::anyhow!("WebP segment workers stopped"));
            self.workers = (self.workers / 2).max(1);
            self.max_frames = (self.max_frames / 2).max(1);
            self.split_pending();
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
            std::thread::sleep(self.config.tuning.retry_backoff * (attempt as u32 + 1));
        }
        let trace = self
            .config
            .trace_id
            .as_deref()
            .map_or("webp", |trace| trace);
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("WebP segment schedule stopped"))).context(
            format!(
                "[{trace}] WebP encoding failed after {} attempts",
                self.config.tuning.max_attempts
            ),
        )
    }
}

#[derive(Default)]
struct Round {
    completed: Vec<Segment>,
    failure: Option<anyhow::Error>,
}
