use crate::assistant::{Message, Role};
use crate::prompts;
use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use mediaa_core::agent::{AgentSettings, AnalysisOptions, ModelRole};
use mediaa_core::library::human_duration;
use mediaa_core::outline::{
    self, Chapter, ChunkVector, Digest, FineLine, Item, Part, PlanTopic, Quote, Section,
    SectionKind, Window, DIGEST_SCHEMA, WINDOW_WORDS,
};
use mediaa_core::transcript::{self, Reply, Transcript};
use mediaa_engine::{
    ChatMessage, ChatRequest, Device, EmbedKind, EmbeddingsRequest, Engine, Priority, Usage,
};
use mediaa_queue::{Child, NewTask, Resource, Store, Task};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

pub const TOPICS_KIND: &str = "topics";
pub const PLAN_KIND: &str = "topics-plan";
pub const DIGEST_KIND: &str = "topic-digest";
pub const MERGE_KIND: &str = "topics-merge";
pub const DETAILS_KIND: &str = "details";
pub const DETAIL_KIND: &str = "topic-detail";

const TOPIC_PRIORITY: i64 = 10;
pub const DETAIL_PRIORITY: i64 = 1000;

const OUTLINE_CTX: u32 = 12288;
const PAIR_TOKENS: u32 = 32;
const CHARS_PER_TOKEN: u32 = 3;
const ITEM_OVERHEAD_TOKENS: u32 = 16;
const ANSWER_OVERHEAD_TOKENS: u32 = 64;
const ANSWER_MARGIN_DIV: u32 = 4;
const TEMPLATE_OVERHEAD_TOKENS: u32 = 32;
const CTX_RESERVE: u32 = 8;

const DIGEST_TITLE_CHARS: usize = 80;
const DIGEST_SUMMARY_CHARS: usize = 400;
const DIGEST_POINT_CHARS: usize = 160;
const DIGEST_POINTS: usize = 5;
const DIGEST_KEYWORD_CHARS: usize = 40;
const DIGEST_QUOTE_CHARS: usize = 200;
const DIGEST_QUOTES: usize = 3;

fn answer_tokens(chars: usize, items: usize) -> u32 {
    let base = (chars as u32).div_ceil(CHARS_PER_TOKEN)
        + items as u32 * ITEM_OVERHEAD_TOKENS
        + ANSWER_OVERHEAD_TOKENS;
    base + base / ANSWER_MARGIN_DIV
}

fn digest_tokens() -> u32 {
    let chars = DIGEST_TITLE_CHARS
        + DIGEST_SUMMARY_CHARS
        + 3 * DIGEST_POINTS * DIGEST_POINT_CHARS
        + DIGEST_POINTS * DIGEST_KEYWORD_CHARS
        + DIGEST_QUOTES * DIGEST_QUOTE_CHARS;
    let items = 2 + 3 * DIGEST_POINTS + DIGEST_POINTS + DIGEST_QUOTES;
    answer_tokens(chars, items)
}

fn sections_tokens() -> u32 {
    let chars = outline::SECTIONS_MAX
        * (outline::SECTION_TITLE_CHARS + outline::SECTION_ITEMS_MAX * outline::SECTION_TEXT_CHARS);
    answer_tokens(
        chars,
        outline::SECTIONS_MAX * (outline::SECTION_ITEMS_MAX + 1),
    )
}

fn topic_section_tokens(source_chars: usize, source_items: usize) -> u32 {
    let chars = source_chars.min(OUTLINE_CTX as usize * CHARS_PER_TOKEN as usize * 8);
    let items = source_items.clamp(1, 10_000);
    answer_tokens(chars, items).max(sections_tokens())
}

fn text_tokens(text: &str) -> u32 {
    (text.chars().count() as u32).div_ceil(CHARS_PER_TOKEN)
}

struct SectionBudget {
    max_tokens: u32,
    wanted: u32,
    prompt_tokens: u32,
    fits: bool,
}

fn section_budget(
    system: &str,
    user: &str,
    source_chars: usize,
    source_items: usize,
) -> SectionBudget {
    let prompt_tokens = text_tokens(system)
        .saturating_add(text_tokens(user))
        .saturating_add(TEMPLATE_OVERHEAD_TOKENS);
    let wanted = topic_section_tokens(source_chars, source_items);
    let room = OUTLINE_CTX
        .saturating_sub(prompt_tokens)
        .saturating_sub(CTX_RESERVE);
    let fits = room > 0 && wanted <= room;
    let max_tokens = wanted.min(room).max(1);
    SectionBudget {
        max_tokens,
        wanted,
        prompt_tokens,
        fits,
    }
}

fn points_tokens() -> u32 {
    answer_tokens(
        outline::POINTS_MAX * outline::POINT_TEXT_CHARS,
        outline::POINTS_MAX,
    )
}
const PARTS_MIN_SECONDS: f64 = 15.0 * 60.0;

const PLAN_KEY: &str = "outline-plan";
pub(crate) const TOPICS_KEY: &str = "outline-topics";
pub(crate) const MERGE_KEY: &str = "outline-merge";
pub(crate) const MERGED_KEY: &str = "outline-merged";
pub(crate) const FINAL_KEY: &str = "outline-final";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
    #[serde(default)]
    pub options: AnalysisOptions,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TopicInput {
    pub path: PathBuf,
    #[serde(default)]
    pub options: AnalysisOptions,
    pub topic: usize,
}

pub fn prompt(
    window: &Window,
    i: usize,
    n: usize,
    previous: Option<&str>,
    language: &str,
) -> String {
    let (start, end) = (human_duration(window.start), human_duration(window.end));
    prompts::render(
        "outline/part.j2",
        minijinja::context! { number => i + 1, parts => n, start, end, language, previous, text => &window.text },
    )
}

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
                .filter_map(|q| {
                    outline::clock_time(&q.time).map(|time| Quote {
                        time,
                        text: q.text.trim().to_string(),
                    })
                })
                .filter(|q| !q.text.is_empty())
                .collect(),
            parts: Vec::new(),
            sections: Vec::new(),
        }
    }
}

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
    fn into_sections(self, lines: &[FineLine]) -> Vec<Section> {
        let sections =
            self.sections
                .into_iter()
                .filter_map(|section| {
                    let items: Vec<Item> = section
                        .items
                        .into_iter()
                        .filter(|item| !item.text.trim().is_empty())
                        .map(|item| {
                            let text = item.text.trim().to_string();
                            let time =
                                item.time.as_deref().and_then(outline::clock_time).and_then(
                                    |time| outline::verified_detail_time(time, &text, lines),
                                );
                            Item { text, time }
                        })
                        .collect();
                    (!items.is_empty()).then(|| Section {
                        title: section.title.trim().to_string(),
                        kind: section.kind,
                        items,
                    })
                })
                .collect();
        merge_sections(sections)
    }
}

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

struct ModelText {
    text: String,
    usage: Option<Usage>,
}

struct SectionAnswer {
    raw: RawSections,
    note: Option<String>,
}

struct Breakdown {
    sections: Vec<Section>,
    note: Option<String>,
}

fn sections_array_at(text: &str) -> Option<usize> {
    let key = text.find("\"sections\"")?;
    let after = key + "\"sections\"".len();
    let rel = text[after..].find('[')?;
    Some(after + rel + 1)
}

