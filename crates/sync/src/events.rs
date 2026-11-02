use std::collections::VecDeque;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventEntry {
    pub seq: u64,
    pub kind: String,
    pub payload: Vec<u8>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct EventsSince {
    pub events: Vec<EventEntry>,
    pub gap: bool,
}

#[derive(Debug)]
pub struct EventLog {
    next: u64,
    capacity: usize,
    entries: VecDeque<EventEntry>,
}

impl EventLog {
    pub fn new(capacity: usize) -> Self {
        Self {
            next: 1,
            capacity: capacity.max(1),
            entries: VecDeque::new(),
        }
    }

    pub fn push(&mut self, kind: &str, payload: Vec<u8>) -> u64 {
        let seq = self.next;
        self.next += 1;
        self.entries.push_back(EventEntry {
            seq,
            kind: kind.to_string(),
            payload,
        });
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
        seq
    }

    pub fn next_seq(&self) -> u64 {
        self.next
    }

    pub fn since(&self, cursor: u64) -> EventsSince {
        let first = self.entries.front().map(|entry| entry.seq);
        let last = self.entries.back().map(|entry| entry.seq);
        let gap = match (first, last) {
            (Some(first), Some(last)) => cursor > last || (cursor + 1 < first && cursor < first),
            _ => false,
        };
        let events = self
            .entries
            .iter()
            .filter(|entry| entry.seq > cursor)
            .cloned()
            .collect();
        EventsSince { events, gap }
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_numbers_are_monotonic() {
        let mut log = EventLog::new(8);
        assert_eq!(log.push("a", Vec::new()), 1);
        assert_eq!(log.push("b", Vec::new()), 2);
        assert_eq!(log.next_seq(), 3);
    }

    #[test]
    fn since_returns_only_events_after_the_cursor() {
        let mut log = EventLog::new(8);
        log.push("a", vec![1]);
        log.push("b", vec![2]);
        log.push("c", vec![3]);
        let seen = log.since(1);
        assert!(!seen.gap);
        assert_eq!(
            seen.events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn a_cursor_before_the_ring_reports_a_gap() {
        let mut log = EventLog::new(2);
        log.push("a", vec![1]);
        log.push("b", vec![2]);
        log.push("c", vec![3]);
        let seen = log.since(0);
        assert!(seen.gap);
        assert_eq!(seen.events.len(), 2);
    }

    #[test]
    fn an_empty_log_has_no_gap() {
        let log = EventLog::new(4);
        let seen = log.since(0);
        assert!(!seen.gap);
        assert!(seen.events.is_empty());
    }
}
