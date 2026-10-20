//! The `outline` queue task: the topics of a recording (`mediaa_core::outline`), found by the
//! meaning of the dialog chunks, a digest per topic written by the agent's model (one call when
//! the topic fits a window, map-reduce over its windows otherwise), and an iterative merge of
//! neighbouring topics that are the same subject — by similarity, with the model asked about a
//! borderline pair only. Resumable: the boundary plan, the merge state and the merge decision
//! live in the transcript's `meta`, digests are written topic by topic.

use crate::assistant::{Message, Role};
use crate::prompts;
use crate::slots::Slots;
use crate::worker::{Checkpoints, TaskHandler};
use mediaa_core::agent::{AgentSettings, ModelRole, WorkKind};
use mediaa_core::library::human_duration;
use mediaa_core::outline::{self, Chapter, ChunkVector, Digest, Part, PlanTopic, Quote, Window, DIGEST_SCHEMA, WINDOW_WORDS};
use mediaa_core::queue::{Queue, Task};
use mediaa_core::transcript::{self, Reply, Transcript};
use mediaa_engine::{ChatMessage, ChatRequest, Device, EmbedKind, EmbeddingsRequest, Engine, Priority};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const KIND: &str = "outline";

/// Context of the model for one window (~1200 words of Russian ≈ 3000 tokens, plus the answer).
const OUTLINE_CTX: u32 = 8192;
/// The model's answer: a digest with quotes (also the answer of a reduce).
const DIGEST_TOKENS: u32 = 800;
/// The model's answer about one pair of topics: `{"same": true}`.
const PAIR_TOKENS: u32 = 32;
/// A merged topic longer than this keeps its pieces as subtopics (`Digest::parts`), seconds.
const PARTS_MIN_SECONDS: f64 = 15.0 * 60.0;

/// The boundary plan (`Plan`) in the transcript's `meta`.
const PLAN_KEY: &str = "outline-plan";
/// The chapters as the digest stage left them, before the merge; kept after it so the merge can
/// be re-run alone (`MEDIAA_BENCH_MERGE`).
pub(crate) const TOPICS_KEY: &str = "outline-topics";
/// The merge groups the model answered, so a restart does not ask again.
pub(crate) const MERGE_KEY: &str = "outline-merge";
/// The merge is decided and the chapters are final (or `outline-final` still has to be written).
pub(crate) const MERGED_KEY: &str = "outline-merged";
/// The final chapters, written after the merge; removed once they are in the table (a restart
/// that finds it writes them again, so a half-written table repairs itself).
pub(crate) const FINAL_KEY: &str = "outline-final";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
}

/// Queues the outline of `path` (after its indexing).
pub fn enqueue(ai_dir: &Path, path: &Path) -> anyhow::Result<()> {
    let title = path.file_name().unwrap_or_default().to_string_lossy();
    Queue::open(ai_dir)?.enqueue_once(KIND, &title, &serde_json::to_value(Input { path: path.to_path_buf() })?, 0)?;
    Ok(())
}

/// The request for window `i` of `n` (of one topic).
pub fn prompt(window: &Window, i: usize, n: usize, previous: Option<&str>) -> String {
    let (start, end, language) = (human_duration(window.start), human_duration(window.end), outline::language(&window.text));
    prompts::render("outline/part.j2", minijinja::context! { number => i + 1, parts => n, start, end, language, previous, text => &window.text })
}

/// What the model wrote, before the quotes are checked against the replies.
#[derive(Debug, Deserialize)]
struct RawDigest {
    title: String,
    summary: String,
    #[serde(default)]
    decisions: Vec<String>,
    #[serde(default)]
    actions: Vec<String>,
    #[serde(default)]
    questions: Vec<String>,
    #[serde(default)]
    topics: Vec<String>,
    #[serde(default)]
    quotes: Vec<RawQuote>,
}

#[derive(Debug, Deserialize)]
struct RawQuote {
    #[serde(default)]
    time: String,
    #[serde(default)]
    text: String,
}

impl RawDigest {
    /// Parses the quote times; the quotes themselves are checked later, against the replies.
    fn into_digest(self) -> Digest {
        Digest {
            title: self.title.trim().trim_end_matches('.').to_string(),
            summary: self.summary.trim().to_string(),
            decisions: self.decisions,
            actions: self.actions,
            questions: self.questions,
            topics: self.topics,
            quotes: self
                .quotes
                .into_iter()
                .filter_map(|q| outline::clock_time(&q.time).map(|time| Quote { time, text: q.text.trim().to_string() }))
                .filter(|q| !q.text.is_empty())
                .collect(),
            parts: Vec::new(),
        }
    }
}

fn chat_request(model: &str, system: String, user: String, schema: Value, max_tokens: u32, device: Device, hf_token: Option<&str>) -> ChatRequest {
    let messages = [Message::new(Role::System, system), Message::new(Role::User, user)];
    ChatRequest {
        model: model.to_string(),
        messages: messages.iter().map(|m| ChatMessage::new(m.role.into(), m.content.clone())).collect(),
        max_tokens,
        temperature: 0.0,
        json_schema: Some(schema),
        no_repeats: true,
        n_ctx: Some(OUTLINE_CTX),
        device: Some(device),
        priority: Some(Priority::Background),
        hf_token: hf_token.map(str::to_string),
    }
}

#[allow(clippy::too_many_arguments)]
fn ask(engine: &dyn Engine, model: &str, device: Device, hf_token: Option<&str>, system: String, user: String, schema: Value, max_tokens: u32, cancel: &AtomicBool) -> anyhow::Result<Digest> {
    let request = chat_request(model, system, user, schema, max_tokens, device, hf_token);
    let text = engine.chat(&request, cancel, &mut |_| true)?.content;
    let raw: RawDigest = serde_json::from_str(text.trim()).map_err(|e| anyhow::anyhow!("the model's digest is not valid: {e}"))?;
    Ok(raw.into_digest())
}

/// Asks the engine's `model` for the digest of a window.
#[allow(clippy::too_many_arguments)]
pub fn digest(engine: &dyn Engine, model: &str, device: Device, hf_token: Option<&str>, window: &Window, i: usize, n: usize, previous: Option<&str>, cancel: &AtomicBool) -> anyhow::Result<Digest> {
    ask(
        engine,
        model,
        device,
        hf_token,
        prompts::render("outline/system.j2", ()),
        prompt(window, i, n, previous),
        serde_json::from_str(DIGEST_SCHEMA)?,
        DIGEST_TOKENS,
        cancel,
    )
}

/// Shortens a list for a prompt that gets many digests: the gist is in the summary.
fn short_list(items: &[String]) -> Vec<String> {
    let short = |s: &String| {
        if s.chars().count() <= 140 {
            s.clone()
        } else {
            format!("{}…", s.chars().take(140).collect::<String>())
        }
    };
    items.iter().take(3).map(short).collect()
}

