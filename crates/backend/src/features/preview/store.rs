use mediaa_protocol::backend::{PreviewKindWire, PreviewRequestWire};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewDecision {
    Ready { path: String },
    Pending { enqueue: bool },
}

#[derive(Debug)]
pub struct PreviewStore {
    db: Connection,
    dir: PathBuf,
}

impl PreviewStore {
    pub fn open(media_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(media_dir)?;
        let cache = media_dir.join(".cache");
        std::fs::create_dir_all(&cache)?;
        let db = Connection::open(cache.join("library.db"))?;
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "synchronous", "NORMAL")?;
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS backend_previews (
                path TEXT NOT NULL,
                kind TEXT NOT NULL,
                preview_path TEXT,
                state TEXT NOT NULL,
                requested_at INTEGER,
                last_requested_at INTEGER,
                error TEXT,
                PRIMARY KEY (path, kind)
            );
            CREATE INDEX IF NOT EXISTS backend_previews_state ON backend_previews(state);",
        )?;
        Ok(Self {
            db,
            dir: media_dir.to_path_buf(),
        })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn request(&mut self, request: &PreviewRequestWire) -> anyhow::Result<PreviewDecision> {
        let kind = preview_kind(request.kind);
        let now = now_ms();
        let row: Option<(Option<String>, String)> = self
            .db
            .query_row(
                "SELECT preview_path, state FROM backend_previews WHERE path=?1 AND kind=?2",
                params![request.path, kind],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((Some(preview), state)) = row.as_ref() {
            if state == "ready" && Path::new(preview).is_file() {
                self.touch(&request.path, kind, now)?;
                return Ok(PreviewDecision::Ready {
                    path: preview.clone(),
                });
            }
            if state == "queued" || state == "generating" {
                self.touch(&request.path, kind, now)?;
                return Ok(PreviewDecision::Pending { enqueue: false });
            }
        }
        self.db.execute(
            "INSERT INTO backend_previews(path, kind, preview_path, state, requested_at, last_requested_at, error)
             VALUES (?1, ?2, NULL, 'queued', ?3, ?3, NULL)
             ON CONFLICT(path, kind) DO UPDATE SET
                state='queued', preview_path=NULL, requested_at=excluded.requested_at,
                last_requested_at=excluded.last_requested_at, error=NULL",
            params![request.path, kind, now as i64],
        )?;
        Ok(PreviewDecision::Pending { enqueue: true })
    }

    pub fn started(&mut self, path: &str, kind: PreviewKindWire) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE backend_previews SET state='generating' WHERE path=?1 AND kind=?2 AND state='queued'",
            params![path, preview_kind(kind)],
        )?;
        Ok(())
    }

    pub fn finished(
        &mut self,
        path: &str,
        kind: PreviewKindWire,
        result: Result<&str, &str>,
    ) -> anyhow::Result<()> {
        let now = now_ms();
        match result {
            Ok(preview) => {
                self.db.execute(
                    "UPDATE backend_previews SET state='ready', preview_path=?3, error=NULL, last_requested_at=?4
                     WHERE path=?1 AND kind=?2",
                    params![path, preview_kind(kind), preview, now as i64],
                )?;
            }
            Err(error) => {
                self.db.execute(
                    "UPDATE backend_previews SET state='failed', preview_path=NULL, error=?3, last_requested_at=?4
                     WHERE path=?1 AND kind=?2",
                    params![path, preview_kind(kind), error, now as i64],
                )?;
            }
        }
        Ok(())
    }

    pub fn cleanup(&mut self, older_than_ms: u64) -> anyhow::Result<usize> {
        let mut statement = self.db.prepare(
            "SELECT path, kind, preview_path FROM backend_previews
             WHERE state='ready' AND last_requested_at IS NOT NULL AND last_requested_at < ?1",
        )?;
        let rows = statement.query_map(params![older_than_ms as i64], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut removed = 0;
        for row in rows {
            let (path, kind, preview) = row?;
            if let Some(preview) = preview {
                let _ = std::fs::remove_file(preview);
            }
            self.db.execute(
                "DELETE FROM backend_previews WHERE path=?1 AND kind=?2",
                params![path, kind],
            )?;
            removed += 1;
        }
        Ok(removed)
    }

    fn touch(&self, path: &str, kind: &str, now: u64) -> anyhow::Result<()> {
        self.db.execute(
            "UPDATE backend_previews SET last_requested_at=?3 WHERE path=?1 AND kind=?2",
            params![path, kind, now as i64],
        )?;
        Ok(())
    }
}

fn preview_kind(kind: PreviewKindWire) -> &'static str {
    match kind {
        PreviewKindWire::Thumbnail => "thumbnail",
        PreviewKindWire::Hover => "hover",
        PreviewKindWire::Player => "player",
    }
}

pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_millis() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_are_lazy_and_cleaned() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = PreviewStore::open(dir.path()).unwrap();
        let request = PreviewRequestWire {
            path: "C:/media/a.png".into(),
            kind: PreviewKindWire::Thumbnail,
        };
        assert_eq!(
            store.request(&request).unwrap(),
            PreviewDecision::Pending { enqueue: true }
        );
        store.started(&request.path, request.kind).unwrap();
        let preview = dir.path().join("a.png");
        std::fs::write(&preview, b"x").unwrap();
        store
            .finished(&request.path, request.kind, Ok(preview.to_str().unwrap()))
            .unwrap();
        assert_eq!(
            store.request(&request).unwrap(),
            PreviewDecision::Ready {
                path: preview.to_string_lossy().into_owned()
            }
        );
        assert_eq!(store.cleanup(i64::MAX as u64).unwrap(), 1);
        assert!(!preview.exists());
    }
}
