use crate::state::StateStore;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_protocol::state::Notice;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

const CAPACITY: usize = 32;

#[derive(Clone)]
pub struct NoticeRing {
    inner: Arc<Mutex<Ring>>,
}

struct Ring {
    notices: VecDeque<Notice>,
    next_id: u64,
}

impl Default for NoticeRing {
    fn default() -> Self {
        Self::new()
    }
}

impl NoticeRing {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Ring {
                notices: VecDeque::with_capacity(CAPACITY),
                next_id: 1,
            })),
        }
    }

    pub fn push(
        &self,
        state: &StateStore,
        level: NoticeLevel,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let published = {
            let mut ring = self.inner.lock().expect("notice ring lock poisoned");
            let notice = Notice {
                id: ring.next_id,
                level,
                summary: summary.into(),
                detail: detail.into(),
                action: None,
            };
            ring.next_id = ring.next_id.saturating_add(1);
            ring.notices.push_back(notice);
            while ring.notices.len() > CAPACITY {
                ring.notices.pop_front();
            }
            ring.notices.iter().cloned().collect::<Vec<_>>()
        };
        state.update(|current| current.notices = published);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::test_state;

    #[test]
    fn notice_ring_caps_and_publishes_to_state() {
        let state = StateStore::new(test_state());
        let ring = NoticeRing::new();
        for index in 0..40 {
            ring.push(
                &state,
                NoticeLevel::Info,
                format!("notice {index}"),
                String::new(),
            );
        }

        let notices = state.snapshot().notices.clone();
        assert_eq!(notices.len(), 32);
        assert_eq!(notices.first().unwrap().summary, "notice 8");
        assert_eq!(notices.last().unwrap().summary, "notice 39");
        assert!(notices.windows(2).all(|pair| pair[0].id < pair[1].id));
    }
}
