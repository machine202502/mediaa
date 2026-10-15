//! The outline of a recording: chapters with a title and a digest (summary, decisions, action
//! items, open questions, topics), one per window of the dialog. Built by a language model in
//! the background (`mediaa_agent::outline`); summary questions are answered from it cheaply and
//! consistently, and it is the map of a long recording.

use crate::library::human_duration;
use crate::transcript::{Reply, Transcript};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Words of dialog per chapter window (~8–10 minutes of conversation).
pub const WINDOW_WORDS: usize = 1200;

/// The language of a text by its letters: «Russian» when Cyrillic prevails, else «English».
pub fn language(text: &str) -> &'static str {
    let (mut cyrillic, mut latin) = (0usize, 0usize);
    for c in text.chars().filter(|c| c.is_alphabetic()) {
        if ('\u{0400}'..='\u{04FF}').contains(&c) {
            cyrillic += 1;
        } else if c.is_ascii_alphabetic() {
            latin += 1;
        }
    }
    if cyrillic >= latin && cyrillic > 0 { "Russian" } else { "English" }
}

/// How a speaker without a name is called in `language`.
pub fn unnamed_speaker(language: &str, speaker: u32) -> String {
    match language {
        "Russian" => format!("Говорящий {}", speaker + 1),
        _ => format!("Speaker {}", speaker + 1),
    }
}

/// What the model writes about a window.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Digest {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub questions: Vec<String>,
    #[serde(default)]
    pub topics: Vec<String>,
}

/// JSON Schema of [`Digest`] for constrained generation; bounded so a small model cannot run
/// on (lists and texts have a maximum).
pub const DIGEST_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "title": {"type": "string", "maxLength": 80},
    "summary": {"type": "string", "maxLength": 400},
    "decisions": {"type": "array", "items": {"type": "string", "maxLength": 160}, "maxItems": 5},
    "actions": {"type": "array", "items": {"type": "string", "maxLength": 160}, "maxItems": 5},
    "questions": {"type": "array", "items": {"type": "string", "maxLength": 160}, "maxItems": 5},
    "topics": {"type": "array", "items": {"type": "string", "maxLength": 40}, "maxItems": 5}
  },
  "required": ["title", "summary", "decisions", "actions", "questions", "topics"]
}"#;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start: f64,
    pub end: f64,
    pub digest: Digest,
}

/// A window of the dialog for one chapter: its time and its text, one reply per line as
/// `[mm:ss] Name: text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Cuts the dialog into windows of about `max_words` words at reply ends (a longer reply is
/// a window of its own). `names`: speaker names (else «Speaker N» from `speaker_label`).
pub fn windows(replies: &[Reply], max_words: usize, names: &BTreeMap<u32, String>, speaker_label: impl Fn(u32) -> String) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    let mut words = 0;
    for r in replies {
        let n = r.words.len();
        let line = match r.speaker {
            Some(s) => format!("[{}] {}: {}", human_duration(r.start), names.get(&s).cloned().unwrap_or_else(|| speaker_label(s)), r.text()),
            None => format!("[{}] {}", human_duration(r.start), r.text()),
        };
        match out.last_mut() {
            Some(w) if words + n <= max_words => {
                w.end = r.end;
                w.text.push('\n');
                w.text.push_str(&line);
                words += n;
            }
            _ => {
                out.push(Window { start: r.start, end: r.end, text: line });
                words = n;
            }
        }
    }
    out
}

impl Transcript {
    /// Stores chapter `idx` (replacing it).
    pub fn put_chapter(&self, idx: usize, chapter: &Chapter) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO chapters (idx, start, end_, data) VALUES (?1, ?2, ?3, ?4)",
            params![idx as i64, chapter.start, chapter.end, serde_json::to_string(&chapter.digest)?],
        )?;
        Ok(())
    }

    pub fn chapters(&self) -> anyhow::Result<Vec<Chapter>> {
        let mut q = self.db.prepare("SELECT start, end_, data FROM chapters ORDER BY idx")?;
        let rows = q.query_map([], |r| {
            let data: String = r.get(2)?;
            Ok(Chapter { start: r.get(0)?, end: r.get(1)?, digest: serde_json::from_str(&data).unwrap_or_default() })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Forgets the outline (before building it again).
    pub fn clear_chapters(&self) -> anyhow::Result<()> {
        self.db.execute("DELETE FROM chapters", [])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Word;

    fn reply(speaker: Option<u32>, start: f64, words: usize) -> Reply {
        let words: Vec<Word> = (0..words).map(|i| Word { text: format!("w{i}"), start: start + i as f64 * 0.3, end: start + i as f64 * 0.3 + 0.2 }).collect();
        Reply { start, end: words.last().map_or(start, |w| w.end), speaker, words }
    }

    #[test]
    fn language_by_letters() {
        assert_eq!(language("Отчёт сдаём в пятницу, OK?"), "Russian");
        assert_eq!(language("The report is due on Friday"), "English");
        assert_eq!(language("1234"), "English");
        assert_eq!(unnamed_speaker("Russian", 0), "Говорящий 1");
        assert_eq!(unnamed_speaker("English", 2), "Speaker 3");
    }

    #[test]
    fn windows_at_reply_ends() {
        let replies = vec![reply(Some(0), 0.0, 50), reply(Some(1), 20.0, 40), reply(Some(0), 65.0, 30), reply(None, 90.0, 200)];
        let names = BTreeMap::from([(0, "Дана".to_string())]);
        let w = windows(&replies, 100, &names, |s| format!("Speaker {}", s + 1));
        assert_eq!(w.len(), 3, "{w:#?}");
        assert_eq!(w[0].text.lines().count(), 2);
        assert!(w[0].text.starts_with("[00:00] Дана: w0 w1"));
        assert!(w[0].text.lines().nth(1).unwrap().starts_with("[00:20] Speaker 2: "));
        assert_eq!((w[1].start, w[2].start), (65.0, 90.0));
        assert!(w[2].text.starts_with("[01:30] w0"), "a long reply alone, no speaker without diarization");
        assert!(windows(&[], 100, &names, |s| s.to_string()).is_empty());
    }

    #[test]
    fn chapters_are_stored_by_index() {
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let ch = |title: &str, start: f64| Chapter { start, end: start + 60.0, digest: Digest { title: title.into(), summary: "s".into(), ..Digest::default() } };
        t.put_chapter(1, &ch("второе", 60.0)).unwrap();
        t.put_chapter(0, &ch("первое", 0.0)).unwrap();
        t.put_chapter(1, &ch("второе снова", 60.0)).unwrap();
        let all = t.chapters().unwrap();
        assert_eq!(all.iter().map(|c| c.digest.title.as_str()).collect::<Vec<_>>(), ["первое", "второе снова"]);
        t.clear_chapters().unwrap();
        assert!(t.chapters().unwrap().is_empty());
        // The schema parses as JSON and names every field of the digest.
        let schema: serde_json::Value = serde_json::from_str(DIGEST_SCHEMA).unwrap();
        assert_eq!(schema["required"].as_array().unwrap().len(), 6);
        let d: Digest = serde_json::from_str(r#"{"title":"t","summary":"s"}"#).unwrap();
        assert!(d.decisions.is_empty());
    }
}
