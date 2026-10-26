use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

pub(crate) struct Job<T> {
    handle: Option<JoinHandle<T>>,
    cancel: Arc<AtomicBool>,
}

impl<T: Send + 'static> Job<T> {
    pub(crate) fn spawn(
        name: &str,
        cancel: Arc<AtomicBool>,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> anyhow::Result<Self> {
        let handle = std::thread::Builder::new().name(name.into()).spawn(work)?;
        Ok(Self {
            handle: Some(handle),
            cancel,
        })
    }

    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub(crate) fn poll(&mut self) -> Option<anyhow::Result<T>> {
        let handle = self.handle.as_ref()?;
        if !handle.is_finished() {
            return None;
        }
        let handle = self.handle.take().expect("finished handle exists");
        Some(
            handle
                .join()
                .map_err(|_| anyhow::anyhow!("background job panicked")),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_job_returns_its_value_once() {
        let mut job = Job::spawn("test-job", Arc::default(), || 7).unwrap();
        let result = (0..100)
            .find_map(|_| {
                let result = job.poll();
                if result.is_none() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                result
            })
            .unwrap();
        assert_eq!(result.unwrap(), 7);
        assert!(job.poll().is_none());
    }

    #[test]
    fn a_job_reports_a_panic() {
        let mut job = Job::spawn("panic-job", Arc::default(), || -> usize {
            panic!("background failure")
        })
        .unwrap();
        let result = (0..100)
            .find_map(|_| {
                let result = job.poll();
                if result.is_none() {
                    std::thread::sleep(Duration::from_millis(1));
                }
                result
            })
            .unwrap();
        assert!(result.is_err());
    }
}
