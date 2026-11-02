use mediaa_core::library::{Details, KindFilter, LibraryEntry};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Title,
    Largest,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Newest, Sort::Oldest, Sort::Title, Sort::Largest];

    pub fn by_date(self) -> bool {
        matches!(self, Sort::Newest | Sort::Oldest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub kind: KindFilter,
    pub search: String,
    pub sort: Sort,
}

pub struct LibraryView {
    dir: PathBuf,
    entries: Vec<LibraryEntry>,
    version: u64,
}

impl LibraryView {
    pub fn from_entries(dir: &Path, entries: Vec<LibraryEntry>, version: u64) -> Self {
        Self {
            dir: dir.to_path_buf(),
            entries,
            version,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn query(&self, query: &Query) -> Vec<usize> {
        let search = query.search.trim().to_lowercase();
        let mut hits: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| query.kind.matches(entry.kind))
            .filter(|(_, entry)| {
                search.is_empty()
                    || entry.title.to_lowercase().contains(&search)
                    || entry.name.to_lowercase().contains(&search)
            })
            .map(|(index, _)| index)
            .collect();
        match query.sort {
            Sort::Newest => {
                hits.sort_by(|a, b| self.entries[*b].modified.cmp(&self.entries[*a].modified))
            }
            Sort::Oldest => {
                hits.sort_by(|a, b| self.entries[*a].modified.cmp(&self.entries[*b].modified))
            }
            Sort::Title => hits.sort_by(|a, b| self.entries[*a].title.cmp(&self.entries[*b].title)),
            Sort::Largest => hits.sort_by_key(|index| std::cmp::Reverse(self.entries[*index].size)),
        }
        hits
    }

    pub fn apply_scan(&mut self, entries: Vec<LibraryEntry>) {
        self.entries = entries;
        self.version = self.version.saturating_add(1);
        self.entries.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    pub fn refresh_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            match std::fs::metadata(path) {
                Ok(metadata) if metadata.is_file() => {
                    if let Some(entry) = self.entries.iter_mut().find(|entry| &entry.path == path) {
                        entry.size = metadata.len();
                        if let Ok(modified) = metadata.modified() {
                            entry.modified = modified;
                        }
                    }
                }
                _ => self.entries.retain(|entry| &entry.path != path),
            }
        }
        self.version = self.version.saturating_add(1);
    }

    pub fn flush(&mut self) {}

    pub fn flush_details(&mut self) {}

    pub fn flush_cache(&mut self) {}

    pub fn set_details(&mut self, path: &Path, details: Details) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.path == path) {
            entry.details = Some(details);
            self.version = self.version.saturating_add(1);
        }
    }

    pub fn remove(&mut self, path: &Path) {
        self.entries.retain(|entry| entry.path != path);
        self.version = self.version.saturating_add(1);
    }

    pub fn rename(&mut self, old: &Path, new: &Path) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.path == old) {
            entry.path = new.to_path_buf();
            entry.name = new
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            self.version = self.version.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::formats::MediaKind;
    use std::time::{Duration, UNIX_EPOCH};

    fn entry(name: &str, title: &str, size: u64) -> LibraryEntry {
        LibraryEntry {
            path: PathBuf::from(name),
            name: name.into(),
            title: title.into(),
            kind: MediaKind::Image,
            size,
            modified: UNIX_EPOCH + Duration::from_secs(size),
            taken: chrono::NaiveDateTime::default(),
            details: None,
        }
    }

    #[test]
    fn view_queries_without_storage() {
        let view = LibraryView::from_entries(
            Path::new("media"),
            vec![entry("a.png", "Alpha", 1), entry("b.png", "Beta", 2)],
            7,
        );
        assert_eq!(view.len(), 2);
        assert_eq!(view.query(&Query::default()).len(), 2);
        let query = Query {
            search: "beta".into(),
            sort: Sort::Largest,
            ..Query::default()
        };
        assert_eq!(view.query(&query), vec![1]);
        assert_eq!(view.version(), 7);
    }
}
