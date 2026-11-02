use crate::infra::ipc;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const NOTICE_CAP: usize = 32;
const NOTICE_TTL: Duration = Duration::from_secs(60);

struct RingNotice {
    id: u64,
    notice: ipc::Notice,
    at: Instant,
}

pub(super) struct NoticeRing {
    items: VecDeque<RingNotice>,
    next_id: u64,
}

impl NoticeRing {
    pub(super) fn new() -> Self {
        Self {
            items: VecDeque::new(),
            next_id: 1,
        }
    }

    pub(super) fn push(&mut self, notice: ipc::Notice, now: Instant) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push_back(RingNotice {
            id,
            notice,
            at: now,
        });
        while self.items.len() > NOTICE_CAP {
            self.items.pop_front();
        }
        id
    }

    pub(super) fn expire(&mut self, now: Instant) -> bool {
        let before = self.items.len();
        self.items
            .retain(|entry| now.duration_since(entry.at) <= NOTICE_TTL);
        before != self.items.len()
    }

    pub(super) fn entries(&self) -> impl Iterator<Item = (u64, &ipc::Notice)> {
        self.items.iter().map(|entry| (entry.id, &entry.notice))
    }
}

pub(super) fn notice_level(level: mediaa_core::notices::NoticeLevel) -> ipc::NoticeLevel {
    match level {
        mediaa_core::notices::NoticeLevel::Info | mediaa_core::notices::NoticeLevel::Success => {
            ipc::NoticeLevel::Info
        }
        mediaa_core::notices::NoticeLevel::Warning => ipc::NoticeLevel::Warning,
        mediaa_core::notices::NoticeLevel::Error => ipc::NoticeLevel::Error,
    }
}
