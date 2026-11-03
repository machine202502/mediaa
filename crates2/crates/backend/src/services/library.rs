use mediaa2_core::formats::MediaKind;
use mediaa2_core::library::LibraryEntry;
use mediaa2_core::naming;
use mediaa2_core::query::{LibraryPage, LibraryQuery};
use mediaa2_platform::files;
use mediaa2_protocol::CleanupReport;
use mediaa2_storage::Db;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone)]
pub struct LibraryHandle {
    inner: Arc<Mutex<Library>>,
}

struct Library {
    db: Db,
    media_dir: PathBuf,
    revision: u64,
}

impl LibraryHandle {
    pub fn open(media_dir: PathBuf) -> anyhow::Result<Self> {
        let db = Db::open(&media_dir.join(".cache").join("library.db"))?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Library {
                db,
                media_dir,
                revision: 0,
            })),
        })
    }

    pub fn query(&self, query: &LibraryQuery) -> anyhow::Result<LibraryPage> {
        let library = self.inner.lock().expect("library lock poisoned");
        let (entries, total) = library.db.query(query)?;
        let next = query.offset.saturating_add(entries.len() as u32);
        Ok(LibraryPage {
            revision: library.revision,
            total,
            next_offset: (next < total).then_some(next),
            entries,
        })
    }

    pub fn refresh(&self) -> anyhow::Result<u32> {
        let mut library = self.inner.lock().expect("library lock poisoned");
        let entries = scan(&library.media_dir)?;
        library.db.upsert(&entries)?;
        library.db.remove_missing()?;
        library.revision = library.revision.saturating_add(1);
        Ok(u32::try_from(entries.len()).unwrap_or(u32::MAX))
    }

    pub fn add_paths(&self, paths: &[PathBuf]) -> anyhow::Result<()> {
        let mut library = self.inner.lock().expect("library lock poisoned");
        let entries: Vec<LibraryEntry> = paths
            .iter()
            .filter_map(|path| entry_from_path(path))
            .collect();
        if entries.is_empty() {
            return Ok(());
        }
        library.db.upsert(&entries)?;
        library.revision = library.revision.saturating_add(1);
        Ok(())
    }

    pub fn rename(&self, from: &Path, to: &Path, title: &str) -> anyhow::Result<()> {
        let mut library = self.inner.lock().expect("library lock poisoned");
        if from != to {
            std::fs::rename(from, to)?;
        }
        library.db.rename_path(from, to)?;
        library.db.set_title(to, title)?;
        library.revision = library.revision.saturating_add(1);
        Ok(())
    }

    pub fn delete(&self, paths: &[PathBuf], permanent: bool) -> anyhow::Result<()> {
        let mut library = self.inner.lock().expect("library lock poisoned");
        for path in paths {
            if permanent {
                match std::fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            } else {
                files::move_to_trash(path)?;
            }
        }
        library.db.remove(paths)?;
        library.revision = library.revision.saturating_add(1);
        Ok(())
    }

    pub fn cleanup(&self, older_days: u32) -> anyhow::Result<CleanupReport> {
        let mut library = self.inner.lock().expect("library lock poisoned");
        let age = Duration::from_secs(u64::from(older_days).saturating_mul(86_400));
        let cutoff = SystemTime::now().checked_sub(age).unwrap_or(UNIX_EPOCH);
        let query = LibraryQuery {
            limit: 1_000_000,
            ..LibraryQuery::default()
        };
        let (entries, _) = library.db.query(&query)?;
        let mut removed = Vec::new();
        let mut failed = 0;
        for entry in entries {
            if entry.modified >= cutoff {
                continue;
            }
            match files::move_to_trash(&entry.path) {
                Ok(()) => removed.push(entry.path),
                Err(error) => {
                    log::warn!("library cleanup: {}: {error:#}", entry.path.display());
                    failed += 1;
                }
            }
        }
        library.db.remove(&removed)?;
        if !removed.is_empty() {
            library.revision = library.revision.saturating_add(1);
        }
        Ok(CleanupReport {
            removed: u32::try_from(removed.len()).unwrap_or(u32::MAX),
            failed,
        })
    }

    pub fn revision(&self) -> u64 {
        self.inner.lock().expect("library lock poisoned").revision
    }
}

