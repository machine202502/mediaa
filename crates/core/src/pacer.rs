use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct SessionClock {
    paused_at: Option<Duration>,
    paused_total: Duration,
}

impl SessionClock {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    pub fn active_time(&self, now: Duration) -> Duration {
        self.paused_at
            .unwrap_or(now)
            .saturating_sub(self.paused_total)
    }

    pub fn pause(&mut self, now: Duration) {
        if self.paused_at.is_none() {
            self.paused_at = Some(now);
        }
    }

    pub fn resume(&mut self, now: Duration) {
        if let Some(at) = self.paused_at.take() {
            self.paused_total += now.saturating_sub(at);
        }
    }
}

#[derive(Debug, Clone)]
pub struct SlotTracker {
    interval: Duration,
    last: Option<u64>,
}

impl SlotTracker {
    pub fn new(fps: u32) -> Self {
        Self {
            interval: Duration::from_secs_f64(1.0 / f64::from(fps.clamp(1, 120))),
            last: None,
        }
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }

    pub fn slot(&mut self, active: Duration) -> Option<u64> {
        let idx = self.slot_at(active);
        match self.last {
            Some(last) if idx <= last => None,
            _ => {
                self.last = Some(idx);
                Some(idx)
            }
        }
    }

    pub fn slot_at(&self, active: Duration) -> u64 {
        (active.as_secs_f64() / self.interval.as_secs_f64()).floor() as u64
    }

    pub fn last(&self) -> Option<u64> {
        self.last
    }

    pub fn next_deadline(&self) -> Duration {
        let next = self.last.map_or(0, |s| s + 1);
        self.interval.mul_f64(next as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(v: u64) -> Duration {
        Duration::from_millis(v)
    }

    #[test]
    fn steady_capture_fills_consecutive_slots() {
        let mut t = SlotTracker::new(10);
        let slots: Vec<_> = (0..5).map(|i| t.slot(ms(i * 100 + 1))).collect();
        assert_eq!(slots, [Some(0), Some(1), Some(2), Some(3), Some(4)]);
    }

    #[test]
    fn early_frames_are_dropped() {
        let mut t = SlotTracker::new(10);
        assert_eq!(t.slot(ms(0)), Some(0));
        assert_eq!(t.slot(ms(50)), None);
        assert_eq!(t.slot(ms(99)), None);
        assert_eq!(t.slot(ms(100)), Some(1));
    }

    #[test]
    fn slow_capture_leaves_gaps_keeping_real_time() {
        let mut t = SlotTracker::new(30);
        assert_eq!(t.slot(ms(0)), Some(0));
        assert_eq!(t.slot(ms(500)), Some(15));
    }

    #[test]
    fn deadline_follows_last_slot() {
        let mut t = SlotTracker::new(10);
        assert_eq!(t.next_deadline(), ms(0));
        t.slot(ms(0));
        assert_eq!(t.next_deadline(), ms(100));
        t.slot(ms(730));
        assert_eq!(t.next_deadline(), ms(800));
    }

    #[test]
    fn fps_is_clamped() {
        assert_eq!(SlotTracker::new(0).interval(), Duration::from_secs(1));
        assert!(SlotTracker::new(1000).interval() >= Duration::from_secs_f64(1.0 / 120.0));
    }

    #[test]
    fn clock_excludes_pauses() {
        let mut c = SessionClock::new();
        assert_eq!(c.active_time(ms(100)), ms(100));
        c.pause(ms(150));
        assert!(c.is_paused());
        assert_eq!(c.active_time(ms(5000)), ms(150));
        c.resume(ms(1150));
        assert_eq!(c.active_time(ms(1200)), ms(200));
    }

    #[test]
    fn double_pause_and_resume_are_idempotent() {
        let mut c = SessionClock::new();
        c.pause(ms(100));
        c.pause(ms(200));
        c.resume(ms(300));
        c.resume(ms(400));
        assert_eq!(c.active_time(ms(400)), ms(200));
    }

    #[test]
    fn clock_and_slots_together() {
        let mut c = SessionClock::new();
        let mut t = SlotTracker::new(10);
        assert_eq!(t.slot(c.active_time(ms(0))), Some(0));
        assert_eq!(t.slot(c.active_time(ms(100))), Some(1));
        c.pause(ms(150));
        c.resume(ms(1150));
        assert_eq!(t.slot(c.active_time(ms(1150))), None);
        assert_eq!(t.slot(c.active_time(ms(1200))), Some(2));
    }
}
