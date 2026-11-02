use std::collections::BTreeMap;

#[derive(Debug, Clone)]
struct Entry {
    version: u64,
    payload: Vec<u8>,
}

#[derive(Debug, Default)]
pub struct StateHub {
    entries: BTreeMap<String, Entry>,
}

impl StateHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&mut self, store: &str, version: u64, payload: Vec<u8>) -> bool {
        if let Some(entry) = self.entries.get(store) {
            if version <= entry.version {
                return false;
            }
        }
        self.entries
            .insert(store.to_string(), Entry { version, payload });
        true
    }

    pub fn snapshot(&self, store: &str) -> Option<(u64, &[u8])> {
        self.entries
            .get(store)
            .map(|entry| (entry.version, entry.payload.as_slice()))
    }

    pub fn changes(&self, sent: &BTreeMap<String, u64>) -> Vec<(String, u64, Vec<u8>)> {
        self.entries
            .iter()
            .filter(|(store, entry)| {
                sent.get(*store)
                    .is_none_or(|version| entry.version > *version)
            })
            .map(|(store, entry)| (store.clone(), entry.version, entry.payload.clone()))
            .collect()
    }

    pub fn stores(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }
}

#[derive(Debug, Default)]
pub struct StateView {
    versions: BTreeMap<String, u64>,
}

impl StateView {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn accept(&mut self, store: &str, version: u64) -> bool {
        if self
            .versions
            .get(store)
            .is_some_and(|known| *known >= version)
        {
            return false;
        }
        self.versions.insert(store.to_string(), version);
        true
    }

    pub fn version(&self, store: &str) -> Option<u64> {
        self.versions.get(store).copied()
    }

    pub fn versions(&self) -> BTreeMap<String, u64> {
        self.versions.clone()
    }

    pub fn reset(&mut self) {
        self.versions.clear();
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_version_replaces_the_state() {
        let mut hub = StateHub::new();
        assert!(hub.publish("recording", 1, b"one".to_vec()));
        assert!(hub.publish("recording", 2, b"two".to_vec()));
        assert_eq!(hub.snapshot("recording"), Some((2, b"two".as_slice())));
    }

    #[test]
    fn equal_or_older_versions_are_ignored() {
        let mut hub = StateHub::new();
        assert!(hub.publish("settings", 5, b"five".to_vec()));
        assert!(!hub.publish("settings", 5, b"five".to_vec()));
        assert!(!hub.publish("settings", 4, b"four".to_vec()));
        assert_eq!(hub.snapshot("settings"), Some((5, b"five".as_slice())));
    }

    #[test]
    fn changes_only_include_versions_newer_than_sent() {
        let mut hub = StateHub::new();
        hub.publish("a", 1, b"a1".to_vec());
        hub.publish("b", 3, b"b3".to_vec());
        let sent = [("a".to_string(), 1)].into_iter().collect();
        let changes = hub.changes(&sent);
        assert_eq!(changes, vec![("b".to_string(), 3, b"b3".to_vec())]);
    }

    #[test]
    fn the_view_only_accepts_newer_versions() {
        let mut view = StateView::new();
        assert!(view.accept("recording", 1));
        assert!(view.accept("recording", 2));
        assert!(!view.accept("recording", 2));
        assert!(!view.accept("recording", 1));
        assert_eq!(view.version("recording"), Some(2));
    }
}
