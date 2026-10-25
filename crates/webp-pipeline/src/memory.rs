use super::*;

pub(super) fn frame_memory(width: u32, height: u32, quality: u8) -> u64 {
    let pixels = u64::from(width).saturating_mul(u64::from(height));
    pixels
        .saturating_mul(u64::from(quality))
        .div_ceil(100)
        .max(1)
}

pub(super) fn segment_memory(config: &PipelineConfig, frame_cost: u64, frames: usize) -> u64 {
    config
        .fixed_encoder_bytes
        .saturating_add(frame_cost.saturating_mul(frames as u64))
}

pub(super) fn initial_frames(config: &PipelineConfig, frame_cost: u64) -> usize {
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

pub(super) fn worker_limit(config: &PipelineConfig, frames: usize, segments: usize) -> usize {
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

pub(super) struct MemoryBudget {
    capacity: u64,
    used: AtomicU64,
    peak: AtomicU64,
}

pub(super) struct MemoryReservation<'a> {
    budget: &'a MemoryBudget,
    bytes: u64,
}

impl MemoryBudget {
    pub(super) fn new(capacity: u64) -> Self {
        Self {
            capacity,
            used: AtomicU64::new(0),
            peak: AtomicU64::new(0),
        }
    }

    pub(super) fn reserve(&self, bytes: u64) -> Option<MemoryReservation<'_>> {
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

    pub(super) fn peak(&self) -> u64 {
        self.peak.load(Ordering::Acquire)
    }
}

impl Drop for MemoryReservation<'_> {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.bytes, Ordering::AcqRel);
    }
}
