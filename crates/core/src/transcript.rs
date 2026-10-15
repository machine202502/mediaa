//! The transcript of a media file in `<media>/.ai/media/<file id>.db`: recognised phrases (with
//! words and their times) written while transcription runs, the replies of the dialog once
//! speakers are known, and the names given to the speakers. The file id comes from the file's
//! content, so a renamed or moved file keeps its transcript.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A recognised word, times in seconds from the start of the recording.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Word {
    pub text: String,
    pub start: f64,
    pub end: f64,
}

fn join(words: &[Word]) -> String {
    words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// A recognised piece of speech (one voice-activity segment).
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

/// A stretch of time one speaker talks.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    pub speaker: u32,
}

/// One reply of the dialog: consecutive words of one speaker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    pub start: f64,
    pub end: f64,
    /// `None` without diarization.
    pub speaker: Option<u32>,
    pub words: Vec<Word>,
}

impl Reply {
    pub fn text(&self) -> String {
        join(&self.words)
    }
}

/// Pauses longer than this start a new reply even when the speaker is the same.
const REPLY_GAP: f64 = 1.5;
/// Replies longer than this are split at the next pause between phrases.
const REPLY_MAX: f64 = 60.0;

/// The speaker of the turn overlapping `[start, end]` the most (or the nearest one).
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
    turns.iter().max_by(|a, b| overlap(a).total_cmp(&overlap(b)).then(distance(b).total_cmp(&distance(a)))).map(|t| t.speaker)
}

/// The dialog: phrases split where the speaker changes (by `turns`; none without diarization),
/// and joined while the same speaker goes on after a short pause.
pub fn replies(phrases: &[Phrase], turns: &[Turn]) -> Vec<Reply> {
    let mut out: Vec<Reply> = Vec::new();
    for phrase in phrases {
        let mut phrase_start = true;
        for w in &phrase.words {
            let speaker = if turns.is_empty() { None } else { speaker_at(turns, w.start, w.end) };
            let joins = out
                .last()
                .is_some_and(|r| r.speaker == speaker && (!phrase_start || (w.start - r.end <= REPLY_GAP && r.end - r.start < REPLY_MAX)));
            match out.last_mut() {
                Some(r) if joins => {
                    r.end = w.end;
                    r.words.push(w.clone());
                }
                _ => out.push(Reply { start: w.start, end: w.end, speaker, words: vec![w.clone()] }),
            }
            phrase_start = false;
        }
    }
    out
}

/// How much of the start and of the end of a file identifies it.
const ID_SAMPLE: u64 = 1 << 20;

/// Content id of a media file: its size and the first and last megabyte (fast for any size).
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

/// Where the transcript of the file with `id` lives.
pub fn db_path(ai_dir: &Path, id: &str) -> PathBuf {
    ai_dir.join("media").join(format!("{id}.db"))
}

