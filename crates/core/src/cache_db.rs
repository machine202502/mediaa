use rusqlite::Connection;
use std::path::Path;
use std::time::Duration;

const DB_FILE: &str = "library.db";
const SCHEMA: i64 = 1;

const CREATE: &str = "
    CREATE TABLE files (
        name TEXT PRIMARY KEY,
        title TEXT NOT NULL,
        kind TEXT NOT NULL,
        size INTEGER NOT NULL,
        modified_ms INTEGER NOT NULL,
        width INTEGER,
        height INTEGER,
        duration REAL,
        audio INTEGER
    ) WITHOUT ROWID;
    CREATE TABLE thumbs (
        name TEXT PRIMARY KEY,
        size INTEGER NOT NULL,
        modified_ms INTEGER NOT NULL,
        page INTEGER NOT NULL,
        x INTEGER NOT NULL,
        y INTEGER NOT NULL,
        w INTEGER NOT NULL,
        h INTEGER NOT NULL
    ) WITHOUT ROWID;
    CREATE INDEX thumbs_page ON thumbs(page);
    CREATE TABLE counters (key TEXT PRIMARY KEY, value INTEGER NOT NULL) WITHOUT ROWID;
";

pub fn open(cache_dir: &Path) -> anyhow::Result<Connection> {
    std::fs::create_dir_all(cache_dir)?;
    let db = Connection::open(cache_dir.join(DB_FILE))?;
    db.busy_timeout(Duration::from_secs(5))?;
    db.pragma_update(None, "journal_mode", "WAL")?;
    db.pragma_update(None, "synchronous", "NORMAL")?;
    let version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
    if version != SCHEMA {
        let tx = db.unchecked_transaction()?;
        tx.execute_batch("DROP TABLE IF EXISTS files; DROP TABLE IF EXISTS thumbs; DROP TABLE IF EXISTS counters;")?;
        tx.execute_batch(CREATE)?;
        tx.pragma_update(None, "user_version", SCHEMA)?;
        tx.commit()?;
    }
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_schema_once_and_rebuilds_other_versions() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(dir.path()).unwrap();
        db.execute("INSERT INTO counters VALUES ('x', 1)", [])
            .unwrap();
        drop(db);
        let db = open(dir.path()).unwrap();
        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM counters", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 1, "same schema: data kept");
        db.pragma_update(None, "user_version", 99).unwrap();
        drop(db);
        let db = open(dir.path()).unwrap();
        let n: i64 = db
            .query_row("SELECT COUNT(*) FROM counters", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0, "other schema: rebuilt");
    }
}
