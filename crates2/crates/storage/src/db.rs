use crate::SCHEMA_VERSION;
use chrono::NaiveDateTime;
use mediaa2_core::formats::MediaKind;
use mediaa2_core::library::{Details, LibraryEntry};
use mediaa2_core::query::{LibraryQuery, LibrarySort};
use rusqlite::types::{Value, ValueRef};
use rusqlite::{params, params_from_iter, Connection, Row};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const TAKEN_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

const OPEN_PRAGMAS: &str = "
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA busy_timeout = 5000;
";

const DROP_ENTRIES: &str = "DROP TABLE IF EXISTS entries;";

const CREATE_ENTRIES: &str = "
CREATE TABLE entries (
    path      TEXT PRIMARY KEY,
    kind      TEXT NOT NULL,
    size      INTEGER NOT NULL,
    modified  INTEGER NOT NULL,
    taken     TEXT,
    title     TEXT NOT NULL,
    width     INTEGER,
    height    INTEGER,
    duration  REAL,
    audio     INTEGER
);
CREATE INDEX entries_modified ON entries(modified DESC);
CREATE INDEX entries_title ON entries(title COLLATE NOCASE);
CREATE INDEX entries_size ON entries(size DESC);
CREATE INDEX entries_kind ON entries(kind);
";

const UPSERT_ENTRY: &str = "
INSERT INTO entries (path, kind, size, modified, taken, title, width, height, duration, audio)
VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
ON CONFLICT(path) DO UPDATE SET
    kind = excluded.kind,
    size = excluded.size,
    modified = excluded.modified,
    taken = excluded.taken,
    title = excluded.title,
    width = excluded.width,
    height = excluded.height,
    duration = excluded.duration,
    audio = excluded.audio;
";

const DELETE_ENTRY: &str = "DELETE FROM entries WHERE path = ?1;";

const UPDATE_TITLE: &str = "UPDATE entries SET title = ?1 WHERE path = ?2;";

const UPDATE_PATH: &str = "UPDATE OR REPLACE entries SET path = ?1 WHERE path = ?2;";

const SELECT_PATHS: &str = "SELECT path FROM entries ORDER BY path;";

const SELECT_COUNT: &str = "SELECT COUNT(*) FROM entries";

const SELECT_ENTRIES: &str =
    "SELECT path, kind, size, modified, taken, title, width, height, duration, audio FROM entries";

struct Filter {
    clause: String,
    values: Vec<Value>,
}

impl Filter {
    fn new(query: &LibraryQuery) -> Self {
        let mut terms: Vec<&'static str> = Vec::new();
        let mut values = Vec::new();
        if let Some(kind) = query.kind {
            terms.push("kind = ?");
            values.push(Value::Text(kind.code().to_string()));
        }
        if let Some(pattern) = like_pattern(&query.search) {
            terms.push("(title LIKE ? ESCAPE '\\' OR path LIKE ? ESCAPE '\\')");
            values.push(Value::Text(pattern.clone()));
            values.push(Value::Text(pattern));
        }
        let clause = if terms.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", terms.join(" AND "))
        };
        Self { clause, values }
    }
}

pub struct Db {
    conn: Connection,
}

