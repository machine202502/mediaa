//! Retrieval over a transcript: the dialog cut into chunks along speaker turns (~200 words, a
//! sliding overlap for long turns), stored with their embeddings and a keyword index (SQLite
//! FTS5 over Snowball stems, Russian and English), and a hybrid search: keyword and embedding
//! rankings merged by reciprocal rank fusion, then thinned so the hits are not all from the same
//! minute.

use crate::transcript::{Reply, Transcript};
use rusqlite::params;
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Words in a chunk (a little more to end at a reply or a sentence).
pub const CHUNK_WORDS: usize = 200;
/// Words a chunk shares with the one before when a long turn is split.
pub const OVERLAP_WORDS: usize = 40;
/// Chunks shorter than this are joined with the next turn.
const MIN_WORDS: usize = 120;
/// Reciprocal rank fusion constant.
const RRF_K: f32 = 60.0;
/// Hits closer than this (seconds) to a better one are dropped (diversity).
const SAME_PLACE: f64 = 30.0;

/// A searchable piece of the dialog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub start: f64,
    pub end: f64,
    /// Speakers in it, in order of first appearance.
    pub speakers: Vec<u32>,
    pub text: String,
}

/// Cuts the dialog into chunks: whole replies while they fit, a long reply with an overlap;
/// a chunk ends at a reply end or a sentence end when one is near.
pub fn chunks(replies: &[Reply]) -> Vec<Chunk> {
    // Every word with its speaker and whether a reply / a sentence ends after it.
    struct W<'a> {
        text: &'a str,
        start: f64,
        end: f64,
        speaker: Option<u32>,
        reply_end: bool,
    }
    let words: Vec<W> = replies
        .iter()
        .flat_map(|r| {
            let n = r.words.len();
            r.words.iter().enumerate().map(move |(i, w)| W { text: &w.text, start: w.start, end: w.end, speaker: r.speaker, reply_end: i + 1 == n })
        })
        .collect();
    let sentence_end = |w: &W| w.text.ends_with(['.', '!', '?', '…']);
    let mut out = Vec::new();
    let mut from = 0;
    while from < words.len() {
        let limit = (from + CHUNK_WORDS).min(words.len());
        let to = if limit == words.len() {
            limit
        } else {
            // The last reply end, else the last sentence end, past the minimum; else the limit.
            let min = from + MIN_WORDS;
            let last = |f: &dyn Fn(&W) -> bool| (min..limit).rev().find(|&i| f(&words[i])).map(|i| i + 1);
            last(&|w| w.reply_end).or_else(|| last(&sentence_end)).unwrap_or(limit)
        };
        let piece = &words[from..to];
        let mut speakers = Vec::new();
        for s in piece.iter().filter_map(|w| w.speaker) {
            if !speakers.contains(&s) {
                speakers.push(s);
            }
        }
        out.push(Chunk {
            start: piece[0].start,
            end: piece[piece.len() - 1].end,
            speakers,
            text: piece.iter().map(|w| w.text).collect::<Vec<_>>().join(" "),
        });
        if to == words.len() {
            break;
        }
        // Continue after a reply end as is; inside a reply, with an overlap.
        from = if words[to - 1].reply_end { to } else { (to - OVERLAP_WORDS).max(from + 1) };
    }
    out
}

/// Keyword stems of `text`: lower case, Russian words by the Russian stemmer, others by the
/// English one; punctuation dropped.
pub fn stems(text: &str) -> Vec<String> {
    let ru = Stemmer::create(Algorithm::Russian);
    let en = Stemmer::create(Algorithm::English);
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| {
            let w = w.to_lowercase().replace('ё', "е");
            let cyrillic = w.chars().any(|c| ('а'..='я').contains(&c));
            if cyrillic { ru.stem(&w) } else { en.stem(&w) }.into_owned()
        })
        .collect()
}

/// A chunk found by a search, best first.
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: i64,
    pub chunk: Chunk,
    /// Fused score (higher is better).
    pub score: f32,
}

