//! The outline of a recording: chapters with a title and a digest (summary, decisions, action
//! items, open questions, topics, quotes), one per topic of the dialog. Topics are found by the
//! meaning of the chunk embeddings ([`semantic_boundaries`]) and digested by a language model in
//! the background (`mediaa_agent::outline`); summary questions are answered from it cheaply and
//! consistently, and it is the map of a long recording.

use crate::library::human_duration;
use crate::transcript::{Reply, Transcript};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Words of dialog per chapter window (~8–10 minutes of conversation).
pub const WINDOW_WORDS: usize = 1200;

/// Chunks compared on each side of a boundary between topics.
pub const BOUNDARY_K: usize = 2;
/// On a long recording the means cover one chunk more (smoother boundaries).
pub const BOUNDARY_K_LONG: usize = 3;
/// From this many chunks on the recording counts as long.
pub const BOUNDARY_LONG_CHUNKS: usize = 30;
/// A boundary's dip must be deeper than the mean dip plus this many σ.
pub const BOUNDARY_SIGMA: f64 = 0.5;
/// The shortest topic, seconds of speech: a shorter piece is merged with a neighbour.
pub const MIN_TOPIC_SECONDS: f64 = 120.0;
/// A quote's time must point within this many seconds of a reply.
pub const QUOTE_NEAR_SECONDS: f64 = 15.0;
/// A reply must contain this share of a quote's words for the quote to count as checked.
pub const QUOTE_MATCH: f64 = 0.6;
/// Quotes kept for one topic.
pub const MAX_QUOTES: usize = 3;

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
    /// Key lines of the topic in the speakers' words, with their times (checked against the replies).
    #[serde(default)]
    pub quotes: Vec<Quote>,
    /// The pieces a long merged topic is made of, in order (empty for an ordinary topic).
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// A line quoted from the recording: when it was said and what.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    pub time: f64,
    pub text: String,
}

/// A piece of a long merged topic: its span and the title of its own digest.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Part {
    pub start: f64,
    pub end: f64,
    pub title: String,
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
    "topics": {"type": "array", "items": {"type": "string", "maxLength": 40}, "maxItems": 5},
    "quotes": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "time": {"type": "string", "maxLength": 16},
          "text": {"type": "string", "maxLength": 200}
        },
        "required": ["time", "text"]
      },
      "maxItems": 3
    }
  },
  "required": ["title", "summary", "decisions", "actions", "questions", "topics"]
}"#;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chapter {
    pub start: f64,
    pub end: f64,
    pub digest: Digest,
}

/// A window of the dialog for one model call: its time and its text, one reply per line as
/// `[mm:ss] Name: text`.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// One topic of the boundary plan: from the start of its first chunk to the start of the next
/// topic (the last one ends with the recording).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PlanTopic {
    pub start: f64,
    pub end: f64,
}

/// The line of a reply in a prompt: `[mm:ss] Name: text` (or without a name).
pub fn line(reply: &Reply, names: &BTreeMap<u32, String>, speaker_label: impl Fn(u32) -> String) -> String {
    match reply.speaker {
        Some(s) => format!("[{}] {}: {}", human_duration(reply.start), names.get(&s).cloned().unwrap_or_else(|| speaker_label(s)), reply.text()),
        None => format!("[{}] {}", human_duration(reply.start), reply.text()),
    }
}

