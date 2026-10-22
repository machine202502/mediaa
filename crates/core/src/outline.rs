//! The outline of a recording: chapters with a title and a digest (summary, decisions, action
//! items, open questions, topics, quotes), one per topic of the dialog. Topics are found by the
//! meaning of the chunk embeddings ([`semantic_boundaries`]) and digested by a language model in
//! the background (`mediaa_agent::outline`); summary questions are answered from it cheaply and
//! consistently, and it is the map of a long recording.

use crate::library::human_duration;
use crate::transcript::{Reply, Transcript};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

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

/// A line of a detail prompt lasts at most this many seconds (it is cut at a sentence end).
pub const FINE_LINE_SECONDS: f64 = 25.0;
/// A line of a detail prompt has at most this many words.
pub const FINE_LINE_WORDS: usize = 60;
/// The lines at a detail item's time must share this much of its words with it (a detail item
/// is a paraphrase, so the share is lower than a quote's).
pub const DETAIL_MATCH: f64 = 0.3;

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
    /// The detailed breakdown of the topic (empty until its `topic-detail` task has run).
    #[serde(default)]
    pub sections: Vec<Section>,
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

/// A section of the detailed breakdown of a topic: what it is and how its items are laid out.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionKind {
    /// A plain list of points (the usual one).
    #[default]
    List,
    /// Numbered steps, in order.
    Steps,
    /// A piece of prose.
    Text,
}

/// One section of [`Digest::sections`]: a titled piece of the topic with its items.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Section {
    pub title: String,
    #[serde(default)]
    pub kind: SectionKind,
    #[serde(default)]
    pub items: Vec<Item>,
}

/// One item of a [`Section`]: what was said and, when the recording confirms it, when.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Item {
    pub text: String,
    /// The start of the line that says this, checked like a quote but against the fine lines of
    /// the topic ([`verified_detail_time`]); `None` when the model's time could not be
    /// confirmed (the item itself stays).
    #[serde(default)]
    pub time: Option<f64>,
}

/// JSON Schema of [`Digest`] for constrained generation. How many list entries is bounded;
/// the texts are not: a `maxLength` cuts a word, and the prompts ask for the brevity.
pub const DIGEST_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "title": {"type": "string"},
    "summary": {"type": "string"},
    "decisions": {"type": "array", "items": {"type": "string"}, "maxItems": 5},
    "actions": {"type": "array", "items": {"type": "string"}, "maxItems": 5},
    "questions": {"type": "array", "items": {"type": "string"}, "maxItems": 5},
    "topics": {"type": "array", "items": {"type": "string"}, "maxItems": 5},
    "quotes": {
      "type": "array",
      "items": {
        "type": "object",
        "properties": {
          "time": {"type": "string", "maxLength": 16},
          "text": {"type": "string"}
        },
        "required": ["time", "text"]
      },
      "maxItems": 3
    }
  },
  "required": ["title", "summary", "decisions", "actions", "questions", "topics"]
}"#;

/// How many sections and items an answer may hold. The character counts are what a full answer
/// is budgeted for (`mediaa_agent::outline`), not a limit: the schema does not cut a text.
pub const SECTIONS_MAX: usize = 6;
pub const SECTION_ITEMS_MAX: usize = 8;
pub const SECTION_TITLE_CHARS: usize = 60;
pub const SECTION_TEXT_CHARS: usize = 200;
/// How many points one window may hold, and the characters a point is budgeted for.
pub const POINTS_MAX: usize = 15;
pub const POINT_TEXT_CHARS: usize = 200;

/// JSON Schema of the detailed breakdown of a topic (`Digest.sections`), for constrained
/// generation. Counts are bounded; the texts are not ([`DIGEST_SCHEMA`]).
pub fn sections_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "sections": {
                "type": "array",
                "maxItems": SECTIONS_MAX,
                "items": {
                    "type": "object",
                    "properties": {
                        "title": {"type": "string"},
                        "kind": {"type": "string", "enum": ["list", "steps", "text"]},
                        "items": {
                            "type": "array",
                            "maxItems": SECTION_ITEMS_MAX,
                            "items": {
                                "type": "object",
                                "properties": {
                                    "time": {"type": "string", "maxLength": 16},
                                    "text": {"type": "string"}
                                },
                                "required": ["text"]
                            }
                        }
                    },
                    "required": ["title", "kind", "items"]
                }
            }
        },
        "required": ["sections"]
    })
}

/// JSON Schema of the points of one window of a long topic, for constrained generation.
pub fn points_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "points": {
                "type": "array",
                "maxItems": POINTS_MAX,
                "items": {
                    "type": "object",
                    "properties": {
                        "time": {"type": "string", "maxLength": 16},
                        "text": {"type": "string"}
                    },
                    "required": ["time", "text"]
                }
            }
        },
        "required": ["points"]
    })
}

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

