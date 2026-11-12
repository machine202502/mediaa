use super::error::EngineError;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// How a model call is repeated when the provider is limiting or unreachable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    /// Total tries, including the first one. 1 means no retries.
    pub attempts: u32,
    /// Delay before the first repeat; doubles after every failed try.
    pub base_ms: u64,
}

impl RetryPolicy {
    pub fn none() -> RetryPolicy {
        RetryPolicy {
            attempts: 1,
            base_ms: 0,
        }
    }
}

/// Kinds that a repeat can actually fix; everything else fails immediately.
pub fn retryable(kind: &str) -> bool {
    matches!(kind, "rate_limited" | "network" | "unavailable")
}

/// Runs `op`, repeating retryable failures with exponential backoff
/// (`base_ms * 2^(n-1)`, capped at 30 s) plus jitter. A `Retry-After` hint
/// from the provider replaces the computed delay when it is longer.
pub fn run<T>(
    policy: &RetryPolicy,
    cancel: &AtomicBool,
    op: impl FnMut() -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    run_if(policy, cancel, &|error| retryable(error.kind), op)
}

/// Same as [`run`], but the caller decides per error whether a repeat makes
/// sense (for example a chat must not be repeated once text reached the UI).
pub fn run_if<T>(
    policy: &RetryPolicy,
    cancel: &AtomicBool,
    allowed: &dyn Fn(&EngineError) -> bool,
    mut op: impl FnMut() -> Result<T, EngineError>,
) -> Result<T, EngineError> {
    let attempts = policy.attempts.max(1);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        match op() {
            Ok(value) => return Ok(value),
            Err(error) => {
                if attempt >= attempts || !allowed(&error) || cancel.load(Ordering::Relaxed) {
                    return Err(error);
                }
                sleep_with_jitter(next_delay(policy, attempt, &error), cancel);
            }
        }
    }
}

/// Backoff of the `attempt`-th repeat: exponential from the policy base,
/// replaced by a longer provider hint, capped at 30 s.
fn next_delay(policy: &RetryPolicy, attempt: u32, error: &EngineError) -> Duration {
    let cap = Duration::from_secs(30);
    let exponential =
        Duration::from_millis(policy.base_ms.saturating_mul(1 << (attempt - 1).min(5)));
    let mut delay = exponential.min(cap);
    if let Some(wait) = error.retry_after {
        if wait > delay {
            delay = wait.min(cap);
        }
    }
    delay
}

/// Sleeps `delay` plus a jitter of up to a quarter of the delay, in small
/// slices so a cancel flag is noticed quickly.
fn sleep_with_jitter(delay: Duration, cancel: &AtomicBool) {
    let jitter_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let jitter = delay.as_millis() as u64 / 4 * (jitter_nanos % 100) / 100;
    let total = delay + Duration::from_millis(jitter);
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50).min(deadline - Instant::now()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag() -> AtomicBool {
        AtomicBool::new(false)
    }

    #[test]
    fn a_retryable_error_is_repeated_until_the_attempts_run_out() {
        let cancel = flag();
        let policy = RetryPolicy {
            attempts: 3,
            base_ms: 1,
        };
        let mut tries = 0;
        let result = run(&policy, &cancel, || {
            tries += 1;
            if tries < 3 {
                Err(EngineError::network("connection refused"))
            } else {
                Ok(42)
            }
        });
        assert_eq!(result, Ok(42));
        assert_eq!(tries, 3);
    }

    #[test]
    fn attempts_limit_stops_the_repeats_and_the_last_error_wins() {
        let cancel = flag();
        let policy = RetryPolicy {
            attempts: 2,
            base_ms: 1,
        };
        let mut tries = 0;
        let result: Result<(), _> = run(&policy, &cancel, || {
            tries += 1;
            Err(EngineError::rate_limited("too many requests", None))
        });
        assert!(result.is_err());
        assert_eq!(tries, 2);
    }

    #[test]
    fn auth_and_bad_request_errors_are_not_repeated() {
        let cancel = flag();
        let policy = RetryPolicy {
            attempts: 4,
            base_ms: 1,
        };
        for error in [
            EngineError::unauthorized("the API key was rejected (401)"),
            EngineError::bad_request("broken event"),
            EngineError::not_found("no such model"),
            EngineError::cancelled("cancelled"),
        ] {
            let mut tries = 0;
            let kind = error.kind;
            let result: Result<(), _> = run(&policy, &cancel, || {
                tries += 1;
                Err(EngineError::new(kind, error.message.clone()))
            });
            assert!(result.is_err(), "{kind} must fail immediately");
            assert_eq!(tries, 1, "{kind} must not be repeated");
        }
    }

    #[test]
    fn a_long_retry_after_wins_over_the_backoff_but_stays_capped() {
        let policy = RetryPolicy {
            attempts: 2,
            base_ms: 1,
        };
        let error = EngineError::rate_limited("too many requests", Some(Duration::from_secs(3600)));
        assert_eq!(next_delay(&policy, 1, &error), Duration::from_secs(30));
        // a short hint does not replace the (already bigger) exponential delay
        let grown = EngineError::rate_limited("too many", Some(Duration::from_millis(5)));
        assert_eq!(next_delay(&policy, 4, &grown), Duration::from_millis(8));
    }

    #[test]
    fn cancel_stops_the_sleep_between_tries() {
        let cancel = flag();
        let policy = RetryPolicy {
            attempts: 5,
            base_ms: 1,
        };
        cancel.store(true, Ordering::Relaxed);
        let mut tries = 0;
        let started = Instant::now();
        let result: Result<(), _> = run(&policy, &cancel, || {
            tries += 1;
            Err(EngineError::network("connection refused"))
        });
        assert!(result.is_err());
        assert_eq!(tries, 1);
        assert!(started.elapsed() < Duration::from_millis(500));
    }
}
