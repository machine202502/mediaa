use mediaa_protocol::backend::{
    DetailsWire, LibraryEditWire, LibraryEntryWire, LibraryPageWire, LibraryQueryWire,
    LibrarySortWire,
};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::Duration;

const LIMIT_MIN: u32 = 1;
const LIMIT_MAX: u32 = 500;
const DEFAULT_LIMIT: u32 = 100;

#[derive(Debug)]
pub struct LibraryStore {
    db: Connection,
    dir: PathBuf,
    version: u64,
}

impl LibraryStore {
    pub fn open(media_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(media_dir)?;
        let cache = media_dir.join(".cache");
        std::fs::create_dir_all(&cache)?;
        let db = Connection::open(cache.join("library.db"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        let has_path: i64 = db.query_row(
            "SELECT COUNT(*) FROM pragma_table_info('files') WHERE name = 'path'",
            [],
            |row| row.get(0),
        )?;
        if has_path == 0 {
            db.execute_batch("DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS previews;")?;
        }
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                title TEXT NOT NULL,
                kind TEXT NOT NULL,
                size INTEGER NOT NULL,
                modified_ms INTEGER NOT NULL,
                taken TEXT,
                details TEXT
            );
",
        )?;
        let version = read_version(&db)?;
        Ok(Self {
            db,
            dir: media_dir.to_path_buf(),
            version,
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn upsert(&mut self, entry: &LibraryEntryWire) -> anyhow::Result<()> {
        let details = entry
            .details
            .as_ref()
            .map(serde_json::to_string)
            .transpose()?;
        self.db.execute(
            "INSERT INTO files(path, name, title, kind, size, modified_ms, taken, details)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(path) DO UPDATE SET
                name=excluded.name,
                title=excluded.title,
                kind=excluded.kind,
                size=excluded.size,
                modified_ms=excluded.modified_ms,
                taken=excluded.taken,
                details=excluded.details",
            params![
                entry.path,
                entry.name,
                entry.title,
                format!("{:?}", entry.kind).to_lowercase(),
                entry.size as i64,
                entry.modified_ms as i64,
                entry.taken,
                details
            ],
        )?;
        self.bump()?;
        Ok(())
    }

    pub fn delete(&mut self, path: &str) -> anyhow::Result<bool> {
        let removed = self
            .db
            .execute("DELETE FROM files WHERE path = ?1", params![path])?;
        if removed > 0 {
            self.bump()?;
        }
        Ok(removed > 0)
    }

    pub fn edit(&mut self, edit: &LibraryEditWire) -> anyhow::Result<bool> {
        match edit {
            LibraryEditWire::Rename { path, title } => {
                let changed = self.db.execute(
                    "UPDATE files SET title = ?2 WHERE path = ?1",
                    params![path, title],
                )?;
                if changed > 0 {
                    self.bump()?;
                }
                Ok(changed > 0)
            }
            LibraryEditWire::Delete { path } => self.delete(path),
        }
    }

    pub fn query(&self, query: &LibraryQueryWire) -> anyhow::Result<LibraryPageWire> {
        let limit = if query.limit == 0 {
            DEFAULT_LIMIT
        } else {
            query.limit.clamp(LIMIT_MIN, LIMIT_MAX)
        };
        let offset = query.cursor.unwrap_or(0) as i64;
        let mut sql = String::from(
            "SELECT path, name, title, kind, size, modified_ms, taken, details FROM files WHERE 1=1",
        );
        let mut values: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
        if !query.search.trim().is_empty() {
            sql.push_str(" AND (title LIKE ?1 OR name LIKE ?1)");
            values.push(Box::new(format!("%{}%", query.search.trim())));
        }
        if let Some(kind) = query.kind {
            let marker = if values.is_empty() { "?1" } else { "?2" };
            sql.push_str(&format!(" AND kind = {marker}"));
            values.push(Box::new(format!("{kind:?}").to_lowercase()));
        }
        sql.push_str(match query.sort {
            LibrarySortWire::Newest => " ORDER BY modified_ms DESC, path ASC",
            LibrarySortWire::Oldest => " ORDER BY modified_ms ASC, path ASC",
            LibrarySortWire::Title => " ORDER BY title COLLATE NOCASE ASC, path ASC",
            LibrarySortWire::Largest => " ORDER BY size DESC, path ASC",
        });
        let limit_marker = values.len() + 1;
        let offset_marker = values.len() + 2;
        sql.push_str(&format!(" LIMIT ?{limit_marker} OFFSET ?{offset_marker}"));
        values.push(Box::new(i64::from(limit) + 1));
        values.push(Box::new(offset));

        let mut statement = self.db.prepare(&sql)?;
        let params: Vec<&dyn rusqlite::ToSql> = values.iter().map(|v| v.as_ref()).collect();
        let rows = statement.query_map(params.as_slice(), row_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        let next_cursor = if entries.len() > limit as usize {
            entries.truncate(limit as usize);
            Some(offset as u64 + limit as u64)
        } else {
            None
        };
        Ok(LibraryPageWire {
            entries,
            next_cursor,
            version: self.version,
        })
    }

    fn bump(&mut self) -> anyhow::Result<()> {
        self.version += 1;
        self.db
            .pragma_update(None, "user_version", self.version as i64)?;
        Ok(())
    }
}

fn read_version(db: &Connection) -> anyhow::Result<u64> {
    let value: i64 = db.pragma_query_value(None, "user_version", |row| row.get(0))?;
    Ok(value.max(0) as u64)
}

fn row_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryEntryWire> {
    let kind: String = row.get(3)?;
    let details: Option<String> = row.get(7)?;
    Ok(LibraryEntryWire {
        path: row.get(0)?,
        name: row.get(1)?,
        title: row.get(2)?,
        kind: parse_kind(&kind),
        size: row.get::<_, i64>(4)?.max(0) as u64,
        modified_ms: row.get::<_, i64>(5)?.max(0) as u64,
        taken: row.get(6)?,
        details: details.and_then(|text| serde_json::from_str::<DetailsWire>(&text).ok()),
    })
}

fn parse_kind(value: &str) -> mediaa_core::formats::MediaKind {
    match value {
        "video" => mediaa_core::formats::MediaKind::Video,
        "animation" => mediaa_core::formats::MediaKind::Animation,
        "audio" => mediaa_core::formats::MediaKind::Audio,
        _ => mediaa_core::formats::MediaKind::Image,
    }
}

impl LibraryStore {
    pub fn reset_file_index(&mut self) -> anyhow::Result<()> {
        self.db.execute_batch("DELETE FROM files;")?;
        self.bump()?;
        Ok(())
    }

    pub fn upsert_path(&mut self, path: &Path) -> anyhow::Result<Option<LibraryEntryWire>> {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_file() => metadata,
            _ => {
                self.delete_path(path)?;
                return Ok(None);
            }
        };
        let modified_ms = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |duration| duration.as_millis() as u64);
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let kind = mediaa_core::formats::MediaKind::detect(path)
            .unwrap_or(mediaa_core::formats::MediaKind::Image);
        let entry = LibraryEntryWire {
            path: path.to_string_lossy().into_owned(),
            name,
            title,
            kind,
            size: metadata.len(),
            modified_ms,
            taken: None,
            details: None,
        };
        self.upsert(&entry)?;
        Ok(Some(entry))
    }

    pub fn delete_path(&mut self, path: &Path) -> anyhow::Result<bool> {
        self.delete(&path.to_string_lossy())
    }

    pub fn snapshot(&self) -> anyhow::Result<LibraryPageWire> {
        Ok(LibraryPageWire {
            entries: self.entries()?,
            next_cursor: None,
            version: self.version,
        })
    }
    pub fn entries(&self) -> anyhow::Result<Vec<LibraryEntryWire>> {
        let mut statement = self.db.prepare(
            "SELECT path, name, title, kind, size, modified_ms, taken, details FROM files",
        )?;
        let rows = statement.query_map([], row_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            entries.push(row?);
        }
        Ok(entries)
    }

    pub fn get(&self, path: &str) -> anyhow::Result<Option<LibraryEntryWire>> {
        self.db
            .query_row(
                "SELECT path, name, title, kind, size, modified_ms, taken, details
                 FROM files WHERE path = ?1",
                params![path],
                row_entry,
            )
            .optional()
            .map_err(Into::into)
    }
}

pub fn library_dir(settings: &mediaa_core::Settings) -> std::path::PathBuf {
    crate::dev_mode::mode()
        .library
        .clone()
        .unwrap_or_else(|| settings.media_dir.clone())
}
#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::formats::MediaKind;

    fn entry(path: &str, title: &str, modified_ms: u64) -> LibraryEntryWire {
        LibraryEntryWire {
            path: path.into(),
            name: path.rsplit('/').next().unwrap_or(path).into(),
            title: title.into(),
            kind: MediaKind::Video,
            size: 10,
            modified_ms,
            taken: None,
            details: None,
        }
    }

    #[test]
    fn paths_can_be_added_to_the_snapshot_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("capture.png");
        std::fs::write(&file, b"png").unwrap();
        let mut store = LibraryStore::open(dir.path()).unwrap();
        let entry = store.upsert_path(&file).unwrap().expect("entry");
        assert_eq!(entry.name, "capture.png");
        assert_eq!(store.snapshot().unwrap().entries.len(), 1);
        assert!(store.delete_path(&file).unwrap());
        assert!(store.snapshot().unwrap().entries.is_empty());
    }
    #[test]
    fn library_store_queries_edits_and_deletes() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LibraryStore::open(dir.path()).unwrap();
        store.upsert(&entry("C:/media/a.mp4", "alpha", 2)).unwrap();
        store.upsert(&entry("C:/media/b.mp4", "beta", 1)).unwrap();
        let page = store
            .query(&LibraryQueryWire {
                search: String::new(),
                kind: Some(MediaKind::Video),
                sort: LibrarySortWire::Newest,
                limit: 1,
                cursor: None,
            })
            .unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].title, "alpha");
        assert!(page.next_cursor.is_some());
        let second = store
            .query(&LibraryQueryWire {
                search: String::new(),
                kind: Some(MediaKind::Video),
                sort: LibrarySortWire::Newest,
                limit: 1,
                cursor: page.next_cursor,
            })
            .unwrap();
        assert_eq!(second.entries[0].title, "beta");
        assert!(store
            .edit(&LibraryEditWire::Rename {
                path: "C:/media/a.mp4".into(),
                title: "renamed".into(),
            })
            .unwrap());
        assert_eq!(
            store.get("C:/media/a.mp4").unwrap().unwrap().title,
            "renamed"
        );
        assert!(store
            .edit(&LibraryEditWire::Delete {
                path: "C:/media/a.mp4".into()
            })
            .unwrap());
        assert!(store.get("C:/media/a.mp4").unwrap().is_none());
    }
}