/// The line of a reply in a prompt: `[mm:ss] @sN: text` (or without a speaker).
pub fn line(reply: &Reply) -> String {
    line_text(reply.start, reply.speaker, &reply.text())
}

/// One line of a detail prompt: a piece of a reply cut at a sentence end, at most
/// [`FINE_LINE_SECONDS`] of speech or [`FINE_LINE_WORDS`] words, with the time of its first word.
#[derive(Debug, Clone, PartialEq)]
pub struct FineLine {
    pub start: f64,
    pub end: f64,
    pub words: usize,
    pub text: String,
}

fn line_text(start: f64, speaker: Option<u32>, text: &str) -> String {
    match speaker {
        Some(s) => format!("[{}] {}: {}", human_duration(start), crate::speakers::label(s), text),
        None => format!("[{}] {}", human_duration(start), text),
    }
}

/// Cuts the replies into short lines for the detail prompts: by sentence ends, so that a line
/// lasts at most [`FINE_LINE_SECONDS`] seconds or has at most [`FINE_LINE_WORDS`] words (a
/// monologue of minutes becomes many lines, each with its own time). A new reply always starts a
/// new line.
pub fn fine_lines(replies: &[Reply]) -> Vec<FineLine> {
    let mut out: Vec<FineLine> = Vec::new();
    for reply in replies {
        let mut line: Vec<&crate::transcript::Word> = Vec::new();
        // Words up to here end with a sentence end (the preferred place to cut).
        let mut sentence = 0;
        for word in &reply.words {
            line.push(word);
            if word.text.chars().last().is_some_and(|c| matches!(c, '.' | '!' | '?' | '…')) {
                sentence = line.len();
            }
            let start = line.first().map_or(reply.start, |w| w.start);
            let end = line.last().map_or(start, |w| w.end);
            if end - start >= FINE_LINE_SECONDS || line.len() >= FINE_LINE_WORDS {
                let cut = if sentence > 0 { sentence } else { line.len() };
                let rest = line.split_off(cut);
                push_fine_line(&mut out, reply.speaker, &line);
                line = rest;
                sentence = 0;
            }
        }
        if !line.is_empty() {
            push_fine_line(&mut out, reply.speaker, &line);
        }
    }
    out
}

fn push_fine_line(out: &mut Vec<FineLine>, speaker: Option<u32>, words: &[&crate::transcript::Word]) {
    let (Some(first), Some(last)) = (words.first(), words.last()) else {
        return;
    };
    let text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
    out.push(FineLine { start: first.start, end: last.end, words: words.len(), text: line_text(first.start, speaker, &text) });
}

/// Groups fine lines into windows of about `max_words` words (the lines themselves are never
/// cut), for the points of a long topic.
pub fn windows_of_lines(lines: &[FineLine], max_words: usize) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    let mut words = 0;
    for l in lines {
        match out.last_mut() {
            Some(w) if words + l.words <= max_words => {
                w.end = l.end;
                w.text.push('\n');
                w.text.push_str(&l.text);
                words += l.words;
            }
            _ => {
                out.push(Window { start: l.start, end: l.end, text: l.text.clone() });
                words = l.words;
            }
        }
    }
    out
}