/// The quotes of the parts, in order, without repeats, at most [`outline::MAX_QUOTES`].
fn quotes_of(parts: &[Digest]) -> Vec<Quote> {
    let mut out: Vec<Quote> = Vec::new();
    for quote in parts.iter().flat_map(|d| d.quotes.iter()) {
        if out.iter().any(|q| (q.time - quote.time).abs() < 0.01) {
            continue;
        }
        out.push(quote.clone());
        if out.len() == outline::MAX_QUOTES {
            break;
        }
    }
    out
}

/// The model calls of the outline: a window's digest, a digest from digests, the merge groups.
struct Ask<'a> {
    engine: &'a dyn Engine,
    model: &'a str,
    device: Device,
    hf_token: Option<&'a str>,
    cancel: &'a AtomicBool,
}

impl Ask<'_> {
    fn digest(&self, window: &Window, i: usize, n: usize, previous: Option<&str>) -> anyhow::Result<Digest> {
        digest(self.engine, self.model, self.device, self.hf_token, window, i, n, previous, self.cancel)
    }

    /// The digest of a topic from the digests of its parts. The quotes of the parts are kept;
    /// the model is asked for an empty list. `previous` is the title of the topic before this
    /// one, so the title names what is new.
    fn reduce(&self, language: &str, previous: Option<&str>, parts: &[Digest]) -> anyhow::Result<Digest> {
        let context: Vec<_> = parts
            .iter()
            .enumerate()
            .map(|(i, d)| {
                minijinja::context! {
                    number => i + 1,
                    title => &d.title,
                    summary => &d.summary,
                    decisions => short_list(&d.decisions),
                    actions => short_list(&d.actions),
                    questions => short_list(&d.questions),
                    topics => short_list(&d.topics),
                }
            })
            .collect();
        let user = prompts::render("outline/reduce.j2", minijinja::context! { language, previous, parts => context });
        let mut digest = ask(
            self.engine,
            self.model,
            self.device,
            self.hf_token,
            prompts::render("outline/reduce_system.j2", ()),
            user,
            serde_json::from_str(DIGEST_SCHEMA)?,
            DIGEST_TOKENS,
            self.cancel,
        )?;
        digest.quotes = quotes_of(parts);
        Ok(digest)
    }

    /// Asks the model about one pair of neighbouring topics: is the second the same subject
    /// continued, or a new one? Both digests are given, so the answer is about this pair only.
    fn same_topic(&self, first: &Chapter, second: &Chapter) -> anyhow::Result<bool> {
        let topic = |c: &Chapter| {
            minijinja::context! {
                start => human_duration(c.start),
                end => human_duration(c.end),
                title => &c.digest.title,
                summary => &c.digest.summary,
                decisions => short_list(&c.digest.decisions),
                actions => short_list(&c.digest.actions),
                questions => short_list(&c.digest.questions),
                topics => short_list(&c.digest.topics),
            }
        };
        let user = prompts::render("outline/pair.j2", minijinja::context! { first => topic(first), second => topic(second) });
        let request = chat_request(
            self.model,
            prompts::render("outline/pair_system.j2", ()),
            user,
            pair_schema(),
            PAIR_TOKENS,
            self.device,
            self.hf_token,
        );
        let text = self.engine.chat(&request, self.cancel, &mut |_| true)?.content;
        #[derive(Deserialize)]
        struct Answer {
            same: bool,
        }
        let answer: Answer = serde_json::from_str(text.trim()).map_err(|e| anyhow::anyhow!("the model's answer about a pair is not valid: {e}"))?;
        Ok(answer.same)
    }
}

/// JSON Schema of the model's answer about one pair of topics.
fn pair_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"same": {"type": "boolean"}},
        "required": ["same"]
    })
}

/// Where the topics start and end: by the meaning of the chunks, or (without embeddings) the
/// windows of `WINDOW_WORDS` as before. The first topic starts with the first reply, the last
/// ends with the last one; topics are contiguous.
fn plan_topics(t: &Transcript, dialog: &[Reply], windows: &[Window]) -> anyhow::Result<Vec<PlanTopic>> {
    let chunks = t.chunks_with_embeddings()?;
    let mut starts: Vec<f64> = if !chunks.is_empty() && chunks.iter().all(|(_, embedding)| embedding.is_some()) {
        let vectors: Vec<outline::ChunkVector> = chunks
            .into_iter()
            .map(|(c, embedding)| outline::ChunkVector { start: c.start, end: c.end, embedding: embedding.unwrap_or_default() })
            .collect();
        outline::semantic_boundaries(&vectors).into_iter().map(|i| vectors[i].start).collect()
    } else {
        windows.iter().skip(1).map(|w| w.start).collect()
    };
    let first = dialog.first().map_or(0.0, |r| r.start);
    let last = dialog.last().map_or(first, |r| r.end);
    starts.push(first);
    starts.retain(|s| s.is_finite());
    starts.sort_by(f64::total_cmp);
    starts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let mut plan: Vec<PlanTopic> = Vec::new();
    for (i, start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(last);
        if end > *start {
            plan.push(PlanTopic { start: *start, end });
        }
    }
    Ok(plan)
}

/// The plan as stored in the transcript's `meta`.
#[derive(Debug, Serialize, Deserialize)]
struct Plan {
    topics: Vec<PlanTopic>,
}

fn load_plan(t: &Transcript) -> anyhow::Result<Option<Vec<PlanTopic>>> {
    Ok(t.meta(PLAN_KEY)?.and_then(|s| serde_json::from_str::<Plan>(&s).ok()).map(|p| p.topics))
}

/// Where the task stopped: the stage and the next topic to digest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Stage {
    Plan,
    Digest,
    Merge,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
struct Cursor {
    stage: Stage,
    #[serde(default)]
    topic: usize,
}

fn cursor_value(cursor: Cursor) -> anyhow::Result<Value> {
    Ok(serde_json::to_value(cursor)?)
}

fn placeholder(topic: &PlanTopic) -> Chapter {
    Chapter { start: topic.start, end: topic.end, digest: Digest::default() }
}

/// The replies of a topic (by their middle, so a reply on a boundary goes to one of them).
fn topic_replies(replies: &[Reply], topic: &PlanTopic, with_end: bool) -> Vec<Reply> {
    replies
        .iter()
        .filter(|r| {
            let middle = (r.start + r.end) / 2.0;
            middle >= topic.start && (middle < topic.end || (with_end && middle <= topic.end))
        })
        .cloned()
        .collect()
}

