//! The topics of a recording (`mediaa_core::outline`) as four queue tasks: the boundary plan,
//! a digest per topic (one call when the topic fits a window, map-reduce over its windows
//! otherwise), an iterative merge of neighbouring topics that are the same subject (by
//! similarity, with the model asked about a borderline pair only), and a detailed breakdown per
//! final topic. Each task resumes from its own checkpoint: the plan, the merge state and the
//! merge decision live in the transcript's `meta`, digests and sections in its chapters.
//!
//! The graph: [`topics_task`] is a composite with a [`plan_task`] child; the plan completes with
//! a [`DIGEST_KIND`] child per topic and one [`MERGE_KIND`] child after all of them; the merge
//! completes with the [`DETAILS_KIND`] composite and a [`DETAIL_KIND`] child per final topic when
//! the analysis options ask for details.

use crate::assistant::{Message, Role};
use crate::prompts;
use crate::slots::Slots;
use crate::worker::{Checkpoints, TaskHandler};
use mediaa_core::agent::{AgentSettings, AnalysisOptions, ModelRole, WorkKind};
use mediaa_core::library::human_duration;
use mediaa_core::outline::{
    self, Chapter, ChunkVector, Digest, FineLine, Item, Part, PlanTopic, Quote, Section, SectionKind, Window, DIGEST_SCHEMA, WINDOW_WORDS,
};
use mediaa_core::queue::{Child, NewTask, Queue, Task};
use mediaa_core::transcript::{self, Reply, Transcript};
use mediaa_engine::{ChatMessage, ChatRequest, Device, EmbedKind, EmbeddingsRequest, Engine, Priority};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// The composite of the topics of one recording.
pub const TOPICS_KIND: &str = "topics";
/// The boundary plan: chapters with placeholders, then the digest and merge tasks.
pub const PLAN_KIND: &str = "topics-plan";
/// The digest of one topic (`TopicInput::topic`).
pub const DIGEST_KIND: &str = "topic-digest";
/// The merge of neighbouring topics and the start of the details.
pub const MERGE_KIND: &str = "topics-merge";
/// The composite of the detailed breakdown of every final topic.
pub const DETAILS_KIND: &str = "details";
/// The breakdown of one final topic (`TopicInput::topic`).
pub const DETAIL_KIND: &str = "topic-detail";

/// Priority of the topic tasks of an analysis (its stages are 10).
const TOPIC_PRIORITY: i64 = 10;
/// The priority of a `topic-detail` the interface is waiting for (raised by
/// [`crate::pipeline::prioritize_detail`]).
pub const DETAIL_PRIORITY: i64 = 1000;

/// Context of the model for one window (~1200 words of Russian ≈ 3000 tokens, plus the answer).
const OUTLINE_CTX: u32 = 8192;
/// The model's answer: a digest with quotes (also the answer of a reduce).
const DIGEST_TOKENS: u32 = 800;
/// The model's answer about one pair of topics: `{"same": true}`.
const PAIR_TOKENS: u32 = 32;
/// Characters of a model's answer per token (~3 for Russian and English alike).
const CHARS_PER_TOKEN: u32 = 3;
/// Tokens of the JSON around one string of an answer: keys, quotes, commas, the time.
const ITEM_OVERHEAD_TOKENS: u32 = 16;
/// Tokens of the braces and the end of an answer.
const ANSWER_OVERHEAD_TOKENS: u32 = 64;

/// The answer limit of a schema: the characters of a fully filled answer at [`CHARS_PER_TOKEN`]
/// plus the overhead of its items, so the caps of the schema always fit and no answer is cut
/// off in the middle of its JSON.
fn answer_tokens(chars: usize, items: usize) -> u32 {
    (chars as u32).div_ceil(CHARS_PER_TOKEN) + items as u32 * ITEM_OVERHEAD_TOKENS + ANSWER_OVERHEAD_TOKENS
}

/// The limit of the sections of a topic (from the caps of [`outline::sections_schema`]).
fn sections_tokens() -> u32 {
    let chars = outline::SECTIONS_MAX * (outline::SECTION_TITLE_CHARS + outline::SECTION_ITEMS_MAX * outline::SECTION_TEXT_CHARS);
    answer_tokens(chars, outline::SECTIONS_MAX * (outline::SECTION_ITEMS_MAX + 1))
}

/// The limit of the points of one window of a long topic (from the caps of
/// [`outline::points_schema`]).
fn points_tokens() -> u32 {
    answer_tokens(outline::POINTS_MAX * outline::POINT_TEXT_CHARS, outline::POINTS_MAX)
}
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

/// The input of the plan and the merge: the recording and the analysis options (details on or
/// off travel with the work, so a restarted task does what it was asked).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
    #[serde(default)]
    pub options: AnalysisOptions,
}

/// The input of a task about one topic: the recording, the options and the topic's index in the
/// plan (digest) or in the final outline (detail).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicInput {
    pub path: PathBuf,
    #[serde(default)]
    pub options: AnalysisOptions,
    pub topic: usize,
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
            sections: Vec::new(),
        }
    }
}

/// What the model wrote about a topic's breakdown, before the item times are checked.
#[derive(Debug, Deserialize)]
struct RawSections {
    #[serde(default)]
    sections: Vec<RawSection>,
}

#[derive(Debug, Deserialize)]
struct RawSection {
    #[serde(default)]
    title: String,
    #[serde(default)]
    kind: SectionKind,
    #[serde(default)]
    items: Vec<RawItem>,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    #[serde(default)]
    text: String,
    #[serde(default)]
    time: Option<String>,
}

