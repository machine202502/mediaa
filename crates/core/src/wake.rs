use std::sync::Arc;
use std::time::Duration;

#[derive(Clone, Default)]
pub struct Wake(Option<Arc<dyn Fn(Option<Duration>) + Send + Sync>>);

impl Wake {
    pub fn new(signal: Arc<dyn Fn(Option<Duration>) + Send + Sync>) -> Self {
        Self(Some(signal))
    }

    pub fn now(&self) {
        if let Some(signal) = &self.0 {
            signal(None);
        }
    }

    pub fn after(&self, delay: Duration) {
        if let Some(signal) = &self.0 {
            signal(Some(delay));
        }
    }
}