/// The digest of one topic: one call when its text fits a window, else the digests of its
/// windows and then a digest of those. Every quote is checked against the topic's replies.
fn digest_topic(ask: &Ask<'_>, replies: &[Reply], names: &std::collections::BTreeMap<u32, String>, language: &str, topic: &PlanTopic, with_end: bool, previous: Option<&str>) -> anyhow::Result<Digest> {
    let picked = topic_replies(replies, topic, with_end);
    let label = |s: u32| outline::unnamed_speaker(language, s);
    let words: usize = picked.iter().map(|r| r.words.len()).sum();
    if words <= WINDOW_WORDS {
        let text = picked.iter().map(|r| outline::line(r, names, label)).collect::<Vec<_>>().join("\n");
        let window = Window { start: topic.start, end: topic.end, text };
        let mut digest = ask.digest(&window, 0, 1, previous)?;
        digest.quotes = outline::verified_quotes(&digest.quotes, &picked);
        return Ok(digest);
    }
    let windows = outline::windows(&picked, WINDOW_WORDS, names, label);
    let n = windows.len();
    let mut parts = Vec::with_capacity(n);
    // The title of the topic before this one, for the digest of the whole; inside the topic the
    // parts see each other.
    let topic_before = previous.map(str::to_string);
    let mut previous = topic_before.clone();
    for (i, window) in windows.iter().enumerate() {
        let mut digest = ask.digest(window, i, n, previous.as_deref())?;
        digest.quotes = outline::verified_quotes(&digest.quotes, &picked);
        previous = Some(digest.title.clone());
        parts.push(digest);
    }
    ask.reduce(language, topic_before.as_deref(), &parts)
}

pub(crate) fn write_chapters(t: &Transcript, chapters: &[Chapter]) -> anyhow::Result<()> {
    t.clear_chapters()?;
    for (i, chapter) in chapters.iter().enumerate() {
        t.put_chapter(i, chapter)?;
    }
    Ok(())
}

/// The chunks of a chapter by their middle (the last chapter includes its end).
fn topic_chunks(chunks: &[ChunkVector], chapter: &Chapter, with_end: bool) -> Vec<ChunkVector> {
    chunks
        .iter()
        .filter(|c| {
            let middle = (c.start + c.end) / 2.0;
            middle >= chapter.start && (middle < chapter.end || (with_end && middle <= chapter.end))
        })
        .cloned()
        .collect()
}

/// The chunk embeddings of a topic, summed (a run of topics has the sum of their sums).
#[derive(Debug, Clone, Default, PartialEq)]
struct ChunkSum {
    sum: Vec<f64>,
    count: usize,
}

impl ChunkSum {
    fn add(&mut self, other: &ChunkSum) {
        if self.sum.len() < other.sum.len() {
            self.sum.resize(other.sum.len(), 0.0);
        }
        for (a, b) in self.sum.iter_mut().zip(&other.sum) {
            *a += b;
        }
        self.count += other.count;
    }

    fn mean(&self) -> Vec<f64> {
        if self.count == 0 {
            return Vec::new();
        }
        self.sum.iter().map(|v| v / self.count as f64).collect()
    }
}

/// The sums of the chunk embeddings of every chapter.
fn chunk_sums(chunks: &[ChunkVector], chapters: &[Chapter]) -> Vec<ChunkSum> {
    chapters
        .iter()
        .enumerate()
        .map(|(i, chapter)| {
            let pieces = topic_chunks(chunks, chapter, i + 1 == chapters.len());
            let mut sum = ChunkSum { sum: vec![0.0; pieces.first().map_or(0, |c| c.embedding.len())], count: pieces.len() };
            for piece in &pieces {
                for (s, v) in sum.sum.iter_mut().zip(&piece.embedding) {
                    *s += f64::from(*v);
                }
            }
            sum
        })
        .collect()
}

/// The value at quantile `q` of sorted values (nearest rank).
fn quantile(sorted: &[f64], q: f64) -> f64 {
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

/// Where the recording itself puts a clear boundary. Its adjacent-chunk similarities are mostly
/// the inside of topics, with a dip wherever a topic changes: the lower tenth is the level of a
/// real change, the median the level of an ordinary boundary inside a topic. A pair of topics
/// scoring at most `different` is clearly two topics, at least `similar` clearly one; in between
/// the code cannot tell and the model is asked about that pair.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Thresholds {
    different: f64,
    similar: f64,
}

fn thresholds(boundaries: &[f64]) -> Option<Thresholds> {
    let mut values: Vec<f64> = boundaries.iter().skip(1).copied().filter(|s| s.is_finite()).collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(Thresholds { different: quantile(&values, 0.1), similar: quantile(&values, 0.5) })
}

/// How alike two neighbouring topics are: the cosine similarity of their mean chunk embeddings,
/// the cosine similarity of the embeddings of their descriptions (“title — summary”), and their
/// mean (the one known when the other is not). The mean is used because each feature is noisy on
/// its own: a topic told in other words still continues the same subject, and a familiar word
/// does not make two different subjects one.
#[derive(Debug, Clone, Copy, PartialEq)]
struct PairScore {
    chunks: Option<f64>,
    descriptions: Option<f64>,
    score: f64,
}

impl PairScore {
    fn new(chunks: Option<f64>, descriptions: Option<f64>) -> Option<PairScore> {
        let score = match (chunks, descriptions) {
            (Some(chunks), Some(descriptions)) => (chunks + descriptions) / 2.0,
            (Some(one), None) | (None, Some(one)) => one,
            (None, None) => return None,
        };
        Some(PairScore { chunks, descriptions, score })
    }

    /// Both features and their mean, for the log.
    fn describe(&self) -> String {
        match (self.chunks, self.descriptions) {
            (Some(chunks), Some(descriptions)) => format!("chunks {chunks:.2}, descriptions {descriptions:.2} — {:.2}", self.score),
            (Some(chunks), None) => format!("chunks {chunks:.2} — {:.2}", self.score),
            (None, Some(descriptions)) => format!("descriptions {descriptions:.2} — {:.2}", self.score),
            (None, None) => format!("{:.2}", self.score),
        }
    }
}

/// The embedding model for the descriptions of topics, with the texts already embedded.
struct Embed<'a> {
    engine: &'a dyn Engine,
    model: String,
    device: Device,
    hf_token: Option<&'a str>,
    cache: HashMap<String, Vec<f64>>,
    /// Set after the first failure: the merge goes on with the chunk similarities alone.
    failed: bool,
}

impl Embed<'_> {
    /// The embeddings of `texts`, in order; `None` for a text the model could not embed.
    fn vectors(&mut self, texts: &[String]) -> Vec<Option<Vec<f64>>> {
        let mut missing: Vec<String> = Vec::new();
        for text in texts {
            if !self.cache.contains_key(text) && !missing.contains(text) {
                missing.push(text.clone());
            }
        }
        if !missing.is_empty() && !self.failed {
            let request = EmbeddingsRequest {
                model: self.model.clone(),
                input: missing.clone(),
                kind: EmbedKind::Passage,
                n_ctx: Some(crate::index::EMBED_CTX),
                device: Some(self.device),
                hf_token: self.hf_token.map(str::to_string),
            };
            match self.engine.embeddings(&request) {
                Ok(vectors) if vectors.len() == missing.len() && vectors.iter().all(|v| !v.is_empty()) => {
                    for (text, vector) in missing.into_iter().zip(vectors) {
                        self.cache.insert(text, vector.into_iter().map(f64::from).collect());
                    }
                }
                Ok(_) => {
                    log::warn!("outline merge: the embedding model returned no vectors for the topics; the merge uses the chunks alone");
                    self.failed = true;
                }
                Err(e) => {
                    log::warn!("outline merge: the descriptions of the topics are not embedded ({e}); the merge uses the chunks alone");
                    self.failed = true;
                }
            }
        }
        texts.iter().map(|text| self.cache.get(text).cloned()).collect()
    }
}