impl RawSections {
    /// The sections whose items have text; an item's time is checked against the fine lines of
    /// the topic ([`outline::verified_detail_time`]): a time the lines do not confirm becomes
    /// `None`, the item itself stays. An empty section is dropped; sections with the same title
    /// are one (their items keep their order, an item said twice is kept once).
    fn into_sections(self, lines: &[FineLine]) -> Vec<Section> {
        let sections = self
            .sections
            .into_iter()
            .filter_map(|section| {
                let items: Vec<Item> = section
                    .items
                    .into_iter()
                    .filter(|item| !item.text.trim().is_empty())
                    .map(|item| {
                        let text = item.text.trim().to_string();
                        let time = item.time.as_deref().and_then(outline::clock_time).and_then(|time| outline::verified_detail_time(time, &text, lines));
                        Item { text, time }
                    })
                    .collect();
                (!items.is_empty()).then(|| Section { title: section.title.trim().to_string(), kind: section.kind, items })
            })
            .collect();
        merge_sections(sections)
    }
}

/// Merges sections with the same title (case-insensitive, trimmed): the items keep their order
/// and an item with the same text is kept once.
fn merge_sections(sections: Vec<Section>) -> Vec<Section> {
    let mut out: Vec<Section> = Vec::new();
    for section in sections {
        let title = section.title.to_lowercase();
        match out.iter_mut().find(|s| s.title.to_lowercase() == title) {
            Some(existing) => {
                for item in section.items {
                    if !existing.items.iter().any(|i| i.text == item.text) {
                        existing.items.push(item);
                    }
                }
            }
            None => out.push(section),
        }
    }
    out
}

/// A point of one window of a long topic, as the model wrote it.
#[derive(Debug, Deserialize)]
struct RawPoint {
    #[serde(default)]
    time: String,
    #[serde(default)]
    text: String,
}

#[derive(Debug, Deserialize)]
struct RawPoints {
    #[serde(default)]
    points: Vec<RawPoint>,
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

    /// The sections of a topic, from its transcript or from the points of its windows; the item
    /// times are checked by the caller, against the topic's replies.
    fn sections(&self, user: String) -> anyhow::Result<RawSections> {
        let request = chat_request(
            self.model,
            prompts::render("outline/detail_system.j2", ()),
            user,
            outline::sections_schema(),
            sections_tokens(),
            self.device,
            self.hf_token,
        );
        let text = self.engine.chat(&request, self.cancel, &mut |_| true)?.content;
        serde_json::from_str(text.trim()).map_err(|e| anyhow::anyhow!("the model's sections are not valid: {e}"))
    }