fn closed_object_end(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    let mut depth = 0;
    let mut in_string = false;
    let mut escape = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if escape {
                escape = false;
            } else if c == b'\\' {
                escape = true;
            } else if c == b'"' {
                in_string = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'{' => depth += 1,
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(i + 1);
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

fn parse_sections(text: &str) -> Option<RawSections> {
    serde_json::from_str(text.trim()).ok()
}

fn closed_sections(text: &str) -> RawSections {
    if let Some(raw) = parse_sections(text) {
        return raw;
    }
    let mut sections = Vec::new();
    let Some(mut i) = sections_array_at(text) else {
        return RawSections { sections };
    };
    let bytes = text.as_bytes();
    while i < bytes.len() {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= bytes.len() || bytes[i] == b']' {
            break;
        }
        if bytes[i] == b',' {
            i += 1;
            continue;
        }
        if bytes[i] != b'{' {
            break;
        }
        let Some(end) = closed_object_end(bytes, i) else {
            break;
        };
        let Ok(section) = serde_json::from_str::<RawSection>(&text[i..end]) else {
            break;
        };
        sections.push(section);
        i = end;
    }
    RawSections { sections }
}

fn nonempty_sections(raw: &RawSections) -> usize {
    raw.sections
        .iter()
        .filter(|section| {
            section
                .items
                .iter()
                .any(|item| !item.text.trim().is_empty())
        })
        .count()
}

fn end_line(text: &str) -> usize {
    text.lines().count().max(1)
}

fn hit_token_limit(usage: Option<Usage>, max_tokens: u32) -> bool {
    usage.is_some_and(|u| u.completion_tokens >= max_tokens)
}

fn hit_context(usage: Option<Usage>) -> bool {
    usage.is_some_and(|u| u.prompt_tokens.saturating_add(u.completion_tokens) + 1 >= OUTLINE_CTX)
}

fn section_note(
    cut_text: Option<&str>,
    kept: Option<usize>,
    budget: &SectionBudget,
    first: Option<Usage>,
    second: Option<Usage>,
) -> Option<String> {
    let mut bits = Vec::new();
    if let Some(text) = cut_text {
        let line = end_line(text);
        match kept {
            Some(k) => bits.push(format!("cut at line {line} after {k} complete sections")),
            None => bits.push(format!("the first answer was cut at line {line}")),
        }
        if hit_token_limit(first, budget.max_tokens) || hit_token_limit(second, budget.max_tokens) {
            bits.push(format!("hit the limit of {} tokens", budget.max_tokens));
        } else if hit_context(first) || hit_context(second) {
            bits.push(format!("hit the context window of {OUTLINE_CTX} tokens"));
        }
    }
    if !budget.fits {
        bits.push(format!(
            "the prompt (~{} tokens) and the answer ({} tokens) do not fit the context of {OUTLINE_CTX} tokens",
            budget.prompt_tokens, budget.wanted
        ));
    }
    if bits.is_empty() {
        None
    } else {
        Some(bits.join(": "))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

fn chat_request(
    model: &str,
    system: String,
    user: String,
    schema: Value,
    max_tokens: u32,
    device: Device,
    hf_token: Option<&str>,
) -> ChatRequest {
    let messages = [
        Message::new(Role::System, system),
        Message::new(Role::User, user),
    ];
    chat_request_messages(model, &messages, schema, max_tokens, device, hf_token)
}

fn chat_request_messages(
    model: &str,
    messages: &[Message],
    schema: Value,
    max_tokens: u32,
    device: Device,
    hf_token: Option<&str>,
) -> ChatRequest {
    ChatRequest {
        model: model.to_string(),
        messages: messages
            .iter()
            .map(|m| ChatMessage::new(m.role.into(), m.content.clone()))
            .collect(),
        max_tokens,
        temperature: 0.0,
        json_schema: Some(schema),
        no_repeats: true,
        n_ctx: Some(OUTLINE_CTX),
        device: Some(device),
        priority: Some(Priority::Background),
        hf_token: hf_token.map(str::to_string),
        ..ChatRequest::default()
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "explicit parameters of one pipeline call"
)]
fn ask(
    engine: &dyn Engine,
    model: &str,
    device: Device,
    hf_token: Option<&str>,
    system: String,
    user: String,
    schema: Value,
    max_tokens: u32,
    cancel: &AtomicBool,
) -> anyhow::Result<Digest> {
    let request = chat_request(model, system, user, schema, max_tokens, device, hf_token);
    let text = engine.chat(&request, cancel, &mut |_| true)?.content;
    let raw: RawDigest = serde_json::from_str(text.trim())
        .map_err(|e| anyhow::anyhow!("the model's digest is not valid: {e}"))?;
    Ok(raw.into_digest())
}

#[allow(
    clippy::too_many_arguments,
    reason = "explicit parameters of one pipeline call"
)]
pub fn digest(
    engine: &dyn Engine,
    model: &str,
    device: Device,
    hf_token: Option<&str>,
    window: &Window,
    i: usize,
    n: usize,
    previous: Option<&str>,
    language: &str,
    cancel: &AtomicBool,
) -> anyhow::Result<Digest> {
    ask(
        engine,
        model,
        device,
        hf_token,
        prompts::render("outline/system.j2", ()),
        prompt(window, i, n, previous, language),
        serde_json::from_str(DIGEST_SCHEMA)?,
        digest_tokens(),
        cancel,
    )
}

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

struct Ask<'a> {
    engine: &'a dyn Engine,
    model: &'a str,
    device: Device,
    hf_token: Option<&'a str>,
    cancel: &'a AtomicBool,
}

impl Ask<'_> {
    fn digest(
        &self,
        window: &Window,
        i: usize,
        n: usize,
        previous: Option<&str>,
        language: &str,
    ) -> anyhow::Result<Digest> {
        digest(
            self.engine,
            self.model,
            self.device,
            self.hf_token,
            window,
            i,
            n,
            previous,
            language,
            self.cancel,
        )
    }

    fn reduce(
        &self,
        language: &str,
        previous: Option<&str>,
        parts: &[Digest],
    ) -> anyhow::Result<Digest> {
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
        let user = prompts::render(
            "outline/reduce.j2",
            minijinja::context! { language, previous, parts => context },
        );
        let mut digest = ask(
            self.engine,
            self.model,
            self.device,
            self.hf_token,
            prompts::render("outline/reduce_system.j2", ()),
            user,
            serde_json::from_str(DIGEST_SCHEMA)?,
            digest_tokens(),
            self.cancel,
        )?;
        digest.quotes = quotes_of(parts);
        Ok(digest)
    }

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
        let user = prompts::render(
            "outline/pair.j2",
            minijinja::context! { first => topic(first), second => topic(second) },
        );
        let request = chat_request(
            self.model,
            prompts::render("outline/pair_system.j2", ()),
            user,
            pair_schema(),
            PAIR_TOKENS,
            self.device,
            self.hf_token,
        );
        let text = self
            .engine
            .chat(&request, self.cancel, &mut |_| true)?
            .content;
        #[derive(Deserialize)]
        struct Answer {
            same: bool,
        }
        let answer: Answer = serde_json::from_str(text.trim())
            .map_err(|e| anyhow::anyhow!("the model's answer about a pair is not valid: {e}"))?;
        Ok(answer.same)
    }

    fn complete(
        &self,
        messages: &[Message],
        schema: Value,
        max_tokens: u32,
    ) -> anyhow::Result<ModelText> {
        let request = chat_request_messages(
            self.model,
            messages,
            schema,
            max_tokens,
            self.device,
            self.hf_token,
        );
        let answer = self.engine.chat(&request, self.cancel, &mut |_| true)?;
        Ok(ModelText {
            text: answer.content,
            usage: answer.usage,
        })
    }

    fn sections(
        &self,
        user: String,
        source_chars: usize,
        source_items: usize,
    ) -> anyhow::Result<SectionAnswer> {
        let system = prompts::render("outline/detail_system.j2", ());
        let budget = section_budget(&system, &user, source_chars, source_items);
        let schema = outline::sections_schema();
        let mut messages = vec![
            Message::new(Role::System, system),
            Message::new(Role::User, user),
        ];
        let first = self.complete(&messages, schema.clone(), budget.max_tokens)?;
        if let Some(raw) = parse_sections(&first.text) {
            return Ok(SectionAnswer {
                raw,
                note: section_note(None, None, &budget, first.usage, None),
            });
        }
        let retry = prompts::render(
            "outline/detail_shorter.j2",
            minijinja::context! { tokens => budget.max_tokens },
        );
        messages.push(Message::new(Role::Assistant, first.text.clone()));
        messages.push(Message::new(Role::User, retry));
        let second = self.complete(&messages, schema, budget.max_tokens)?;
        if let Some(raw) = parse_sections(&second.text) {
            if nonempty_sections(&raw) > 0 {
                let note =
                    section_note(Some(&first.text), None, &budget, first.usage, second.usage);
                return Ok(SectionAnswer { raw, note });
            }
        }
        let from_second = closed_sections(&second.text);
        let (raw, cut_text) = if nonempty_sections(&from_second) > 0 {
            (from_second, second.text)
        } else {
            (closed_sections(&first.text), first.text)
        };
        let kept = nonempty_sections(&raw);
        let note = section_note(
            Some(&cut_text),
            Some(kept),
            &budget,
            first.usage,
            second.usage,
        );
        Ok(SectionAnswer { raw, note })
    }

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
        let text = self
            .engine
            .chat(&request, self.cancel, &mut |_| true)?
            .content;
        let raw: RawPoints = serde_json::from_str(text.trim())
            .map_err(|e| anyhow::anyhow!("the model's points are not valid: {e}"))?;
        Ok(raw.points)
    }
}

fn pair_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"same": {"type": "boolean"}},
        "required": ["same"]
    })
}