/// The model's answer about one borderline pair, by the first original chapter of each side.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct PairAnswer {
    left: usize,
    right: usize,
    same: bool,
}

/// Where the merge stopped: the runs of original chapters, the digests of merged runs and the
/// model's answers. Kept in the transcript's `meta`, so a restart goes on from here.
#[derive(Debug, Default, Serialize, Deserialize)]
struct MergeState {
    runs: Vec<Vec<usize>>,
    /// Merged digests by the first original chapter of their run (a run of one has none here).
    #[serde(default)]
    digests: BTreeMap<usize, Digest>,
    #[serde(default)]
    answers: Vec<PairAnswer>,
}

impl MergeState {
    fn fresh(n: usize) -> MergeState {
        MergeState { runs: (0..n).map(|i| vec![i]).collect(), ..MergeState::default() }
    }

    /// Whether the state is about exactly these chapters (a state of another outline is dropped).
    fn fits(&self, n: usize) -> bool {
        !self.runs.is_empty()
            && self.runs.iter().all(|run| !run.is_empty())
            && self.runs.iter().flatten().copied().eq(0..n)
            && self.runs.iter().all(|run| run.len() == 1 || self.digests.contains_key(&run[0]))
    }
}

/// The similarity of the neighbouring runs `i` and `i + 1` (their sums and description
/// embeddings are prepared for all runs at once).
fn pair_score(sums: &[ChunkSum], descriptions: &[Option<Vec<f64>>], i: usize) -> Option<PairScore> {
    let chunks = (sums[i].count > 0 && sums[i + 1].count > 0).then(|| outline::cosine(&sums[i].mean(), &sums[i + 1].mean()));
    let descriptions = match (&descriptions[i], &descriptions[i + 1]) {
        (Some(a), Some(b)) => Some(outline::cosine(a, b)),
        _ => None,
    };
    PairScore::new(chunks, descriptions)
}

/// The agglomerative merge: repeatedly take the most alike neighbouring pair; the code merges it
/// when the recording's own similarities say it is clearly one topic, keeps it when they say it
/// is clearly two, and the model decides a borderline pair. After a merge the similarities of
/// the new topic are computed again.
struct Merger<'a, 'b> {
    ask: &'a Ask<'b>,
    language: &'a str,
    chapters: &'a [Chapter],
    chunk_sums: Vec<ChunkSum>,
    embed: Option<Embed<'a>>,
    thresholds: Thresholds,
    state: MergeState,
}

impl Merger<'_, '_> {
    /// What a run looks like as a chapter: its span is the first original chapter's start to the
    /// last one's end; its digest is the merged one when the run has several chapters.
    fn run_chapter(&self, run: &[usize]) -> Chapter {
        let first = run[0];
        let last = *run.last().unwrap_or(&first);
        let digest = self.state.digests.get(&first).cloned().unwrap_or_else(|| self.chapters[first].digest.clone());
        Chapter { start: self.chapters[first].start, end: self.chapters[last].end, digest }
    }

    /// The description of a run: the title and summary of its digest.
    fn description_text(&self, run: &[usize]) -> String {
        let chapter = self.run_chapter(run);
        format!("{} — {}", chapter.digest.title.trim(), chapter.digest.summary.trim())
    }

    /// One step: merge the most alike pair the code is sure of, or ask the model about the most
    /// alike borderline pair. `false` when no pair is left to merge.
    fn step(&mut self) -> anyhow::Result<bool> {
        let n = self.state.runs.len();
        if n < 2 {
            return Ok(false);
        }
        let mut sums = Vec::with_capacity(n);
        for run in &self.state.runs {
            let mut sum = ChunkSum::default();
            for &i in run {
                sum.add(&self.chunk_sums[i]);
            }
            sums.push(sum);
        }
        let texts: Vec<String> = self.state.runs.iter().map(|run| self.description_text(run)).collect();
        let descriptions = match self.embed.as_mut() {
            Some(embed) => embed.vectors(&texts),
            None => vec![None; n],
        };
        // A pair the model said is two topics stays apart: the first original chapter of each
        // side does not change while the merge goes on.
        let rejected: HashSet<(usize, usize)> = self.state.answers.iter().filter(|a| !a.same).map(|a| (a.left, a.right)).collect();
        let keys: Vec<(usize, usize)> = self.state.runs.windows(2).map(|w| (w[0][0], w[1][0])).collect();
        let mut best: Option<(usize, PairScore)> = None;
        for (i, &(left, right)) in keys.iter().enumerate() {
            if rejected.contains(&(left, right)) {
                continue;
            }
            let Some(score) = pair_score(&sums, &descriptions, i) else { continue };
            if best.as_ref().is_none_or(|(_, b)| score.score > b.score) {
                best = Some((i, score));
            }
        }
        let Some((i, score)) = best else { return Ok(false) };
        let (left, right) = keys[i];
        if score.score >= self.thresholds.similar {
            log::info!(
                "outline merge: topics {}–{}: {}; clearly one topic (at least {:.2}) — merged by the code",
                left + 1,
                right + 1,
                score.describe(),
                self.thresholds.similar
            );
            self.merge(i)?;
            return Ok(true);
        }
        if score.score <= self.thresholds.different {
            // Every other pair is less alike, so none of them can be clearly one topic either.
            log::info!(
                "outline merge: topics {}–{}: {}; clearly different (at most {:.2}) — kept apart",
                left + 1,
                right + 1,
                score.describe(),
                self.thresholds.different
            );
            return Ok(false);
        }
        let same = match self.state.answers.iter().find(|a| a.left == left && a.right == right) {
            Some(answer) => answer.same,
            None => {
                // The question is about the current runs, not their first chapters: a merged run
                // is described by its merged digest and its whole span.
                let first = self.run_chapter(&self.state.runs[i]);
                let second = self.run_chapter(&self.state.runs[i + 1]);
                let same = self.ask.same_topic(&first, &second)?;
                self.state.answers.push(PairAnswer { left, right, same });
                same
            }
        };
        log::info!(
            "outline merge: topics {}–{}: {}; between {:.2} and {:.2} — the model says {}",
            left + 1,
            right + 1,
            score.describe(),
            self.thresholds.different,
            self.thresholds.similar,
            if same { "one topic — merged" } else { "a new topic — kept apart" }
        );
        if same {
            self.merge(i)?;
        }
        Ok(true)
    }