const SCHEMA: i64 = 4;

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
    /// Opens (creating) the transcript of the file with `id` in the library's `.ai` folder.
    pub fn open(ai_dir: &Path, id: &str) -> anyhow::Result<Transcript> {
        let path = db_path(ai_dir, id);
        std::fs::create_dir_all(path.parent().expect("in media/"))?;
        let mut db = Connection::open(&path)?;
        let mut version: i64 = db.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if version != SCHEMA && version != 0 {
            // Another layout (an older build): transcribed again from scratch.
            log::warn!("{}: schema {version}, expected {SCHEMA}; starting over", path.display());
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
                 CREATE TABLE speakers (id INTEGER PRIMARY KEY, name TEXT NOT NULL);
                 CREATE TABLE chunks (id INTEGER PRIMARY KEY, start REAL NOT NULL, end_ REAL NOT NULL, speakers TEXT NOT NULL, text TEXT NOT NULL, embedding BLOB);
                 CREATE VIRTUAL TABLE chunks_fts USING fts5(stems, content='', contentless_delete=1, tokenize='unicode61 remove_diacritics 2');
                 CREATE TABLE chapters (idx INTEGER PRIMARY KEY, start REAL NOT NULL, end_ REAL NOT NULL, data TEXT NOT NULL);
                 CREATE TABLE chats (id INTEGER PRIMARY KEY, title TEXT NOT NULL, created_ms INTEGER NOT NULL);
                 CREATE TABLE messages (id INTEGER PRIMARY KEY, chat INTEGER NOT NULL REFERENCES chats(id) ON DELETE CASCADE, role TEXT NOT NULL, text TEXT NOT NULL, details TEXT NOT NULL, created_ms INTEGER NOT NULL);
                 CREATE INDEX messages_chat ON messages (chat, id);
                 PRAGMA user_version = 4;",
            )?;
        }
        Ok(Transcript { db })
    }

    /// Whether a transcript of the file with `id` exists (without creating one).
    pub fn exists(ai_dir: &Path, id: &str) -> bool {
        db_path(ai_dir, id).is_file()
    }

    pub fn meta(&self, key: &str) -> anyhow::Result<Option<String>> {
        Ok(self.db.query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0)).optional()?)
    }

    pub fn set_meta(&self, key: &str, value: &str) -> anyhow::Result<()> {
        self.db.execute("INSERT OR REPLACE INTO meta (key, value) VALUES (?1, ?2)", params![key, value])?;
        Ok(())
    }

    /// Replaces everything recognised from `from` seconds on with `phrases` (a resumed
    /// transcription repeats the part after its last checkpoint), in one transaction.
    pub fn put_phrases(&mut self, from: f64, phrases: &[Phrase]) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM phrases WHERE start >= ?1", [from])?;
        {
            let mut insert = tx.prepare("INSERT INTO phrases (start, end_, words) VALUES (?1, ?2, ?3)")?;
            for p in phrases {
                insert.execute(params![p.start, p.end, words_json(&p.words)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn phrases(&self) -> anyhow::Result<Vec<Phrase>> {
        let mut q = self.db.prepare("SELECT start, end_, words FROM phrases ORDER BY start")?;
        let rows = q.query_map([], |r| Ok(Phrase { start: r.get(0)?, end: r.get(1)?, words: words_from(r.get(2)?) }))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Stores the final dialog (replacing an earlier one).
    pub fn set_replies(&mut self, replies: &[Reply]) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM replies", [])?;
        {
            let mut insert = tx.prepare("INSERT INTO replies (start, end_, speaker, words) VALUES (?1, ?2, ?3, ?4)")?;
            for r in replies {
                insert.execute(params![r.start, r.end, r.speaker, words_json(&r.words)])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The dialog: the final replies, or (while transcription runs) the phrases so far.
    pub fn dialog(&self) -> anyhow::Result<Vec<Reply>> {
        let mut q = self.db.prepare("SELECT start, end_, speaker, words FROM replies ORDER BY start")?;
        let rows: Vec<Reply> =
            q.query_map([], |r| Ok(Reply { start: r.get(0)?, end: r.get(1)?, speaker: r.get(2)?, words: words_from(r.get(3)?) }))?.collect::<Result<_, _>>()?;
        if !rows.is_empty() {
            return Ok(rows);
        }
        Ok(replies(&self.phrases()?, &[]))
    }

    /// Names given to speakers.
    pub fn speaker_names(&self) -> anyhow::Result<BTreeMap<u32, String>> {
        let mut q = self.db.prepare("SELECT id, name FROM speakers")?;
        let rows = q.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Names a speaker; an empty name removes it.
    pub fn rename_speaker(&self, speaker: u32, name: &str) -> anyhow::Result<()> {
        match name.trim() {
            "" => self.db.execute("DELETE FROM speakers WHERE id = ?1", [speaker])?,
            name => self.db.execute("INSERT OR REPLACE INTO speakers (id, name) VALUES (?1, ?2)", params![speaker, name])?,
        };
        Ok(())
    }

    /// Forgets the recognition and what was built from it (to transcribe again); speaker names
    /// stay.
    pub fn clear(&mut self) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute_batch("DELETE FROM phrases; DELETE FROM replies; DELETE FROM meta; DELETE FROM chunks; DELETE FROM chunks_fts; DELETE FROM chapters;")?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str, start: f64, end: f64) -> Word {
        Word { text: text.into(), start, end }
    }

    fn sample() -> Vec<Phrase> {
        vec![
            Phrase { start: 0.0, end: 3.0, words: vec![word("привет,", 0.0, 0.5), word("как", 0.6, 0.9), word("дела?", 1.0, 1.4), word("хорошо.", 2.0, 2.6)] },
            Phrase { start: 3.0, end: 4.0, words: vec![word("а", 3.0, 3.2), word("у", 3.3, 3.4), word("тебя?", 3.5, 3.9)] },
            Phrase { start: 9.0, end: 10.0, words: vec![word("тоже.", 9.0, 9.5)] },
        ]
    }

    #[test]
    fn replies_follow_speakers_and_pauses() {
        let phrases = sample();
        // Without diarization: phrases joined across short pauses only.
        let plain = replies(&phrases, &[]);
        assert_eq!(plain.iter().map(Reply::text).collect::<Vec<_>>(), ["привет, как дела? хорошо. а у тебя?", "тоже."]);
        assert!(plain.iter().all(|r| r.speaker.is_none()));
        // Speaker 0 asks, 1 answers and asks back, 0 answers after a pause.
        let turns = [Turn { start: 0.0, end: 1.6, speaker: 0 }, Turn { start: 1.8, end: 4.0, speaker: 1 }, Turn { start: 8.8, end: 9.6, speaker: 0 }];
        let r = replies(&phrases, &turns);
        assert_eq!(
            r.iter().map(|r| (r.speaker, r.text())).collect::<Vec<_>>(),
            [(Some(0), "привет, как дела?".to_string()), (Some(1), "хорошо. а у тебя?".to_string()), (Some(0), "тоже.".to_string())]
        );
        assert_eq!((r[1].start, r[1].end), (2.0, 3.9));
    }

    #[test]
    fn ids_follow_content_not_names() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.mp4");
        let big: Vec<u8> = (0..3 * ID_SAMPLE as usize).map(|i| (i % 251) as u8).collect();
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
        assert_eq!(t.dialog().unwrap().len(), 1, "a dialog while transcription runs");
        // Resumed from 3.0: the phrase from 3.0 is recognised again, not doubled.
        t.put_phrases(3.0, &phrases[1..]).unwrap();
        assert_eq!(t.phrases().unwrap(), phrases);
        t.set_meta("model", "giga").unwrap();
        assert_eq!(t.meta("model").unwrap().as_deref(), Some("giga"));

        let turns = [Turn { start: 0.0, end: 1.6, speaker: 0 }, Turn { start: 1.8, end: 10.0, speaker: 1 }];
        t.set_replies(&replies(&phrases, &turns)).unwrap();
        assert_eq!(t.dialog().unwrap().iter().map(|r| r.speaker).collect::<Vec<_>>(), [Some(0), Some(1), Some(1)]);
        t.rename_speaker(1, " Дана ").unwrap();
        assert_eq!(t.speaker_names().unwrap().get(&1).map(String::as_str), Some("Дана"));
        drop(t);

        let mut t = Transcript::open(dir.path(), "f").unwrap();
        assert_eq!(t.dialog().unwrap().len(), 3, "stored");
        t.clear().unwrap();
        assert!(t.dialog().unwrap().is_empty());
        assert_eq!(t.speaker_names().unwrap().len(), 1, "names stay");
        t.rename_speaker(1, "").unwrap();
        assert!(t.speaker_names().unwrap().is_empty());
        drop(t);

        // A file of another layout is started over.
        let c = Connection::open(db_path(dir.path(), "f")).unwrap();
        c.pragma_update(None, "user_version", 1).unwrap();
        drop(c);
        let t = Transcript::open(dir.path(), "f").unwrap();
        assert!(t.phrases().unwrap().is_empty() && t.chunks().unwrap().is_empty());
    }
}
