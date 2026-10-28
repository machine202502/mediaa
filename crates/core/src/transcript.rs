use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

fn join(words: &[Word]) -> String {
    words
        .iter()
        .map(|w| w.text.as_str())
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Phrase {
    pub start: f64,
    pub end: f64,
    pub words: Vec<Word>,
}

impl Phrase {
    pub fn text(&self) -> String {
        join(&self.words)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub start: f64,
    pub end: f64,
    pub speaker: Option<u32>,
    pub words: Vec<Word>,
}

impl Reply {
    pub fn text(&self) -> String {
        join(&self.words)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NameSource {
    Manual,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NameEvidence {
    pub start: f64,
    pub text: String,
    pub how: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeakerName {
    pub name: String,
    pub source: NameSource,
    pub evidence: Vec<NameEvidence>,
}

const REPLY_GAP: f64 = 1.5;
const REPLY_MAX: f64 = 60.0;

fn speaker_at(turns: &[Turn], start: f64, end: f64) -> Option<u32> {
    let overlap = |t: &Turn| (end.min(t.end) - start.max(t.start)).max(0.0);
    let distance = |t: &Turn| {
        if end < t.start {
            t.start - end
        } else if start > t.end {
            start - t.end
        } else {
            0.0
        }
    };
    turns
        .iter()
        .max_by(|a, b| {
            overlap(a)
                .total_cmp(&overlap(b))
                .then(distance(b).total_cmp(&distance(a)))
        })
        .map(|t| t.speaker)
}

pub fn replies(phrases: &[Phrase], turns: &[Turn]) -> Vec<Reply> {
    let mut out: Vec<Reply> = Vec::new();
    for phrase in phrases {
        let mut phrase_start = true;
        for w in &phrase.words {
            let speaker = if turns.is_empty() {
                None
            } else {
                speaker_at(turns, w.start, w.end)
            };
            let joins = out.last().is_some_and(|r| {
                r.speaker == speaker
                    && (!phrase_start
                        || (w.start - r.end <= REPLY_GAP && r.end - r.start < REPLY_MAX))
            });
            match out.last_mut() {
                Some(r) if joins => {
                    r.end = w.end;
                    r.words.push(w.clone());
                }
                _ => out.push(Reply {
                    start: w.start,
                    end: w.end,
                    speaker,
                    words: vec![w.clone()],
                }),
            }
            phrase_start = false;
        }
    }
    out
}

const ID_SAMPLE: u64 = 1 << 20;

pub fn file_id(path: &Path) -> anyhow::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let size = f.metadata()?.len();
    let mut hasher = blake3::Hasher::new();
    hasher.update(&size.to_le_bytes());
    let mut buf = vec![0; ID_SAMPLE.min(size) as usize];
    f.read_exact(&mut buf)?;
    hasher.update(&buf);
    if size > ID_SAMPLE {
        let tail = ID_SAMPLE.min(size - ID_SAMPLE);
        f.seek(SeekFrom::Start(size - tail))?;
        buf.resize(tail as usize, 0);
        f.read_exact(&mut buf)?;
        hasher.update(&buf);
    }
    Ok(hasher.finalize().to_hex()[..32].to_string())
}

pub fn db_path(ai_dir: &Path, id: &str) -> PathBuf {
    ai_dir.join("media").join(format!("{id}.db"))
}

const SCHEMA: i64 = 5;

pub struct Transcript {
    pub(crate) db: Connection,
}

fn words_json(words: &[Word]) -> String {
    serde_json::to_string(words).expect("words serialise")
}

fn words_from(s: String) -> Vec<Word> {
    serde_json::from_str(&s).unwrap_or_default()
}

impl Transcript {
    pub fn open(ai_dir: &Path, id: &str) -> anyhow::Result<Transcript> {
        let path = db_path(ai_dir, id);
        std::fs::create_dir_all(path.parent().expect("in media/"))?;
        let mut db = Connection::open(&path)?;
        let mut version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA && version != 0 {
            log::warn!(
                "{}: schema {version}, expected {SCHEMA}; starting over",
                path.display()
            );
            drop(db);
            for suffix in ["", "-wal", "-shm"] {
                let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
            }
            db = Connection::open(&path)?;
            version = 0;
        }
        db.busy_timeout(Duration::from_secs(5))?;
        db.pragma_update(None, "journal_mode", "WAL")?;
        db.pragma_update(None, "foreign_keys", "ON")?;
        if version == 0 {
            db.execute_batch(
                "CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 CREATE TABLE phrases (start REAL NOT NULL, end_ REAL NOT NULL, words TEXT NOT NULL);
                 CREATE INDEX phrases_start ON phrases (start);
                 CREATE TABLE replies (start REAL NOT NULL, end_ REAL NOT NULL, speaker INTEGER, words TEXT NOT NULL);
                 CREATE TABLE speakers (id INTEGER PRIMARY KEY, name TEXT NOT NULL, source TEXT NOT NULL, evidence TEXT NOT NULL);
                 CREATE TABLE chunks (id INTEGER PRIMARY KEY, start REAL NOT NULL, end_ REAL NOT NULL, speakers TEXT NOT NULL, text TEXT NOT NULL, embedding BLOB);
                 CREATE VIRTUAL TABLE chunks_fts USING fts5(stems, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2');
                 CREATE TABLE chapters (idx INTEGER PRIMARY KEY, start REAL NOT NULL, end_ REAL NOT NULL, data TEXT NOT NULL);
                 CREATE TABLE chats (id INTEGER PRIMARY KEY, title TEXT NOT NULL, created_ms INTEGER NOT NULL);
                 CREATE TABLE messages (id INTEGER PRIMARY KEY, chat INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE, role TEXT NOT NULL, text TEXT NOT NULL, details TEXT NOT NULL, created_ms INTEGER NOT NULL);
                 CREATE INDEX messages_chat ON messages (chat, id);
                 PRAGMA user_version = 5;",
            )?;
        }
        Ok(Transcript { db })
    }

    pub fn exists(ai_dir: &Path, id: &str) -> bool {
        db_path(ai_dir, id).is_file()
    }

    pub fn meta(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self
            .db
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)",
            params![key, value],
        )?;
        Ok(())
    }

    pub fn remove_meta(&self, key: &str) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM meta WHERE key = ?1", [key])?;
        Ok(())
    }

    pub fn put_phrases(&mut self, from: f64, phrases: &[Phrase]) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM phrases WHERE start >= ?1", [from])?;
        {
            let mut insert =
                tx.prepare("INSERT INTO phrases (start, end_, words) VALUES (?1, ?2, ?3)")?;
            for p in phrases {
                insert.execute(params![p.start, p.end, words_json(&p.words)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn phrases(&self) -> anyhow::Result<Vec<Phrase>> {
        let mut q = self
            .db
            .prepare("SELECT start, end_, words FROM phrases ORDER BY start")?;
        let rows = q.query_map([], |r| {
            Ok(Phrase {
                start: r.get(0)?,
                end: r.get(1)?,
                words: words_from(r.get(2)?),
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_replies(&mut self, replies: &[Reply]) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM replies", [])?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO replies (start, end_, speaker, words) VALUES (?1, ?2, ?3, ?4)",
            )?;
            for r in replies {
                insert.execute(params![r.start, r.end, r.speaker, words_json(&r.words)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn dialog(&self) -> anyhow::Result<Vec<Reply>> {
        let mut q = self
            .db
            .prepare("SELECT start, end_, speaker, words FROM replies ORDER BY start")?;
        let rows: Vec<Reply> = q
            .query_map([], |r| {
                Ok(Reply {
                    start: r.get(0)?,
                    end: r.get(1)?,
                    speaker: r.get(2)?,
                    words: words_from(r.get(3)?),
                })
            })?
            .collect::<Result<_, _>>()?;
        if !rows.is_empty() {
            return Ok(rows);
        }
        Ok(replies(&self.phrases()?, &[]))
    }

    pub fn speaker_ids(&self) -> anyhow::Result<Vec<u32>> {
        let mut q = self.db.prepare(
            "SELECT DISTINCT speaker FROM replies WHERE speaker IS NOT NULL ORDER BY speaker",
        )?;
        let rows = q.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn speaker_names(&self) -> anyhow::Result<BTreeMap<u32, String>> {
        let mut q = self.db.prepare("SELECT id, name FROM speakers")?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn speaker_entries(&self) -> anyhow::Result<BTreeMap<u32, SpeakerName>> {
        let mut q = self
            .db
            .prepare("SELECT id, name, source, evidence FROM speakers")?;
        let rows = q.query_map([], |r| {
            let source: String = r.get(2)?;
            let evidence: String = r.get(3)?;
            Ok((
                r.get(0)?,
                SpeakerName {
                    name: r.get(1)?,
                    source: if source == "auto" {
                        NameSource::Auto
                    } else {
                        NameSource::Manual
                    },
                    evidence: serde_json::from_str(&evidence).unwrap_or_default(),
                },
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn rename_speaker(&self, speaker: u32, name: &str) -> anyhow::Result<()> {
        match name.trim() {
            "" => self.db.execute("DELETE FROM speakers WHERE id = ?1", [speaker])?,
            name => self.db.execute(
                "INSERT OR REPLACE INTO speakers (id, name, source, evidence) VALUES (?1, ?2, 'manual', '[]')",
                params![speaker, name],
            )?,
        };
        Ok(())
    }

    pub fn set_auto_name(
        &self,
        speaker: u32,
        name: &str,
        evidence: &[NameEvidence],
    ) -> anyhow::Result<bool> {
        let name = name.trim();
        if name.is_empty() {
            return Ok(false);
        }
        let evidence = serde_json::to_string(evidence)?;
        let written = self.db.execute(
            "INSERT INTO speakers (id, name, source, evidence) VALUES (?1, ?2, 'auto', ?3)
             ON CONFLICT(id) DO UPDATE SET name = excluded.name, evidence = excluded.evidence WHERE speakers.source = 'auto'",
            params![speaker, name, evidence],
        )?;
        Ok(written > 0)
    }

    pub fn clear_auto_names(&self) -> anyhow::Result<()> {
        self.db
            .execute("DELETE FROM speakers WHERE source = 'auto'", [])?;
        Ok(())
    }

    pub fn clear(&mut self) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute_batch("DELETE FROM phrases; DELETE FROM replies; DELETE FROM meta; DELETE FROM chunks; DELETE FROM chunks_fts; DELETE FROM chapters;")?;
        tx.commit()?;
        Ok(())
    }

    pub fn clear_embeddings(&self) -> anyhow::Result<()> {
        self.db.execute("UPDATE chunks SET embedding = NULL", [])?;
        Ok(())
    }

    pub fn clear_speakers(&self) -> anyhow::Result<()> {
        let tx = self.db.unchecked_transaction()?;
        tx.execute_batch(
            "UPDATE replies SET speaker = NULL;
             DELETE FROM speakers;
             DELETE FROM meta WHERE key = 'diarized';",
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn remove_meta_prefix(&self, prefix: &str) -> anyhow::Result<()> {
        self.db.execute(
            "DELETE FROM meta WHERE substr(key, 1, length(?1)) = ?1",
            [prefix],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64, end: f64) -> Word {
        Word {
            text: text.into(),
            start,
            end,
        }
    }

    fn sample() -> Vec<Phrase> {
        vec![
            Phrase {
                start: 0.0,
                end: 3.0,
                words: vec![
                    word("привет,", 0.0, 0.5),
                    word("как", 0.6, 0.9),
                    word("дела?", 1.0, 1.4),
                    word("хорошо.", 2.0, 2.6),
                ],
            },
            Phrase {
                start: 3.0,
                end: 4.0,
                words: vec![
                    word("а", 3.0, 3.2),
                    word("у", 3.3, 3.4),
                    word("тебя?", 3.5, 3.9),
                ],
            },
            Phrase {
                start: 9.0,
                end: 10.0,
                words: vec![word("тоже.", 9.0, 9.5)],
            },
        ]
    }

    #[test]
    fn replies_follow_speakers_and_pauses() {
        let phrases = sample();
        let plain = replies(&phrases, &[]);
        assert_eq!(
            plain.iter().map(Reply::text).collect::<Vec<_>>(),
            ["привет, как дела? хорошо. а у тебя?", "тоже."]
        );
        assert!(plain.iter().all(|r| r.speaker.is_none()));
        let turns = [
            Turn {
                start: 0.0,
                end: 1.6,
                speaker: 0,
            },
            Turn {
                start: 1.8,
                end: 4.0,
                speaker: 1,
            },
            Turn {
                start: 8.8,
                end: 9.6,
                speaker: 0,
            },
        ];
        let r = replies(&phrases, &turns);
        assert_eq!(
            r.iter().map(|r| (r.speaker, r.text())).collect::<Vec<_>>(),
            [
                (Some(0), "привет, как дела?".to_string()),
                (Some(1), "хорошо. а у тебя?".to_string()),
                (Some(0), "тоже.".to_string())
            ]
        );
        assert_eq!((r[1].start, r[1].end), (2.0, 3.9));
    }

    #[test]
    fn ids_follow_content_not_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        let big: Vec<u8> = (0..3 * ID_SAMPLE as usize)
            .map(|i| (i % 251) as u8)
            .collect();
        std::fs::write(&a, &big).unwrap();
        let id = file_id(&a).unwrap();
        let b = dir.path().join("renamed.mp4");
        std::fs::rename(&a, &b).unwrap();
        assert_eq!(file_id(&b).unwrap(), id, "renaming keeps the id");
        let mut changed = big.clone();
        *changed.last_mut().unwrap() ^= 1;
        std::fs::write(&b, &changed).unwrap();
        assert_ne!(file_id(&b).unwrap(), id, "the end of the file counts");
        std::fs::write(&b, b"tiny").unwrap();
        assert_eq!(file_id(&b).unwrap().len(), 32);
    }

    #[test]
    fn transcript_grows_resumes_and_keeps_names() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!Transcript::exists(dir.path(), "f"));
        let mut t = Transcript::open(dir.path(), "f").unwrap();
        assert!(Transcript::exists(dir.path(), "f"));
        let phrases = sample();
        t.put_phrases(0.0, &phrases[..2]).unwrap();
        assert_eq!(
            t.dialog().unwrap().len(),
            1,
            "a dialog while transcription runs"
        );
        t.put_phrases(3.0, &phrases[1..]).unwrap();
        assert_eq!(t.phrases().unwrap(), phrases);
        t.set_meta("model", "giga").unwrap();
        assert_eq!(t.meta("model").unwrap().as_deref(), Some("giga"));
        t.remove_meta("model").unwrap();
        assert!(t.meta("model").unwrap().is_none());

        let turns = [
            Turn {
                start: 0.0,
                end: 1.6,
                speaker: 0,
            },
            Turn {
                start: 1.8,
                end: 10.0,
                speaker: 1,
            },
        ];
        t.set_replies(&replies(&phrases, &turns)).unwrap();
        assert_eq!(
            t.dialog()
                .unwrap()
                .iter()
                .map(|r| r.speaker)
                .collect::<Vec<_>>(),
            [Some(0), Some(1), Some(1)]
        );
        t.rename_speaker(1, " Дана ").unwrap();
        assert_eq!(
            t.speaker_names().unwrap().get(&1).map(String::as_str),
            Some("Дана")
        );
        drop(t);

        let mut t = Transcript::open(dir.path(), "f").unwrap();
        assert_eq!(t.dialog().unwrap().len(), 3, "stored");
        t.clear().unwrap();
        assert!(t.dialog().unwrap().is_empty());
        assert_eq!(t.speaker_names().unwrap().len(), 1, "names stay");
        t.rename_speaker(1, "").unwrap();
        assert!(t.speaker_names().unwrap().is_empty());
        drop(t);

        let c = Connection::open(db_path(dir.path(), "f")).unwrap();
        c.pragma_update(None, "user_version", 1).unwrap();
        drop(c);
        let t = Transcript::open(dir.path(), "f").unwrap();
        assert!(
            t.phrases().unwrap().is_empty() && t.chunks().unwrap().is_empty(),
            "a file of another layout is transcribed again from scratch"
        );
    }

    #[test]
    fn an_automatic_name_never_replaces_the_users() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let evidence = [NameEvidence {
            start: 12.0,
            text: "Меня зовут Антон.".into(),
            how: "self".into(),
        }];
        assert!(t.set_auto_name(0, " Антон ", &evidence).unwrap());
        let entries = t.speaker_entries().unwrap();
        assert_eq!(entries[&0].name, "Антон");
        assert_eq!(entries[&0].source, NameSource::Auto);
        assert_eq!(entries[&0].evidence, evidence);
        assert_eq!(t.speaker_names().unwrap()[&0], "Антон");

        t.rename_speaker(0, "Антоша").unwrap();
        assert!(!t.set_auto_name(0, "Антон", &evidence).unwrap());
        let entries = t.speaker_entries().unwrap();
        assert_eq!(
            (entries[&0].name.as_str(), entries[&0].source),
            ("Антоша", NameSource::Manual)
        );
        assert!(entries[&0].evidence.is_empty());

        t.rename_speaker(0, "").unwrap();
        t.set_auto_name(0, "Антон", &evidence).unwrap();
        t.set_auto_name(0, "Антон Петров", &[]).unwrap();
        assert_eq!(t.speaker_names().unwrap()[&0], "Антон Петров");

        t.rename_speaker(1, "Дана").unwrap();
        t.clear_auto_names().unwrap();
        assert_eq!(
            t.speaker_names().unwrap(),
            BTreeMap::from([(1, "Дана".to_string())])
        );

        assert!(!t.set_auto_name(2, "   ", &[]).unwrap());
        assert!(!t.speaker_names().unwrap().contains_key(&2));
    }
}
