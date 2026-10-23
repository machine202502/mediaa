//! Searching transcripts for a question — one recording or many (a catalog folder): hybrid
//! candidates per file (keywords + the query's embedding, computed once), re-ordered together by
//! the reranker, then varied. Each model step is optional: without an embedding model the
//! search is by keywords, without a reranker the fused order stays.
//!
//! A search may name a speaker (that speaker's replies come first, not instead of the rest) and
//! a part of the recording: only fragments inside it, and the chunks of that part that have no
//! embedding yet are embedded on the spot and kept for the full index.

use crate::index::EMBED_CTX;
use crate::slots::Slots;
use mediaa_core::agent::{AgentSettings, ModelRole, ModelSource, WorkKind};
use mediaa_core::retrieval::{Chunk, Hit};
use mediaa_core::speakers::Parsed;
use mediaa_core::transcript::Transcript;
use mediaa_engine::{EmbedKind, EmbeddingsRequest, Engine, RerankRequest};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Candidates the reranker reads in all (more: better recall, slower).
pub const CANDIDATES: usize = 24;
/// Candidates taken from each file when searching many.
const PER_FILE: usize = 8;
/// Hits of one file closer than this (seconds) to a better one are dropped.
const SAME_PLACE: f64 = 30.0;

/// What a search asks for besides the query.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Options {
    /// A speaker whose replies come first (0-based index, and its recording in a folder).
    pub speaker: Option<Parsed>,
    /// Only fragments that touch this span (seconds, both ends inclusive).
    pub span: Option<(f64, f64)>,
}

/// Whether a chunk touches `[from, to]`.
fn within(chunk: &Chunk, from: f64, to: f64) -> bool {
    chunk.end >= from && chunk.start <= to
}

/// How a search went (for the honest trace of the agent and for tests).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Trace {
    pub by_meaning: bool,
    pub reranked: bool,
    /// Files searched / without a searchable transcript yet.
    pub files: usize,
    pub not_indexed: usize,
    /// Chunks of the asked part embedded on the spot.
    pub embedded: usize,
}

/// A recording to search: its content id and where it is.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub id: String,
    pub path: PathBuf,
}

/// A hit in one of the searched files.
#[derive(Debug, Clone, PartialEq)]
pub struct FileHit {
    pub path: PathBuf,
    pub hit: Hit,
    /// The hit answers in a reply of the speaker the search asked for.
    pub speaker: bool,
}

/// Hits from many files: the best first, at most `limit`; within one file not two from the
/// same place.
pub fn diverse(hits: Vec<FileHit>, limit: usize) -> Vec<FileHit> {
    let mut out: Vec<FileHit> = Vec::new();
    for h in hits {
        if out.len() == limit {
            break;
        }
        if out.iter().all(|o| o.path != h.path || (o.hit.chunk.start - h.hit.chunk.start).abs() >= SAME_PLACE) {
            out.push(h);
        }
    }
    out
}

/// Hits for `query` in the transcripts of `files` in `ai_dir`, best first, at most `limit`.
/// `interactive`: a user waits (agent slots, ahead of background work).
#[allow(clippy::too_many_arguments)]
pub fn search_files(engine: &dyn Engine, slots: &Slots, settings: &AgentSettings, ai_dir: &Path, files: &[Source], query: &str, limit: usize, interactive: bool) -> anyhow::Result<(Vec<FileHit>, Trace)> {
    search_files_with(engine, slots, settings, ai_dir, files, query, limit, Options::default(), interactive)
}

