//! Searching transcripts for a question — one recording or many (a catalog folder): hybrid
//! candidates per file (keywords + the query's embedding, computed once), re-ordered together by
//! the reranker, then varied. Each model step is optional: without an embedding model the
//! search is by keywords, without a reranker the fused order stays.

use crate::index::EMBED_CTX;
use crate::slots::Slots;
use mediaa_core::agent::{AgentSettings, ModelRole, WorkKind};
use mediaa_engine::{EmbedKind, EmbeddingsRequest, Engine, RerankRequest};
use mediaa_core::retrieval::Hit;
use mediaa_core::transcript::Transcript;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

/// Candidates the reranker reads in all (more: better recall, slower).
pub const CANDIDATES: usize = 24;
/// Candidates taken from each file when searching many.
const PER_FILE: usize = 8;
/// Hits of one file closer than this (seconds) to a better one are dropped.
const SAME_PLACE: f64 = 30.0;

/// How a search went (for the honest trace of the agent and for tests).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Trace {
    pub by_meaning: bool,
    pub reranked: bool,
    /// Files searched / without a searchable transcript yet.
    pub files: usize,
    pub not_indexed: usize,
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
    let cancel = AtomicBool::new(false);
    let kind = if interactive { WorkKind::Agent } else { WorkKind::Indexing };
    let device = |role| settings.devices.get(role);
    let hf_token = crate::assistant::hf_token(settings);
    let mut trace = Trace { files: files.len(), ..Trace::default() };
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
    let mut hits: Vec<FileHit> = Vec::new();
    for f in files {
        if !Transcript::exists(ai_dir, &f.id) {
            trace.not_indexed += 1;
            continue;
        }
        let t = Transcript::open(ai_dir, &f.id)?;
        if t.chunks()?.is_empty() {
            trace.not_indexed += 1;
            continue;
        }
        hits.extend(t.candidates(query, embedding.as_deref(), per_file)?.into_iter().map(|hit| FileHit { path: f.path.clone(), hit }));
    }
    // Fused scores are rank based, so files compare; the reranker reads the best of them.
    hits.sort_by(|a, b| b.hit.score.total_cmp(&a.hit.score));
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
    Ok((diverse(hits, limit), trace))
}

/// Hits for `query` in one transcript (see [`search_files`]).
#[allow(clippy::too_many_arguments)]
pub fn search(engine: &dyn Engine, slots: &Slots, settings: &AgentSettings, ai_dir: &Path, id: &str, query: &str, limit: usize, interactive: bool) -> anyhow::Result<(Vec<Hit>, Trace)> {
    let file = Source { id: id.to_string(), path: PathBuf::new() };
    let (hits, trace) = search_files(engine, slots, settings, ai_dir, &[file], query, limit, interactive)?;
    Ok((hits.into_iter().map(|h| h.hit).collect(), trace))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{Budget, ModelChoices};
    use mediaa_core::retrieval::Chunk;

    fn hit(path: &str, start: f64, score: f32) -> FileHit {
        FileHit { path: path.into(), hit: Hit { id: 0, chunk: Chunk { start, end: start + 60.0, speakers: vec![], text: String::new() }, score } }
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
        let chunk = |text: &str| Chunk { start: 0.0, end: 60.0, speakers: vec![0], text: text.into() };
        Transcript::open(ai, "a").unwrap().set_chunks(&[chunk("Отчёт сдаём в пятницу.")]).unwrap();
        Transcript::open(ai, "b").unwrap().set_chunks(&[chunk("Про отчёты говорили на прошлой неделе."), chunk("Погода.")]).unwrap();
        Transcript::open(ai, "c").unwrap();
        let files: Vec<Source> = ["a", "b", "c", "missing"].iter().map(|id| Source { id: id.to_string(), path: PathBuf::from(format!("/m/{id}.mp4")) }).collect();
        let settings = AgentSettings { models: ModelChoices { embedding: None, reranker: None, ..ModelChoices::default() }, ..AgentSettings::default() };
        // No models are chosen, so the engine is never asked anything.
        let engine = mediaa_engine::testing::FakeEngine::new();
        let slots = Slots::new(Budget::default());
        let (hits, trace) = search_files(&engine, &slots, &settings, ai, &files, "отчёт", 5, true).unwrap();
        assert!(engine.chats().is_empty() && engine.embeddings_count() == 0, "the engine is not asked anything");
        assert_eq!(trace, Trace { by_meaning: false, reranked: false, files: 4, not_indexed: 2 });
        let paths: Vec<_> = hits.iter().map(|h| h.path.to_string_lossy().into_owned()).collect();
        assert_eq!(paths.len(), 2, "{paths:?}");
        assert!(paths.contains(&"/m/a.mp4".to_string()) && paths.contains(&"/m/b.mp4".to_string()));
    }
}