fn entry_from_path(path: &Path) -> Option<LibraryEntry> {
    let kind = MediaKind::detect(path)?;
    let parsed = naming::parse_path(path)?;
    let metadata = std::fs::metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    Some(LibraryEntry {
        path: path.to_path_buf(),
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        title: parsed.title,
        kind,
        size: metadata.len(),
        modified: metadata.modified().unwrap_or(UNIX_EPOCH),
        taken: parsed.taken,
        details: None,
    })
}

fn scan(media_dir: &Path) -> anyhow::Result<Vec<LibraryEntry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(media_dir)? {
        let item = item?;
        let path = item.path();
        if path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == ".cache")
        {
            continue;
        }
        if let Some(entry) = entry_from_path(&path) {
            entries.push(entry);
        }
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    #[test]
    fn library_refresh_indexes_media_files_and_ignores_others() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(media.join(".cache")).unwrap();
        write(&media, "Recording_2026-01-02_03-04-05.mp4");
        write(&media, "note.txt");
        write(&media.join(".cache"), "x.png");

        let library = LibraryHandle::open(media).unwrap();
        assert_eq!(library.refresh().unwrap(), 1);

        let page = library.query(&LibraryQuery::default()).unwrap();
        assert_eq!(page.revision, 1);
        assert_eq!(page.total, 1);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].name, "Recording_2026-01-02_03-04-05.mp4");
        assert_eq!(page.entries[0].title, "Recording");
        assert_eq!(page.entries[0].kind, MediaKind::Video);
        assert_eq!(page.entries[0].size, 1);
        assert_eq!(library.revision(), 1);
    }

    #[test]
    fn library_query_respects_kind_and_search() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        write(&media, "Screenshot_2026-01-02_03-04-05.png");
        write(&media, "Holiday_2026-01-03_03-04-05.mp4");
        write(&media, "Voice_2026-01-04_03-04-05.mp3");

        let library = LibraryHandle::open(media).unwrap();
        library.refresh().unwrap();

        let all = library.query(&LibraryQuery::default()).unwrap();
        assert_eq!(all.total, 3);

        let videos = library
            .query(&LibraryQuery {
                kind: Some(MediaKind::Video),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(videos.total, 1);
        assert_eq!(videos.entries[0].title, "Holiday");

        let search = library
            .query(&LibraryQuery {
                search: "holiday".into(),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(search.total, 1);
        assert_eq!(search.entries[0].name, "Holiday_2026-01-03_03-04-05.mp4");

        let images = library
            .query(&LibraryQuery {
                search: "screenshot".into(),
                kind: Some(MediaKind::Image),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(images.total, 1);
        assert_eq!(images.entries[0].kind, MediaKind::Image);
    }

    #[test]
    fn library_cleanup_removes_only_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        let old = media.join("Old_2026-01-02_03-04-05.png");
        let fresh = media.join("Fresh_2026-01-03_03-04-05.png");
        std::fs::write(&old, b"x").unwrap();
        std::fs::write(&fresh, b"x").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(UNIX_EPOCH)
            .unwrap();
        let library = LibraryHandle::open(media).unwrap();
        library.refresh().unwrap();
        let report = library.cleanup(7).unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.failed, 0);
        assert!(!old.exists());
        assert!(fresh.exists());
        assert_eq!(library.query(&LibraryQuery::default()).unwrap().total, 1);
    }

    #[test]
    fn library_renames_and_deletes_files() {
        let dir = tempfile::tempdir().unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        let old = media.join("Old_2026-01-02_03-04-05.png");
        let new = media.join("New_2026-01-02_03-04-05.png");
        std::fs::write(&old, b"x").unwrap();

        let library = LibraryHandle::open(media).unwrap();
        library.refresh().unwrap();
        let revision = library.revision();

        library.rename(&old, &new, "New").unwrap();
        assert!(!old.exists());
        assert!(new.exists());
        let page = library.query(&LibraryQuery::default()).unwrap();
        assert_eq!(page.total, 1);
        assert_eq!(page.entries[0].path, new);
        assert_eq!(page.entries[0].title, "New");
        assert!(library.revision() > revision);

        let revision = library.revision();
        library.delete(std::slice::from_ref(&new), true).unwrap();
        assert!(!new.exists());
        let page = library.query(&LibraryQuery::default()).unwrap();
        assert_eq!(page.total, 0);
        assert!(page.entries.is_empty());
        assert!(library.revision() > revision);
    }
}
