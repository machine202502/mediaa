use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Default)]
pub(crate) struct Wake(Option<Arc<dyn Fn(Option<Duration>) + Send + Sync>>);

impl Wake {
    pub(crate) fn new(signal: Arc<dyn Fn(Option<Duration>) + Send + Sync>) -> Self {
        Self(Some(signal))
    }

    pub(crate) fn now(&self) {
        if let Some(signal) = &self.0 {
            signal(None);
        }
    }

    pub(crate) fn after(&self, delay: Duration) {
        if let Some(signal) = &self.0 {
            signal(Some(delay));
        }
    }
}
