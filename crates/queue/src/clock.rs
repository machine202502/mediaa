use std::sync::atomic::{AtomicI64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

pub trait Clock: Send + Sync {
    fn now_ms(&self) -> i64;
}

#[derive(Debug, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64)
    }
}

#[derive(Debug)]
pub struct FakeClock {
    ms: AtomicI64,
}

impl FakeClock {
    pub fn new(ms: i64) -> FakeClock {
        FakeClock {
            ms: AtomicI64::new(ms),
        }
    }

    pub fn set(&self, ms: i64) {
        self.ms.store(ms, Ordering::Relaxed);
    }

    pub fn advance(&self, by: i64) {
        self.ms.fetch_add(by, Ordering::Relaxed);
    }
}

impl Clock for FakeClock {
    fn now_ms(&self) -> i64 {
        self.ms.load(Ordering::Relaxed)
    }
}
