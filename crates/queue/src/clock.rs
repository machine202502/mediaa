//! Time source. The scheduler and the store stamp tasks with [`Clock::now_ms`].
//! Tests use [`FakeClock`]; the process uses [`SystemClock`].

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

/// A clock the test moves by hand. Shared behind an `Arc`.
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