    /// Merges the runs at `i` and `i + 1`; the merged digest is a summary of the digests of all
    /// the original topics of the new run. The run before it (if any) tells the model which title
    /// not to repeat.
    fn merge(&mut self, i: usize) -> anyhow::Result<()> {
        let start = self.state.runs[i][0];
        let right = self.state.runs[i + 1][0];
        let run: Vec<usize> = self.state.runs[i].iter().chain(&self.state.runs[i + 1]).copied().collect();
        let digests: Vec<Digest> = run.iter().map(|&k| self.chapters[k].digest.clone()).collect();
        let previous = (i > 0).then(|| self.run_chapter(&self.state.runs[i - 1]).digest.title).filter(|title| !title.is_empty());
        let digest = self.ask.reduce(self.language, previous.as_deref(), &digests)?;
        self.state.runs[i] = run;
        self.state.runs.remove(i + 1);
        self.state.digests.insert(start, digest);
        if right != start {
            self.state.digests.remove(&right);
        }
        Ok(())
    }

    /// The chapters after the merge: a run is one chapter; a merged topic longer than
    /// [`PARTS_MIN_SECONDS`] keeps its pieces as subtopics.
    fn finish(&mut self) -> Vec<Chapter> {
        self.state
            .runs
            .iter()
            .map(|run| {
                let first = run[0];
                let last = *run.last().unwrap_or(&first);
                let mut chapter = if run.len() == 1 {
                    self.chapters[first].clone()
                } else {
                    let mut digest = self.state.digests.get(&first).cloned().unwrap_or_default();
                    digest.parts.clear();
                    Chapter { start: self.chapters[first].start, end: self.chapters[last].end, digest }
                };
                if run.len() > 1 && chapter.end - chapter.start > PARTS_MIN_SECONDS {
                    chapter.digest.parts = run
                        .iter()
                        .map(|&i| Part { start: self.chapters[i].start, end: self.chapters[i].end, title: self.chapters[i].digest.title.clone() })
                        .collect();
                }
                chapter
            })
            .collect()
    }
}

/// The merge stage: merge the neighbouring topics the recording's own similarities say are one,
/// asking the model about a borderline pair only, until no pair is clearly one topic. The table
/// is replaced only at the end; `outline-final` lets a restart finish a half-written table,
/// `outline-merged` says the table is final.
#[allow(clippy::too_many_arguments)]
fn merge_stage(
    t: &Transcript,
    ask: &Ask<'_>,
    language: &str,
    embedding: Option<(&str, Device)>,
    hf_token: Option<&str>,
    cp: &mut Checkpoints,
    cursor: Cursor,
) -> anyhow::Result<usize> {
    if t.meta(MERGED_KEY)?.is_some() {
        if let Some(json) = t.meta(FINAL_KEY)? {
            write_chapters(t, &serde_json::from_str::<Vec<Chapter>>(&json)?)?;
            t.remove_meta(FINAL_KEY)?;
        }
        return Ok(t.chapters()?.len());
    }
    let chapters = t.chapters()?;
    if chapters.len() < 2 {
        t.set_meta(MERGED_KEY, "1")?;
        return Ok(chapters.len());
    }
    let chunks = t.chunks_with_embeddings()?;
    if chunks.is_empty() || chunks.iter().any(|(_, embedding)| embedding.is_none()) {
        log::info!("outline merge: the chunks have no embeddings, so the topics are not merged");
        t.set_meta(MERGED_KEY, "1")?;
        return Ok(chapters.len());
    }
    let vectors: Vec<ChunkVector> = chunks
        .into_iter()
        .map(|(chunk, embedding)| ChunkVector { start: chunk.start, end: chunk.end, embedding: embedding.unwrap_or_default() })
        .collect();
    let Some(thresholds) = thresholds(&outline::boundary_similarities(&vectors)) else {
        t.set_meta(MERGED_KEY, "1")?;
        return Ok(chapters.len());
    };
    let state = match t.meta(MERGE_KEY)?.and_then(|json| serde_json::from_str::<MergeState>(&json).ok()) {
        Some(state) if state.fits(chapters.len()) => state,
        _ => MergeState::fresh(chapters.len()),
    };
    let embed = embedding.map(|(model, device)| Embed { engine: ask.engine, model: model.to_string(), device, hf_token, cache: HashMap::new(), failed: false });
    let mut merger = Merger { ask, language, chapters: &chapters, chunk_sums: chunk_sums(&vectors, &chapters), embed, thresholds, state };
    let keep = cursor_value(cursor)?;
    cp.save(&keep, &Value::Null, 0.97)?;
    loop {
        let more = merger.step()?;
        t.set_meta(MERGE_KEY, &serde_json::to_string(&merger.state)?)?;
        let merged = 1.0 - merger.state.runs.len() as f32 / chapters.len() as f32;
        cp.save(&keep, &Value::Null, 0.97 + 0.02 * merged)?;
        if !more {
            break;
        }
    }
    let final_chapters = merger.finish();
    t.set_meta(FINAL_KEY, &serde_json::to_string(&final_chapters)?)?;
    t.set_meta(MERGED_KEY, "1")?;
    write_chapters(t, &final_chapters)?;
    t.remove_meta(FINAL_KEY)?;
    Ok(final_chapters.len())
}