/// Reciprocal rank fusion of rankings (ids best first): ids with their fused scores, best first.
pub fn fuse(rankings: &[Vec<i64>]) -> Vec<(i64, f32)> {
    let mut score: HashMap<i64, f32> = HashMap::new();
    for ranking in rankings {
        for (rank, id) in ranking.iter().enumerate() {
            *score.entry(*id).or_default() += 1.0 / (RRF_K + rank as f32 + 1.0);
        }
    }
    let mut out: Vec<(i64, f32)> = score.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Drops hits that start within [`SAME_PLACE`] seconds of a better one.
pub fn diverse(hits: Vec<Hit>, limit: usize) -> Vec<Hit> {
    let mut out: Vec<Hit> = Vec::new();
    for h in hits {
        if out.len() == limit {
            break;
        }
        if out.iter().all(|o| (o.chunk.start - h.chunk.start).abs() >= SAME_PLACE) {
            out.push(h);
        }
    }
    out
}

fn blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

impl Transcript {
    /// Replaces the chunks (and their keyword index); embeddings are added later.
    pub fn set_chunks(&mut self, chunks: &[Chunk]) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute_batch("DELETE FROM chunks; DELETE FROM chunks_fts;")?;
        {
            let mut insert = tx.prepare("INSERT INTO chunks (start, end_, speakers, text) VALUES (?1, ?2, ?3, ?4)")?;
            let mut fts = tx.prepare("INSERT INTO chunks_fts (rowid, stems) VALUES (?1, ?2)")?;
            for c in chunks {
                insert.execute(params![c.start, c.end, serde_json::to_string(&c.speakers)?, c.text])?;
                fts.execute(params![tx.last_insert_rowid(), stems(&c.text).join(" ")])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Chunks in order with their ids and whether they have an embedding.
    pub fn chunks(&self) -> anyhow::Result<Vec<(i64, Chunk, bool)>> {
        let mut q = self.db.prepare("SELECT id, start, end_, speakers, text, embedding IS NOT NULL FROM chunks ORDER BY id")?;
        let rows = q.query_map([], |r| {
            let speakers: String = r.get(3)?;
            Ok((r.get(0)?, Chunk { start: r.get(1)?, end: r.get(2)?, speakers: serde_json::from_str(&speakers).unwrap_or_default(), text: r.get(4)? }, r.get(5)?))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_embedding(&self, id: i64, v: &[f32]) -> anyhow::Result<()> {
        self.db.execute("UPDATE chunks SET embedding = ?1 WHERE id = ?2", params![blob(v), id])?;
        Ok(())
    }

    /// Chunk ids by keyword relevance (BM25 over stems; any of the words), best first.
    pub fn keyword_search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<i64>> {
        let terms: Vec<String> = stems(query).into_iter().map(|s| format!("\"{s}\"*")).collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut q = self.db.prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT ?2")?;
        let rows = q.query_map(params![terms.join(" OR "), limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Chunk ids by embedding similarity to `query` (normalised vectors), best first.
    pub fn vector_search(&self, query: &[f32], limit: usize) -> anyhow::Result<Vec<i64>> {
        let mut q = self.db.prepare("SELECT id, embedding FROM chunks WHERE embedding IS NOT NULL")?;
        let mut scored: Vec<(i64, f32)> = q
            .query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?)))?
            .filter_map(Result::ok)
            .map(|(id, b)| (id, floats(&b).iter().zip(query).map(|(a, b)| a * b).sum()))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(scored.into_iter().take(limit).map(|(id, _)| id).collect())
    }

    /// Hybrid search: keywords and (when given) the query's embedding, fused, varied.
    pub fn search(&self, query: &str, embedding: Option<&[f32]>, limit: usize) -> anyhow::Result<Vec<Hit>> {
        Ok(diverse(self.candidates(query, embedding, limit * 4)?, limit))
    }

    /// Up to `n` hybrid-search candidates (fused, best first) for a reranker to re-order.
    pub fn candidates(&self, query: &str, embedding: Option<&[f32]>, n: usize) -> anyhow::Result<Vec<Hit>> {
        let mut rankings = vec![self.keyword_search(query, n)?];
        if let Some(e) = embedding {
            rankings.push(self.vector_search(e, n)?);
        }
        let by_id: HashMap<i64, Chunk> = self.chunks()?.into_iter().map(|(id, c, _)| (id, c)).collect();
        Ok(fuse(&rankings).into_iter().take(n).filter_map(|(id, score)| by_id.get(&id).map(|c| Hit { id, chunk: c.clone(), score })).collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transcript::Word;

    fn reply(speaker: u32, start: f64, text: &str) -> Reply {
        let words: Vec<Word> = text.split(' ').enumerate().map(|(i, w)| Word { text: w.into(), start: start + i as f64 * 0.4, end: start + i as f64 * 0.4 + 0.3 }).collect();
        Reply { start, end: words.last().unwrap().end, speaker: Some(speaker), words }
    }

    fn many(n: usize, sentence_every: usize) -> String {
        (0..n).map(|i| if (i + 1) % sentence_every == 0 { format!("w{i}.") } else { format!("w{i}") }).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn chunks_follow_turns_and_overlap_long_ones() {
        // Short turns are joined; a chunk ends at a reply end past the minimum.
        let short: Vec<Reply> = (0..10).map(|i| reply(i % 2, i as f64 * 20.0, &many(30, 10))).collect();
        let c = chunks(&short);
        assert!(c.len() >= 2);
        assert!(c.iter().all(|c| c.text.split(' ').count() <= CHUNK_WORDS));
        assert_eq!(c[0].text.split(' ').count() % 30, 0, "whole replies");
        assert_eq!(c[0].speakers, vec![0, 1]);
        assert_eq!(c.iter().map(|c| c.text.split(' ').count()).sum::<usize>(), 300, "no overlap between whole replies");
        // One long monologue: windows with an overlap, cut at sentence ends.
        let long = vec![reply(3, 0.0, &many(500, 25))];
        let c = chunks(&long);
        assert!(c.len() >= 3);
        assert!(c[..c.len() - 1].iter().all(|c| c.text.ends_with('.')), "sentence ends");
        let first: Vec<&str> = c[0].text.split(' ').collect();
        assert!(c[1].text.starts_with(first[first.len() - OVERLAP_WORDS]), "overlap");
        assert!(c.last().unwrap().text.ends_with(" w499."));
        assert!(chunks(&[]).is_empty());
    }

    #[test]
    fn stems_match_word_forms() {
        assert_eq!(stems("Отчёты, отчёта!"), stems("отчет отчету"));
        assert_eq!(stems("Releases released"), vec!["releas", "releas"]);
        assert_eq!(stems("сдать 2025 года"), vec!["сдат", "2025", "год"]);
    }

    #[test]
    fn fusion_and_diversity() {
        let f = fuse(&[vec![1, 2, 3], vec![3, 1, 9]]);
        assert_eq!(f[0].0, 1, "high in both");
        assert_eq!(f[1].0, 3);
        assert!(f.iter().any(|(id, _)| *id == 9));
        let hit = |id: i64, start: f64| Hit { id, chunk: Chunk { start, end: start + 60.0, speakers: vec![], text: String::new() }, score: 0.0 };
        let d = diverse(vec![hit(1, 100.0), hit(2, 110.0), hit(3, 200.0), hit(4, 400.0)], 2);
        assert_eq!(d.iter().map(|h| h.id).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn hybrid_search_over_stored_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = Transcript::open(dir.path(), "f").unwrap();
        let cs = vec![
            Chunk { start: 0.0, end: 60.0, speakers: vec![0], text: "Обсудили отчёт для заказчика, сдаём в пятницу.".into() },
            Chunk { start: 60.0, end: 120.0, speakers: vec![1], text: "Погода на даче была отличная.".into() },
            Chunk { start: 120.0, end: 180.0, speakers: vec![0, 1], text: "Релиз сдвигается, если отчёты задержатся.".into() },
        ];
        t.set_chunks(&cs).unwrap();
        let stored = t.chunks().unwrap();
        assert_eq!(stored.len(), 3);
        assert!(stored.iter().all(|(_, _, embedded)| !embedded));
        // Keywords find word forms.
        let found = t.keyword_search("отчетов", 10).unwrap();
        assert_eq!(found.len(), 2);
        assert!(t.keyword_search("!!!", 10).unwrap().is_empty());
        // Vectors: the dacha chunk points along y, the others along x.
        for (id, c, _) in &stored {
            let v = if c.text.contains("даче") { [0.0, 1.0] } else { [1.0, 0.0] };
            t.set_embedding(*id, &v).unwrap();
        }
        assert_eq!(t.vector_search(&[0.0, 1.0], 1).unwrap(), vec![stored[1].0]);
        // Hybrid: a keyword miss is still found by meaning.
        let hits = t.search("загородный дом", Some(&[0.0, 1.0]), 3).unwrap();
        assert_eq!(hits[0].chunk.text, cs[1].text);
        let hits = t.search("отчёт", None, 3).unwrap();
        assert_eq!(hits.len(), 2);
        // Re-chunking replaces everything.
        t.set_chunks(&cs[..1]).unwrap();
        assert_eq!(t.chunks().unwrap().len(), 1);
        assert_eq!(t.keyword_search("релиз", 5).unwrap(), Vec::<i64>::new());
    }
}
