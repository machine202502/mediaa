use crate::transcript_db::Transcript;
use mediaa2_core::transcript::Reply;
use rusqlite::params;
use rust_stemmers::{Algorithm, Stemmer};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const CHUNK_WORDS: usize = 200;
pub const OVERLAP_WORDS: usize = 40;
const MIN_WORDS: usize = 120;
const RRF_K: f32 = 60.0;
const SAME_PLACE: f64 = 30.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Chunk {
    pub start: f64,
    pub end: f64,
    pub speakers: Vec<u32>,
    pub text: String,
}

pub fn chunks(replies: &[Reply]) -> Vec<Chunk> {
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
            r.words.iter().enumerate().map(move |(i, w)| W {
                text: &w.text,
                start: w.start,
                end: w.end,
                speaker: r.speaker,
                reply_end: i + 1 == n,
            })
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
            let min = from + MIN_WORDS;
            let last =
                |f: &dyn Fn(&W) -> bool| (min..limit).rev().find(|&i| f(&words[i])).map(|i| i + 1);
            last(&|w| w.reply_end)
                .or_else(|| last(&sentence_end))
                .unwrap_or(limit)
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
        from = if words[to - 1].reply_end {
            to
        } else {
            (to - OVERLAP_WORDS).max(from + 1)
        };
    }
    out
}

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

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub id: i64,
    pub chunk: Chunk,
    pub score: f32,
}

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

pub fn diverse(hits: Vec<Hit>, limit: usize) -> Vec<Hit> {
    let mut out: Vec<Hit> = Vec::new();
    for h in hits {
        if out.len() == limit {
            break;
        }
        if out
            .iter()
            .all(|o| (o.chunk.start - h.chunk.start).abs() >= SAME_PLACE)
        {
            out.push(h);
        }
    }
    out
}

fn blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn floats(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}