pub struct Outline {
    ai_dir: PathBuf,
    engine: Arc<dyn Engine>,
    slots: Slots,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl Outline {
    pub fn new(ai_dir: &Path, engine: Arc<dyn Engine>, slots: Slots, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Outline {
        Outline { ai_dir: ai_dir.to_path_buf(), engine, slots, settings }
    }
}

impl TaskHandler for Outline {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(Transcript::exists(&self.ai_dir, &id), "{} has no transcript", input.path.display());
        let t = Transcript::open(&self.ai_dir, &id)?;
        // A cursor of another shape (or none) starts over: the old checkpoint cannot be used.
        let saved: Option<Cursor> = serde_json::from_value(task.cursor.clone()).ok();
        if saved.is_none() {
            t.clear_chapters()?;
            for key in [PLAN_KEY, TOPICS_KEY, MERGE_KEY, MERGED_KEY, FINAL_KEY] {
                t.remove_meta(key)?;
            }
        }
        let mut cursor = saved.unwrap_or(Cursor { stage: Stage::Plan, topic: 0 });
        let settings = (self.settings)();
        // Provider models write outlines once the agent talks to providers (P4).
        let model = settings.models.agent.as_ref().and_then(|source| crate::models::local_spec(source).ok());
        let Some(model) = model else { return Ok(json!({ "chapters": 0, "skipped": "the agent model is not local" })) };
        let names = t.speaker_names()?;
        let dialog = t.dialog()?;
        let language = outline::language(&dialog.iter().take(20).map(|r| r.text()).collect::<Vec<_>>().join(" "));
        let windows = outline::windows(&dialog, outline::WINDOW_WORDS, &names, |s| outline::unnamed_speaker(language, s));
        let device = settings.devices.get(ModelRole::Agent);
        let embedding = settings.models.embedding.as_ref().and_then(|source| crate::models::local_spec(source).ok());
        let embedding_device = settings.devices.get(ModelRole::Embedding);
        let hf_token = crate::assistant::hf_token(&settings);
        let cancel = cp.stopping();
        let _lease = self.slots.request(WorkKind::Indexing, false).wait(cancel)?;
        let ask = Ask { engine: self.engine.as_ref(), model: &model, device, hf_token: hf_token.as_deref(), cancel };

        // The boundary plan (also when a restart finds it missing).
        let mut plan = load_plan(&t)?;
        if cursor.stage == Stage::Plan || (cursor.stage == Stage::Digest && plan.is_none()) {
            let topics = plan_topics(&t, &dialog, &windows)?;
            t.set_meta(PLAN_KEY, &serde_json::to_string(&Plan { topics: topics.clone() })?)?;
            let chapters: Vec<Chapter> = topics.iter().map(placeholder).collect();
            write_chapters(&t, &chapters)?;
            plan = Some(topics);
            if cursor.stage == Stage::Plan {
                cursor = Cursor { stage: Stage::Digest, topic: 0 };
                cp.save(&cursor_value(cursor)?, &Value::Null, 0.03)?;
            }
        }
        let plan = plan.unwrap_or_default();
        if cursor.stage == Stage::Digest {
            let n = plan.len();
            let known = t.chapters()?;
            for (i, topic) in plan.iter().enumerate().skip(known.len()) {
                t.put_chapter(i, &placeholder(topic))?;
            }
            let mut previous = t.chapters()?.iter().take(cursor.topic).rev().find(|c| !c.digest.title.is_empty()).map(|c| c.digest.title.clone());
            for (i, topic) in plan.iter().enumerate().skip(cursor.topic) {
                let started = std::time::Instant::now();
                let digest = digest_topic(&ask, &dialog, &names, language, topic, i + 1 == n, previous.as_deref())?;
                log::info!("outline of {}: topic {}/{n} in {:.1} s", input.path.display(), i + 1, started.elapsed().as_secs_f32());
                previous = Some(digest.title.clone());
                t.put_chapter(i, &Chapter { start: topic.start, end: topic.end, digest })?;
                cursor = Cursor { stage: Stage::Digest, topic: i + 1 };
                cp.save(&cursor_value(cursor)?, &Value::Null, 0.03 + 0.93 * (i + 1) as f32 / n.max(1) as f32)?;
            }
            cursor = Cursor { stage: Stage::Merge, topic: 0 };
            // The chapters of the digest stage are kept whole: the merge can be re-run alone.
            t.set_meta(TOPICS_KEY, &serde_json::to_string(&t.chapters()?)?)?;
            cp.save(&cursor_value(cursor)?, &Value::Null, 0.96)?;
        }
        if cursor.stage == Stage::Merge {
            let chapters = merge_stage(
                &t,
                &ask,
                language,
                embedding.as_deref().map(|model| (model, embedding_device)),
                hf_token.as_deref(),
                cp,
                cursor,
            )?;
            cp.save(&cursor_value(Cursor { stage: Stage::Done, topic: 0 })?, &Value::Null, 1.0)?;
            return Ok(json!({ "chapters": chapters }));
        }
        Ok(json!({ "chapters": t.chapters()?.len() }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Models are required (`MEDIAA_TEST_MODELS`, the app's folder by default); the default agent
    /// model is downloaded on first use (~2.5 GB). It digests a short meeting: a title, the
    /// decision and the action items with names.
    #[test]
    #[ignore]
    fn digests_a_meeting() {
        // `RUST_LOG=info` shows the merge decisions of the bench.
        let _ = env_logger::Builder::from_env(env_logger::Env::default()).try_init();
        let engine = crate::live::engine();
        let model = match ModelRole::Agent.default_source().unwrap() {
            mediaa_core::agent::ModelSource::HuggingFace { repo } => repo,
            other => panic!("{other:?}"),
        };
        let text = "[00:00] Дана: Давайте по отчёту для заказчика. Олег, где мы?\n\
                    [00:05] Олег: Цифры за квартал собраны, не хватает графиков по продажам.\n\
                    [00:12] Дана: Графики сделает Ира до четверга. Отчёт отправляем в пятницу утром, решено.\n\
                    [00:20] Ира: Хорошо, сделаю. А презентацию для созвона кто готовит?\n\
                    [00:26] Дана: Пока не знаю, обсудим в понедельник.\n\
                    [00:30] Олег: Я ещё проверю цифры по региону Север, там были вопросы.";
        let w = Window { start: 0.0, end: 35.0, text: text.into() };
        let d = digest(engine.as_ref(), &model, Device::Auto, None, &w, 0, 1, None, &AtomicBool::new(false)).unwrap();
        eprintln!("{d:#?}");
        assert!(!d.title.is_empty() && d.title.split_whitespace().count() <= 12, "{}", d.title);
        assert!(!d.summary.is_empty());
        assert!(d.decisions.iter().any(|x| x.to_lowercase().contains("пятниц")), "{:?}", d.decisions);
        assert!(d.actions.iter().any(|x| x.contains("Ира")), "{:?}", d.actions);
    }

    #[test]
    fn prompts_carry_the_part_and_its_neighbour() {
        let w = Window { start: 600.0, end: 1200.0, text: "[10:00] Дана: начнём.".into() };
        // A later part sees the part before it.
        let p = prompt(&w, 1, 3, Some("Планы на квартал"));
        assert!(p.starts_with("Part 2 of 3 of one topic of the recording (10:00 – 20:00).\nWrite everything in Russian.\n"), "{p}");
        assert!(p.contains("The previous part of this topic was about: Планы на квартал\nDescribe what this part adds; do not repeat the previous part."), "{p}");
        assert!(p.contains("[10:00] Дана: начнём."));
        // The first window of a topic sees the topic before it.
        let first = prompt(&w, 0, 1, Some("Планы на квартал"));
        assert!(first.starts_with("One topic of the recording (10:00 – 20:00).\nWrite everything in Russian.\n"), "{first}");
        assert!(first.contains("The previous topic of the recording was: Планы на квартал\nName this topic by what is new in it; do not reuse that title."), "{first}");
        let single = prompt(&w, 0, 1, None);
        assert!(single.starts_with("One topic of the recording (10:00 – 20:00).\nWrite everything in Russian.\n"), "{single}");
        assert!(!single.contains("previous"));
    }

    fn chapter(title: &str, start: f64, end: f64) -> Chapter {
        Chapter { start, end, digest: Digest { title: title.into(), summary: format!("сводка: {title}"), ..Digest::default() } }
    }

    fn sums(specs: &[&[[f32; 2]]]) -> Vec<ChunkSum> {
        specs
            .iter()
            .map(|vectors| {
                let mut sum = ChunkSum { sum: vec![0.0; 2], count: vectors.len() };
                for vector in *vectors {
                    for (s, v) in sum.sum.iter_mut().zip(vector) {
                        *s += f64::from(*v);
                    }
                }
                sum
            })
            .collect()
    }

    fn merger<'a, 'b>(ask: &'a Ask<'b>, chapters: &'a [Chapter], chunk_sums: Vec<ChunkSum>, thresholds: Thresholds, embed: Option<Embed<'a>>) -> Merger<'a, 'b> {
        Merger { ask, language: "Russian", chapters, chunk_sums, embed, thresholds, state: MergeState::fresh(chapters.len()) }
    }

    #[test]
    fn thresholds_come_from_the_recording() {
        // Nine boundaries: four inside topics (1.0) and a slope down to the dips.
        let boundaries = [0.0, 1.0, 1.0, 1.0, 1.0, 0.9, 0.7, 0.6, 0.4, 0.1];
        assert_eq!(thresholds(&boundaries), Some(Thresholds { different: 0.4, similar: 0.9 }));
        // Nothing to learn from without at least one boundary between chunks.
        assert_eq!(thresholds(&[0.0]), None);
        assert_eq!(thresholds(&[]), None);
    }

    #[test]
    fn a_pair_score_is_the_mean_of_both_features() {
        let score = PairScore::new(Some(0.9), Some(0.5)).unwrap();
        assert_eq!((score.chunks, score.descriptions), (Some(0.9), Some(0.5)));
        assert!((score.score - 0.7).abs() < 1e-9);
        assert_eq!(PairScore::new(Some(0.4), None).unwrap().score, 0.4);
        assert_eq!(PairScore::new(None, Some(0.6)).unwrap().score, 0.6);
        assert_eq!(PairScore::new(None, None), None);
    }

    #[test]
    fn four_identical_topics_become_one() {
        use mediaa_engine::testing::FakeEngine;
        let fake = FakeEngine::new();
        let reduce = |title: &str| format!(r#"{{"title":"{title}","summary":"всё об одном","decisions":[],"actions":[],"questions":[],"topics":[]}}"#);
        fake.chat_answers(&[&reduce("Одна тема"), &reduce("Одна тема"), &reduce("Одна тема")]);
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let chapters: Vec<Chapter> = (0..4).map(|i| chapter(&format!("Тема {}", i + 1), i as f64 * 360.0, (i + 1) as f64 * 360.0)).collect();
        // Every topic is made of the same meaning: similarity 1.0 at every step.
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[1.0, 0.0]], &[[1.0, 0.0]], &[[1.0, 0.0]]]);
        let mut m = merger(&ask, &chapters, chunk_sums, Thresholds { different: 0.5, similar: 0.9 }, None);
        while m.step().unwrap() {}
        let merged = m.finish();
        assert_eq!(merged.len(), 1);
        assert_eq!((merged[0].start, merged[0].end), (0.0, 1440.0));
        // 24 minutes: longer than 15, so the pieces stay as subtopics.
        assert_eq!(merged[0].digest.parts.iter().map(|p| p.title.as_str()).collect::<Vec<_>>(), ["Тема 1", "Тема 2", "Тема 3", "Тема 4"]);
        assert_eq!(fake.chats().len(), 3, "one reduce per merge, the model is not asked about pairs");
    }

    #[test]
    fn different_topics_are_kept_apart_without_asking_the_model() {
        use mediaa_engine::testing::FakeEngine;
        let fake = FakeEngine::new();
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let chapters: Vec<Chapter> = (0..4).map(|i| chapter(&format!("Тема {}", i + 1), i as f64 * 600.0, (i + 1) as f64 * 600.0)).collect();
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[0.0, 1.0]], &[[-1.0, 0.0]], &[[0.0, -1.0]]]);
        let mut m = merger(&ask, &chapters, chunk_sums, Thresholds { different: 0.5, similar: 0.9 }, None);
        assert!(!m.step().unwrap(), "the most alike pair is already clearly different");
        assert_eq!(m.finish().len(), 4);
        assert!(fake.chats().is_empty(), "the model is not even asked");
    }