/// [`search_files`] over a speaker and/or a part of the recordings (see [`Options`]).
#[allow(clippy::too_many_arguments)]
pub fn search_files_with(
    engine: &dyn Engine,
    slots: &Slots,
    settings: &AgentSettings,
    ai_dir: &Path,
    files: &[Source],
    query: &str,
    limit: usize,
    options: Options,
    interactive: bool,
) -> anyhow::Result<(Vec<FileHit>, Trace)> {
    let cancel = AtomicBool::new(false);
    let kind = if interactive { WorkKind::Agent } else { WorkKind::Indexing };
    let device = |role| settings.devices.get(role);
    let hf_token = crate::assistant::hf_token(settings);
    let mut trace = Trace { files: files.len(), ..Trace::default() };
    // The part of the recording the search is about is embedded now, if it was not before:
    // the work is done once, inside this call, and the full index reuses the vectors.
    if let (Some(source), Some(span)) = (settings.models.embedding.as_ref(), options.span) {
        match embed_span(engine, slots, settings, source, ai_dir, files, span, kind, interactive, &cancel) {
            Ok(n) => trace.embedded = n,
            Err(e) => log::info!("search without indexing the span: {e:#}"),
        }
    }
    let embedding = settings.models.embedding.as_ref().and_then(|source| {
        let run = || {
            let model = crate::models::local_spec(source)?;
            let _lease = slots.request(kind, interactive).wait(&cancel)?;
            let found = engine.embeddings(&EmbeddingsRequest {
                model,
                input: vec![query.to_string()],
                kind: EmbedKind::Query,
                n_ctx: Some(EMBED_CTX),
                device: Some(device(ModelRole::Embedding)),
                hf_token: hf_token.clone(),
            }, &cancel)?;
            found.into_iter().next().filter(|v| !v.is_empty()).ok_or_else(|| anyhow::anyhow!("the model returned no embedding"))
        };
        run().map_err(|e| log::info!("search without meaning: {e:#}")).ok()
    });
    trace.by_meaning = embedding.is_some();

    let per_file = if files.len() == 1 { CANDIDATES } else { PER_FILE };
    // A filter is applied to the hits, so every chunk must be ranked to choose from: the
    // keyword and vector searches then return all of them, not just the best few.
    let filtered = options.span.is_some() || options.speaker.is_some();
    let mut hits: Vec<FileHit> = Vec::new();
    for (position, f) in files.iter().enumerate() {
        if !Transcript::exists(ai_dir, &f.id) {
            trace.not_indexed += 1;
            continue;
        }
        let t = Transcript::open(ai_dir, &f.id)?;
        let chunks = t.chunks()?;
        if chunks.is_empty() {
            trace.not_indexed += 1;
            continue;
        }
        let n = if filtered { per_file.max(chunks.len()) } else { per_file };
        // In a folder the label names its recording (`@s2#3`); a label without one means
        // that index in every recording.
        let speaker = options.speaker.filter(|p| p.recording.is_none() || p.recording == Some(position as u32 + 1)).map(|p| p.index);
        for hit in t.candidates(query, embedding.as_deref(), n)? {
            if options.span.is_some_and(|(from, to)| !within(&hit.chunk, from, to)) {
                continue;
            }
            let speaker = speaker.is_some_and(|s| hit.chunk.speakers.contains(&s));
            hits.push(FileHit { path: f.path.clone(), hit, speaker });
        }
    }
    // Fused scores are rank based, so files compare; the reranker reads the best of them.
    hits.sort_by(|a, b| b.hit.score.total_cmp(&a.hit.score));
    // The asked speaker's replies lead into the reranker too, so they are always in reach.
    if options.speaker.is_some() {
        hits.sort_by_key(|h| !h.speaker);
    }
    hits.truncate(CANDIDATES.max(limit));
    if let (Some(source), false) = (&settings.models.reranker, hits.is_empty()) {
        let rerank = || -> anyhow::Result<Vec<f32>> {
            let model = crate::models::local_spec(source)?;
            let _lease = slots.request(kind, interactive).wait(&cancel)?;
            let results = engine.rerank(&RerankRequest {
                model,
                query: query.to_string(),
                documents: hits.iter().map(|h| h.hit.chunk.text.clone()).collect(),
                n_ctx: Some(EMBED_CTX),
                device: Some(device(ModelRole::Reranker)),
                hf_token: hf_token.clone(),
            }, &cancel)?;
            let mut scores = vec![0.0; hits.len()];
            for result in results {
                if let Some(score) = scores.get_mut(result.index) {
                    *score = result.relevance_score;
                }
            }
            Ok(scores)
        };
        match rerank() {
            Ok(scores) => {
                for (h, s) in hits.iter_mut().zip(&scores) {
                    h.hit.score = *s;
                }
                hits.sort_by(|a, b| b.hit.score.total_cmp(&a.hit.score));
                trace.reranked = true;
            }
            Err(e) => log::info!("search without reranking: {e:#}"),
        }
    }
    // Relevance first within each group, the asked speaker's replies ahead of the rest.
    if options.speaker.is_some() {
        hits.sort_by_key(|h| !h.speaker);
    }
    Ok((diverse(hits, limit), trace))
}