impl Transcript {
    pub fn set_chunks(&mut self, chunks: &[Chunk]) -> anyhow::Result<()> {
        let tx = self.db.transaction()?;
        tx.execute_batch(
            "DELETE FROM chunks; DELETE FROM chunks_fts; DELETE FROM embeddings;
             DELETE FROM meta WHERE key LIKE 'embed_rev:%';",
        )?;
        {
            let mut insert = tx.prepare(
                "INSERT INTO chunks (start, end_, speakers, text) VALUES (?1, ?2, ?3, ?4)",
            )?;
            let mut fts = tx.prepare("INSERT INTO chunks_fts (rowid, stems) VALUES (?1, ?2)")?;
            for c in chunks {
                insert.execute(params![
                    c.start,
                    c.end,
                    serde_json::to_string(&c.speakers)?,
                    c.text
                ])?;
                fts.execute(params![tx.last_insert_rowid(), stems(&c.text).join(" ")])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn chunks(&self) -> anyhow::Result<Vec<(i64, Chunk, bool)>> {
        let mut q = self.db.prepare(
            "SELECT id, start, end_, speakers, text,
                    EXISTS (SELECT 1 FROM embeddings e WHERE e.chunk = chunks.id)
             FROM chunks ORDER BY id",
        )?;
        let rows = q.query_map([], |r| {
            let speakers: String = r.get(3)?;
            Ok((
                r.get(0)?,
                Chunk {
                    start: r.get(1)?,
                    end: r.get(2)?,
                    speakers: serde_json::from_str(&speakers).unwrap_or_default(),
                    text: r.get(4)?,
                },
                r.get(5)?,
            ))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// The vectors of one model; the rows of other models stay untouched.
    pub fn chunks_with_embeddings(
        &self,
        model: &str,
    ) -> anyhow::Result<Vec<(Chunk, Option<Vec<f32>>)>> {
        let mut q = self.db.prepare(
            "SELECT c.start, c.end_, c.speakers, c.text, e.vector
             FROM chunks c
             LEFT JOIN embeddings e ON e.chunk = c.id AND e.model = ?1
             ORDER BY c.id",
        )?;
        let rows = q.query_map([model], |r| {
            let speakers: String = r.get(2)?;
            let embedding: Option<Vec<u8>> = r.get(4)?;
            let chunk = Chunk {
                start: r.get(0)?,
                end: r.get(1)?,
                speakers: serde_json::from_str(&speakers).unwrap_or_default(),
                text: r.get(3)?,
            };
            Ok((chunk, embedding.map(|b| floats(&b))))
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn set_embedding(&self, id: i64, model: &str, v: &[f32]) -> anyhow::Result<()> {
        self.db.execute(
            "INSERT INTO embeddings (chunk, model, vector) VALUES (?1, ?2, ?3)
             ON CONFLICT(chunk, model) DO UPDATE SET vector = excluded.vector",
            params![id, model, blob(v)],
        )?;
        Ok(())
    }

    /// Chunks of this transcript that the model has no vector for yet.
    pub fn chunks_todo(&self, model: &str) -> anyhow::Result<Vec<(i64, String)>> {
        let mut q = self.db.prepare(
            "SELECT c.id, c.text FROM chunks c
             WHERE NOT EXISTS (SELECT 1 FROM embeddings e WHERE e.chunk = c.id AND e.model = ?1)
             ORDER BY c.id",
        )?;
        let rows = q.query_map([model], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Whether the stored index of the model matches the current text.
    pub fn embed_index_ok(&self, model: &str) -> anyhow::Result<bool> {
        let rev = self.transcript_rev()?;
        Ok(self.meta(&format!("embed_rev:{model}"))?.as_deref() == Some(rev.as_str()))
    }

    pub fn set_embed_rev(&self, model: &str) -> anyhow::Result<()> {
        let rev = self.transcript_rev()?;
        self.set_meta(&format!("embed_rev:{model}"), &rev)
    }

    pub fn keyword_search(&self, query: &str, limit: usize) -> anyhow::Result<Vec<i64>> {
        let terms: Vec<String> = stems(query)
            .into_iter()
            .map(|s| format!("\"{s}\"*"))
            .collect();
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        let mut q = self.db.prepare("SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ?1 ORDER BY bm25(chunks_fts) LIMIT ?2")?;
        let rows = q.query_map(params![terms.join(" OR "), limit as i64], |r| r.get(0))?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn vector_search(
        &self,
        query: &[f32],
        model: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<i64>> {
        let mut q = self.db.prepare(
            "SELECT e.chunk, e.vector FROM embeddings e
             WHERE e.model = ?1 AND EXISTS (SELECT 1 FROM chunks c WHERE c.id = e.chunk)",
        )?;
        let mut scored: Vec<(i64, f32)> = q
            .query_map([model], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, Vec<u8>>(1)?))
            })?
            .filter_map(Result::ok)
            .map(|(id, b)| (id, floats(&b).iter().zip(query).map(|(a, b)| a * b).sum()))
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        Ok(scored.into_iter().take(limit).map(|(id, _)| id).collect())
    }

    pub fn search(
        &self,
        query: &str,
        embedding: Option<&[f32]>,
        model: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<Hit>> {
        Ok(diverse(
            self.candidates(query, embedding, model, limit * 4)?,
            limit,
        ))
    }

    pub fn candidates(
        &self,
        query: &str,
        embedding: Option<&[f32]>,
        model: &str,
        n: usize,
    ) -> anyhow::Result<Vec<Hit>> {
        let mut rankings = vec![self.keyword_search(query, n)?];
        // vectors of a stale revision do not match the chunks: words only
        if let Some(e) = embedding {
            if self.embed_index_ok(model)? {
                rankings.push(self.vector_search(e, model, n)?);
            }
        }
        let by_id: HashMap<i64, Chunk> = self
            .chunks()?
            .into_iter()
            .map(|(id, c, _)| (id, c))
            .collect();
        Ok(fuse(&rankings)
            .into_iter()
            .take(n)
            .filter_map(|(id, score)| {
                by_id.get(&id).map(|c| Hit {
                    id,
                    chunk: c.clone(),
                    score,
                })
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::transcript::Word;

    fn reply(speaker: u32, start: f64, text: &str) -> Reply {
        let words: Vec<Word> = text
            .split(' ')
            .enumerate()
            .map(|(i, w)| Word {
                text: w.into(),
                start: start + i as f64 * 0.4,
                end: start + i as f64 * 0.4 + 0.3,
            })
            .collect();
        Reply {
            start,
            end: words.last().unwrap().end,
            speaker: Some(speaker),
            words,
        }
    }

    fn many(n: usize, sentence_every: usize) -> String {
        (0..n)
            .map(|i| {
                if (i + 1) % sentence_every == 0 {
                    format!("w{i}.")
                } else {
                    format!("w{i}")
                }
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn chunks_follow_turns_and_overlap_long_ones() {
        let short: Vec<Reply> = (0..10)
            .map(|i| reply(i % 2, i as f64 * 20.0, &many(30, 10)))
            .collect();
        let c = chunks(&short);
        assert!(c.len() >= 2);
        assert!(c.iter().all(|c| c.text.split(' ').count() <= CHUNK_WORDS));
        assert_eq!(c[0].text.split(' ').count() % 30, 0, "whole replies");
        assert_eq!(c[0].speakers, vec![0, 1]);
        assert_eq!(
            c.iter().map(|c| c.text.split(' ').count()).sum::<usize>(),
            300,
            "no overlap between whole replies"
        );
        let long = vec![reply(3, 0.0, &many(500, 25))];
        let c = chunks(&long);
        assert!(c.len() >= 3);
        assert!(
            c[..c.len() - 1].iter().all(|c| c.text.ends_with('.')),
            "sentence ends"
        );
        let first: Vec<&str> = c[0].text.split(' ').collect();
        assert!(
            c[1].text.starts_with(first[first.len() - OVERLAP_WORDS]),
            "overlap"
        );
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
        let hit = |id: i64, start: f64| Hit {
            id,
            chunk: Chunk {
                start,
                end: start + 60.0,
                speakers: vec![],
                text: String::new(),
            },
            score: 0.0,
        };
        let d = diverse(
            vec![hit(1, 100.0), hit(2, 110.0), hit(3, 200.0), hit(4, 400.0)],
            2,
        );
        assert_eq!(d.iter().map(|h| h.id).collect::<Vec<_>>(), [1, 3]);
    }

    #[test]
    fn hybrid_search_over_stored_chunks() {
        let dir = tempfile::tempdir().unwrap();
        let mut t = Transcript::open(dir.path(), "f").unwrap();
        let cs = vec![
            Chunk {
                start: 0.0,
                end: 60.0,
                speakers: vec![0],
                text: "Обсудили отчёт для заказчика, сдаём в пятницу.".into(),
            },
            Chunk {
                start: 60.0,
                end: 120.0,
                speakers: vec![1],
                text: "Погода на даче была отличная.".into(),
            },
            Chunk {
                start: 120.0,
                end: 180.0,
                speakers: vec![0, 1],
                text: "Релиз сдвигается, если отчёты задержатся.".into(),
            },
        ];
        t.set_chunks(&cs).unwrap();
        let stored = t.chunks().unwrap();
        assert_eq!(stored.len(), 3);
        assert!(stored.iter().all(|(_, _, embedded)| !embedded));
        assert!(t
            .chunks_with_embeddings("e5")
            .unwrap()
            .iter()
            .all(|(_, e)| e.is_none()));
        let found = t.keyword_search("отчетов", 10).unwrap();
        assert_eq!(found.len(), 2);
        assert!(t.keyword_search("!!!", 10).unwrap().is_empty());
        for (id, c, _) in &stored {
            let v = if c.text.contains("даче") {
                [0.0, 1.0]
            } else {
                [1.0, 0.0]
            };
            t.set_embedding(*id, "e5", &v).unwrap();
        }
        // the vectors belong to one model: another model sees nothing
        assert!(t.chunks_todo("e5").unwrap().is_empty());
        assert_eq!(t.chunks_todo("other").unwrap().len(), 3);
        assert_eq!(
            t.vector_search(&[0.0, 1.0], "other", 1).unwrap(),
            Vec::<i64>::new()
        );
        assert_eq!(
            t.vector_search(&[0.0, 1.0], "e5", 1).unwrap(),
            vec![stored[1].0]
        );
        assert_eq!(
            t.chunks_with_embeddings("e5")
                .unwrap()
                .iter()
                .map(|(_, e)| e.clone().unwrap())
                .collect::<Vec<_>>(),
            [vec![1.0, 0.0], vec![0.0, 1.0], vec![1.0, 0.0]]
        );
        assert!(t
            .chunks_with_embeddings("other")
            .unwrap()
            .iter()
            .all(|(_, e)| e.is_none()));
        // no revision recorded yet: the index counts as stale, words only
        assert!(!t.embed_index_ok("e5").unwrap());
        t.set_embed_rev("e5").unwrap();
        assert!(t.embed_index_ok("e5").unwrap());
        let hits = t
            .search("загородный дом", Some(&[0.0, 1.0]), "e5", 3)
            .unwrap();
        assert_eq!(hits[0].chunk.text, cs[1].text);
        let hits = t.search("отчёт", None, "e5", 3).unwrap();
        assert_eq!(hits.len(), 2);
        // a text change invalidates the index of every model
        t.set_chunks(&cs[..1]).unwrap();
        assert_eq!(t.chunks().unwrap().len(), 1);
        assert_eq!(t.keyword_search("релиз", 5).unwrap(), Vec::<i64>::new());
        assert!(!t.embed_index_ok("e5").unwrap(), "stale after re-chunking");
        assert!(t.vector_search(&[0.0, 1.0], "e5", 1).unwrap().is_empty());
    }
}