    #[test]
    fn the_model_decides_a_borderline_pair() {
        use mediaa_engine::testing::{FakeEngine, RecordedChat};

        fn decide(same: bool) -> (Vec<Chapter>, Vec<RecordedChat>) {
            let fake = FakeEngine::new();
            let answer = if same { r#"{"same": true}"# } else { r#"{"same": false}"# };
            fake.chat_answers(&[answer, r#"{"title":"Одна тема","summary":"сводка","decisions":[],"actions":[],"questions":[],"topics":[]}"#]);
            let cancel = AtomicBool::new(false);
            let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
            let chapters = [chapter("Отчёт", 0.0, 300.0), chapter("Бюджет", 300.0, 600.0)];
            // 0.7 between the topics: between "clearly different" (0.5) and "clearly one" (0.9).
            let chunk_sums = sums(&[&[[1.0, 0.0]], &[[0.7, 0.714]]]);
            let mut m = merger(&ask, &chapters, chunk_sums, Thresholds { different: 0.5, similar: 0.9 }, None);
            while m.step().unwrap() {}
            (m.finish(), fake.chats())
        }

        let (merged, calls) = decide(true);
        assert_eq!(merged.len(), 1, "the model said it is one topic");
        assert!(merged[0].digest.parts.is_empty(), "10 minutes: no subtopics");
        assert_eq!(calls.len(), 2, "the question and the reduce");
        assert_eq!(calls[0].messages, 2);
        assert_eq!(calls[0].max_tokens, PAIR_TOKENS);
        assert!(calls[0].json_schema.as_deref().unwrap_or_default().contains("\"same\""));

        let (merged, calls) = decide(false);
        assert_eq!(merged.len(), 2, "the model said it is a new topic");
        assert_eq!(calls.len(), 1, "only the question");
    }

    #[test]
    fn a_pair_question_describes_the_current_runs() {
        use mediaa_engine::testing::FakeEngine;
        let fake = FakeEngine::new();
        let reduce = r#"{"title":"Общий отчёт","summary":"всё об отчёте","decisions":[],"actions":[],"questions":[],"topics":[]}"#;
        fake.chat_answers(&[r#"{"same": true}"#, reduce, r#"{"same": false}"#]);
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let chapters = [chapter("Отчёт", 0.0, 300.0), chapter("Бюджет", 300.0, 600.0), chapter("Найм", 600.0, 900.0)];
        // A and B are alike (0.87, borderline); B and C are further apart (0.77); after A and B
        // are merged, the run against C is borderline again (0.57).
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[0.866, 0.5]], &[[0.342, 0.940]]]);
        let mut m = merger(&ask, &chapters, chunk_sums, Thresholds { different: 0.5, similar: 0.9 }, None);
        while m.step().unwrap() {}
        assert_eq!(m.finish().len(), 2, "A and B merged, C kept apart");
        let calls = fake.chats();
        assert_eq!(calls.len(), 3, "two questions and one reduce");
        assert!(calls[0].texts[1].contains("Chapter A (00:00 – 05:00):\nTitle: Отчёт"), "{}", calls[0].texts[1]);
        // The next question is about the merged run: its merged title and its whole span.
        let second = &calls[2].texts[1];
        assert!(second.contains("Chapter A (00:00 – 10:00):\nTitle: Общий отчёт\nSummary: всё об отчёте"), "{second}");
        assert!(second.contains("Chapter B (10:00 – 15:00):\nTitle: Найм"), "{second}");
    }

    #[test]
    fn the_descriptions_of_topics_are_embedded_and_weigh_in() {
        use mediaa_engine::testing::FakeEngine;
        let fake = FakeEngine::new();
        fake.embed_vectors(vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let chapters = [chapter("Отчёт", 0.0, 600.0), chapter("Бюджет", 600.0, 1200.0)];
        // The chunks are identical (1.0) but the descriptions unrelated (0.0): the mean 0.5 is
        // clearly different, so the topics stay apart.
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[1.0, 0.0]]]);
        let embed = Some(Embed { engine: &fake, model: "ggml-org/embeddinggemma-300M-GGUF".into(), device: Device::Auto, hf_token: None, cache: HashMap::new(), failed: false });
        let mut m = merger(&ask, &chapters, chunk_sums, Thresholds { different: 0.5, similar: 0.9 }, embed);
        assert!(!m.step().unwrap());
        assert_eq!(m.finish().len(), 2);
        assert_eq!(fake.embeddings_count(), 2, "both descriptions are embedded in one request");
        assert!(fake.chats().is_empty());
    }

