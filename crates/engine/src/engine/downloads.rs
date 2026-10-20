//! Model downloads in flight, shown in the engine's status while they run.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A model download in progress.
#[derive(Debug, Clone)]
pub struct Download {
    pub spec: String,
    pub done: u64,
    pub total: u64,
}

/// Model downloads in flight, by id.
#[derive(Clone, Default)]
pub struct Downloads {
    inner: Arc<DownloadsInner>,
}

#[derive(Default)]
struct DownloadsInner {
    next: AtomicU64,
    list: Mutex<HashMap<u64, Download>>,
}

impl Downloads {
    /// Registers a download; the id is used to report its progress.
    pub fn start(&self, spec: &str) -> u64 {
        let id = self.inner.next.fetch_add(1, Ordering::Relaxed) + 1;
        self.inner.list.lock().expect("downloads lock").insert(id, Download { spec: spec.to_string(), done: 0, total: 0 });
        id
    }

    pub fn progress(&self, id: u64, done: u64, total: u64) {
        if let Some(download) = self.inner.list.lock().expect("downloads lock").get_mut(&id) {
            download.done = done;
            download.total = total;
        }
    }

    /// Removes a finished (or failed) download from the list.
    pub fn finish(&self, id: u64) {
        self.inner.list.lock().expect("downloads lock").remove(&id);
    }

    /// The downloads in flight, oldest first.
    pub fn list(&self) -> Vec<(u64, Download)> {
        let mut list: Vec<(u64, Download)> = self.inner.list.lock().expect("downloads lock").iter().map(|(id, d)| (*id, d.clone())).collect();
        list.sort_by_key(|(id, _)| *id);
        list
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_report_and_end() {
        let downloads = Downloads::default();
        let a = downloads.start("a/b");
        let b = downloads.start("c/d");
        downloads.progress(a, 10, 100);
        let list = downloads.list();
        assert_eq!(list.len(), 2);
        assert_eq!((list[0].0, list[0].1.done, list[0].1.total), (a, 10, 100));
        downloads.finish(a);
        assert_eq!(downloads.list().len(), 1);
        assert_eq!(downloads.list()[0].0, b);
    }
}