/// Cuts the dialog into windows of about `max_words` words at reply ends (a longer reply is
/// a window of its own). Speakers are the labels `@sN`.
pub fn windows(replies: &[Reply], max_words: usize) -> Vec<Window> {
    let mut out: Vec<Window> = Vec::new();
    let mut words = 0;
    for r in replies {
        let n = r.words.len();
        let line = line(r);
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

/// The start of the reply that says `text` near `time`: a reply within [`QUOTE_NEAR_SECONDS`]
/// of `time` whose own words contain most of the words of `text` ([`QUOTE_MATCH`]); `None`
/// when there is no such reply. The rule shared by quotes and the times of detail items.
fn said_near(time: f64, text: &str, replies: &[Reply]) -> Option<f64> {
    let words = quote_words(text);
    if words.is_empty() {
        return None;
    }
    let mut best: Option<(f64, f64, f64)> = None; // share, -distance, reply start
    for reply in replies {
        let distance = if time < reply.start {
            reply.start - time
        } else if time > reply.end {
            time - reply.end
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
    best.map(|(_, _, start)| start)
}

/// Checks a time written by the model against the replies the same way a quote is checked:
/// `Some(start of the reply)` when the reply near `time` says `text`, else `None`.
pub fn verified_time(time: f64, text: &str, replies: &[Reply]) -> Option<f64> {
    said_near(time, text, replies)
}

/// Checks a detail item's time against the fine lines of its topic: `Some(start of the line)`
/// when a line within [`QUOTE_NEAR_SECONDS`] of `time` says the item — its own text shares at
/// least [`DETAIL_MATCH`] of the item's words, or, when no line does, a line with its
/// neighbours does (an item may cover a sentence or two); else `None`. The line that says the
/// words wins over a neighbour of it.
pub fn verified_detail_time(time: f64, text: &str, lines: &[FineLine]) -> Option<f64> {
    let words = quote_words(text);
    if words.is_empty() {
        return None;
    }
    let share = |said: &HashSet<String>| words.iter().filter(|w| said.contains(*w)).count() as f64 / words.len() as f64;
    let mut own: Option<(f64, f64, f64)> = None; // share, -distance, line start
    let mut near: Option<(f64, f64, f64)> = None;
    for (i, line) in lines.iter().enumerate() {
        let distance = if time < line.start {
            line.start - time
        } else if time > line.end {
            time - line.end
        } else {
            0.0
        };
        if distance > QUOTE_NEAR_SECONDS {
            continue;
        }
        let said: HashSet<String> = quote_words(&line.text).into_iter().collect();
        best_match(&mut own, share(&said), distance, line.start);
        let context: HashSet<String> = lines[i.saturating_sub(1)..(i + 2).min(lines.len())].iter().flat_map(|l| quote_words(&l.text)).collect();
        best_match(&mut near, share(&context), distance, line.start);
    }
    own.or(near).map(|(_, _, start)| start)
}

/// Keeps the better match: the higher share, then the closer line.
fn best_match(best: &mut Option<(f64, f64, f64)>, share: f64, distance: f64, start: f64) {
    if share < DETAIL_MATCH {
        return;
    }
    let better = best.is_none_or(|(s, d, _)| share > s || (share == s && -distance > d));
    if better {
        *best = Some((share, -distance, start));
    }
}

/// Checks the model's quotes against the replies: a quote stays only if a reply within
/// [`QUOTE_NEAR_SECONDS`] of its time contains most of its words ([`QUOTE_MATCH`]); its time
/// becomes the start of that reply. At most [`MAX_QUOTES`] stay.
pub fn verified_quotes(quotes: &[Quote], replies: &[Reply]) -> Vec<Quote> {
    let mut out: Vec<Quote> = Vec::new();
    for quote in quotes {
        if let Some(start) = said_near(quote.time, &quote.text, replies) {
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

    /// A monologue of `words` words, `step` seconds per word; with `stops`, a sentence end every
    /// ten words.
    fn monologue(words: usize, step: f64, stops: bool) -> Reply {
        let words: Vec<Word> = (0..words)
            .map(|i| Word {
                text: if stops && i % 10 == 9 { format!("слово{i}.") } else { format!("слово{i}") },
                start: i as f64 * step,
                end: i as f64 * step + step * 0.8,
            })
            .collect();
        Reply { start: 0.0, end: words.last().unwrap().end, speaker: Some(0), words }
    }

    #[test]
    fn language_by_letters() {
        assert_eq!(language("Отчёт сдаём в пятницу, OK?"), "Russian");
        assert_eq!(language("The report is due on Friday"), "English");
        assert_eq!(language("1234"), "English");
    }

    #[test]
    fn windows_at_reply_ends() {
        let replies = vec![reply(Some(0), 0.0, 50), reply(Some(1), 20.0, 40), reply(Some(0), 65.0, 30), reply(None, 90.0, 200)];
        let w = windows(&replies, 100);
        assert_eq!(w.len(), 3, "{w:#?}");
        assert_eq!(w[0].text.lines().count(), 2);
        assert!(w[0].text.starts_with("[00:00] @s1: w0 w1"));
        assert!(w[0].text.lines().nth(1).unwrap().starts_with("[00:20] @s2: "));
        assert_eq!((w[1].start, w[2].start), (65.0, 90.0));
        assert!(w[2].text.starts_with("[01:30] w0"), "a long reply alone, no speaker without diarization");
        assert!(windows(&[], 100).is_empty());
    }

    #[test]
    fn a_long_monologue_is_cut_into_fine_lines() {
        // 100 words, half a second each, a sentence end every ten words: the 25-second limit cuts
        // at the sentence end of the fiftieth word, so the next line starts at 25 s.
        let lines = fine_lines(&[monologue(100, 0.5, true)]);
        assert_eq!(lines.len(), 2, "{lines:#?}");
        assert_eq!(lines.iter().map(|l| l.words).collect::<Vec<_>>(), [50, 50]);
        assert_eq!((lines[0].start, lines[1].start), (0.0, 25.0));
        assert!(lines[0].text.starts_with("[00:00] @s1: слово0 "), "{}", lines[0].text);
        assert!(lines[1].text.starts_with("[00:25] @s1: слово50"), "{}", lines[1].text);
        // Without a sentence end the word limit cuts the line.
        let lines = fine_lines(&[monologue(100, 0.1, false)]);
        assert_eq!(lines.iter().map(|l| l.words).collect::<Vec<_>>(), [60, 40]);
        assert_eq!(lines[1].start, 6.0);
        assert!(fine_lines(&[]).is_empty());
    }

    #[test]
    fn a_detail_item_gets_the_time_of_the_line_that_says_it() {
        // Two lines: 0:00–0:24.9 and 0:25–0:49.9.
        let lines = fine_lines(&[monologue(100, 0.5, true)]);
        assert_eq!(verified_detail_time(30.0, "слово55 слово56", &lines), Some(25.0), "the middle of the monologue");
        assert_eq!(verified_detail_time(70.0, "слово55 слово56", &lines), None, "too far from the line");
        assert_eq!(verified_detail_time(30.0, "погода на даче", &lines), None, "not said there");
        assert_eq!(verified_detail_time(30.0, "", &lines), None);
        // The lines are grouped into windows without cutting them.
        let windows = windows_of_lines(&lines, 60);
        assert_eq!(windows.len(), 2);
        assert_eq!((windows[0].start, windows[1].start), (0.0, 25.0));
        assert_eq!(windows[0].text.lines().count(), 1);
        assert_eq!(windows_of_lines(&lines, 1200).len(), 1);
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
        // A detail item's time follows the same rule: confirmed → the reply start, else none.
        assert_eq!(verified_time(29.0, "графики сделает Ира", &replies), Some(30.0));
        assert_eq!(verified_time(10.0, "графики сделает Ира", &replies), None, "too far from the reply");
        assert_eq!(verified_time(30.0, "погода на даче", &replies), None, "not said there");
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
        assert!(d.decisions.is_empty() && d.quotes.is_empty() && d.parts.is_empty() && d.sections.is_empty());
        let d: Digest = serde_json::from_str(r#"{"title":"t","summary":"s","parts":[{"start":0.0,"end":60.0,"title":"p"}]}"#).unwrap();
        assert_eq!(d.parts, [Part { start: 0.0, end: 60.0, title: "p".into() }]);
        // Sections and their items round-trip; a section without a kind is a list, an item
        // without a time is unconfirmed.
        let d: Digest = serde_json::from_str(
            r#"{"title":"t","summary":"s","sections":[{"title":"Сроки","items":[{"text":"графики к четвергу","time":30.0},{"text":"без времени"}]},{"title":"Шаги","kind":"steps","items":[]}]}"#,
        )
        .unwrap();
        assert_eq!(d.sections[0].kind, SectionKind::List);
        assert_eq!(d.sections[0].items, [Item { text: "графики к четвергу".into(), time: Some(30.0) }, Item { text: "без времени".into(), time: None }]);
        assert_eq!(d.sections[1].kind, SectionKind::Steps);
        assert_eq!(serde_json::to_string(&SectionKind::Text).unwrap(), r#""text""#);
        let schema = sections_schema();
        assert_eq!(schema["properties"]["sections"]["items"]["properties"]["kind"]["enum"], serde_json::json!(["list", "steps", "text"]));
        assert_eq!(schema["properties"]["sections"]["maxItems"], SECTIONS_MAX);
        assert_eq!(schema["properties"]["sections"]["items"]["properties"]["items"]["maxItems"], SECTION_ITEMS_MAX);
        assert_eq!(points_schema()["required"], serde_json::json!(["points"]));
        assert_eq!(points_schema()["properties"]["points"]["maxItems"], POINTS_MAX);
        // A text the model writes has no maxLength: that cut a word. A time is a clock, not prose.
        let digest: serde_json::Value = serde_json::from_str(DIGEST_SCHEMA).unwrap();
        for key in ["title", "summary"] {
            assert!(digest["properties"][key].get("maxLength").is_none(), "{key}");
        }
        for key in ["decisions", "actions", "questions", "topics"] {
            assert!(digest["properties"][key]["items"].get("maxLength").is_none(), "{key}");
        }
        assert!(digest["properties"]["quotes"]["items"]["properties"]["text"].get("maxLength").is_none());
        assert!(schema["properties"]["sections"]["items"]["properties"]["title"].get("maxLength").is_none());
        assert!(schema["properties"]["sections"]["items"]["properties"]["items"]["items"]["properties"]["text"].get("maxLength").is_none());
        assert!(points_schema()["properties"]["points"]["items"]["properties"]["text"].get("maxLength").is_none());
    }
}