fn plan_topics(
    t: &Transcript,
    dialog: &[Reply],
    windows: &[Window],
) -> anyhow::Result<Vec<PlanTopic>> {
    let chunks = t.chunks_with_embeddings()?;
    let mut starts: Vec<f64> =
        if !chunks.is_empty() && chunks.iter().all(|(_, embedding)| embedding.is_some()) {
            let vectors: Vec<outline::ChunkVector> = chunks
                .into_iter()
                .map(|(c, embedding)| outline::ChunkVector {
                    start: c.start,
                    end: c.end,
                    embedding: embedding.unwrap_or_default(),
                })
                .collect();
            outline::semantic_boundaries(&vectors)
                .into_iter()
                .map(|i| vectors[i].start)
                .collect()
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

#[derive(Debug, Serialize, Deserialize)]
struct Plan {
    topics: Vec<PlanTopic>,
}

fn load_plan(t: &Transcript) -> anyhow::Result<Option<Vec<PlanTopic>>> {
    Ok(t.meta(PLAN_KEY)?
        .and_then(|s| serde_json::from_str::<Plan>(&s).ok())
        .map(|p| p.topics))
}

fn placeholder(topic: &PlanTopic) -> Chapter {
    Chapter {
        start: topic.start,
        end: topic.end,
        digest: Digest::default(),
    }
}

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

#[derive(Debug, Default, Serialize, Deserialize)]
struct DigestCursor {
    done: usize,
    parts: Vec<Digest>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct DetailCursor {
    done: usize,
    points: Vec<RawPoint>,
}

fn digest_topic(
    ask: &Ask<'_>,
    replies: &[Reply],
    topic: &PlanTopic,
    with_end: bool,
    previous: Option<&str>,
    language: &str,
) -> anyhow::Result<Digest> {
    let picked = topic_replies(replies, topic, with_end);
    let words: usize = picked.iter().map(|r| r.words.len()).sum();
    if words <= WINDOW_WORDS {
        let text = picked
            .iter()
            .map(outline::line)
            .collect::<Vec<_>>()
            .join("\n");
        let window = Window {
            start: topic.start,
            end: topic.end,
            text,
        };
        let mut digest = ask.digest(&window, 0, 1, previous, language)?;
        digest.quotes = outline::verified_quotes(&digest.quotes, &picked);
        return Ok(digest);
    }
    let windows = outline::windows(&picked, WINDOW_WORDS);
    let n = windows.len();
    let mut parts = Vec::with_capacity(n);
    let topic_before = previous.map(str::to_string);
    let mut previous = topic_before.clone();
    for (i, window) in windows.iter().enumerate() {
        let mut digest = ask.digest(window, i, n, previous.as_deref(), language)?;
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

fn part_context(parts: &[Part]) -> Vec<minijinja::Value> {
    parts
        .iter()
        .map(|part| minijinja::context! { start => human_duration(part.start), end => human_duration(part.end), title => &part.title })
        .collect()
}

fn detail_topic(
    ask: &Ask<'_>,
    replies: &[Reply],
    language: &str,
    chapter: &Chapter,
    last: bool,
) -> anyhow::Result<Breakdown> {
    let topic = PlanTopic {
        start: chapter.start,
        end: chapter.end,
    };
    let picked = topic_replies(replies, &topic, last);
    let lines = outline::fine_lines(&picked);
    let words: usize = picked.iter().map(|r| r.words.len()).sum();
    let (title, summary) = (&chapter.digest.title, &chapter.digest.summary);
    let (start, end) = (human_duration(chapter.start), human_duration(chapter.end));
    let parts = part_context(&chapter.digest.parts);
    let parsed = if words <= WINDOW_WORDS {
        let text = lines
            .iter()
            .map(|l| l.text.clone())
            .collect::<Vec<_>>()
            .join("\n");
        let source_chars = text.chars().count();
        let source_items = lines.len();
        let user = prompts::render(
            "outline/detail.j2",
            minijinja::context! { title, summary, start, end, language, parts, points => Vec::<minijinja::Value>::new(), text },
        );
        ask.sections(user, source_chars, source_items)?
    } else {
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
        let source_chars: usize = points.iter().map(|point| point.text.chars().count()).sum();
        let source_items = points.len();
        let points: Vec<minijinja::Value> = points
            .iter()
            .map(|point| minijinja::context! { time => &point.time, text => &point.text })
            .collect();
        let user = prompts::render(
            "outline/detail.j2",
            minijinja::context! { title, summary, start, end, language, parts, points, text => "" },
        );
        ask.sections(user, source_chars, source_items)?
    };
    Ok(Breakdown {
        sections: parsed.raw.into_sections(&lines),
        note: parsed.note,
    })
}

fn file_title(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

fn topic_title(title: &str, number: usize) -> String {
    match title.trim() {
        "" => format!("#{number}"),
        title => title.to_string(),
    }
}

pub fn topics_task(path: &Path, options: &AnalysisOptions) -> NewTask {
    NewTask::new(
        TOPICS_KIND,
        &file_title(path),
        &json!({ "path": path, "options": options }),
    )
    .composite()
}

pub fn plan_task(path: &Path, options: &AnalysisOptions) -> NewTask {
    analysis_leaf(
        NewTask::new(
            PLAN_KIND,
            &file_title(path),
            &json!({ "path": path, "options": options }),
        ),
        path,
    )
}

pub fn details_task(path: &Path) -> NewTask {
    NewTask::new(DETAILS_KIND, &file_title(path), &json!({ "path": path })).composite()
}

pub fn detail_tasks(
    path: &Path,
    options: &AnalysisOptions,
    chapters: &[Chapter],
    replies: &[Reply],
) -> Vec<Child> {
    chapters
        .iter()
        .enumerate()
        .map(|(i, chapter)| {
            let topic = PlanTopic {
                start: chapter.start,
                end: chapter.end,
            };
            let words: usize = topic_replies(replies, &topic, i + 1 == chapters.len())
                .iter()
                .map(|r| r.words.len())
                .sum();
            let title = topic_title(&chapter.digest.title, i + 1);
            Child::new(
                analysis_leaf(
                    NewTask::new(
                        DETAIL_KIND,
                        &title,
                        &json!({ "path": path, "options": options, "topic": i }),
                    ),
                    path,
                )
                .units(words as f64 / 1000.0)
                .priority(TOPIC_PRIORITY),
            )
        })
        .collect()
}

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

fn chunk_sums(chunks: &[ChunkVector], chapters: &[Chapter]) -> Vec<ChunkSum> {
    chapters
        .iter()
        .enumerate()
        .map(|(i, chapter)| {
            let pieces = topic_chunks(chunks, chapter, i + 1 == chapters.len());
            let mut sum = ChunkSum {
                sum: vec![0.0; pieces.first().map_or(0, |c| c.embedding.len())],
                count: pieces.len(),
            };
            for piece in &pieces {
                for (s, v) in sum.sum.iter_mut().zip(&piece.embedding) {
                    *s += f64::from(*v);
                }
            }
            sum
        })
        .collect()
}

fn quantile(sorted: &[f64], q: f64) -> f64 {
    let i = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[i.min(sorted.len() - 1)]
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct Thresholds {
    different: f64,
    similar: f64,
}

fn thresholds(boundaries: &[f64]) -> Option<Thresholds> {
    let mut values: Vec<f64> = boundaries
        .iter()
        .skip(1)
        .copied()
        .filter(|s| s.is_finite())
        .collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    Some(Thresholds {
        different: quantile(&values, 0.1),
        similar: quantile(&values, 0.5),
    })
}

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
        Some(PairScore {
            chunks,
            descriptions,
            score,
        })
    }

    fn describe(&self) -> String {
        match (self.chunks, self.descriptions) {
            (Some(chunks), Some(descriptions)) => format!(
                "chunks {chunks:.2}, descriptions {descriptions:.2} — {:.2}",
                self.score
            ),
            (Some(chunks), None) => format!("chunks {chunks:.2} — {:.2}", self.score),
            (None, Some(descriptions)) => {
                format!("descriptions {descriptions:.2} — {:.2}", self.score)
            }
            (None, None) => format!("{:.2}", self.score),
        }
    }
}

struct Embed<'a> {
    engine: &'a dyn Engine,
    model: String,
    device: Device,
    hf_token: Option<&'a str>,
    cancel: &'a AtomicBool,
    cache: HashMap<String, Vec<f64>>,
    failed: bool,
}

impl Embed<'_> {
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
            match self.engine.embeddings(&request, self.cancel) {
                Ok(vectors)
                    if vectors.len() == missing.len() && vectors.iter().all(|v| !v.is_empty()) =>
                {
                    for (text, vector) in missing.into_iter().zip(vectors) {
                        self.cache
                            .insert(text, vector.into_iter().map(f64::from).collect());
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
        texts
            .iter()
            .map(|text| self.cache.get(text).cloned())
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct PairAnswer {
    left: usize,
    right: usize,
    same: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct MergeState {
    runs: Vec<Vec<usize>>,
    #[serde(default)]
    digests: BTreeMap<usize, Digest>,
    #[serde(default)]
    answers: Vec<PairAnswer>,
}

impl MergeState {
    fn fresh(n: usize) -> MergeState {
        MergeState {
            runs: (0..n).map(|i| vec![i]).collect(),
            ..MergeState::default()
        }
    }

    fn fits(&self, n: usize) -> bool {
        !self.runs.is_empty()
            && self.runs.iter().all(|run| !run.is_empty())
            && self.runs.iter().flatten().copied().eq(0..n)
            && self
                .runs
                .iter()
                .all(|run| run.len() == 1 || self.digests.contains_key(&run[0]))
    }
}

fn pair_score(sums: &[ChunkSum], descriptions: &[Option<Vec<f64>>], i: usize) -> Option<PairScore> {
    let chunks = (sums[i].count > 0 && sums[i + 1].count > 0)
        .then(|| outline::cosine(&sums[i].mean(), &sums[i + 1].mean()));
    let descriptions = match (&descriptions[i], &descriptions[i + 1]) {
        (Some(a), Some(b)) => Some(outline::cosine(a, b)),
        _ => None,
    };
    PairScore::new(chunks, descriptions)
}

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
    fn run_chapter(&self, run: &[usize]) -> Chapter {
        let first = run[0];
        let last = *run.last().unwrap_or(&first);
        let digest = self
            .state
            .digests
            .get(&first)
            .cloned()
            .unwrap_or_else(|| self.chapters[first].digest.clone());
        Chapter {
            start: self.chapters[first].start,
            end: self.chapters[last].end,
            digest,
        }
    }

    fn description_text(&self, run: &[usize]) -> String {
        let chapter = self.run_chapter(run);
        format!(
            "{} — {}",
            chapter.digest.title.trim(),
            chapter.digest.summary.trim()
        )
    }

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
        let texts: Vec<String> = self
            .state
            .runs
            .iter()
            .map(|run| self.description_text(run))
            .collect();
        let descriptions = match self.embed.as_mut() {
            Some(embed) => embed.vectors(&texts),
            None => vec![None; n],
        };
        let rejected: HashSet<(usize, usize)> = self
            .state
            .answers
            .iter()
            .filter(|a| !a.same)
            .map(|a| (a.left, a.right))
            .collect();
        let keys: Vec<(usize, usize)> = self
            .state
            .runs
            .windows(2)
            .map(|w| (w[0][0], w[1][0]))
            .collect();
        let mut best: Option<(usize, PairScore)> = None;
        for (i, &(left, right)) in keys.iter().enumerate() {
            if rejected.contains(&(left, right)) {
                continue;
            }
            let Some(score) = pair_score(&sums, &descriptions, i) else {
                continue;
            };
            if best.as_ref().is_none_or(|(_, b)| score.score > b.score) {
                best = Some((i, score));
            }
        }
        let Some((i, score)) = best else {
            return Ok(false);
        };
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
            log::info!(
                "outline merge: topics {}–{}: {}; clearly different (at most {:.2}) — kept apart",
                left + 1,
                right + 1,
                score.describe(),
                self.thresholds.different
            );
            return Ok(false);
        }
        let same = match self
            .state
            .answers
            .iter()
            .find(|a| a.left == left && a.right == right)
        {
            Some(answer) => answer.same,
            None => {
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
            if same {
                "one topic — merged"
            } else {
                "a new topic — kept apart"
            }
        );
        if same {
            self.merge(i)?;
        }
        Ok(true)
    }

    fn merge(&mut self, i: usize) -> anyhow::Result<()> {
        let start = self.state.runs[i][0];
        let right = self.state.runs[i + 1][0];
        let run: Vec<usize> = self.state.runs[i]
            .iter()
            .chain(&self.state.runs[i + 1])
            .copied()
            .collect();
        let digests: Vec<Digest> = run
            .iter()
            .map(|&k| self.chapters[k].digest.clone())
            .collect();
        let previous = (i > 0)
            .then(|| self.run_chapter(&self.state.runs[i - 1]).digest.title)
            .filter(|title| !title.is_empty());
        let digest = self
            .ask
            .reduce(self.language, previous.as_deref(), &digests)?;
        self.state.runs[i] = run;
        self.state.runs.remove(i + 1);
        self.state.digests.insert(start, digest);
        if right != start {
            self.state.digests.remove(&right);
        }
        Ok(())
    }

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
                    Chapter {
                        start: self.chapters[first].start,
                        end: self.chapters[last].end,
                        digest,
                    }
                };
                if run.len() > 1 && chapter.end - chapter.start > PARTS_MIN_SECONDS {
                    chapter.digest.parts = run
                        .iter()
                        .map(|&i| Part {
                            start: self.chapters[i].start,
                            end: self.chapters[i].end,
                            title: self.chapters[i].digest.title.clone(),
                        })
                        .collect();
                }
                chapter
            })
            .collect()
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "explicit parameters of one pipeline call"
)]
fn merge_stage(
    t: &Transcript,
    ask: &Ask<'_>,
    language: &str,
    embedding: Option<(&str, Device)>,
    hf_token: Option<&str>,
) -> anyhow::Result<Option<usize>> {
    if t.meta(MERGED_KEY)?.is_some() {
        if let Some(json) = t.meta(FINAL_KEY)? {
            write_chapters(t, &serde_json::from_str::<Vec<Chapter>>(&json)?)?;
            t.remove_meta(FINAL_KEY)?;
        }
        return Ok(Some(t.chapters()?.len()));
    }
    let chapters = t.chapters()?;
    if t.meta(TOPICS_KEY)?.is_none() {
        t.set_meta(TOPICS_KEY, &serde_json::to_string(&chapters)?)?;
    }
    if chapters.len() < 2 {
        t.set_meta(MERGED_KEY, "1")?;
        return Ok(Some(chapters.len()));
    }
    let chunks = t.chunks_with_embeddings()?;
    if chunks.is_empty() || chunks.iter().any(|(_, embedding)| embedding.is_none()) {
        log::info!("outline merge: the chunks have no embeddings, so the topics are not merged");
        t.set_meta(MERGED_KEY, "1")?;
        return Ok(Some(chapters.len()));
    }
    let vectors: Vec<ChunkVector> = chunks
        .into_iter()
        .map(|(chunk, embedding)| ChunkVector {
            start: chunk.start,
            end: chunk.end,
            embedding: embedding.unwrap_or_default(),
        })
        .collect();
    let Some(thresholds) = thresholds(&outline::boundary_similarities(&vectors)) else {
        t.set_meta(MERGED_KEY, "1")?;
        return Ok(Some(chapters.len()));
    };
    let state = match t
        .meta(MERGE_KEY)?
        .and_then(|json| serde_json::from_str::<MergeState>(&json).ok())
    {
        Some(state) if state.fits(chapters.len()) => state,
        _ => MergeState::fresh(chapters.len()),
    };
    let embed = embedding.map(|(model, device)| Embed {
        engine: ask.engine,
        model: model.to_string(),
        device,
        hf_token,
        cancel: ask.cancel,
        cache: HashMap::new(),
        failed: false,
    });
    let mut merger = Merger {
        ask,
        language,
        chapters: &chapters,
        chunk_sums: chunk_sums(&vectors, &chapters),
        embed,
        thresholds,
        state,
    };
    let more = merger.step()?;
    t.set_meta(MERGE_KEY, &serde_json::to_string(&merger.state)?)?;
    if more {
        return Ok(None);
    }
    let final_chapters = merger.finish();
    t.set_meta(FINAL_KEY, &serde_json::to_string(&final_chapters)?)?;
    t.set_meta(MERGED_KEY, "1")?;
    write_chapters(t, &final_chapters)?;
    t.remove_meta(FINAL_KEY)?;
    Ok(Some(final_chapters.len()))
}

struct Run {
    transcript: Transcript,
    dialog: Vec<Reply>,
    language: &'static str,
    model: String,
    device: Device,
    embedding: Option<String>,
    embedding_device: Device,
    hf_token: Option<String>,
}

fn open_run(ai_dir: &Path, path: &Path, settings: &AgentSettings) -> anyhow::Result<Option<Run>> {
    let id = transcript::file_id(path)?;
    anyhow::ensure!(
        Transcript::exists(ai_dir, &id),
        "{} has no transcript",
        path.display()
    );
    let Some(model) = settings
        .models
        .agent
        .as_ref()
        .and_then(|source| crate::models::local_spec(source).ok())
    else {
        return Ok(None);
    };
    let transcript = Transcript::open(ai_dir, &id)?;
    let dialog = transcript.dialog()?;
    Ok(Some(Run {
        transcript,
        dialog,
        language: settings.language_name(),
        model,
        device: settings.devices.get(ModelRole::Agent),
        embedding: settings
            .models
            .embedding
            .as_ref()
            .and_then(|source| crate::models::local_spec(source).ok()),
        embedding_device: settings.devices.get(ModelRole::Embedding),
        hf_token: crate::assistant::hf_token(settings),
    }))
}

fn complete_with_children(
    store: &dyn Store,
    task: &Task,
    result: &Value,
    children: Vec<Child>,
) -> anyhow::Result<()> {
    store.complete_with(task.id, result, children)?;
    Ok(())
}

fn analysis_leaf(task: NewTask, path: &Path) -> NewTask {
    let id = transcript::file_id(path).unwrap_or_default();
    task.resource(Resource::Llm).scope(&id).analysis(&id)
}

pub struct TopicTask {
    kind: &'static str,
    ai_dir: PathBuf,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

impl TopicTask {
    fn new(
        kind: &'static str,
        ai_dir: &Path,
        settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    ) -> TopicTask {
        TopicTask {
            kind,
            ai_dir: ai_dir.to_path_buf(),
            settings,
        }
    }
}

pub fn stages(
    ai_dir: &Path,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
) -> Vec<Arc<dyn Stage>> {
    [PLAN_KIND, DIGEST_KIND, MERGE_KIND, DETAIL_KIND]
        .into_iter()
        .map(|kind| Arc::new(TopicTask::new(kind, ai_dir, settings.clone())) as Arc<dyn Stage>)
        .collect()
}

impl Stage for TopicTask {
    fn handles(&self, kind: &str) -> bool {
        kind == self.kind
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        match self.kind {
            PLAN_KIND => self.plan(task, cancel, host),
            DIGEST_KIND => self.digest(task, cancel, host),
            MERGE_KIND => self.merge(task, cancel, host),
            DETAIL_KIND => self.detail(task, cancel, host),
            other => anyhow::bail!("unknown topic task {other}"),
        }
    }
}

impl TopicTask {
    fn plan(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(Work::Done(
                json!({ "topics": 0, "skipped": "the agent model is not local" }),
            ));
        };
        let t = &run.transcript;
        let topics = match load_plan(t)? {
            Some(plan) => plan,
            None => {
                let windows = outline::windows(&run.dialog, WINDOW_WORDS);
                let plan = plan_topics(t, &run.dialog, &windows)?;
                t.set_meta(
                    PLAN_KEY,
                    &serde_json::to_string(&Plan {
                        topics: plan.clone(),
                    })?,
                )?;
                let chapters: Vec<Chapter> = plan.iter().map(placeholder).collect();
                write_chapters(t, &chapters)?;
                plan
            }
        };
        let known = t.chapters()?;
        let placeholders: Vec<Chapter> = topics.iter().map(placeholder).collect();
        if known.len() != topics.len()
            || known.iter().zip(&topics).any(|(c, topic)| {
                (c.start - topic.start).abs() > 0.01 || (c.end - topic.end).abs() > 0.01
            })
        {
            write_chapters(t, &placeholders)?;
        }
        let _ = cancel;
        let mut children = Vec::with_capacity(topics.len() + 1);
        for (i, topic) in topics.iter().enumerate() {
            let words: usize = topic_replies(&run.dialog, topic, i + 1 == topics.len())
                .iter()
                .map(|r| r.words.len())
                .sum();
            children.push(Child::new(
                analysis_leaf(
                    NewTask::new(
                        DIGEST_KIND,
                        &topic_title("", i + 1),
                        &json!({ "path": input.path, "options": input.options, "topic": i }),
                    ),
                    &input.path,
                )
                .units(words as f64 / 1000.0)
                .priority(TOPIC_PRIORITY)
                .model(&run.model),
            ));
        }
        let all: Vec<usize> = (0..topics.len()).collect();
        children.push(
            Child::new(
                analysis_leaf(
                    NewTask::new(
                        MERGE_KIND,
                        &file_title(&input.path),
                        &json!({ "path": input.path, "options": input.options }),
                    ),
                    &input.path,
                )
                .units(topics.len() as f64)
                .priority(TOPIC_PRIORITY)
                .model(&run.model),
            )
            .after(&all),
        );
        let minutes = run.dialog.last().map_or(0.0, |r| r.end) / 60.0;
        let result = json!({ "topics": topics.len(), "minutes": minutes });
        complete_with_children(host.store.as_ref(), task, &result, children)?;
        Ok(Work::Done(result))
    }

    fn digest(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: TopicInput = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(Work::Done(
                json!({ "topic": input.topic, "skipped": "the agent model is not local" }),
            ));
        };
        let t = &run.transcript;
        let plan = load_plan(t)?.ok_or_else(|| anyhow::anyhow!("the topic plan is missing"))?;
        let topic = *plan
            .get(input.topic)
            .ok_or_else(|| anyhow::anyhow!("topic {} is not in the plan", input.topic + 1))?;
        let already = t
            .chapters()?
            .into_iter()
            .nth(input.topic)
            .filter(|chapter| {
                !chapter.digest.title.trim().is_empty()
                    && (chapter.start - topic.start).abs() < 0.01
                    && (chapter.end - topic.end).abs() < 0.01
            });
        if let Some(chapter) = already {
            return Ok(Work::Done(
                json!({ "topic": input.topic, "title": chapter.digest.title, "kept": true }),
            ));
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let scope = Scope::attach(
            host.store.clone(),
            task.id,
            host.engine.clone(),
            task.scope.clone(),
        );
        let engine = scope.engine();
        let ask = Ask {
            engine: engine.as_ref(),
            model: &run.model,
            device: run.device,
            hf_token: run.hf_token.as_deref(),
            cancel: cancel.as_ref(),
        };
        let picked = topic_replies(&run.dialog, &topic, input.topic + 1 == plan.len());
        let words: usize = picked.iter().map(|r| r.words.len()).sum();
        let previous = t
            .chapters()?
            .iter()
            .take(input.topic)
            .rev()
            .find(|c| !c.digest.title.is_empty())
            .map(|c| c.digest.title.clone());
        if words <= WINDOW_WORDS {
            let digest = digest_topic(
                &ask,
                &run.dialog,
                &topic,
                input.topic + 1 == plan.len(),
                previous.as_deref(),
                run.language,
            )?;
            t.put_chapter(
                input.topic,
                &Chapter {
                    start: topic.start,
                    end: topic.end,
                    digest: digest.clone(),
                },
            )?;
            return Ok(Work::Done(
                json!({ "topic": input.topic, "title": digest.title }),
            ));
        }
        let windows = outline::windows(&picked, WINDOW_WORDS);
        let n = windows.len().max(1);
        let mut cursor: DigestCursor =
            serde_json::from_value(task.cursor.clone()).unwrap_or_default();
        if cursor.done < windows.len() {
            let mut part = ask.digest(
                &windows[cursor.done],
                cursor.done,
                n,
                if cursor.done == 0 {
                    previous.as_deref()
                } else {
                    cursor.parts.last().map(|p| p.title.as_str())
                },
                run.language,
            )?;
            part.quotes = outline::verified_quotes(&part.quotes, &picked);
            cursor.parts.push(part);
            cursor.done += 1;
            return Ok(Work::More {
                cursor: serde_json::to_value(&cursor)?,
                partial: Value::Null,
                progress: cursor.done as f32 / n as f32,
            });
        }
        let digest = ask.reduce(run.language, previous.as_deref(), &cursor.parts)?;
        t.put_chapter(
            input.topic,
            &Chapter {
                start: topic.start,
                end: topic.end,
                digest: digest.clone(),
            },
        )?;
        Ok(Work::Done(
            json!({ "topic": input.topic, "title": digest.title }),
        ))
    }

    fn merge(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(Work::Done(
                json!({ "chapters": 0, "skipped": "the agent model is not local" }),
            ));
        };
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let t = &run.transcript;
        let scope = Scope::attach(
            host.store.clone(),
            task.id,
            host.engine.clone(),
            task.scope.clone(),
        );
        let engine = scope.engine();
        let ask = Ask {
            engine: engine.as_ref(),
            model: &run.model,
            device: run.device,
            hf_token: run.hf_token.as_deref(),
            cancel: cancel.as_ref(),
        };
        let chapters = merge_stage(
            t,
            &ask,
            run.language,
            run.embedding
                .as_deref()
                .map(|model| (model, run.embedding_device)),
            run.hf_token.as_deref(),
        )?;
        let Some(chapters) = chapters else {
            let n = task.cursor.get("n").and_then(|v| v.as_u64()).unwrap_or(0) + 1;
            return Ok(Work::More {
                cursor: json!({ "n": n }),
                partial: Value::Null,
                progress: 0.98,
            });
        };
        let final_chapters = t.chapters()?;
        let children = if input.options.details && !final_chapters.is_empty() {
            vec![
                Child::new(details_task(&input.path).priority(TOPIC_PRIORITY)).with_children(
                    detail_tasks(&input.path, &input.options, &final_chapters, &run.dialog),
                ),
            ]
        } else {
            Vec::new()
        };
        let result = json!({ "chapters": chapters });
        if children.is_empty() {
            return Ok(Work::Done(result));
        }
        complete_with_children(host.store.as_ref(), task, &result, children)?;
        Ok(Work::Done(result))
    }

    fn detail(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: TopicInput = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let Some(run) = open_run(&self.ai_dir, &input.path, &settings)? else {
            return Ok(Work::Done(
                json!({ "topic": input.topic, "skipped": "the agent model is not local" }),
            ));
        };
        let t = &run.transcript;
        let chapters = t.chapters()?;
        let chapter = chapters
            .get(input.topic)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("topic {} is not in the outline", input.topic + 1))?;
        if !chapter.digest.sections.is_empty() {
            let items: usize = chapter.digest.sections.iter().map(|s| s.items.len()).sum();
            return Ok(Work::Done(
                json!({ "topic": input.topic, "sections": chapter.digest.sections.len(), "items": items, "kept": true }),
            ));
        }
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        let scope = Scope::attach(
            host.store.clone(),
            task.id,
            host.engine.clone(),
            task.scope.clone(),
        );
        let engine = scope.engine();
        let ask = Ask {
            engine: engine.as_ref(),
            model: &run.model,
            device: run.device,
            hf_token: run.hf_token.as_deref(),
            cancel: cancel.as_ref(),
        };
        let topic = PlanTopic {
            start: chapter.start,
            end: chapter.end,
        };
        let last = input.topic + 1 == chapters.len();
        let picked = topic_replies(&run.dialog, &topic, last);
        let words: usize = picked.iter().map(|r| r.words.len()).sum();
        if words <= WINDOW_WORDS {
            let breakdown = detail_topic(&ask, &run.dialog, run.language, &chapter, last)?;
            return self.write_detail(t, input.topic, task.id, chapter, breakdown);
        }
        let lines = outline::fine_lines(&picked);
        let windows = outline::windows_of_lines(&lines, WINDOW_WORDS);
        let n = windows.len().max(1);
        let mut cursor: DetailCursor =
            serde_json::from_value(task.cursor.clone()).unwrap_or_default();
        if cursor.done < windows.len() {
            let window = &windows[cursor.done];
            let user = prompts::render(
                "outline/detail_window.j2",
                minijinja::context! {
                    title => &chapter.digest.title,
                    summary => &chapter.digest.summary,
                    start => human_duration(window.start),
                    end => human_duration(window.end),
                    number => cursor.done + 1,
                    parts => n,
                    language => run.language,
                    text => &window.text,
                },
            );
            cursor.points.extend(ask.points(user)?);
            cursor.done += 1;
            return Ok(Work::More {
                cursor: serde_json::to_value(&cursor)?,
                partial: Value::Null,
                progress: cursor.done as f32 / (n as f32 + 1.0),
            });
        }
        let (title, summary) = (&chapter.digest.title, &chapter.digest.summary);
        let (start, end) = (human_duration(chapter.start), human_duration(chapter.end));
        let parts = part_context(&chapter.digest.parts);
        let source_chars: usize = cursor
            .points
            .iter()
            .map(|point| point.text.chars().count())
            .sum();
        let source_items = cursor.points.len();
        let points: Vec<minijinja::Value> = cursor
            .points
            .iter()
            .map(|point| minijinja::context! { time => &point.time, text => &point.text })
            .collect();
        let user = prompts::render(
            "outline/detail.j2",
            minijinja::context! { title, summary, start, end, language => run.language, parts, points, text => "" },
        );
        let parsed = ask.sections(user, source_chars, source_items)?;
        let breakdown = Breakdown {
            sections: parsed.raw.into_sections(&lines),
            note: parsed.note,
        };
        self.write_detail(t, input.topic, task.id, chapter, breakdown)
    }

    fn write_detail(
        &self,
        t: &Transcript,
        topic: usize,
        task_id: i64,
        mut chapter: Chapter,
        breakdown: Breakdown,
    ) -> anyhow::Result<Work> {
        if let Some(note) = &breakdown.note {
            log::warn!("task {task_id} topic-detail: {note}");
        }
        let items: usize = breakdown.sections.iter().map(|s| s.items.len()).sum();
        let sections = breakdown.sections.len();
        chapter.digest.sections = breakdown.sections;
        t.put_chapter(topic, &chapter)?;
        let mut result = json!({ "topic": topic, "sections": sections, "items": items });
        if let Some(note) = breakdown.note {
            result["cut"] = json!(note);
        }
        Ok(Work::Done(result))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn digests_a_meeting() {
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
        let w = Window {
            start: 0.0,
            end: 35.0,
            text: text.into(),
        };
        let d = digest(
            engine.as_ref(),
            &model,
            Device::Auto,
            None,
            &w,
            0,
            1,
            None,
            "Russian",
            &AtomicBool::new(false),
        )
        .unwrap();
        eprintln!("{d:#?}");
        assert!(
            !d.title.is_empty() && d.title.split_whitespace().count() <= 12,
            "{}",
            d.title
        );
        assert!(!d.summary.is_empty());
        assert!(
            d.decisions
                .iter()
                .any(|x| x.to_lowercase().contains("пятниц")),
            "{:?}",
            d.decisions
        );
        assert!(
            d.actions.iter().any(|x| x.contains("Ира")),
            "{:?}",
            d.actions
        );
    }

    #[test]
    fn prompts_carry_the_part_and_its_neighbour() {
        let w = Window {
            start: 600.0,
            end: 1200.0,
            text: "[10:00] Дана: начнём.".into(),
        };
        let p = prompt(&w, 1, 3, Some("Планы на квартал"), "English");
        assert!(p.starts_with("Part 2 of 3 of one topic of the recording (10:00 – 20:00).\nWrite everything in English.\n"), "{p}");
        assert!(p.contains("The previous part of this topic was about: Планы на квартал\nDescribe what this part adds; do not repeat the previous part."), "{p}");
        assert!(p.contains("[10:00] Дана: начнём."));
        let first = prompt(&w, 0, 1, Some("Планы на квартал"), "English");
        assert!(
            first.starts_with(
                "One topic of the recording (10:00 – 20:00).\nWrite everything in English.\n"
            ),
            "{first}"
        );
        assert!(first.contains("The previous topic of the recording was: Планы на квартал\nName this topic by what is new in it; do not reuse that title."), "{first}");
        let single = prompt(&w, 0, 1, None, "English");
        assert!(
            single.starts_with(
                "One topic of the recording (10:00 – 20:00).\nWrite everything in English.\n"
            ),
            "{single}"
        );
        assert!(!single.contains("previous"));
    }

    fn chapter(title: &str, start: f64, end: f64) -> Chapter {
        Chapter {
            start,
            end,
            digest: Digest {
                title: title.into(),
                summary: format!("сводка: {title}"),
                ..Digest::default()
            },
        }
    }

    fn sums(specs: &[&[[f32; 2]]]) -> Vec<ChunkSum> {
        specs
            .iter()
            .map(|vectors| {
                let mut sum = ChunkSum {
                    sum: vec![0.0; 2],
                    count: vectors.len(),
                };
                for vector in *vectors {
                    for (s, v) in sum.sum.iter_mut().zip(vector) {
                        *s += f64::from(*v);
                    }
                }
                sum
            })
            .collect()
    }

    fn merger<'a, 'b>(
        ask: &'a Ask<'b>,
        chapters: &'a [Chapter],
        chunk_sums: Vec<ChunkSum>,
        thresholds: Thresholds,
        embed: Option<Embed<'a>>,
    ) -> Merger<'a, 'b> {
        Merger {
            ask,
            language: "Russian",
            chapters,
            chunk_sums,
            embed,
            thresholds,
            state: MergeState::fresh(chapters.len()),
        }
    }

    #[test]
    fn thresholds_come_from_the_recording() {
        let boundaries = [0.0, 1.0, 1.0, 1.0, 1.0, 0.9, 0.7, 0.6, 0.4, 0.1];
        assert_eq!(
            thresholds(&boundaries),
            Some(Thresholds {
                different: 0.4,
                similar: 0.9
            })
        );
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
        let reduce = |title: &str| {
            format!(
                r#"{{"title":"{title}","summary":"всё об одном","decisions":[],"actions":[],"questions":[],"topics":[]}}"#
            )
        };
        fake.chat_answers(&[
            &reduce("Одна тема"),
            &reduce("Одна тема"),
            &reduce("Одна тема"),
        ]);
        let cancel = AtomicBool::new(false);
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapters: Vec<Chapter> = (0..4)
            .map(|i| {
                chapter(
                    &format!("Тема {}", i + 1),
                    i as f64 * 360.0,
                    (i + 1) as f64 * 360.0,
                )
            })
            .collect();
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[1.0, 0.0]], &[[1.0, 0.0]], &[[1.0, 0.0]]]);
        let mut m = merger(
            &ask,
            &chapters,
            chunk_sums,
            Thresholds {
                different: 0.5,
                similar: 0.9,
            },
            None,
        );
        while m.step().unwrap() {}
        let merged = m.finish();
        assert_eq!(merged.len(), 1);
        assert_eq!((merged[0].start, merged[0].end), (0.0, 1440.0));
        assert_eq!(
            merged[0]
                .digest
                .parts
                .iter()
                .map(|p| p.title.as_str())
                .collect::<Vec<_>>(),
            ["Тема 1", "Тема 2", "Тема 3", "Тема 4"]
        );
        assert_eq!(
            fake.chats().len(),
            3,
            "one reduce per merge, the model is not asked about pairs"
        );
    }

    #[test]
    fn different_topics_are_kept_apart_without_asking_the_model() {
        use mediaa_engine::testing::FakeEngine;
        let fake = FakeEngine::new();
        let cancel = AtomicBool::new(false);
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapters: Vec<Chapter> = (0..4)
            .map(|i| {
                chapter(
                    &format!("Тема {}", i + 1),
                    i as f64 * 600.0,
                    (i + 1) as f64 * 600.0,
                )
            })
            .collect();
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[0.0, 1.0]], &[[-1.0, 0.0]], &[[0.0, -1.0]]]);
        let mut m = merger(
            &ask,
            &chapters,
            chunk_sums,
            Thresholds {
                different: 0.5,
                similar: 0.9,
            },
            None,
        );
        assert!(
            !m.step().unwrap(),
            "the most alike pair is already clearly different"
        );
        assert_eq!(m.finish().len(), 4);
        assert!(fake.chats().is_empty(), "the model is not even asked");
    }

    #[test]
    fn the_model_decides_a_borderline_pair() {
        use mediaa_engine::testing::{FakeEngine, RecordedChat};

        fn decide(same: bool) -> (Vec<Chapter>, Vec<RecordedChat>) {
            let fake = FakeEngine::new();
            let answer = if same {
                r#"{"same": true}"#
            } else {
                r#"{"same": false}"#
            };
            fake.chat_answers(&[answer, r#"{"title":"Одна тема","summary":"сводка","decisions":[],"actions":[],"questions":[],"topics":[]}"#]);
            let cancel = AtomicBool::new(false);
            let ask = Ask {
                engine: &fake,
                model: "unsloth/Qwen3.5-4B-GGUF",
                device: Device::Auto,
                hf_token: None,
                cancel: &cancel,
            };
            let chapters = [
                chapter("Отчёт", 0.0, 300.0),
                chapter("Бюджет", 300.0, 600.0),
            ];
            let chunk_sums = sums(&[&[[1.0, 0.0]], &[[0.7, 0.714]]]);
            let mut m = merger(
                &ask,
                &chapters,
                chunk_sums,
                Thresholds {
                    different: 0.5,
                    similar: 0.9,
                },
                None,
            );
            while m.step().unwrap() {}
            (m.finish(), fake.chats())
        }

        let (merged, calls) = decide(true);
        assert_eq!(merged.len(), 1, "the model said it is one topic");
        assert!(
            merged[0].digest.parts.is_empty(),
            "10 minutes: no subtopics"
        );
        assert_eq!(calls.len(), 2, "the question and the reduce");
        assert_eq!(calls[0].messages, 2);
        assert_eq!(calls[0].max_tokens, PAIR_TOKENS);
        assert!(calls[0]
            .json_schema
            .as_deref()
            .unwrap_or_default()
            .contains("\"same\""));

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
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapters = [
            chapter("Отчёт", 0.0, 300.0),
            chapter("Бюджет", 300.0, 600.0),
            chapter("Найм", 600.0, 900.0),
        ];
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[0.866, 0.5]], &[[0.342, 0.940]]]);
        let mut m = merger(
            &ask,
            &chapters,
            chunk_sums,
            Thresholds {
                different: 0.5,
                similar: 0.9,
            },
            None,
        );
        while m.step().unwrap() {}
        assert_eq!(m.finish().len(), 2, "A and B merged, C kept apart");
        let calls = fake.chats();
        assert_eq!(calls.len(), 3, "two questions and one reduce");
        assert!(
            calls[0].texts[1].contains("Chapter A (00:00 – 05:00):\nTitle: Отчёт"),
            "{}",
            calls[0].texts[1]
        );
        let second = &calls[2].texts[1];
        assert!(
            second
                .contains("Chapter A (00:00 – 10:00):\nTitle: Общий отчёт\nSummary: всё об отчёте"),
            "{second}"
        );
        assert!(
            second.contains("Chapter B (10:00 – 15:00):\nTitle: Найм"),
            "{second}"
        );
    }

    #[test]
    fn the_descriptions_of_topics_are_embedded_and_weigh_in() {
        use mediaa_engine::testing::FakeEngine;
        let fake = FakeEngine::new();
        fake.embed_vectors(vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        let cancel = AtomicBool::new(false);
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapters = [
            chapter("Отчёт", 0.0, 600.0),
            chapter("Бюджет", 600.0, 1200.0),
        ];
        let chunk_sums = sums(&[&[[1.0, 0.0]], &[[1.0, 0.0]]]);
        let embed = Some(Embed {
            engine: &fake,
            model: "ggml-org/embeddinggemma-300M-GGUF".into(),
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
            cache: HashMap::new(),
            failed: false,
        });
        let mut m = merger(
            &ask,
            &chapters,
            chunk_sums,
            Thresholds {
                different: 0.5,
                similar: 0.9,
            },
            embed,
        );
        assert!(!m.step().unwrap());
        assert_eq!(m.finish().len(), 2);
        assert_eq!(
            fake.embeddings_count(),
            2,
            "both descriptions are embedded in one request"
        );
        assert!(fake.chats().is_empty());
    }

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
                    .map(|(k, w)| Word {
                        text: w.into(),
                        start: start + k as f64,
                        end: start + k as f64 + 0.5,
                    })
                    .collect();
                Reply {
                    start,
                    end: words.last().unwrap().end,
                    speaker: Some(0),
                    words,
                }
            })
            .collect();
        t.set_replies(&replies).unwrap();
        let chunks: Vec<Chunk> = (0..9)
            .map(|i| Chunk {
                start: i as f64 * 40.0,
                end: (i + 1) as f64 * 40.0,
                speakers: vec![0],
                text: format!("чанк {i}"),
            })
            .collect();
        t.set_chunks(&chunks).unwrap();
        let vectors = [
            [1.0_f32, 0.0],
            [1.0, 0.0],
            [1.0, 0.0],
            [0.0, 1.0],
            [0.0, 1.0],
            [0.0, 1.0],
            [-1.0, 0.0],
            [-1.0, 0.0],
            [-1.0, 0.0],
        ];
        for ((chunk_id, _, _), vector) in t.chunks().unwrap().iter().zip(vectors) {
            t.set_embedding(*chunk_id, &vector).unwrap();
        }
        (dir, ai, file, id)
    }

    fn settings() -> AgentSettings {
        let mut settings = AgentSettings::default();
        settings.models.embedding = None;
        settings.budget.agent = 1;
        settings
    }

    fn topic_sched(
        ai: &Path,
        engine: Arc<dyn Engine>,
        settings: &AgentSettings,
    ) -> Arc<mediaa_queue::Scheduler> {
        let sched = crate::run::open_scheduler(ai, &settings.budget).unwrap();
        let slots = crate::slots::Slots::new(settings.budget.clone());
        let s = settings.clone();
        let current: Arc<dyn Fn() -> AgentSettings + Send + Sync> = Arc::new(move || s.clone());
        crate::run::register_stages(
            &sched,
            ai,
            engine,
            slots,
            current,
            Arc::new(crate::run::Streams::default()),
        );
        sched
    }

    fn drain(sched: &mediaa_queue::Scheduler) {
        crate::run::drain(sched, std::time::Duration::from_secs(60));
    }

    fn pause_until_quiet(sched: &mediaa_queue::Scheduler) {
        let store = sched.store();
        for task in store.list().unwrap() {
            if !task.status.is_finished() && task.status != mediaa_queue::Status::Paused {
                let _ = sched.pause(task.id);
            }
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while store.list().unwrap().iter().any(|task| {
            matches!(
                task.status,
                mediaa_queue::Status::Running | mediaa_queue::Status::Stopping
            )
        }) {
            if std::time::Instant::now() > deadline {
                break;
            }
            let _ = sched.tick();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        for task in store.list().unwrap() {
            if task.status == mediaa_queue::Status::Paused {
                let _ = sched.resume(task.id);
            }
        }
    }

    fn digest_json(title: &str, quotes: &str) -> String {
        format!(
            r#"{{"title":"{title}","summary":"сводка","decisions":[],"actions":[],"questions":[],"topics":[],"quotes":{quotes}}}"#
        )
    }

    fn sections_json(items: &str) -> String {
        format!(r#"{{"sections":[{{"title":"Итоги","kind":"list","items":{items}}}]}}"#)
    }

    fn points_json(points: &str) -> String {
        format!(r#"{{"points":{points}}}"#)
    }

    #[test]
    fn topics_run_as_plan_digests_merge_and_details() {
        use mediaa_engine::testing::FakeEngine;
        use mediaa_queue::Status as TaskStatus;

        let (_dir, ai, file, id) = recording();
        let settings = settings();
        let fake = FakeEngine::new();
        let section = sections_json(
            r#"[{"time":"0:20","text":"реплика1 обсуждение"},{"time":"0:20","text":"этого не говорили"}]"#,
        );
        fake.chat_answers(&[
            &digest_json("Тема раз", r#"[{"time":"0:20","text":"реплика1 обсуждение"},{"time":"0:20","text":"этого не говорили"}]"#),
            &digest_json("Тема два", r#"[{"time":"2:00","text":"реплика6 обсуждение"}]"#),
            &digest_json("Тема три", "[]"),
            &section,
            &section,
            &section,
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let sched = topic_sched(&ai, engine, &settings);
        let options = AnalysisOptions::default();
        let (topics, planned) = sched
            .enqueue_tree(
                topics_task(&file, &options).scope(&id),
                vec![Child::new(plan_task(&file, &options).scope(&id))],
            )
            .unwrap();
        let plan = planned[0];
        drain(&sched);
        let queue = sched.store();

        let tasks = queue.list().unwrap();
        let failed: Vec<_> = tasks
            .iter()
            .filter(|t| t.status != TaskStatus::Done)
            .map(|t| (&t.kind, t.status, &t.error))
            .collect();
        assert!(failed.is_empty(), "{failed:?}");
        let count = |kind: &str| tasks.iter().filter(|t| t.kind == kind).count();
        assert_eq!(
            (
                count(PLAN_KIND),
                count(DIGEST_KIND),
                count(MERGE_KIND),
                count(DETAILS_KIND),
                count(DETAIL_KIND)
            ),
            (1, 3, 1, 1, 3),
            "the whole graph"
        );
        assert_eq!(queue.get(topics).unwrap().unwrap().status, TaskStatus::Done);
        assert_eq!(queue.get(plan).unwrap().unwrap().status, TaskStatus::Done);
        let kinds: Vec<String> = queue
            .children(topics)
            .unwrap()
            .into_iter()
            .map(|t| t.kind)
            .collect();
        assert_eq!(
            kinds,
            [
                PLAN_KIND,
                DIGEST_KIND,
                DIGEST_KIND,
                DIGEST_KIND,
                MERGE_KIND,
                DETAILS_KIND
            ]
        );
        let details = tasks.iter().find(|t| t.kind == DETAILS_KIND).unwrap();
        assert_eq!(queue.children(details.id).unwrap().len(), 3);

        let t = Transcript::open(&ai, &id).unwrap();
        let chapters = t.chapters().unwrap();
        assert_eq!(
            chapters
                .iter()
                .map(|c| c.digest.title.as_str())
                .collect::<Vec<_>>(),
            ["Тема раз", "Тема два", "Тема три"]
        );
        assert_eq!(
            chapters[0].digest.quotes,
            [Quote {
                time: 20.0,
                text: "реплика1 обсуждение".into()
            }],
            "the quote is checked, the invented one dropped"
        );
        assert_eq!(
            chapters[1].digest.quotes,
            [Quote {
                time: 120.0,
                text: "реплика6 обсуждение".into()
            }]
        );
        assert!(t.meta(MERGED_KEY).unwrap().is_some() && t.meta(FINAL_KEY).unwrap().is_none());
        assert_eq!(chapters[0].digest.sections.len(), 1);
        assert_eq!(
            chapters[0].digest.sections[0]
                .items
                .iter()
                .map(|i| i.time)
                .collect::<Vec<_>>(),
            [Some(20.0), None],
            "the found time, the invented one"
        );
        assert_eq!(
            chapters[1].digest.sections[0].items[0].time, None,
            "0:20 does not belong to this topic"
        );
        assert_eq!(
            chapters[2].digest.sections[0].items[0].text, "реплика1 обсуждение",
            "the item stays without a time"
        );
        assert_eq!(
            fake.chats().len(),
            6,
            "three digests and three details — the merge asks nothing"
        );
        assert!(fake.chats()[0]
            .json_schema
            .as_deref()
            .unwrap_or_default()
            .contains("\"quotes\""));
        assert!(fake.chats()[3]
            .json_schema
            .as_deref()
            .unwrap_or_default()
            .contains("\"sections\""));
        for (i, chat) in fake.chats().iter().enumerate() {
            assert!(
                chat.texts[1].contains("Write everything in English."),
                "call {i}: {:?}",
                chat.texts
            );
        }
    }

    #[test]
    fn a_restart_in_the_middle_does_not_redo_a_digest() {
        use mediaa_engine::testing::FakeEngine;
        use mediaa_queue::Status as TaskStatus;

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
        let sched = topic_sched(&ai, engine.clone(), &settings);
        let options = AnalysisOptions::default();
        sched
            .enqueue_tree(
                topics_task(&file, &options).scope(&id),
                vec![Child::new(plan_task(&file, &options).scope(&id))],
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while fake.chats().is_empty() {
            if std::time::Instant::now() > deadline {
                break;
            }
            sched.tick().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        pause_until_quiet(&sched);
        let mid = fake.chats().len();
        assert!((1..6).contains(&mid), "stopped in the middle, {mid} calls");
        let t = Transcript::open(&ai, &id).unwrap();
        assert_eq!(t.chapters().unwrap()[0].digest.title, "Тема раз");
        drop(t);
        drop(sched);

        let sched = topic_sched(&ai, engine, &settings);
        drain(&sched);
        let tasks = sched.store().list().unwrap();
        assert!(
            tasks.iter().all(|t| t.status == TaskStatus::Done),
            "{tasks:?}"
        );
        let t = Transcript::open(&ai, &id).unwrap();
        let titles: Vec<String> = t
            .chapters()
            .unwrap()
            .iter()
            .map(|c| c.digest.title.clone())
            .collect();
        assert_eq!(titles, ["Тема раз", "Тема два", "Тема три"]);
        assert!(t
            .chapters()
            .unwrap()
            .iter()
            .all(|c| !c.digest.sections.is_empty()));
        assert_eq!(
            fake.chats().len(),
            6,
            "three digests and three details — a written digest was not asked twice"
        );
    }

    #[test]
    fn a_language_change_keeps_written_topics_and_writes_the_rest_in_the_new_language() {
        use mediaa_engine::testing::FakeEngine;
        use mediaa_queue::Status as TaskStatus;

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
        let sched = topic_sched(&ai, engine.clone(), &settings);
        let options = AnalysisOptions::default();
        sched
            .enqueue_tree(
                topics_task(&file, &options).scope(&id),
                vec![Child::new(plan_task(&file, &options).scope(&id))],
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while fake.chats().is_empty() {
            if std::time::Instant::now() > deadline {
                break;
            }
            sched.tick().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        pause_until_quiet(&sched);
        let written = fake.chats();
        assert_eq!(written.len(), 1, "stopped after the first digest");
        assert!(
            written[0].texts[1].contains("Write everything in English."),
            "the interface was English: {:?}",
            written[0].texts
        );
        let t = Transcript::open(&ai, &id).unwrap();
        assert_eq!(t.chapters().unwrap()[0].digest.title, "Тема раз");
        drop(t);
        drop(sched);

        let mut russian = settings.clone();
        russian.ui_language = Some("ru".into());
        let sched = topic_sched(&ai, engine, &russian);
        drain(&sched);
        let tasks = sched.store().list().unwrap();
        assert!(
            tasks.iter().all(|t| t.status == TaskStatus::Done),
            "{tasks:?}"
        );
        let t = Transcript::open(&ai, &id).unwrap();
        let titles: Vec<String> = t
            .chapters()
            .unwrap()
            .iter()
            .map(|c| c.digest.title.clone())
            .collect();
        assert_eq!(
            titles,
            ["Тема раз", "Тема два", "Тема три"],
            "the written digest is not redone"
        );
        let calls = fake.chats();
        assert_eq!(
            calls.len(),
            6,
            "three digests and three details: the written digest was not asked again"
        );
        for (i, chat) in calls.iter().skip(1).enumerate() {
            assert!(
                chat.texts[1].contains("Write everything in Russian."),
                "call {}: {:?}",
                i + 1,
                chat.texts
            );
        }
    }

    #[test]
    fn a_restart_in_the_middle_does_not_redo_a_detail() {
        use mediaa_engine::testing::FakeEngine;
        use mediaa_queue::Status as TaskStatus;

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
        let sched = topic_sched(&ai, engine.clone(), &settings);
        let options = AnalysisOptions::default();
        sched
            .enqueue_tree(
                topics_task(&file, &options).scope(&id),
                vec![Child::new(plan_task(&file, &options).scope(&id))],
            )
            .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while fake.chats().len() < 4 {
            if std::time::Instant::now() > deadline {
                break;
            }
            sched.tick().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        pause_until_quiet(&sched);
        let mid = fake.chats().len();
        assert!(
            (4..6).contains(&mid),
            "stopped during the details, {mid} calls"
        );
        let t = Transcript::open(&ai, &id).unwrap();
        assert!(t
            .chapters()
            .unwrap()
            .iter()
            .any(|c| !c.digest.sections.is_empty()));
        drop(t);
        drop(sched);

        let sched = topic_sched(&ai, engine, &settings);
        drain(&sched);
        let tasks = sched.store().list().unwrap();
        assert!(
            tasks.iter().all(|t| t.status == TaskStatus::Done),
            "{tasks:?}"
        );
        assert_eq!(
            fake.chats().len(),
            6,
            "the stopped detail was not asked twice"
        );
    }

    #[test]
    fn without_the_details_option_the_merge_starts_nothing() {
        use mediaa_engine::testing::FakeEngine;
        use mediaa_queue::Status as TaskStatus;

        let (_dir, ai, file, id) = recording();
        let settings = settings();
        let fake = FakeEngine::new();
        fake.chat_answers(&[
            &digest_json("Тема раз", "[]"),
            &digest_json("Тема два", "[]"),
            &digest_json("Тема три", "[]"),
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let sched = topic_sched(&ai, engine, &settings);
        let options = AnalysisOptions {
            details: false,
            ..AnalysisOptions::default()
        };
        let topics = sched
            .enqueue_tree(
                topics_task(&file, &options).scope(&id),
                vec![Child::new(plan_task(&file, &options).scope(&id))],
            )
            .unwrap()
            .0;
        drain(&sched);
        let queue = sched.store();

        let tasks = queue.list().unwrap();
        assert!(
            tasks.iter().all(|t| t.status == TaskStatus::Done),
            "{tasks:?}"
        );
        assert!(
            tasks
                .iter()
                .all(|t| t.kind != DETAILS_KIND && t.kind != DETAIL_KIND),
            "no details tasks"
        );
        assert_eq!(queue.get(topics).unwrap().unwrap().status, TaskStatus::Done);
        assert_eq!(fake.chats().len(), 3);
        let t = Transcript::open(&ai, &id).unwrap();
        assert!(t
            .chapters()
            .unwrap()
            .iter()
            .all(|c| c.digest.sections.is_empty()));
        let topics: Vec<Chapter> =
            serde_json::from_str(&t.meta(TOPICS_KEY).unwrap().unwrap()).unwrap();
        assert_eq!(
            topics.len(),
            3,
            "the digest-stage chapters are kept for a merge re-run"
        );
    }

    fn long_reply(i: usize) -> Reply {
        use mediaa_core::transcript::Word;
        let start = i as f64 * 10.0;
        let words: Vec<Word> = std::iter::once(format!("реплика{i}"))
            .chain((0..99).map(|k| format!("слово{k}")))
            .enumerate()
            .map(|(k, text)| Word {
                text,
                start: start + k as f64 * 0.05,
                end: start + k as f64 * 0.05 + 0.05,
            })
            .collect();
        Reply {
            start,
            end: words.last().unwrap().end,
            speaker: None,
            words,
        }
    }

    fn saying(start: f64, text: &str) -> Reply {
        use mediaa_core::transcript::Word;
        let words: Vec<Word> = text
            .split_whitespace()
            .enumerate()
            .map(|(k, w)| Word {
                text: w.into(),
                start: start + k as f64,
                end: start + k as f64 + 0.5,
            })
            .collect();
        Reply {
            start,
            end: words.last().unwrap().end,
            speaker: None,
            words,
        }
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
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapter = Chapter {
            start: 0.0,
            end: replies.last().unwrap().end,
            digest: Digest {
                title: "Отчёт".into(),
                summary: "сводка".into(),
                ..Digest::default()
            },
        };
        let breakdown = detail_topic(&ask, &replies, "Russian", &chapter, true).unwrap();
        assert!(breakdown.note.is_none(), "{:?}", breakdown.note);
        let sections = breakdown.sections;
        let calls = fake.chats();
        assert_eq!(
            calls.len(),
            3,
            "the points of two windows and one call that assembles the sections"
        );
        assert!(
            calls[0].texts[1].contains("Part 1 of 2"),
            "{}",
            calls[0].texts[1]
        );
        assert!(
            calls[1].texts[1].contains("Part 2 of 2"),
            "{}",
            calls[1].texts[1]
        );
        assert!(
            calls[2].texts[1].contains("The points said in the topic, in order"),
            "{}",
            calls[2].texts[1]
        );
        assert!(
            !calls[2].texts[1].contains("Transcript of the topic"),
            "the transcript is not read again"
        );
        assert!(calls[0]
            .json_schema
            .as_deref()
            .unwrap_or_default()
            .contains("\"points\""));
        assert!(calls[2]
            .json_schema
            .as_deref()
            .unwrap_or_default()
            .contains("\"sections\""));
        assert_eq!(
            (calls[0].max_tokens, calls[2].max_tokens),
            (points_tokens(), sections_tokens())
        );
        assert_eq!(
            sections[0].items.iter().map(|i| i.time).collect::<Vec<_>>(),
            [Some(120.0), None]
        );
        assert_eq!(sections[0].items[1].text, "вывод");
    }

    #[test]
    fn the_times_of_detail_items_are_checked_against_the_lines() {
        use mediaa_engine::testing::FakeEngine;

        let fake = FakeEngine::new();
        fake.chat_answers(&[&sections_json(
            r#"[{"time":"0:05","text":"обсудили сроки отчёта"},{"time":"0:14","text":"Олег проверит цифры"},{"time":"5:00","text":"Олег проверит цифры"}]"#,
        )]);
        let replies = [
            saying(10.0, "Мы обсудили сроки отчёта"),
            saying(20.0, "Олег проверит цифры"),
        ];
        let cancel = AtomicBool::new(false);
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapter = Chapter {
            start: 0.0,
            end: 25.0,
            digest: Digest {
                title: "Отчёт".into(),
                summary: "сводка".into(),
                ..Digest::default()
            },
        };
        let breakdown = detail_topic(&ask, &replies, "Russian", &chapter, true).unwrap();
        assert!(breakdown.note.is_none(), "{:?}", breakdown.note);
        let sections = breakdown.sections;
        assert_eq!(fake.chats().len(), 1, "a short topic: one call");
        assert!(fake.chats()[0].texts[1].contains("Transcript of the topic"));
        assert_eq!(sections[0].title, "Итоги");
        assert_eq!(sections[0].kind, SectionKind::List);
        assert_eq!(
            sections[0].items.iter().map(|i| i.time).collect::<Vec<_>>(),
            [Some(10.0), Some(20.0), None]
        );
        assert_eq!(
            sections[0].items[2].text, "Олег проверит цифры",
            "the item stays without its time"
        );
    }

    #[test]
    fn the_items_of_a_monologue_get_the_times_of_their_lines() {
        use mediaa_core::transcript::Word;
        use mediaa_engine::testing::FakeEngine;

        let words: Vec<Word> = (0..100)
            .map(|i| Word {
                text: format!("слово{i}"),
                start: i as f64 * 0.5,
                end: i as f64 * 0.5 + 0.4,
            })
            .collect();
        let replies = [Reply {
            start: 0.0,
            end: words.last().unwrap().end,
            speaker: None,
            words,
        }];
        let fake = FakeEngine::new();
        fake.chat_answers(&[&sections_json(
            r#"[{"time":"0:30","text":"слово55 слово56"},{"time":"0:30","text":"погода на даче"}]"#,
        )]);
        let cancel = AtomicBool::new(false);
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapter = Chapter {
            start: 0.0,
            end: 50.0,
            digest: Digest {
                title: "Длинный монолог".into(),
                summary: "сводка".into(),
                ..Digest::default()
            },
        };
        let breakdown = detail_topic(&ask, &replies, "Russian", &chapter, true).unwrap();
        assert!(breakdown.note.is_none(), "{:?}", breakdown.note);
        let sections = breakdown.sections;
        let prompt = &fake.chats()[0].texts[1];
        assert!(prompt.contains("[00:00] слово0 "), "{prompt}");
        assert!(
            prompt.contains("[00:26] слово51"),
            "the second line has its own time: {prompt}"
        );
        assert_eq!(
            sections[0].items.iter().map(|i| i.time).collect::<Vec<_>>(),
            [Some(25.5), None]
        );
    }

    #[test]
    fn sections_with_the_same_title_are_merged() {
        use mediaa_engine::testing::FakeEngine;

        let fake = FakeEngine::new();
        let answer = r#"{"sections":[
            {"title":"Предлагаемые действия","kind":"steps","items":[{"text":"лайкнуть видео"}]},
            {"title":" Другие ","kind":"list","items":[{"text":"первое"}]},
            {"title":"предлагаемые ДЕЙСТВИЯ ","kind":"steps","items":[{"text":"лайкнуть видео"},{"text":"подписаться"}]}
        ]}"#;
        fake.chat_answers(&[answer]);
        let replies = [saying(0.0, "лайкнуть видео подписаться первое")];
        let cancel = AtomicBool::new(false);
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapter = Chapter {
            start: 0.0,
            end: 3.0,
            digest: Digest {
                title: "Ролик".into(),
                summary: "сводка".into(),
                ..Digest::default()
            },
        };
        let breakdown = detail_topic(&ask, &replies, "Russian", &chapter, true).unwrap();
        assert!(breakdown.note.is_none(), "{:?}", breakdown.note);
        let sections = breakdown.sections;
        assert_eq!(
            sections
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>(),
            ["Предлагаемые действия", "Другие"]
        );
        let texts: Vec<&str> = sections[0].items.iter().map(|i| i.text.as_str()).collect();
        assert_eq!(
            texts,
            ["лайкнуть видео", "подписаться"],
            "the order is kept, the repeat dropped"
        );
        assert_eq!(sections[1].items[0].text, "первое");
    }

    #[test]
    fn the_answer_limits_cover_a_maximal_answer() {
        let fits = |answer: &str, limit: u32| {
            let tokens = answer.chars().count().div_ceil(CHARS_PER_TOKEN as usize);
            assert!(
                tokens <= limit as usize,
                "{tokens} tokens do not fit in {limit}"
            );
            assert!(
                limit as usize >= tokens + tokens / ANSWER_MARGIN_DIV as usize,
                "{limit} has no margin over {tokens}"
            );
        };
        let title = "т".repeat(outline::SECTION_TITLE_CHARS);
        let text = "я".repeat(outline::SECTION_TEXT_CHARS);
        let sections: Vec<Value> = (0..outline::SECTIONS_MAX)
            .map(|_| {
                let items: Vec<Value> = (0..outline::SECTION_ITEMS_MAX)
                    .map(|_| json!({ "time": "0:00", "text": text }))
                    .collect();
                json!({ "title": title, "kind": "list", "items": items })
            })
            .collect();
        fits(
            &serde_json::to_string(&json!({ "sections": sections })).unwrap(),
            sections_tokens(),
        );
        let points: Vec<Value> = (0..outline::POINTS_MAX)
            .map(|_| json!({ "time": "0:00", "text": "я".repeat(outline::POINT_TEXT_CHARS) }))
            .collect();
        fits(
            &serde_json::to_string(&json!({ "points": points })).unwrap(),
            points_tokens(),
        );
        let list = vec!["я".repeat(DIGEST_POINT_CHARS); DIGEST_POINTS];
        let quotes: Vec<Value> = (0..DIGEST_QUOTES)
            .map(|_| json!({ "time": "0:00", "text": "я".repeat(DIGEST_QUOTE_CHARS) }))
            .collect();
        let digest = json!({
            "title": "т".repeat(DIGEST_TITLE_CHARS),
            "summary": "я".repeat(DIGEST_SUMMARY_CHARS),
            "decisions": list.clone(),
            "actions": list.clone(),
            "questions": list,
            "topics": vec!["я".repeat(DIGEST_KEYWORD_CHARS); DIGEST_POINTS],
            "quotes": quotes,
        });
        fits(&serde_json::to_string(&digest).unwrap(), digest_tokens());
    }

    fn cut_off_sections() -> String {
        "{\n\"sections\":[\n\
         {\"title\":\"Первый\",\"kind\":\"list\",\"items\":[{\"text\":\"один\"}]},\n\
         {\"title\":\"Второй\",\"kind\":\"steps\",\"items\":[{\"text\":\"два\"}]},\n\
         {\"title\":\"Третий\",\"kind\":\"list\",\"items\":[{\"text\":\"три"
            .into()
    }

    #[test]
    fn a_section_cut_off_mid_item_is_dropped() {
        let raw = closed_sections(&cut_off_sections());
        assert_eq!(
            raw.sections
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>(),
            ["Первый", "Второй"]
        );
        let quoted = r#"{"sections":[{"title":"A","kind":"list","items":[{"text":"say \"hi\""}]},{"title":"B","kind":"list","items":[{"text":"cut"}"#;
        let raw = closed_sections(quoted);
        assert_eq!(raw.sections.len(), 1);
        assert_eq!(raw.sections[0].items[0].text, "say \"hi\"");
    }

    #[test]
    fn a_cut_off_answer_keeps_the_sections_that_closed() {
        use mediaa_engine::testing::FakeEngine;

        let fake = FakeEngine::new();
        let cut = cut_off_sections();
        fake.chat_answers(&[&cut, &cut]);
        fake.chat_usage(Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 12_000,
        }));
        let replies = [saying(0.0, "один два")];
        let cancel = AtomicBool::new(false);
        let ask = Ask {
            engine: &fake,
            model: "unsloth/Qwen3.5-4B-GGUF",
            device: Device::Auto,
            hf_token: None,
            cancel: &cancel,
        };
        let chapter = Chapter {
            start: 0.0,
            end: 2.0,
            digest: Digest {
                title: "Тема".into(),
                summary: "сводка".into(),
                ..Digest::default()
            },
        };
        let breakdown = detail_topic(&ask, &replies, "Russian", &chapter, true).unwrap();
        assert_eq!(
            breakdown
                .sections
                .iter()
                .map(|s| s.title.as_str())
                .collect::<Vec<_>>(),
            ["Первый", "Второй"]
        );
        let calls = fake.chats();
        assert_eq!(
            calls.len(),
            2,
            "the cut answer and one shorter retry, then the closed sections"
        );
        let note = breakdown.note.expect("the cut is reported");
        assert!(note.contains("after 2 complete sections"), "{note}");
        assert!(
            note.contains(&format!("hit the limit of {} tokens", calls[0].max_tokens)),
            "{note}"
        );
        assert_eq!(calls[0].max_tokens, calls[1].max_tokens);
    }

    #[test]
    fn a_cut_off_answer_is_asked_once_to_be_shorter_and_the_task_says_why() {
        use mediaa_engine::testing::FakeEngine;
        use mediaa_queue::Status as TaskStatus;

        let (_dir, ai, file, id) = recording();
        let t = Transcript::open(&ai, &id).unwrap();
        let replies = t.dialog().unwrap();
        let chapter = Chapter {
            start: replies[0].start,
            end: replies.last().unwrap().end,
            digest: Digest {
                title: "Тема".into(),
                summary: "сводка".into(),
                ..Digest::default()
            },
        };
        write_chapters(&t, &[chapter]).unwrap();
        drop(t);

        let fake = FakeEngine::new();
        fake.chat_usage(Some(Usage {
            prompt_tokens: 10,
            completion_tokens: 12_000,
        }));
        let shorter = sections_json(r#"[{"text":"реплика1 обсуждение"}]"#);
        let cut = cut_off_sections();
        fake.chat_answers(&[&cut, &shorter]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let settings = settings();
        let sched = topic_sched(&ai, engine, &settings);
        let options = AnalysisOptions::default();
        let task_id = sched
            .enqueue(
                NewTask::new(
                    DETAIL_KIND,
                    "Тема",
                    &json!({ "path": file, "options": options, "topic": 0 }),
                )
                .resource(Resource::Llm)
                .scope(&id)
                .analysis(&id),
            )
            .unwrap();
        drain(&sched);

        let task = sched.store().get(task_id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        let note = task.result.unwrap()["cut"].as_str().unwrap().to_string();
        let calls = fake.chats();
        assert_eq!(calls.len(), 2, "one retry, not a third call");
        assert_eq!(
            calls[1].messages, 4,
            "system, task, the cut answer, the shorter ask"
        );
        assert_eq!(
            calls[1].texts[3],
            prompts::render(
                "outline/detail_shorter.j2",
                minijinja::context! { tokens => calls[0].max_tokens }
            )
        );
        assert!(note.contains("the first answer was cut at line"), "{note}");
        assert!(
            note.contains(&format!("hit the limit of {} tokens", calls[0].max_tokens)),
            "{note}"
        );
        let saved = Transcript::open(&ai, &id).unwrap().chapters().unwrap();
        assert_eq!(
            saved[0].digest.sections[0].items[0].text,
            "реплика1 обсуждение"
        );
    }

    #[test]
    fn the_section_limit_follows_the_topic_and_the_context() {
        assert_eq!(
            topic_section_tokens(10, 1),
            sections_tokens(),
            "a short topic keeps the floor"
        );
        let chars = 14_000;
        let wanted = topic_section_tokens(chars, 10);
        assert!(
            wanted > sections_tokens(),
            "the topic is larger than the old schema caps"
        );
        let bare = (chars as u32).div_ceil(CHARS_PER_TOKEN);
        assert!(
            wanted >= bare + bare / ANSWER_MARGIN_DIV,
            "{wanted} has no margin over {bare}"
        );
        let user = "я".repeat(chars);
        let fitting = section_budget("sys", &user, chars, 10);
        assert!(
            fitting.fits,
            "prompt {} + {} tokens",
            fitting.prompt_tokens, fitting.wanted
        );
        assert_eq!(fitting.max_tokens, wanted);

        let medium = "я".repeat(6_000);
        let clamped = section_budget("sys", &medium, 30_000, 20);
        assert!(
            !clamped.fits,
            "prompt {} wanted {}",
            clamped.prompt_tokens, clamped.wanted
        );
        assert!(clamped.prompt_tokens + CTX_RESERVE < OUTLINE_CTX);
        assert!(clamped.max_tokens < clamped.wanted);
        assert!(clamped.prompt_tokens + clamped.max_tokens + CTX_RESERVE <= OUTLINE_CTX);

        let huge = "я".repeat(40_000);
        let cramped = section_budget("sys", &huge, huge.chars().count(), 80);
        assert!(!cramped.fits);
        assert!(cramped.max_tokens < cramped.wanted);
        assert!(cramped.prompt_tokens > OUTLINE_CTX);
        assert_eq!(cramped.max_tokens, 1);
        let note = section_note(None, None, &cramped, None, None).unwrap();
        assert!(
            note.contains(&format!("do not fit the context of {OUTLINE_CTX} tokens")),
            "{note}"
        );
    }
}