/// Cuts the dialog into windows of about `max_words` words at reply ends (a longer reply is
/// a window of its own). `names`: speaker names (else «Speaker N» from `speaker_label`).
pub fn windows(replies: &[Reply], max_words: usize, names: &BTreeMap<u32, String>, speaker_label: impl Fn(u32) -> String) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    let mut words = 0;
    for r in replies {
        let n = r.words.len();
        let line = line(r, names, &speaker_label);
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

/// A chunk of the dialog with its embedding, for finding where topics change.
#[derive(Debug, Clone, PartialEq)]
pub struct ChunkVector {
    pub start: f64,
    pub end: f64,
    /// As stored by the embedding model (cosine similarity is used, so the length does not matter).
    pub embedding: Vec<f32>,
}

fn mean(vectors: &[ChunkVector]) -> Vec<f64> {
    let mut mean = vec![0.0; vectors.first().map_or(0, |c| c.embedding.len())];
    if vectors.is_empty() {
        return mean;
    }
    for c in vectors {
        for (m, v) in mean.iter_mut().zip(&c.embedding) {
            *m += f64::from(*v);
        }
    }
    for m in &mut mean {
        *m /= vectors.len() as f64;
    }
    mean
}

/// Cosine similarity of two vectors; 0 when one of them has no length.
pub fn cosine(a: &[f64], b: &[f64]) -> f64 {
    let (mut dot, mut na, mut nb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 { 0.0 } else { dot / (na.sqrt() * nb.sqrt()) }
}

/// Seconds of speech in `chunks[from..to]`: the durations of the chunks (chunks overlap a
/// little where a long turn was split, so this is a slight over-count).
fn speech_seconds(chunks: &[ChunkVector], from: usize, to: usize) -> f64 {
    chunks[from..to].iter().map(|c| (c.end - c.start).max(0.0)).sum()
}

/// Starts of topics as indices into `chunks` (without 0): where one topic ends and the next
/// begins.
///
/// TextTiling over the chunk embeddings: the similarity of the mean embeddings of `k` chunks on
/// each side of every boundary, the depth of the dip against the nearest similarity peaks on
/// both sides, and a boundary wherever the dip is deeper than the mean depth plus
/// [`BOUNDARY_SIGMA`] σ. Pieces shorter than [`MIN_TOPIC_SECONDS`] of speech are then merged with
/// the neighbour across the boundary with the higher similarity.
pub fn semantic_boundaries(chunks: &[ChunkVector]) -> Vec<usize> {
    let n = chunks.len();
    let k = if n >= BOUNDARY_LONG_CHUNKS { BOUNDARY_K_LONG } else { BOUNDARY_K };
    if n < 2 * k + 2 {
        return Vec::new();
    }
    let similarity = boundary_similarities(chunks);
    // Depth of the dip: how far it falls below the nearest peak on each side (TextTiling).
    let mut depth = vec![0.0; n];
    for i in 1..n {
        let mut l = i;
        while l > 1 && similarity[l - 1] >= similarity[l] {
            l -= 1;
        }
        let mut r = i;
        while r + 1 < n && similarity[r + 1] >= similarity[r] {
            r += 1;
        }
        depth[i] = (similarity[l] - similarity[i]).max(0.0) + (similarity[r] - similarity[i]).max(0.0);
    }
    let values = &depth[1..];
    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|d| (d - mean).powi(2)).sum::<f64>() / values.len() as f64;
    let threshold = mean + BOUNDARY_SIGMA * variance.sqrt();
    let starts: Vec<usize> = (k..=n - k).filter(|&i| depth[i] > threshold).collect();
    merge_short_topics(chunks, &similarity, starts)
}

/// The similarity of the mean embeddings of `k` chunks on each side of every chunk boundary,
/// the same curve [`semantic_boundaries`] reads: `similarity[i]` is the boundary before chunk
/// `i`, index 0 is unused (0). A topic boundary is a dip in this curve.
pub fn boundary_similarities(chunks: &[ChunkVector]) -> Vec<f64> {
    let n = chunks.len();
    let k = if n >= BOUNDARY_LONG_CHUNKS { BOUNDARY_K_LONG } else { BOUNDARY_K };
    let mut similarity = vec![0.0; n];
    for i in 1..n {
        let left = &chunks[i.saturating_sub(k)..i];
        let right = &chunks[i..(i + k).min(n)];
        similarity[i] = cosine(&mean(left), &mean(right));
    }
    similarity
}

/// Merges every piece shorter than [`MIN_TOPIC_SECONDS`] with the neighbour it resembles more.
fn merge_short_topics(chunks: &[ChunkVector], similarity: &[f64], starts: Vec<usize>) -> Vec<usize> {
    let n = chunks.len();
    let mut cuts: Vec<usize> = std::iter::once(0).chain(starts).chain(std::iter::once(n)).collect();
    while cuts.len() > 2 {
        let mut merged = false;
        for j in 0..cuts.len() - 1 {
            let (a, b) = (cuts[j], cuts[j + 1]);
            if speech_seconds(chunks, a, b) >= MIN_TOPIC_SECONDS {
                continue;
            }
            // The first and the last piece have only one neighbour; otherwise the closer one wins.
            let with_left = match (a, b == n) {
                (_, true) => true,
                (0, false) => false,
                _ => similarity[a] >= similarity[b],
            };
            cuts.remove(if with_left { j } else { j + 1 });
            merged = true;
            break;
        }
        if !merged {
            break;
        }
    }
    cuts.into_iter().filter(|&c| c > 0 && c < n).collect()
}

/// Seconds of a time as the model writes it in a quote — `m:ss`, `mm:ss`, `h:mm:ss` (also
/// `[m:ss]` and spaces). `None` when it is not such a time.
pub fn clock_time(text: &str) -> Option<f64> {
    let t = text.trim().trim_matches(|c: char| c == '[' || c == ']' || c.is_whitespace());
    if t.is_empty() {
        return None;
    }
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let v: f64 = part.trim().parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        // Minutes and seconds of a clock time stay below 60.
        if i > 0 && v >= 60.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

fn quote_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase().replace('ё', "е"))
        .collect()
}