    #[test]
    fn topics_boundaries_resume_merge_and_quotes() {
        use crate::worker;
        use mediaa_core::agent::AgentSettings;
        use mediaa_core::queue::{Queue, TaskStatus};
        use mediaa_core::retrieval::Chunk;
        use mediaa_core::transcript::Word;
        use mediaa_engine::testing::FakeEngine;

        let dir = tempfile::tempdir().unwrap();
        let ai = dir.path().join(".ai");
        let file = dir.path().join("talk.mp4");
        std::fs::write(&file, b"video").unwrap();
        let id = transcript::file_id(&file).unwrap();
        let mut t = Transcript::open(&ai, &id).unwrap();
        let replies: Vec<Reply> = (0..18)
            .map(|i| {
                let start = i as f64 * 20.0;
                let words: Vec<Word> = format!("реплика{i} обсуждение")
                    .split(' ')
                    .enumerate()
                    .map(|(k, w)| Word { text: w.into(), start: start + k as f64, end: start + k as f64 + 0.5 })
                    .collect();
                Reply { start, end: words.last().unwrap().end, speaker: Some(0), words }
            })
            .collect();
        t.set_replies(&replies).unwrap();
        let chunks: Vec<Chunk> = (0..9)
            .map(|i| Chunk { start: i as f64 * 40.0, end: (i + 1) as f64 * 40.0, speakers: vec![0], text: format!("чанк {i}") })
            .collect();
        t.set_chunks(&chunks).unwrap();
        let vectors = [[1.0_f32, 0.0], [1.0, 0.0], [1.0, 0.0], [0.0, 1.0], [0.0, 1.0], [0.0, 1.0], [-1.0, 0.0], [-1.0, 0.0], [-1.0, 0.0]];
        for ((id, _, _), vector) in t.chunks().unwrap().iter().zip(vectors) {
            t.set_embedding(*id, &vector).unwrap();
        }
        drop(t);

        let digest = |title: &str, quotes: &str| {
            format!(r#"{{"title":"{title}","summary":"сводка","decisions":[],"actions":[],"questions":[],"topics":[],"quotes":{quotes}}}"#)
        };
        // The topics are orthogonal in meaning, so the merge must leave them alone; without an
        // embedding model the pair scores come from the chunks alone.
        let mut settings = AgentSettings::default();
        settings.models.embedding = None;
        let slots = Slots::new(settings.budget.clone());
        let fake = FakeEngine::new();
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        // The first topic's digest, then a broken answer: the task stops after checkpointing topic 1.
        fake.chat_answers(&[
            &digest("Тема раз", r#"[{"time":"0:20","text":"реплика1 обсуждение"},{"time":"0:20","text":"этого не говорили"}]"#),
            "не json",
        ]);
        let s = settings.clone();
        let handlers: Vec<Box<dyn TaskHandler>> = vec![Box::new(Outline::new(&ai, engine, slots, Arc::new(move || s.clone())))];
        let q = Queue::open(&ai).unwrap();
        enqueue(&ai, &file).unwrap();
        let stop = AtomicBool::new(false);
        assert!(worker::run_next(&q, &handlers, &stop).unwrap());
        let task = q.list().unwrap().into_iter().find(|t| t.kind == KIND).unwrap();
        assert_eq!(task.status, TaskStatus::Failed, "{:?}", task.error);
        assert_eq!(task.cursor, json!({ "stage": "digest", "topic": 1 }), "a restart continues from the second topic");
        let t = Transcript::open(&ai, &id).unwrap();
        assert!(t.meta(PLAN_KEY).unwrap().is_some(), "the plan is kept in meta");
        assert_eq!(t.chapters().unwrap().len(), 3, "placeholders of all topics");
        assert_eq!(t.chapters().unwrap()[0].digest.title, "Тема раз");
        assert_eq!(t.chapters().unwrap()[0].digest.quotes, [Quote { time: 20.0, text: "реплика1 обсуждение".into() }], "the quote is checked, the invented one dropped");
        drop(t);

        q.resume(task.id).unwrap();
        fake.chat_answers(&[&digest("Тема два", r#"[{"time":"2:00","text":"реплика6 обсуждение"}]"#), &digest("Тема три", "[]")]);
        assert!(worker::run_next(&q, &handlers, &stop).unwrap());
        let task = q.get(task.id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        assert_eq!(task.result, Some(json!({ "chapters": 3 })), "the merge leaves different topics apart");
        let t = Transcript::open(&ai, &id).unwrap();
        let chapters = t.chapters().unwrap();
        assert_eq!(chapters.iter().map(|c| c.digest.title.as_str()).collect::<Vec<_>>(), ["Тема раз", "Тема два", "Тема три"]);
        assert_eq!((chapters[1].start, chapters[1].end), (120.0, 240.0), "the topics keep their spans");
        assert_eq!(chapters[2].end, replies[17].end);
        assert_eq!(chapters[1].digest.quotes, [Quote { time: 120.0, text: "реплика6 обсуждение".into() }], "the quote is kept");
        assert!(t.meta(MERGED_KEY).unwrap().is_some() && t.meta(FINAL_KEY).unwrap().is_none());
        let topics: Vec<Chapter> = serde_json::from_str(&t.meta(TOPICS_KEY).unwrap().unwrap()).unwrap();
        assert_eq!(topics.iter().map(|c| c.digest.title.as_str()).collect::<Vec<_>>(), ["Тема раз", "Тема два", "Тема три"], "the digest-stage chapters are kept for a merge re-run");
        assert_eq!(fake.chats().len(), 4, "two digests, a failure, two digests — no merge question, no reduce");
        assert!(fake.chats()[0].json_schema.as_deref().unwrap_or_default().contains("\"quotes\""));
    }
}