/// Embeds the chunks inside `span` that have none in the files searched; returns how many.
#[allow(clippy::too_many_arguments)]
fn embed_span(
    engine: &dyn Engine,
    slots: &Slots,
    settings: &AgentSettings,
    source: &ModelSource,
    ai_dir: &Path,
    files: &[Source],
    span: (f64, f64),
    kind: WorkKind,
    interactive: bool,
    cancel: &AtomicBool,
) -> anyhow::Result<usize> {
    let model = crate::models::local_spec(source)?;
    let device = Some(settings.devices.get(ModelRole::Embedding));
    let hf_token = crate::assistant::hf_token(settings);
    let mut embedded = 0;
    for f in files {
        if !Transcript::exists(ai_dir, &f.id) {
            continue;
        }
        let t = Transcript::open(ai_dir, &f.id)?;
        let todo: Vec<(i64, String)> = t
            .chunks()?
            .into_iter()
            .filter(|(_, c, done)| !done && within(c, span.0, span.1))
            .map(|(id, c, _)| (id, c.text))
            .collect();
        if todo.is_empty() {
            continue;
        }
        let _lease = slots.request(kind, interactive).wait(cancel)?;
        for (id, text) in todo {
            let found = engine.embeddings(&EmbeddingsRequest {
                model: model.clone(),
                input: vec![text],
                kind: EmbedKind::Passage,
                n_ctx: Some(EMBED_CTX),
                device,
                hf_token: hf_token.clone(),
            }, cancel)?;
            let vector = found.into_iter().next().filter(|v| !v.is_empty()).ok_or_else(|| anyhow::anyhow!("the model returned no embedding"))?;
            t.set_embedding(id, &vector)?;
            embedded += 1;
        }
    }
    Ok(embedded)
}

/// Hits for `query` in one transcript (see [`search_files`]).
#[allow(clippy::too_many_arguments)]
pub fn search(engine: &dyn Engine, slots: &Slots, settings: &AgentSettings, ai_dir: &Path, id: &str, query: &str, limit: usize, interactive: bool) -> anyhow::Result<(Vec<Hit>, Trace)> {
    search_with(engine, slots, settings, ai_dir, id, query, limit, Options::default(), interactive)
}