/// Checks the model's quotes against the replies: a quote stays only if a reply within
/// [`QUOTE_NEAR_SECONDS`] of its time contains most of its words ([`QUOTE_MATCH`]); its time
/// becomes the start of that reply. At most [`MAX_QUOTES`] stay.
pub fn verified_quotes(quotes: &[Quote], replies: &[Reply]) -> Vec<Quote> {
    let mut out: Vec<Quote> = Vec::new();
    for quote in quotes {
        let words = quote_words(&quote.text);
        if words.is_empty() {
            continue;
        }
        let mut best: Option<(f64, f64, f64)> = None; // share, -distance, reply start
        for reply in replies {
            let distance = if quote.time < reply.start {
                reply.start - quote.time
            } else if quote.time > reply.end {
                quote.time - reply.end
            } else {
                0.0
            };
            if distance > QUOTE_NEAR_SECONDS {
                continue;
            }
            let said: HashSet<String> = quote_words(&reply.text()).into_iter().collect();
            let share = words.iter().filter(|w| said.contains(*w)).count() as f64 / words.len() as f64;
            if share < QUOTE_MATCH {
                continue;
            }
            let better = best.is_none_or(|(s, d, _)| share > s || (share == s && -distance > d));
            if better {
                best = Some((share, -distance, reply.start));
            }
        }
        if let Some((_, _, start)) = best {
            if out.iter().any(|q| (q.time - start).abs() < 0.01) {
                continue;
            }
            out.push(Quote { time: start, text: quote.text.trim().to_string() });
            if out.len() == MAX_QUOTES {
                break;
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

    fn chunk(start: f64, v: [f32; 2]) -> ChunkVector {
        ChunkVector { start, end: start + 60.0, embedding: v.to_vec() }
    }

    #[test]
    fn boundaries_where_the_meaning_dips() {
        // Three groups of six chunks with orthogonal meanings: boundaries after chunks 6 and 12.
        let mut chunks = Vec::new();
        for (range, v) in [(0..6, [1.0_f32, 0.0]), (6..12, [0.0, 1.0]), (12..18, [-1.0, 0.0])] {
            for j in range {
                chunks.push(chunk(f64::from(j) * 60.0, v));
            }
        }
        assert_eq!(semantic_boundaries(&chunks), [6, 12]);
        // The same curve the boundaries come from: a dip at each cut, a peak inside a topic.
        let similarity = boundary_similarities(&chunks);
        assert!(similarity[6] < 0.1 && similarity[12] < 0.1, "{similarity:?}");
        assert!(similarity[2] > 0.99 && similarity[8] > 0.99, "{similarity:?}");
        // One meaning throughout: no boundaries.
        let flat: Vec<ChunkVector> = (0..12).map(|i| chunk(f64::from(i) * 60.0, [1.0, 0.0])).collect();
        assert!(semantic_boundaries(&flat).is_empty());
        // Too short to compare k chunks on each side.
        assert!(semantic_boundaries(&chunks[..4]).is_empty());
        assert!(semantic_boundaries(&[]).is_empty());
    }

    #[test]
    fn a_short_topic_merges_with_the_closer_neighbour() {
        // Five chunks of topic A, one short chunk between A and C, five of C.
        let mut chunks: Vec<ChunkVector> = (0..5).map(|i| chunk(f64::from(i) * 60.0, [1.0, 0.0])).collect();
        chunks.push(chunk(300.0, [0.866, 0.5]));
        chunks.extend((0..5).map(|i| chunk(360.0 + f64::from(i) * 60.0, [0.0, 1.0])));
        // The lone chunk is closer to A than to C, so it joins A; the remaining boundary is at 6.
        assert_eq!(semantic_boundaries(&chunks), [6]);
    }

    #[test]
    fn quotes_are_checked_against_the_replies() {
        let text = |s: &str, start: f64| {
            let words: Vec<Word> = s.split(' ').enumerate().map(|(i, w)| Word { text: w.into(), start: start + i as f64, end: start + i as f64 + 0.5 }).collect();
            Reply { start, end: words.last().unwrap().end, speaker: Some(0), words }
        };
        let replies = vec![text("давайте обсудим отчёт", 0.0), text("графики сделает ира до четверга", 30.0)];
        let quote = |time: f64, text: &str| Quote { time, text: text.into() };
        let kept = verified_quotes(
            &[
                quote(29.0, "Графики сделает Ира до четверга"), // a little off, like the model rounds
                quote(30.0, "погода на даче была отличная"),    // not said there
                quote(600.0, "давайте обсудим отчёт"),          // no reply near
            ],
            &replies,
        );
        assert_eq!(kept, [Quote { time: 30.0, text: "Графики сделает Ира до четверга".into() }], "time of the reply start");
        assert!(verified_quotes(&[quote(0.0, "")], &replies).is_empty());
        assert_eq!(clock_time("0:20"), Some(20.0));
        assert_eq!(clock_time("00:05"), Some(5.0));
        assert_eq!(clock_time("1:02:30"), Some(3750.0));
        assert_eq!(clock_time("[12:30]"), Some(750.0));
        assert_eq!(clock_time("soon"), None);
        assert_eq!(clock_time("5:75"), None);
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
        assert_eq!(schema["properties"]["quotes"]["maxItems"], 3);
        let d: Digest = serde_json::from_str(r#"{"title":"t","summary":"s"}"#).unwrap();
        assert!(d.decisions.is_empty() && d.quotes.is_empty() && d.parts.is_empty());
        let d: Digest = serde_json::from_str(r#"{"title":"t","summary":"s","parts":[{"start":0.0,"end":60.0,"title":"p"}]}"#).unwrap();
        assert_eq!(d.parts, [Part { start: 0.0, end: 60.0, title: "p".into() }]);
    }
}