    /// The points of one window of a long topic.
    fn points(&self, user: String) -> anyhow::Result<Vec<RawPoint>> {
        let request = chat_request(
            self.model,
            prompts::render("outline/detail_system.j2", ()),
            user,
            outline::points_schema(),
            points_tokens(),
            self.device,
            self.hf_token,
        );
        let text = self.engine.chat(&request, self.cancel, &mut |_| true)?.content;
        let raw: RawPoints = serde_json::from_str(text.trim()).map_err(|e| anyhow::anyhow!("the model's points are not valid: {e}"))?;
        Ok(raw.points)
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

/// The parts of a merged topic, as a template sees them.
fn part_context(parts: &[Part]) -> Vec<minijinja::Value> {
    parts
        .iter()
        .map(|part| minijinja::context! { start => human_duration(part.start), end => human_duration(part.end), title => &part.title })
        .collect()
}

/// The detailed breakdown of one topic: one model call when its text fits a window; otherwise
/// the points of every window and one more call that assembles the sections from them (the
/// transcript is not read again). Every item's time is checked against the topic's replies like
/// a quote: an unconfirmed time becomes `None`, the item stays.
fn detail_topic(
    ask: &Ask<'_>,
    replies: &[Reply],
    names: &BTreeMap<u32, String>,
    language: &str,
    chapter: &Chapter,
    last: bool,
) -> anyhow::Result<Vec<Section>> {
    let topic = PlanTopic { start: chapter.start, end: chapter.end };
    let picked = topic_replies(replies, &topic, last);
    let label = |s: u32| outline::unnamed_speaker(language, s);
    // Short lines with their own times: one long reply would otherwise give every item the
    // time of its start.
    let lines = outline::fine_lines(&picked, names, label);
    let words: usize = picked.iter().map(|r| r.words.len()).sum();
    let (title, summary) = (&chapter.digest.title, &chapter.digest.summary);
    let (start, end) = (human_duration(chapter.start), human_duration(chapter.end));
    let parts = part_context(&chapter.digest.parts);
    if words <= WINDOW_WORDS {
        let text = lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>().join("\n");
        let user = prompts::render(
            "outline/detail.j2",
            minijinja::context! { title, summary, start, end, language, parts, points => Vec::<minijinja::Value>::new(), text },
        );
        return Ok(ask.sections(user)?.into_sections(&lines));
    }
    let windows = outline::windows_of_lines(&lines, WINDOW_WORDS);
    let n = windows.len();
    let mut points: Vec<RawPoint> = Vec::new();
    for (i, window) in windows.iter().enumerate() {
        let user = prompts::render(
            "outline/detail_window.j2",
            minijinja::context! {
                title, summary,
                start => human_duration(window.start),
                end => human_duration(window.end),
                number => i + 1,
                parts => n,
                language,
                text => &window.text,
            },
        );
        points.extend(ask.points(user)?);
    }
    let points: Vec<minijinja::Value> =
        points.iter().map(|point| minijinja::context! { time => &point.time, text => &point.text }).collect();
    let user = prompts::render(
        "outline/detail.j2",
        minijinja::context! { title, summary, start, end, language, parts, points, text => "" },
    );
    Ok(ask.sections(user)?.into_sections(&lines))
}

/// The title of a task in the queue: the file (what the task is about; the queue page builds the
/// visible label from the kind and this).
fn file_title(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

/// The title of a topic's task: the topic's own title, or its number (`#3`) while it has none.
fn topic_title(title: &str, number: usize) -> String {
    match title.trim() {
        "" => format!("#{number}"),
        title => title.to_string(),
    }
}

/// The composite task of the topics of a recording. It brings its [`plan_task`] child; the plan
/// adds the digests and the merge, the merge adds the details.
pub fn topics_task(path: &Path, options: &AnalysisOptions) -> NewTask {
    NewTask::new(TOPICS_KIND, &file_title(path), &json!({ "path": path, "options": options })).composite()
}

/// The plan task of the topics of a recording.
pub fn plan_task(path: &Path, options: &AnalysisOptions) -> NewTask {
    NewTask::new(PLAN_KIND, &file_title(path), &json!({ "path": path, "options": options }))
}

/// The composite task of the breakdown of every final topic.
pub fn details_task(path: &Path) -> NewTask {
    NewTask::new(DETAILS_KIND, &file_title(path), &json!({ "path": path })).composite()
}

/// One `topic-detail` child per final topic; each carries its index and the analysis options.
pub fn detail_tasks(path: &Path, options: &AnalysisOptions, chapters: &[Chapter], replies: &[Reply]) -> Vec<Child> {
    chapters
        .iter()
        .enumerate()
        .map(|(i, chapter)| {
            let topic = PlanTopic { start: chapter.start, end: chapter.end };
            let words: usize = topic_replies(replies, &topic, i + 1 == chapters.len()).iter().map(|r| r.words.len()).sum();
            let title = topic_title(&chapter.digest.title, i + 1);
            Child::new(
                NewTask::new(DETAIL_KIND, &title, &json!({ "path": path, "options": options, "topic": i }))
                    .units(words as f64 / 1000.0)
                    .priority(TOPIC_PRIORITY),
            )
        })
        .collect()
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
/// `outline-merged` says the table is final. The digest-stage chapters are kept in `outline-topics`
/// so the merge can be re-run alone.
#[allow(clippy::too_many_arguments)]
fn merge_stage(
    t: &Transcript,
    ask: &Ask<'_>,
    language: &str,
    embedding: Option<(&str, Device)>,
    hf_token: Option<&str>,
    cp: &mut Checkpoints,
) -> anyhow::Result<usize> {
    if t.meta(MERGED_KEY)?.is_some() {
        if let Some(json) = t.meta(FINAL_KEY)? {
            write_chapters(t, &serde_json::from_str::<Vec<Chapter>>(&json)?)?;
            t.remove_meta(FINAL_KEY)?;
        }
        return Ok(t.chapters()?.len());
    }
    let chapters = t.chapters()?;
    if t.meta(TOPICS_KEY)?.is_none() {
        t.set_meta(TOPICS_KEY, &serde_json::to_string(&chapters)?)?;
    }
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
    let keep = json!({ "stage": "merge" });
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

/// Everything a topic task needs from the recording once the model is known.
struct Run {
    transcript: Transcript,
    dialog: Vec<Reply>,
    names: BTreeMap<u32, String>,
    language: &'static str,
    model: String,
    device: Device,
    embedding: Option<String>,
    embedding_device: Device,
    hf_token: Option<String>,
}

/// Opens the recording of `path` and reads the settings; `None` when the agent's model is not
/// local (a provider model writes topics once the agent talks to providers, P4).
fn open_run(ai_dir: &Path, path: &Path, settings: &AgentSettings) -> anyhow::Result<Option<Run>> {
    let id = transcript::file_id(path)?;
    anyhow::ensure!(Transcript::exists(ai_dir, &id), "{} has no transcript", path.display());
    let Some(model) = settings.models.agent.as_ref().and_then(|source| crate::models::local_spec(source).ok()) else {
        return Ok(None);
    };
    let transcript = Transcript::open(ai_dir, &id)?;
    let dialog = transcript.dialog()?;
    let names = transcript.speaker_names()?;
    let language = outline::language(&dialog.iter().take(20).map(|r| r.text()).collect::<Vec<_>>().join(" "));
    Ok(Some(Run {
        transcript,
        dialog,
        names,
        language,
        model,
        device: settings.devices.get(ModelRole::Agent),
        embedding: settings.models.embedding.as_ref().and_then(|source| crate::models::local_spec(source).ok()),
        embedding_device: settings.devices.get(ModelRole::Embedding),
        hf_token: crate::assistant::hf_token(settings),
    }))
}

/// Completes the task together with the children it has just planned, in one transaction. The
/// worker completes it again afterwards with no children, which changes nothing.
fn complete_with_children(ai_dir: &Path, task: &Task, result: &Value, children: &[Child]) -> anyhow::Result<()> {
    Queue::open(ai_dir)?.complete_with(task.id, result, children)?;
    Ok(())
}

/// A task of the topics stage: the plan, a digest, the merge or a detail. One structure for all
/// four, because they share the settings, the folder and the lease; `kind` picks the work.
pub struct TopicTask {
    kind: &'static str,
    ai_dir: PathBuf,
    slots: Slots,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl TopicTask {
    fn new(kind: &'static str, ai_dir: &Path, slots: Slots, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> TopicTask {
        TopicTask { kind, ai_dir: ai_dir.to_path_buf(), slots, settings }
    }
}

/// The handlers of the topics stage and the details, one per task kind.
pub fn handlers(ai_dir: &Path, slots: Slots, settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>) -> Vec<Box<dyn TaskHandler>> {
    [PLAN_KIND, DIGEST_KIND, MERGE_KIND, DETAIL_KIND]
        .into_iter()
        .map(|kind| Box::new(TopicTask::new(kind, ai_dir, slots.clone(), settings.clone())) as Box<dyn TaskHandler>)
        .collect()
}

impl TaskHandler for TopicTask {
    fn kind(&self) -> &'static str {
        self.kind
    }

    fn run(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        match self.kind {
            PLAN_KIND => self.plan(task, cp),
            DIGEST_KIND => self.digest(task, cp),
            MERGE_KIND => self.merge(task, cp),
            DETAIL_KIND => self.detail(task, cp),
            other => anyhow::bail!("unknown topic task {other}"),
        }
    }
}

impl TopicTask {
    /// The boundary plan: the chapters with placeholders, then a digest per topic and one merge
    /// after all of them. A restored plan (a restart in the middle) only adds the children.
    fn plan(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(json!({ "topics": 0, "skipped": "the agent model is not local" }));
        };
        let t = &run.transcript;
        let topics = match load_plan(t)? {
            Some(plan) => plan,
            None => {
                let windows = outline::windows(&run.dialog, WINDOW_WORDS, &run.names, |s| outline::unnamed_speaker(run.language, s));
                let plan = plan_topics(t, &run.dialog, &windows)?;
                t.set_meta(PLAN_KEY, &serde_json::to_string(&Plan { topics: plan.clone() })?)?;
                let chapters: Vec<Chapter> = plan.iter().map(placeholder).collect();
                write_chapters(t, &chapters)?;
                plan
            }
        };
        // The placeholders of every topic; digests already written by a resumed digest task stay.
        let known = t.chapters()?;
        let placeholders: Vec<Chapter> = topics.iter().map(placeholder).collect();
        if known.len() != topics.len() || known.iter().zip(&topics).any(|(c, topic)| (c.start - topic.start).abs() > 0.01 || (c.end - topic.end).abs() > 0.01) {
            write_chapters(t, &placeholders)?;
        }
        cp.save(&json!({ "stage": "plan" }), &Value::Null, 0.03)?;

        let mut children = Vec::with_capacity(topics.len() + 1);
        for (i, topic) in topics.iter().enumerate() {
            let words: usize = topic_replies(&run.dialog, topic, i + 1 == topics.len()).iter().map(|r| r.words.len()).sum();
            children.push(Child::new(
                NewTask::new(DIGEST_KIND, &topic_title("", i + 1),&json!({ "path": input.path, "options": input.options, "topic": i }))
                    .units(words as f64 / 1000.0)
                    .priority(TOPIC_PRIORITY),
            ));
        }
        let all: Vec<usize> = (0..topics.len()).collect();
        children.push(
            Child::new(
                NewTask::new(MERGE_KIND, &file_title(&input.path), &json!({ "path": input.path, "options": input.options }))
                    .units(topics.len() as f64)
                    .priority(TOPIC_PRIORITY),
            )
            .after(&all),
        );
        let minutes = run.dialog.last().map_or(0.0, |r| r.end) / 60.0;
        let result = json!({ "topics": topics.len(), "minutes": minutes });
        complete_with_children(&self.ai_dir, task, &result, &children)?;
        Ok(result)
    }

    /// The digest of one topic; a topic whose digest is already in the table (a restart after
    /// the checkpoint) is not asked again.
    fn digest(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: TopicInput = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(json!({ "topic": input.topic, "skipped": "the agent model is not local" }));
        };
        let t = &run.transcript;
        let plan = load_plan(t)?.ok_or_else(|| anyhow::anyhow!("the topic plan is missing"))?;
        let topic = *plan.get(input.topic).ok_or_else(|| anyhow::anyhow!("topic {} is not in the plan", input.topic + 1))?;
        let already = t.chapters()?.into_iter().nth(input.topic).filter(|chapter| {
            !chapter.digest.title.trim().is_empty() && (chapter.start - topic.start).abs() < 0.01 && (chapter.end - topic.end).abs() < 0.01
        });
        if let Some(chapter) = already {
            return Ok(json!({ "topic": input.topic, "title": chapter.digest.title, "kept": true }));
        }
        let _lease = self.slots.request(WorkKind::Indexing, false).wait(cp.stopping())?;
        let engine = cp.engine();
        let ask = Ask {
            engine: engine.as_ref(),
            model: &run.model,
            device: run.device,
            hf_token: run.hf_token.as_deref(),
            cancel: cp.stopping(),
        };
        let previous = t.chapters()?.iter().take(input.topic).rev().find(|c| !c.digest.title.is_empty()).map(|c| c.digest.title.clone());
        let started = std::time::Instant::now();
        let digest = digest_topic(&ask, &run.dialog, &run.names, run.language, &topic, input.topic + 1 == plan.len(), previous.as_deref())?;
        log::info!(
            "outline of {}: topic {}/{} in {:.1} s",
            input.path.display(),
            input.topic + 1,
            plan.len(),
            started.elapsed().as_secs_f32()
        );
        t.put_chapter(input.topic, &Chapter { start: topic.start, end: topic.end, digest: digest.clone() })?;
        cp.save(&json!({ "topic": input.topic }), &Value::Null, 1.0)?;
        Ok(json!({ "topic": input.topic, "title": digest.title }))
    }

    /// The merge; when the options ask for details, it adds the `details` composite with one
    /// `topic-detail` per final topic in the same transaction.
    fn merge(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(json!({ "chapters": 0, "skipped": "the agent model is not local" }));
        };
        let t = &run.transcript;
        let _lease = self.slots.request(WorkKind::Indexing, false).wait(cp.stopping())?;
        let engine = cp.engine();
        let ask = Ask {
            engine: engine.as_ref(),
            model: &run.model,
            device: run.device,
            hf_token: run.hf_token.as_deref(),
            cancel: cp.stopping(),
        };
        let chapters = merge_stage(
            t,
            &ask,
            run.language,
            run.embedding.as_deref().map(|model| (model, run.embedding_device)),
            run.hf_token.as_deref(),
            cp,
        )?;
        let final_chapters = t.chapters()?;
        let children = if input.options.details && !final_chapters.is_empty() {
            vec![
                Child::new(details_task(&input.path).priority(TOPIC_PRIORITY))
                    .with_children(detail_tasks(&input.path, &input.options, &final_chapters, &run.dialog)),
            ]
        } else {
            Vec::new()
        };
        let result = json!({ "chapters": chapters });
        if children.is_empty() {
            // Nothing to add: the worker completes the task itself, once.
            return Ok(result);
        }
        complete_with_children(&self.ai_dir, task, &result, &children)?;
        Ok(result)
    }

    /// The breakdown of one final topic, written into its chapter (`Digest.sections`).
    fn detail(&self, task: &Task, cp: &mut Checkpoints) -> anyhow::Result<Value> {
        let input: TopicInput = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(json!({ "topic": input.topic, "skipped": "the agent model is not local" }));
        };
        let t = &run.transcript;
        let chapters = t.chapters()?;
        let chapter = chapters.get(input.topic).cloned().ok_or_else(|| anyhow::anyhow!("topic {} is not in the outline", input.topic + 1))?;
        // A restart after the checkpoint: the sections are written, the task was released.
        if !chapter.digest.sections.is_empty() {
            let items: usize = chapter.digest.sections.iter().map(|s| s.items.len()).sum();
            return Ok(json!({ "topic": input.topic, "sections": chapter.digest.sections.len(), "items": items, "kept": true }));
        }
        let _lease = self.slots.request(WorkKind::Indexing, false).wait(cp.stopping())?;
        let engine = cp.engine();
        let ask = Ask {
            engine: engine.as_ref(),
            model: &run.model,
            device: run.device,
            hf_token: run.hf_token.as_deref(),
            cancel: cp.stopping(),
        };
        let sections = detail_topic(&ask, &run.dialog, &run.names, run.language, &chapter, input.topic + 1 == chapters.len())?;
        let items: usize = sections.iter().map(|s| s.items.len()).sum();
        let mut chapter = chapter;
        chapter.digest.sections = sections;
        t.put_chapter(input.topic, &chapter)?;
        cp.save(&json!({ "topic": input.topic }), &Value::Null, 1.0)?;
        Ok(json!({ "topic": input.topic, "sections": chapter.digest.sections.len(), "items": items }))
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

    /// A recording of three topics by the meaning of its chunks: three chunks of 40 s each mean
    /// one subject, then three another, then three the opposite of the first.
    fn recording() -> (tempfile::TempDir, PathBuf, PathBuf, String) {
        use mediaa_core::retrieval::Chunk;
        use mediaa_core::transcript::Word;
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
        for ((chunk_id, _, _), vector) in t.chunks().unwrap().iter().zip(vectors) {
            t.set_embedding(*chunk_id, &vector).unwrap();
        }
        (dir, ai, file, id)
    }

    /// Without an embedding model the merge describes the topics by their chunks alone and the
    /// fake chat answers stay in a known order.
    fn settings() -> AgentSettings {
        let mut settings = AgentSettings::default();
        settings.models.embedding = None;
        settings
    }

    fn topic_handlers(ai: &Path, settings: &AgentSettings) -> Vec<Box<dyn TaskHandler>> {
        let slots = Slots::new(settings.budget.clone());
        let s = settings.clone();
        let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(move || s.clone());
        handlers(ai, slots, current)
    }

    /// Runs the queue to the end; returns whether a task was done.
    fn drain(q: &Queue, engine: &Arc<dyn Engine>, handlers: &[Box<dyn TaskHandler>]) -> bool {
        let stop = AtomicBool::new(false);
        let mut ran = false;
        while crate::worker::run_next(q, engine, handlers, &stop).unwrap() {
            ran = true;
        }
        ran
    }

    fn digest_json(title: &str, quotes: &str) -> String {
        format!(r#"{{"title":"{title}","summary":"сводка","decisions":[],"actions":[],"questions":[],"topics":[],"quotes":{quotes}}}"#)
    }

    fn sections_json(items: &str) -> String {
        format!(r#"{{"sections":[{{"title":"Итоги","kind":"list","items":{items}}}]}}"#)
    }

    fn points_json(points: &str) -> String {
        format!(r#"{{"points":{points}}}"#)
    }

    /// The topics composite: the plan brings a digest per topic and the merge, the merge brings
    /// the details; everything ends done.
    #[test]
    fn topics_run_as_plan_digests_merge_and_details() {
        use mediaa_core::queue::TaskStatus;
        use mediaa_engine::testing::FakeEngine;

        let (_dir, ai, file, id) = recording();
        let settings = settings();
        let fake = FakeEngine::new();
        let section = sections_json(r#"[{"time":"0:20","text":"реплика1 обсуждение"},{"time":"0:20","text":"этого не говорили"}]"#);
        fake.chat_answers(&[
            &digest_json("Тема раз", r#"[{"time":"0:20","text":"реплика1 обсуждение"},{"time":"0:20","text":"этого не говорили"}]"#),
            &digest_json("Тема два", r#"[{"time":"2:00","text":"реплика6 обсуждение"}]"#),
            &digest_json("Тема три", "[]"),
            &section,
            &section,
            &section,
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let queue = Queue::open(&ai).unwrap();
        let options = AnalysisOptions::default();
        let (topics, planned) = queue
            .enqueue_tree(topics_task(&file, &options).scope(&id), &[Child::new(plan_task(&file, &options).scope(&id))])
            .unwrap();
        let plan = planned[0];
        assert!(drain(&queue, &engine, &topic_handlers(&ai, &settings)));

        let tasks = queue.list().unwrap();
        let failed: Vec<_> = tasks.iter().filter(|t| t.status != TaskStatus::Done).map(|t| (&t.kind, t.status, &t.error)).collect();
        assert!(failed.is_empty(), "{failed:?}");
        let count = |kind: &str| tasks.iter().filter(|t| t.kind == kind).count();
        assert_eq!(
            (count(PLAN_KIND), count(DIGEST_KIND), count(MERGE_KIND), count(DETAILS_KIND), count(DETAIL_KIND)),
            (1, 3, 1, 1, 3),
            "the whole graph"
        );
        assert_eq!(queue.get(topics).unwrap().unwrap().status, TaskStatus::Done);
        assert_eq!(queue.get(plan).unwrap().unwrap().status, TaskStatus::Done);
        // The plan's children join the topics composite; the details hang under their composite.
        let kinds: Vec<String> = queue.children(topics).unwrap().into_iter().map(|t| t.kind).collect();
        assert_eq!(kinds, [PLAN_KIND, DIGEST_KIND, DIGEST_KIND, DIGEST_KIND, MERGE_KIND, DETAILS_KIND]);
        let details = tasks.iter().find(|t| t.kind == DETAILS_KIND).unwrap();
        assert_eq!(queue.children(details.id).unwrap().len(), 3);

        let t = Transcript::open(&ai, &id).unwrap();
        let chapters = t.chapters().unwrap();
        assert_eq!(chapters.iter().map(|c| c.digest.title.as_str()).collect::<Vec<_>>(), ["Тема раз", "Тема два", "Тема три"]);
        assert_eq!(chapters[0].digest.quotes, [Quote { time: 20.0, text: "реплика1 обсуждение".into() }], "the quote is checked, the invented one dropped");
        assert_eq!(chapters[1].digest.quotes, [Quote { time: 120.0, text: "реплика6 обсуждение".into() }]);
        assert!(t.meta(MERGED_KEY).unwrap().is_some() && t.meta(FINAL_KEY).unwrap().is_none());
        // The details: an item's time is checked against the topic's own replies.
        assert_eq!(chapters[0].digest.sections.len(), 1);
        assert_eq!(chapters[0].digest.sections[0].items.iter().map(|i| i.time).collect::<Vec<_>>(), [Some(20.0), None], "the found time, the invented one");
        assert_eq!(chapters[1].digest.sections[0].items[0].time, None, "0:20 does not belong to this topic");
        assert_eq!(chapters[2].digest.sections[0].items[0].text, "реплика1 обсуждение", "the item stays without a time");
        assert_eq!(fake.chats().len(), 6, "three digests and three details — the merge asks nothing");
        assert!(fake.chats()[0].json_schema.as_deref().unwrap_or_default().contains("\"quotes\""));
        assert!(fake.chats()[3].json_schema.as_deref().unwrap_or_default().contains("\"sections\""));
    }

    /// A program stop after one digest: the finished digest is not asked again, the rest of the
    /// chain goes on.
    #[test]
    fn a_restart_in_the_middle_does_not_redo_a_digest() {
        use mediaa_core::queue::TaskStatus;
        use mediaa_engine::testing::FakeEngine;

        let (_dir, ai, file, id) = recording();
        let settings = settings();
        let fake = FakeEngine::new();
        let section = sections_json(r#"[{"text":"итог темы"}]"#);
        fake.chat_answers(&[
            &digest_json("Тема раз", "[]"),
            &digest_json("Тема два", "[]"),
            &digest_json("Тема три", "[]"),
            &section,
            &section,
            &section,
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let queue = Queue::open(&ai).unwrap();
        let options = AnalysisOptions::default();
        queue
            .enqueue_tree(topics_task(&file, &options).scope(&id), &[Child::new(plan_task(&file, &options).scope(&id))])
            .unwrap();
        // Only the plan first: it brings the digests and the merge.
        let plan_only: Vec<Box<dyn TaskHandler>> = topic_handlers(&ai, &settings).into_iter().filter(|h| h.kind() == PLAN_KIND).collect();
        drain(&queue, &engine, &plan_only);
        // The program closes after the first digest: the worker stops at its checkpoint and the
        // digest goes back to the queue with its chapter already written.
        let stop = AtomicBool::new(true);
        assert!(crate::worker::run_next(&queue, &engine, &topic_handlers(&ai, &settings), &stop).unwrap());
        let t = Transcript::open(&ai, &id).unwrap();
        assert_eq!(t.chapters().unwrap()[0].digest.title, "Тема раз");
        drop(t);
        assert_eq!(fake.chats().len(), 1);

        // Starting again: the first digest is in the table, so it is not asked again.
        let queue = Queue::open(&ai).unwrap();
        queue.requeue_running().unwrap();
        drain(&queue, &engine, &topic_handlers(&ai, &settings));
        let tasks = queue.list().unwrap();
        assert!(tasks.iter().all(|t| t.status == TaskStatus::Done), "{tasks:?}");
        let t = Transcript::open(&ai, &id).unwrap();
        let titles: Vec<String> = t.chapters().unwrap().iter().map(|c| c.digest.title.clone()).collect();
        assert_eq!(titles, ["Тема раз", "Тема два", "Тема три"]);
        assert!(t.chapters().unwrap().iter().all(|c| !c.digest.sections.is_empty()));
        assert_eq!(fake.chats().len(), 6, "three digests and three details — the stopped digest was not asked twice");
        assert!(fake.chats()[1].texts[1].contains("реплика6 обсуждение"), "the next call is the second topic: {}", fake.chats()[1].texts[1]);
    }

    /// A stop after one detail: its sections are in the chapter, so the detail is not asked again.
    #[test]
    fn a_restart_in_the_middle_does_not_redo_a_detail() {
        use mediaa_core::queue::TaskStatus;
        use mediaa_engine::testing::FakeEngine;

        let (_dir, ai, file, id) = recording();
        let settings = settings();
        let fake = FakeEngine::new();
        let section = sections_json(r#"[{"text":"итог темы"}]"#);
        fake.chat_answers(&[
            &digest_json("Тема раз", "[]"),
            &digest_json("Тема два", "[]"),
            &digest_json("Тема три", "[]"),
            &section,
            &section,
            &section,
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let queue = Queue::open(&ai).unwrap();
        let options = AnalysisOptions::default();
        queue
            .enqueue_tree(topics_task(&file, &options).scope(&id), &[Child::new(plan_task(&file, &options).scope(&id))])
            .unwrap();
        // The plan, the digests and the merge; the merge leaves the details in the queue.
        let before: Vec<Box<dyn TaskHandler>> = topic_handlers(&ai, &settings)
            .into_iter()
            .filter(|h| [PLAN_KIND, DIGEST_KIND, MERGE_KIND].contains(&h.kind()))
            .collect();
        drain(&queue, &engine, &before);
        assert_eq!(fake.chats().len(), 3);
        // The program closes after the first detail: its chapter is written, the task released.
        let details_only: Vec<Box<dyn TaskHandler>> =
            topic_handlers(&ai, &settings).into_iter().filter(|h| h.kind() == DETAIL_KIND).collect();
        let stop = AtomicBool::new(true);
        assert!(crate::worker::run_next(&queue, &engine, &details_only, &stop).unwrap());
        let t = Transcript::open(&ai, &id).unwrap();
        assert!(!t.chapters().unwrap()[0].digest.sections.is_empty());
        drop(t);
        assert_eq!(fake.chats().len(), 4);

        let queue = Queue::open(&ai).unwrap();
        queue.requeue_running().unwrap();
        drain(&queue, &engine, &topic_handlers(&ai, &settings));
        let tasks = queue.list().unwrap();
        assert!(tasks.iter().all(|t| t.status == TaskStatus::Done), "{tasks:?}");
        assert_eq!(fake.chats().len(), 6, "the stopped detail was not asked twice");
    }

    /// The details are off: the merge finishes the topics and starts nothing.
    #[test]
    fn without_the_details_option_the_merge_starts_nothing() {
        use mediaa_core::queue::TaskStatus;
        use mediaa_engine::testing::FakeEngine;

        let (_dir, ai, file, id) = recording();
        let settings = settings();
        let fake = FakeEngine::new();
        fake.chat_answers(&[&digest_json("Тема раз", "[]"), &digest_json("Тема два", "[]"), &digest_json("Тема три", "[]")]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let queue = Queue::open(&ai).unwrap();
        let options = AnalysisOptions { details: false, ..AnalysisOptions::default() };
        let topics = queue
            .enqueue_tree(topics_task(&file, &options).scope(&id), &[Child::new(plan_task(&file, &options).scope(&id))])
            .unwrap()
            .0;
        drain(&queue, &engine, &topic_handlers(&ai, &settings));

        let tasks = queue.list().unwrap();
        assert!(tasks.iter().all(|t| t.status == TaskStatus::Done), "{tasks:?}");
        assert!(tasks.iter().all(|t| t.kind != DETAILS_KIND && t.kind != DETAIL_KIND), "no details tasks");
        assert_eq!(queue.get(topics).unwrap().unwrap().status, TaskStatus::Done);
        assert_eq!(fake.chats().len(), 3);
        let t = Transcript::open(&ai, &id).unwrap();
        assert!(t.chapters().unwrap().iter().all(|c| c.digest.sections.is_empty()));
        let topics: Vec<Chapter> = serde_json::from_str(&t.meta(TOPICS_KEY).unwrap().unwrap()).unwrap();
        assert_eq!(topics.len(), 3, "the digest-stage chapters are kept for a merge re-run");
    }

    /// A reply of 100 words: fifteen of them are more than one window.
    fn long_reply(i: usize) -> Reply {
        use mediaa_core::transcript::Word;
        let start = i as f64 * 10.0;
        let words: Vec<Word> = std::iter::once(format!("реплика{i}"))
            .chain((0..99).map(|k| format!("слово{k}")))
            .enumerate()
            .map(|(k, text)| Word { text, start: start + k as f64 * 0.05, end: start + k as f64 * 0.05 + 0.05 })
            .collect();
        Reply { start, end: words.last().unwrap().end, speaker: None, words }
    }

    fn saying(start: f64, text: &str) -> Reply {
        use mediaa_core::transcript::Word;
        let words: Vec<Word> = text
            .split_whitespace()
            .enumerate()
            .map(|(k, w)| Word { text: w.into(), start: start + k as f64, end: start + k as f64 + 0.5 })
            .collect();
        Reply { start, end: words.last().unwrap().end, speaker: None, words }
    }

    #[test]
    fn a_long_topic_is_detailed_from_the_points_of_its_windows() {
        use mediaa_engine::testing::FakeEngine;

        let fake = FakeEngine::new();
        let points = points_json(r#"[{"time":"0:10","text":"первая мысль"}]"#);
        fake.chat_answers(&[
            &points,
            &points,
            &sections_json(r#"[{"time":"2:00","text":"реплика12 слово0"},{"text":"вывод"}]"#),
        ]);
        let replies: Vec<Reply> = (0..15).map(long_reply).collect();
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let names = BTreeMap::new();
        let chapter = Chapter {
            start: 0.0,
            end: replies.last().unwrap().end,
            digest: Digest { title: "Отчёт".into(), summary: "сводка".into(), ..Digest::default() },
        };
        let sections = detail_topic(&ask, &replies, &names, "Russian", &chapter, true).unwrap();
        let calls = fake.chats();
        assert_eq!(calls.len(), 3, "the points of two windows and one call that assembles the sections");
        assert!(calls[0].texts[1].contains("Part 1 of 2"), "{}", calls[0].texts[1]);
        assert!(calls[1].texts[1].contains("Part 2 of 2"), "{}", calls[1].texts[1]);
        assert!(calls[2].texts[1].contains("The points said in the topic, in order"), "{}", calls[2].texts[1]);
        assert!(!calls[2].texts[1].contains("Transcript of the topic"), "the transcript is not read again");
        assert!(calls[0].json_schema.as_deref().unwrap_or_default().contains("\"points\""));
        assert!(calls[2].json_schema.as_deref().unwrap_or_default().contains("\"sections\""));
        assert_eq!((calls[0].max_tokens, calls[2].max_tokens), (points_tokens(), sections_tokens()));
        assert_eq!(sections[0].items.iter().map(|i| i.time).collect::<Vec<_>>(), [Some(120.0), None]);
        assert_eq!(sections[0].items[1].text, "вывод");
    }

    #[test]
    fn the_times_of_detail_items_are_checked_against_the_lines() {
        use mediaa_engine::testing::FakeEngine;

        let fake = FakeEngine::new();
        fake.chat_answers(&[&sections_json(
            r#"[{"time":"0:05","text":"обсудили сроки отчёта"},{"time":"0:14","text":"Олег проверит цифры"},{"time":"5:00","text":"Олег проверит цифры"}]"#,
        )]);
        let replies = [saying(10.0, "Мы обсудили сроки отчёта"), saying(20.0, "Олег проверит цифры")];
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let chapter = Chapter {
            start: 0.0,
            end: 25.0,
            digest: Digest { title: "Отчёт".into(), summary: "сводка".into(), ..Digest::default() },
        };
        let sections = detail_topic(&ask, &replies, &BTreeMap::new(), "Russian", &chapter, true).unwrap();
        assert_eq!(fake.chats().len(), 1, "a short topic: one call");
        assert!(fake.chats()[0].texts[1].contains("Transcript of the topic"));
        assert_eq!(sections[0].title, "Итоги");
        assert_eq!(sections[0].kind, SectionKind::List);
        assert_eq!(sections[0].items.iter().map(|i| i.time).collect::<Vec<_>>(), [Some(10.0), Some(20.0), None]);
        assert_eq!(sections[0].items[2].text, "Олег проверит цифры", "the item stays without its time");
    }

    /// One reply of minutes becomes many lines with their own times, so an item about the middle
    /// of it gets the middle time, not the start of the reply (a live run collapsed them all to
    /// the topic start).
    #[test]
    fn the_items_of_a_monologue_get_the_times_of_their_lines() {
        use mediaa_core::transcript::Word;
        use mediaa_engine::testing::FakeEngine;

        // 100 words, half a second each: two lines, at 0:00 and 0:26 (25.5 s).
        let words: Vec<Word> = (0..100).map(|i| Word { text: format!("слово{i}"), start: i as f64 * 0.5, end: i as f64 * 0.5 + 0.4 }).collect();
        let replies = [Reply { start: 0.0, end: words.last().unwrap().end, speaker: None, words }];
        let fake = FakeEngine::new();
        fake.chat_answers(&[&sections_json(
            r#"[{"time":"0:30","text":"слово55 слово56"},{"time":"0:30","text":"погода на даче"}]"#,
        )]);
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let chapter = Chapter {
            start: 0.0,
            end: 50.0,
            digest: Digest { title: "Длинный монолог".into(), summary: "сводка".into(), ..Digest::default() },
        };
        let sections = detail_topic(&ask, &replies, &BTreeMap::new(), "Russian", &chapter, true).unwrap();
        let prompt = &fake.chats()[0].texts[1];
        assert!(prompt.contains("[00:00] слово0 "), "{prompt}");
        assert!(prompt.contains("[00:26] слово51"), "the second line has its own time: {prompt}");
        assert_eq!(sections[0].items.iter().map(|i| i.time).collect::<Vec<_>>(), [Some(25.5), None]);
    }

    #[test]
    fn sections_with_the_same_title_are_merged() {
        use mediaa_engine::testing::FakeEngine;

        let fake = FakeEngine::new();
        // A small model repeats a section with the same item; the titles differ in case and
        // spaces only.
        let answer = r#"{"sections":[
            {"title":"Предлагаемые действия","kind":"steps","items":[{"text":"лайкнуть видео"}]},
            {"title":" Другие ","kind":"list","items":[{"text":"первое"}]},
            {"title":"предлагаемые ДЕЙСТВИЯ ","kind":"steps","items":[{"text":"лайкнуть видео"},{"text":"подписаться"}]}
        ]}"#;
        fake.chat_answers(&[answer]);
        let replies = [saying(0.0, "лайкнуть видео подписаться первое")];
        let cancel = AtomicBool::new(false);
        let ask = Ask { engine: &fake, model: "unsloth/Qwen3.5-4B-GGUF", device: Device::Auto, hf_token: None, cancel: &cancel };
        let chapter = Chapter { start: 0.0, end: 3.0, digest: Digest { title: "Ролик".into(), summary: "сводка".into(), ..Digest::default() } };
        let sections = detail_topic(&ask, &replies, &BTreeMap::new(), "Russian", &chapter, true).unwrap();
        assert_eq!(sections.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["Предлагаемые действия", "Другие"]);
        let texts: Vec<&str> = sections[0].items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(texts, ["лайкнуть видео", "подписаться"], "the order is kept, the repeat dropped");
        assert_eq!(sections[1].items[0].text, "первое");
    }

    /// A fully filled answer of each detail schema fits in its token limit: the limit is computed
    /// from the caps, so the grammar cannot overrun it and cut the JSON.
    #[test]
    fn the_answer_limits_cover_a_maximal_answer() {
        let title = "т".repeat(outline::SECTION_TITLE_CHARS);
        let text = "я".repeat(outline::SECTION_TEXT_CHARS);
        let sections: Vec<Value> = (0..outline::SECTIONS_MAX)
            .map(|_| {
                let items: Vec<Value> = (0..outline::SECTION_ITEMS_MAX).map(|_| json!({ "time": "0:00", "text": text })).collect();
                json!({ "title": title, "kind": "list", "items": items })
            })
            .collect();
        let answer = serde_json::to_string(&json!({ "sections": sections })).unwrap();
        let chars = answer.chars().count();
        assert!(chars.div_ceil(CHARS_PER_TOKEN as usize) <= sections_tokens() as usize, "{chars} chars do not fit in {}", sections_tokens());
        let points: Vec<Value> = (0..outline::POINTS_MAX).map(|_| json!({ "time": "0:00", "text": "я".repeat(outline::POINT_TEXT_CHARS) })).collect();
        let answer = serde_json::to_string(&json!({ "points": points })).unwrap();
        let chars = answer.chars().count();
        assert!(chars.div_ceil(CHARS_PER_TOKEN as usize) <= points_tokens() as usize, "{chars} chars do not fit in {}", points_tokens());
    }
}