impl Db {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.execute_batch(OPEN_PRAGMAS)?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version != i64::from(SCHEMA_VERSION) {
            let transaction = conn.unchecked_transaction()?;
            transaction.execute_batch(DROP_ENTRIES)?;
            transaction.execute_batch(CREATE_ENTRIES)?;
            transaction.pragma_update(None, "user_version", i64::from(SCHEMA_VERSION))?;
            transaction.commit()?;
        }
        Ok(Self { conn })
    }

    pub fn query(&self, query: &LibraryQuery) -> anyhow::Result<(Vec<LibraryEntry>, u32)> {
        let filter = Filter::new(query);
        let total = self.count_entries(&filter)?;
        let limit = query.limit.clamp(1, 500);
        let sql = format!(
            "{}{} ORDER BY {} LIMIT ? OFFSET ?",
            SELECT_ENTRIES,
            filter.clause,
            order_by(query.sort)
        );
        let mut values = filter.values.clone();
        values.push(Value::Integer(i64::from(limit)));
        values.push(Value::Integer(i64::from(query.offset)));
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(params_from_iter(values), parse_entry)?;
        let mut entries = Vec::new();
        for row in rows {
            if let Some(entry) = row? {
                entries.push(entry);
            }
        }
        Ok((entries, total))
    }

    pub fn upsert(&mut self, entries: &[LibraryEntry]) -> anyhow::Result<()> {
        if entries.is_empty() {
            return Ok(());
        }
        let transaction = self.conn.transaction()?;
        {
            let mut statement = transaction.prepare_cached(UPSERT_ENTRY)?;
            for entry in entries {
                let (width, height, duration, audio) = details_columns(entry.details);
                statement.execute(params![
                    path_to_text(&entry.path),
                    entry.kind.code(),
                    i64::try_from(entry.size)?,
                    millis_from_system_time(entry.modified),
                    taken_to_text(entry.taken),
                    &entry.title,
                    width,
                    height,
                    duration,
                    audio,
                ])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn remove(&mut self, paths: &[PathBuf]) -> anyhow::Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        let transaction = self.conn.transaction()?;
        {
            let mut statement = transaction.prepare_cached(DELETE_ENTRY)?;
            for path in paths {
                statement.execute(params![path_to_text(path)])?;
            }
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn set_title(&mut self, path: &Path, title: &str) -> anyhow::Result<()> {
        self.conn
            .execute(UPDATE_TITLE, params![title, path_to_text(path)])?;
        Ok(())
    }

    pub fn rename_path(&mut self, from: &Path, to: &Path) -> anyhow::Result<()> {
        self.conn
            .execute(UPDATE_PATH, params![path_to_text(to), path_to_text(from)])?;
        Ok(())
    }

    pub fn paths(&self) -> anyhow::Result<Vec<PathBuf>> {
        Ok(self
            .stored_paths()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn remove_missing(&mut self) -> anyhow::Result<Vec<PathBuf>> {
        let missing: Vec<String> = self
            .stored_paths()?
            .into_iter()
            .filter(|path| file_is_missing(Path::new(path)))
            .collect();
        if missing.is_empty() {
            return Ok(Vec::new());
        }
        let transaction = self.conn.transaction()?;
        {
            let mut statement = transaction.prepare_cached(DELETE_ENTRY)?;
            for path in &missing {
                statement.execute(params![path.as_str()])?;
            }
        }
        transaction.commit()?;
        Ok(missing.into_iter().map(PathBuf::from).collect())
    }

    fn count_entries(&self, filter: &Filter) -> anyhow::Result<u32> {
        let sql = format!("{}{}", SELECT_COUNT, filter.clause);
        let total: i64 =
            self.conn
                .query_row(&sql, params_from_iter(filter.values.iter()), |row| {
                    row.get(0)
                })?;
        Ok(u32::try_from(total).unwrap_or(u32::MAX))
    }

    fn stored_paths(&self) -> anyhow::Result<Vec<String>> {
        let mut statement = self.conn.prepare(SELECT_PATHS)?;
        let rows =
            statement.query_map(params![], |row| Ok(text_column(row, 0).unwrap_or_default()))?;
        let mut paths = Vec::new();
        for row in rows {
            let path = row?;
            if !path.is_empty() {
                paths.push(path);
            }
        }
        Ok(paths)
    }
}

fn order_by(sort: LibrarySort) -> &'static str {
    match sort {
        LibrarySort::Newest => "modified DESC, path ASC",
        LibrarySort::Oldest => "modified ASC, path ASC",
        LibrarySort::Title => "title COLLATE NOCASE ASC, modified DESC, path ASC",
        LibrarySort::Largest => "size DESC, modified DESC, path ASC",
    }
}

fn like_pattern(search: &str) -> Option<String> {
    let term = search.trim();
    if term.is_empty() {
        return None;
    }
    let mut pattern = String::with_capacity(term.len() + 2);
    pattern.push('%');
    for ch in term.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            pattern.push('\\');
        }
        pattern.push(ch);
    }
    pattern.push('%');
    Some(pattern)
}

fn parse_entry(row: &Row<'_>) -> rusqlite::Result<Option<LibraryEntry>> {
    let Some(path) = text_column(row, 0).filter(|path| !path.is_empty()) else {
        return Ok(None);
    };
    let Some(size) = integer_column(row, 2).and_then(|size| u64::try_from(size).ok()) else {
        return Ok(None);
    };
    let Some(modified) = integer_column(row, 3).and_then(system_time_from_millis) else {
        return Ok(None);
    };
    let path = PathBuf::from(path);
    let name = name_from_path(&path);
    let kind = text_column(row, 1)
        .and_then(|code| MediaKind::from_code(&code))
        .unwrap_or(MediaKind::Image);
    let title = text_column(row, 5)
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| name.clone());
    let taken = text_column(row, 4)
        .and_then(|text| taken_from_text(&text))
        .unwrap_or_default();
    let width = integer_column(row, 6).and_then(|width| u32::try_from(width).ok());
    let details = width.map(|width| Details {
        width,
        height: integer_column(row, 7)
            .and_then(|height| u32::try_from(height).ok())
            .unwrap_or(0),
        duration_secs: real_column(row, 8).filter(|duration| duration.is_finite()),
        audio: integer_column(row, 9).is_some_and(|audio| audio != 0),
    });
    Ok(Some(LibraryEntry {
        path,
        name,
        title,
        kind,
        size,
        modified,
        taken,
        details,
    }))
}

fn text_column(row: &Row<'_>, index: usize) -> Option<String> {
    match row.get_ref(index).ok()? {
        ValueRef::Text(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Real(value) => Some(value.to_string()),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn integer_column(row: &Row<'_>, index: usize) -> Option<i64> {
    match row.get_ref(index).ok()? {
        ValueRef::Integer(value) => Some(value),
        ValueRef::Real(value) if value.is_finite() => Some(value as i64),
        ValueRef::Real(_) => None,
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse().ok(),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn real_column(row: &Row<'_>, index: usize) -> Option<f64> {
    match row.get_ref(index).ok()? {
        ValueRef::Real(value) => Some(value),
        ValueRef::Integer(value) => Some(value as f64),
        ValueRef::Text(bytes) => std::str::from_utf8(bytes).ok()?.trim().parse().ok(),
        ValueRef::Null | ValueRef::Blob(_) => None,
    }
}

fn system_time_from_millis(millis: i64) -> Option<SystemTime> {
    let span = Duration::from_millis(millis.unsigned_abs());
    if millis >= 0 {
        UNIX_EPOCH.checked_add(span)
    } else {
        UNIX_EPOCH.checked_sub(span)
    }
}

fn millis_from_system_time(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_millis()).unwrap_or(i64::MAX),
        Err(before) => -i64::try_from(before.duration().as_millis()).unwrap_or(i64::MAX),
    }
}

fn taken_to_text(taken: NaiveDateTime) -> String {
    taken.format(TAKEN_FORMAT).to_string()
}

fn taken_from_text(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text, TAKEN_FORMAT).ok()
}

fn path_to_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

fn name_from_path(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn details_columns(
    details: Option<Details>,
) -> (Option<i64>, Option<i64>, Option<f64>, Option<i64>) {
    match details {
        Some(details) => (
            Some(i64::from(details.width)),
            Some(i64::from(details.height)),
            details
                .duration_secs
                .filter(|duration| duration.is_finite()),
            Some(if details.audio { 1_i64 } else { 0 }),
        ),
        None => (None, None, None, None),
    }
}

fn file_is_missing(path: &Path) -> bool {
    match std::fs::metadata(path) {
        Ok(_) => false,
        Err(error) => error.kind() == std::io::ErrorKind::NotFound,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;
    use rusqlite::Connection;

    fn taken() -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 1, 2)
            .and_then(|date| date.and_hms_opt(3, 4, 5))
            .unwrap()
    }

    fn entry(
        dir: &Path,
        name: &str,
        title: &str,
        kind: MediaKind,
        size: u64,
        modified_ms: u64,
    ) -> LibraryEntry {
        LibraryEntry {
            path: dir.join(name),
            name: name.to_string(),
            title: title.to_string(),
            kind,
            size,
            modified: UNIX_EPOCH + Duration::from_millis(modified_ms),
            taken: taken(),
            details: None,
        }
    }

    fn open(dir: &Path) -> Db {
        Db::open(&dir.join("library.db")).unwrap()
    }

    fn names(db: &Db, sort: LibrarySort) -> Vec<String> {
        let query = LibraryQuery {
            sort,
            ..Default::default()
        };
        let (entries, total) = db.query(&query).unwrap();
        assert_eq!(total, 3);
        entries.into_iter().map(|entry| entry.name).collect()
    }

    #[test]
    fn upsert_query_roundtrip_covers_every_sort() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(dir.path());
        let alpha = entry(
            dir.path(),
            "alpha.png",
            "Alpha",
            MediaKind::Image,
            30,
            1_000,
        );
        let beta = entry(dir.path(), "beta.mp4", "beta", MediaKind::Video, 10, 3_000);
        let gamma = entry(
            dir.path(),
            "gamma.gif",
            "Gamma",
            MediaKind::Animation,
            20,
            2_000,
        );
        let mut beta_details = beta.clone();
        beta_details.details = Some(Details {
            width: 1920,
            height: 1080,
            duration_secs: Some(12.5),
            audio: true,
        });
        db.upsert(&[alpha.clone(), beta_details.clone(), gamma.clone()])
            .unwrap();

        assert_eq!(
            names(&db, LibrarySort::Newest),
            vec![
                "beta.mp4".to_string(),
                "gamma.gif".to_string(),
                "alpha.png".to_string()
            ]
        );
        assert_eq!(
            names(&db, LibrarySort::Oldest),
            vec![
                "alpha.png".to_string(),
                "gamma.gif".to_string(),
                "beta.mp4".to_string()
            ]
        );
        assert_eq!(
            names(&db, LibrarySort::Title),
            vec![
                "alpha.png".to_string(),
                "beta.mp4".to_string(),
                "gamma.gif".to_string()
            ]
        );
        assert_eq!(
            names(&db, LibrarySort::Largest),
            vec![
                "alpha.png".to_string(),
                "gamma.gif".to_string(),
                "beta.mp4".to_string()
            ]
        );

        let (entries, total) = db.query(&LibraryQuery::default()).unwrap();
        assert_eq!(total, 3);
        let loaded = entries
            .iter()
            .find(|entry| entry.path == beta_details.path)
            .unwrap();
        assert_eq!(loaded, &beta_details);
    }

    #[test]
    fn query_filters_kind_and_searches_title_and_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(dir.path());
        let alpha = entry(
            dir.path(),
            "alpha.png",
            "Alpha",
            MediaKind::Image,
            30,
            1_000,
        );
        let clip = entry(
            dir.path(),
            "clip.mp4",
            "Holiday",
            MediaKind::Video,
            10,
            3_000,
        );
        let weird = entry(
            dir.path(),
            "weird.gif",
            "100% done",
            MediaKind::Animation,
            20,
            2_000,
        );
        db.upsert(&[alpha, clip, weird]).unwrap();

        let only_video = LibraryQuery {
            kind: Some(MediaKind::Video),
            ..Default::default()
        };
        let (entries, total) = db.query(&only_video).unwrap();
        assert_eq!(total, 1);
        assert_eq!(entries[0].name, "clip.mp4");

        let by_title = LibraryQuery {
            search: "hOlIdAy".to_string(),
            ..Default::default()
        };
        let (entries, total) = db.query(&by_title).unwrap();
        assert_eq!((total, entries.len()), (1, 1));
        assert_eq!(entries[0].name, "clip.mp4");

        let by_name = LibraryQuery {
            search: "alp".to_string(),
            ..Default::default()
        };
        let (entries, total) = db.query(&by_name).unwrap();
        assert_eq!((total, entries.len()), (1, 1));
        assert_eq!(entries[0].name, "alpha.png");

        let literal_percent = LibraryQuery {
            search: "%".to_string(),
            ..Default::default()
        };
        let (entries, total) = db.query(&literal_percent).unwrap();
        assert_eq!((total, entries.len()), (1, 1));
        assert_eq!(entries[0].title, "100% done");
    }

    #[test]
    fn pagination_reports_total_and_next_offset() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(dir.path());
        let entries: Vec<LibraryEntry> = (0_u32..5)
            .map(|index| {
                entry(
                    dir.path(),
                    &format!("{index}.png"),
                    &format!("Item {index}"),
                    MediaKind::Image,
                    u64::from(index),
                    1_000 + u64::from(index),
                )
            })
            .collect();
        db.upsert(&entries).unwrap();

        let first = LibraryQuery {
            limit: 2,
            ..Default::default()
        };
        let (page, total) = db.query(&first).unwrap();
        assert_eq!(total, 5);
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].name, "4.png");
        assert_eq!(page[1].name, "3.png");
        let next = first.offset + page.len() as u32;
        assert_eq!(next, 2);
        assert!(next < total);

        let last = LibraryQuery {
            offset: 4,
            limit: 2,
            ..Default::default()
        };
        let (page, total) = db.query(&last).unwrap();
        assert_eq!(total, 5);
        assert_eq!(page.len(), 1);
        let next = last.offset + page.len() as u32;
        assert_eq!(next, 5);
        assert!(!(next < total));

        let clamped = LibraryQuery {
            limit: 0,
            ..Default::default()
        };
        let (page, _) = db.query(&clamped).unwrap();
        assert_eq!(page.len(), 1);

        let beyond = LibraryQuery {
            offset: 10,
            limit: 2,
            ..Default::default()
        };
        let (page, total) = db.query(&beyond).unwrap();
        assert!(page.is_empty());
        assert_eq!(total, 5);
    }

    #[test]
    fn rename_path_and_set_title_update_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(dir.path());
        let old_path = dir.path().join("old.png");
        let mut item = entry(dir.path(), "old.png", "Old", MediaKind::Image, 7, 1_000);
        db.upsert(std::slice::from_ref(&item)).unwrap();
        db.set_title(&old_path, "New title").unwrap();
        let (entries, _) = db.query(&LibraryQuery::default()).unwrap();
        assert_eq!(entries[0].title, "New title");
        assert_eq!(entries[0].name, "old.png");

        let new_path = dir.path().join("new.png");
        db.rename_path(&old_path, &new_path).unwrap();
        let (entries, total) = db.query(&LibraryQuery::default()).unwrap();
        assert_eq!(total, 1);
        assert_eq!(entries[0].path, new_path);
        assert_eq!(entries[0].name, "new.png");
        assert_eq!(entries[0].title, "New title");
        assert_eq!(db.paths().unwrap(), vec![new_path.clone()]);

        item.path = new_path;
        item.name = "new.png".to_string();
        item.title = "New title".to_string();
        assert_eq!(entries[0], item);
    }

    #[test]
    fn remove_missing_deletes_only_absent_files() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(dir.path());
        let a = entry(dir.path(), "a.png", "A", MediaKind::Image, 1, 1_000);
        let b = entry(dir.path(), "b.png", "B", MediaKind::Image, 2, 2_000);
        let c = entry(dir.path(), "c.png", "C", MediaKind::Image, 3, 3_000);
        for item in [&a, &b, &c] {
            std::fs::write(&item.path, b"x").unwrap();
        }
        db.upsert(&[a.clone(), b.clone(), c.clone()]).unwrap();
        std::fs::remove_file(&b.path).unwrap();

        let removed = db.remove_missing().unwrap();
        assert_eq!(removed, vec![b.path.clone()]);
        let (entries, total) = db.query(&LibraryQuery::default()).unwrap();
        assert_eq!(total, 2);
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|entry| entry.path != b.path));
        assert!(db.remove_missing().unwrap().is_empty());

        db.remove(std::slice::from_ref(&c.path)).unwrap();
        assert_eq!(db.query(&LibraryQuery::default()).unwrap().1, 1);
    }

    #[test]
    fn opening_a_foreign_schema_rebuilds_the_cache() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("library.db");
        let item = entry(dir.path(), "a.png", "A", MediaKind::Image, 1, 1_000);
        {
            let mut db = Db::open(&path).unwrap();
            db.upsert(std::slice::from_ref(&item)).unwrap();
            assert_eq!(db.query(&LibraryQuery::default()).unwrap().1, 1);
        }
        {
            let db = Db::open(&path).unwrap();
            assert_eq!(db.query(&LibraryQuery::default()).unwrap().1, 1);
        }
        {
            let connection = Connection::open(&path).unwrap();
            connection.pragma_update(None, "user_version", 99).unwrap();
        }
        let db = Db::open(&path).unwrap();
        let (entries, total) = db.query(&LibraryQuery::default()).unwrap();
        assert_eq!(total, 0);
        assert!(entries.is_empty());
        let version: i64 = db
            .conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, 1);
    }

    #[test]
    fn corrupt_rows_do_not_abort_queries() {
        let dir = tempfile::tempdir().unwrap();
        let mut db = open(dir.path());
        let good = entry(dir.path(), "good.png", "Good", MediaKind::Image, 4, 1_000);
        db.upsert(std::slice::from_ref(&good)).unwrap();
        db.conn
            .execute(
                "INSERT INTO entries (path, kind, size, modified, taken, title, width, height, duration, audio) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    "negative.png",
                    "image",
                    -1_i64,
                    0_i64,
                    "2026-01-01 00:00:00",
                    "Negative",
                    10_i64,
                    10_i64,
                    Option::<f64>::None,
                    Option::<i64>::None,
                ],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO entries (path, kind, size, modified, taken, title, width, height, duration, audio) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    "text-modified.png",
                    "image",
                    1_i64,
                    "not a number",
                    "2026-01-01 00:00:00",
                    "Text modified",
                    Option::<i64>::None,
                    Option::<i64>::None,
                    Option::<f64>::None,
                    Option::<i64>::None,
                ],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO entries (path, kind, size, modified, taken, title, width, height, duration, audio) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    "mystery.bin",
                    "hologram",
                    5_i64,
                    2_000_i64,
                    "not a date",
                    "",
                    -1_i64,
                    Option::<i64>::None,
                    Option::<f64>::None,
                    Option::<i64>::None,
                ],
            )
            .unwrap();
        db.conn
            .execute(
                "INSERT INTO entries (path, kind, size, modified, taken, title, width, height, duration, audio) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    "",
                    "image",
                    1_i64,
                    0_i64,
                    "2026-01-01 00:00:00",
                    "Empty path",
                    Option::<i64>::None,
                    Option::<i64>::None,
                    Option::<f64>::None,
                    Option::<i64>::None,
                ],
            )
            .unwrap();

        let (entries, _total) = db.query(&LibraryQuery::default()).unwrap();
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert!(names.contains(&"good.png"));
        assert!(names.contains(&"mystery.bin"));
        assert!(!names.contains(&"negative.png"));
        assert!(!names.contains(&"text-modified.png"));
        assert!(!names.contains(&""));
        let mystery = entries
            .iter()
            .find(|entry| entry.name == "mystery.bin")
            .unwrap();
        assert_eq!(mystery.kind, MediaKind::Image);
        assert_eq!(mystery.title, "mystery.bin");
        assert!(mystery.details.is_none());
        assert_eq!(mystery.taken, NaiveDateTime::default());
    }
}