/// [`search`] over a speaker and/or a part of the recording (see [`Options`]).
#[allow(clippy::too_many_arguments)]
pub fn search_with(
    engine: &dyn Engine,
    slots: &Slots,
    settings: &AgentSettings,
    ai_dir: &Path,
    id: &str,
    query: &str,
    limit: usize,
    options: Options,
    interactive: bool,
) -> anyhow::Result<(Vec<Hit>, Trace)> {
    let file = Source { id: id.to_string(), path: PathBuf::new() };
    let (hits, trace) = search_files_with(engine, slots, settings, ai_dir, &[file], query, limit, options, interactive)?;
    Ok((hits.into_iter().map(|h| h.hit).collect(), trace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{Budget, ModelChoices};
    use mediaa_core::retrieval::Chunk;
    use mediaa_engine::testing::FakeEngine;

    fn hit(path: &str, start: f64, score: f32) -> FileHit {
        FileHit { path: path.into(), hit: Hit { id: 0, chunk: Chunk { start, end: start + 60.0, speakers: vec![], text: String::new() }, score }, speaker: false }
    }

    fn chunk(start: f64, end: f64, speakers: Vec<u32>, text: &str) -> Chunk {
        Chunk { start, end, speakers, text: text.into() }
    }

    fn keywords_only() -> AgentSettings {
        AgentSettings { models: ModelChoices { embedding: None, reranker: None, ..ModelChoices::default() }, ..AgentSettings::default() }
    }

    fn with_embedding() -> AgentSettings {
        let source = ModelSource::Local { path: PathBuf::from("C:/models/embed.gguf") };
        AgentSettings { models: ModelChoices { embedding: Some(source), reranker: None, ..ModelChoices::default() }, ..AgentSettings::default() }
    }

    #[test]
    fn variety_is_per_file() {
        let d = diverse(vec![hit("a", 100.0, 3.0), hit("b", 100.0, 2.0), hit("a", 110.0, 1.0), hit("a", 200.0, 0.5)], 10);
        assert_eq!(d.iter().map(|h| (h.path.to_string_lossy().into_owned(), h.hit.chunk.start)).collect::<Vec<_>>(), [
            ("a".to_string(), 100.0),
            ("b".to_string(), 100.0),
            ("a".to_string(), 200.0)
        ]);
        assert_eq!(diverse(vec![hit("a", 0.0, 1.0), hit("b", 0.0, 1.0)], 1).len(), 1);
    }

    /// Keywords only (no models): files without a transcript or an index are counted, the
    /// others searched together.
    #[test]
    fn many_files_by_keywords() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let c = |text: &str| chunk(0.0, 60.0, vec![0], text);
        Transcript::open(ai, "a").unwrap().set_chunks(&[c("Отчёт сдаём в пятницу.")]).unwrap();
        Transcript::open(ai, "b").unwrap().set_chunks(&[c("Про отчёты говорили на прошлой неделе."), c("Погода.")]).unwrap();
        Transcript::open(ai, "c").unwrap();
        let files: Vec<Source> = ["a", "b", "c", "missing"].iter().map(|id| Source { id: id.to_string(), path: PathBuf::from(format!("/m/{id}.mp4")) }).collect();
        // No models are chosen, so the engine is never asked anything.
        let engine = FakeEngine::new();
        let slots = Slots::new(Budget::default());
        let (hits, trace) = search_files(&engine, &slots, &keywords_only(), ai, &files, "отчёт", 5, true).unwrap();
        assert!(engine.chats().is_empty() && engine.embeddings_count() == 0, "the engine is not asked anything");
        assert_eq!(trace, Trace { by_meaning: false, reranked: false, files: 4, not_indexed: 2, embedded: 0 });
        let paths: Vec<_> = hits.iter().map(|h| h.path.to_string_lossy().into_owned()).collect();
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert!(paths.contains(&"/m/a.mp4".to_string()) && paths.contains(&"/m/b.mp4".to_string()));
    }

    /// A span keeps only the fragments that touch it, both ends inclusive.
    #[test]
    fn span_keeps_the_edges() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let chunks = [
            chunk(0.0, 100.0, vec![0], "Отчёт в пятницу."),
            chunk(200.0, 300.0, vec![0], "Отчёт готов."),
            chunk(400.0, 500.0, vec![0], "Отчёт позже."),
        ];
        Transcript::open(ai, "a").unwrap().set_chunks(&chunks).unwrap();
        let files = [Source { id: "a".into(), path: "/m/a.mp4".into() }];
        let engine = FakeEngine::new();
        let slots = Slots::new(Budget::default());
        let find = |span| {
            let options = Options { span: Some(span), ..Options::default() };
            let (hits, _) = search_files_with(&engine, &slots, &keywords_only(), ai, &files, "отчёт", 5, options, true).unwrap();
            hits.iter().map(|h| h.hit.chunk.start).collect::<Vec<_>>()
        };
        // The first chunk ends where the span starts, the second starts at 200: both touch it.
        let mut touching = find((100.0, 300.0));
        touching.sort_by(f64::total_cmp);
        assert_eq!(touching, [0.0, 200.0]);
        assert_eq!(find((101.0, 399.0)), [200.0]);
        assert!(find((600.0, 700.0)).is_empty(), "past the recording");
    }

    /// The asked speaker's replies come first, the rest still follows (not a filter).
    #[test]
    fn the_asked_speaker_leads() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        let filler = "и ещё длинный хвост из посторонних слов, чтобы вес слова в документе был ниже";
        let chunks = [
            chunk(0.0, 60.0, vec![0], "Отчёт."),
            chunk(120.0, 180.0, vec![1], &format!("Отчёт, {filler}.")),
        ];
        Transcript::open(ai, "a").unwrap().set_chunks(&chunks).unwrap();
        let files = [Source { id: "a".into(), path: "/m/a.mp4".into() }];
        let engine = FakeEngine::new();
        let slots = Slots::new(Budget::default());
        let starts = |speaker| {
            let options = Options { speaker, ..Options::default() };
            let (hits, _) = search_files_with(&engine, &slots, &keywords_only(), ai, &files, "отчёт", 5, options, true).unwrap();
            hits.iter().map(|h| h.hit.chunk.start).collect::<Vec<_>>()
        };
        // By relevance alone the short chunk wins.
        assert_eq!(starts(None), [0.0, 120.0]);
        // Asked for @s2, its reply comes first, and the other one is still there.
        assert_eq!(starts(Some(Parsed { index: 1, recording: None })), [120.0, 0.0]);
        // A label of another recording in a folder names nothing here.
        assert_eq!(starts(Some(Parsed { index: 1, recording: Some(2) })), [0.0, 120.0]);
    }

    /// Only the chunks of the span are embedded, they are kept, and a second search reuses them.
    #[test]
    fn a_span_is_indexed_on_demand() {
        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path();
        // Two hours in 5-minute chunks, none embedded yet.
        let chunks: Vec<Chunk> = (0..24).map(|i| {
            let start = i as f64 * 300.0 + 10.0;
            chunk(start, start + 290.0, vec![(i % 2) as u32], &format!("кусок {i} слово отчёт"))
        }).collect();
        Transcript::open(ai, "a").unwrap().set_chunks(&chunks).unwrap();
        let files = [Source { id: "a".into(), path: "/m/a.mp4".into() }];
        let engine = FakeEngine::new();
        let slots = Slots::new(Budget::default());
        let span = Options { span: Some((0.0, 1800.0)), ..Options::default() };
        let (_, trace) = search_files_with(&engine, &slots, &with_embedding(), ai, &files, "отчёт", 3, span, true).unwrap();
        // Six chunks reach into the first 30 minutes: start = 10, 310, 610, 910, 1210, 1510.
        assert_eq!(trace.embedded, 6);
        assert_eq!(engine.kinds().iter().filter(|k| **k == EmbedKind::Passage).count(), 6);
        assert_eq!(engine.kinds().iter().filter(|k| **k == EmbedKind::Query).count(), 1);
        let embedded: Vec<bool> = Transcript::open(ai, "a").unwrap().chunks().unwrap().into_iter().map(|(_, _, done)| done).collect();
        assert_eq!(embedded, (0..24).map(|i| i < 6).collect::<Vec<_>>(), "only the span was embedded, and it was saved");
        // The second search embeds only its query: the span is already indexed.
        let (_, again) = search_files_with(&engine, &slots, &with_embedding(), ai, &files, "отчёт", 3, span, true).unwrap();
        assert_eq!(again.embedded, 0);
        assert_eq!(engine.embeddings_count(), 8, "6 passages + one query + one query");
    }
}
