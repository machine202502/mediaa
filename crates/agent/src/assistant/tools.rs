use super::brain::ToolSpec;
use crate::prompts;
use crate::search::Options;
use crate::tools::{between, calculate, format_time, parse_time};
use mediaa_core::outline::{Chapter, SectionKind};
use mediaa_core::retrieval::{stems, Hit};
use mediaa_core::speakers::Parsed;
use mediaa_core::transcript::Reply;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, Default)]
pub struct Recording {
    pub title: String,
    pub duration: f64,
    pub diarized: bool,
    pub names: BTreeMap<u32, String>,
    pub replies: Vec<Reply>,
    pub chapters: Vec<Chapter>,
    pub position: Option<f64>,
}

impl Recording {
    pub fn speaker(&self, s: u32) -> String {
        mediaa_core::speakers::label(s)
    }
}

pub fn whos(rec: &Recording, number: Option<u32>) -> Vec<mediaa_core::speakers::Who> {
    let mut ids: Vec<u32> = rec.replies.iter().filter_map(|r| r.speaker).collect();
    ids.extend(rec.names.keys().copied());
    ids.sort_unstable();
    ids.dedup();
    let title = if number.is_some() {
        rec.title.clone()
    } else {
        String::new()
    };
    ids.into_iter()
        .map(|index| mediaa_core::speakers::Who {
            index,
            recording: number,
            name: rec.names.get(&index).cloned().unwrap_or_default(),
            title: title.clone(),
        })
        .collect()
}

impl Recording {
    pub fn card(&self) -> String {
        let mut ids: Vec<u32> = self.replies.iter().filter_map(|r| r.speaker).collect();
        ids.sort_unstable();
        ids.dedup();
        let speakers: Vec<_> = if self.diarized {
            ids.iter()
                .map(|&s| minijinja::context! { label => self.speaker(s), name => self.names.get(&s).filter(|n| !n.trim().is_empty()).cloned() })
                .collect()
        } else {
            Vec::new()
        };
        prompts::render(
            "agent/card_recording.j2",
            minijinja::context! {
                title => &self.title,
                length => format_time(self.duration),
                minutes => self.duration / 60.0,
                speakers,
                topics => self.chapters.iter().enumerate().map(|(i, c)| minijinja::context! {
                    number => i + 1,
                    start => format_time(c.start),
                    end => format_time(c.end),
                    title => &c.digest.title,
                    summary => &c.digest.summary,
                }).collect::<Vec<_>>(),
                position => self.position.map(format_time),
            },
        )
    }
}

pub fn specs() -> Vec<ToolSpec> {
    let none = json!({"type": "object", "properties": {}});
    vec![
        ToolSpec {
            name: "outline",
            description: prompts::tool("outline"),
            params: json!({"type": "object", "properties": {"topic": {"type": "integer"}}}),
            example: json!({}),
        },
        ToolSpec {
            name: "search",
            description: prompts::tool("search"),
            params: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "speaker": {"type": "string"},
                    "from": {"type": "string"},
                    "to": {"type": "string"}
                },
                "required": ["query"]
            }),
            example: json!({"query": "сроки сдачи отчёта"}),
        },
        ToolSpec {
            name: "read",
            description: prompts::tool("read"),
            params: json!({"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]}),
            example: json!({"from": "12:00", "to": "15:30"}),
        },
        ToolSpec {
            name: "mentions",
            description: prompts::tool("mentions"),
            params: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            example: json!({"query": "самозанятость"}),
        },
        ToolSpec {
            name: "speakers",
            description: prompts::tool("speakers"),
            params: none,
            example: json!({}),
        },
        ToolSpec {
            name: "calculator",
            description: prompts::tool("calculator"),
            params: json!({"type": "object", "properties": {"expression": {"type": "string"}}, "required": ["expression"]}),
            example: json!({"expression": "(45 - 12) / 3"}),
        },
        ToolSpec {
            name: "time",
            description: prompts::tool("time"),
            params: json!({"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]}),
            example: json!({"from": "5:59", "to": "1:02:30"}),
        },
    ]
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub text: String,
    pub seen: Option<Vec<String>>,
}

impl Outcome {
    fn plain(text: impl Into<String>) -> Outcome {
        Outcome {
            text: text.into(),
            seen: None,
        }
    }
}

pub(super) fn bounded(mut text: String, max: usize, note: &str) -> String {
    if text.chars().count() > max {
        text = text.chars().take(max).collect();
        text.push_str(&format!(
            "\n… {}",
            prompts::render(&format!("results/cut/{note}.j2"), ())
        ));
    }
    text
}

pub(super) fn error(name: &str, ctx: impl serde::Serialize) -> anyhow::Error {
    anyhow::anyhow!(prompts::render(&format!("errors/{name}.j2"), ctx))
}

pub(super) fn needs(tool: &str, what: &str) -> anyhow::Error {
    error(&format!("needs_{what}"), minijinja::context! { tool })
}

fn line(rec: &Recording, r: &Reply) -> String {
    prompts::render(
        "results/recording/line.j2",
        minijinja::context! { time => format_time(r.start), speaker => r.speaker.map(|s| rec.speaker(s)), text => r.text() },
    )
}

pub type Scanner<'a> = dyn Fn(&str) -> anyhow::Result<super::scan::Scan> + Sync + 'a;

fn section_kind(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::List => "list",
        SectionKind::Steps => "steps",
        SectionKind::Text => "text",
    }
}

fn topic_number(args: &Value, count: usize) -> anyhow::Result<Option<usize>> {
    let raw = match &args["topic"] {
        Value::Null => return Ok(None),
        Value::Number(n) => n.to_string(),
        Value::String(s) if s.trim().is_empty() => return Ok(None),
        Value::String(s) => s.trim().to_string(),
        other => other.to_string(),
    };
    match raw.parse::<usize>() {
        Ok(n) if (1..=count).contains(&n) => Ok(Some(n)),
        _ => Err(error(
            "no_topic",
            minijinja::context! { topic => raw, count },
        )),
    }
}

pub type Search<'a> = dyn Fn(&str, Options, usize) -> anyhow::Result<Vec<Hit>> + Sync + 'a;

pub struct Tools<'a> {
    pub recording: &'a Recording,
    pub search: &'a Search<'a>,
    pub max_chars: usize,
    pub scan: Option<&'a Scanner<'a>>,
    pub rename: Option<&'a Renamer<'a>>,
    pub web: Option<&'a super::internet::Web<'a>>,
}

pub type Renamer<'a> = dyn Fn(u32, &str) -> anyhow::Result<()> + Sync + 'a;

impl<'a> Tools<'a> {
    pub fn new(recording: &'a Recording, search: &'a Search<'a>, max_chars: usize) -> Tools<'a> {
        Tools {
            recording,
            search,
            max_chars,
            scan: None,
            rename: None,
            web: None,
        }
    }
}

pub fn speaker_of(recording: &Recording, label: &str) -> Option<u32> {
    let s = mediaa_core::speakers::parse_label(label)?.index;
    (recording.replies.iter().any(|r| r.speaker == Some(s)) || recording.names.contains_key(&s))
        .then_some(s)
}

impl Tools<'_> {
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut s = specs();
        if self.scan.is_some() {
            s.push(ToolSpec {
                name: "scan",
                description: prompts::tool("scan"),
                params: json!({"type": "object", "properties": {"instruction": {"type": "string"}}, "required": ["instruction"]}),
                example: json!({"instruction": "<what the user wants listed, in their words>"}),
            });
        }
        if self.rename.is_some() && self.recording.diarized {
            s.push(ToolSpec {
                name: "rename_speakers",
                description: prompts::tool("rename_speakers"),
                params: json!({"type": "object", "properties": {"names": {"type": "array", "maxItems": 12, "items": {"type": "object", "properties": {"speaker": {"type": "string"}, "name": {"type": "string"}}, "required": ["speaker", "name"]}}}, "required": ["names"]}),
                example: json!({"names": [{"speaker": "@s1", "name": "<the name>"}]}),
            });
        }
        s
    }

    pub fn run(&self, tool: &str, args: &Value) -> anyhow::Result<Outcome> {
        let arg = |k: &str| {
            args.get(k)
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default()
                .to_string()
        };
        let rec = self.recording;
        match tool {
            "outline" => {
                if rec.chapters.is_empty() {
                    return Ok(Outcome::plain(prompts::render(
                        "results/recording/no_outline.j2",
                        (),
                    )));
                }
                let Some(n) = topic_number(args, rec.chapters.len())? else {
                    let chapters: Vec<_> = rec
                        .chapters
                        .iter()
                        .map(|c| {
                            let sections: Vec<&str> = c.digest.sections.iter().map(|s| s.title.as_str()).collect();
                            minijinja::context! { start => format_time(c.start), end => format_time(c.end), title => &c.digest.title, summary => &c.digest.summary, sections }
                        })
                        .collect();
                    let text = prompts::render(
                        "results/recording/outline.j2",
                        minijinja::context! { chapters },
                    );
                    let seen = rec
                        .chapters
                        .iter()
                        .map(|c| format!("chapter@{}", c.start as i64))
                        .collect();
                    return Ok(Outcome {
                        text: bounded(text, self.max_chars, "chapters"),
                        seen: Some(seen),
                    });
                };
                let c = &rec.chapters[n - 1];
                let seen = Some(vec![format!("topic@{n}")]);
                if c.digest.sections.is_empty() {
                    let text = prompts::render(
                        "results/recording/topic_empty.j2",
                        minijinja::context! { number => n, title => &c.digest.title },
                    );
                    return Ok(Outcome { text, seen });
                }
                let sections: Vec<_> = c
                    .digest
                    .sections
                    .iter()
                    .map(|s| {
                        let items: Vec<_> = s.items.iter().map(|item| minijinja::context! { time => item.time.map(format_time).unwrap_or_default(), text => &item.text }).collect();
                        minijinja::context! { title => &s.title, kind => section_kind(s.kind), items }
                    })
                    .collect();
                let quotes: Vec<_> = c
                    .digest
                    .quotes
                    .iter()
                    .map(|q| minijinja::context! { time => format_time(q.time), text => &q.text })
                    .collect();
                let text = prompts::render(
                    "results/recording/topic.j2",
                    minijinja::context! { number => n, start => format_time(c.start), end => format_time(c.end), title => &c.digest.title, summary => &c.digest.summary, sections, quotes },
                );
                Ok(Outcome {
                    text: bounded(text, self.max_chars, "plain"),
                    seen,
                })
            }
            "search" => {
                let query = arg("query");
                anyhow::ensure!(!query.is_empty(), needs("search", "query"));
                anyhow::ensure!(
                    !vague(&query),
                    error("vague_query", minijinja::context! { query })
                );
                let speaker = match arg("speaker") {
                    label if label.is_empty() => None,
                    label => {
                        let index = speaker_of(rec, &label).ok_or_else(|| {
                            error("no_speaker", minijinja::context! { label => &label })
                        })?;
                        Some(Parsed {
                            index,
                            recording: None,
                        })
                    }
                };
                let span = span_of(rec, &arg("from"), &arg("to"))?;
                let hits = (self.search)(&query, Options { speaker, span }, 6)?;
                if hits.is_empty() {
                    return Ok(Outcome {
                        text: prompts::render(
                            "results/recording/nothing.j2",
                            minijinja::context! { query },
                        ),
                        seen: Some(vec![]),
                    });
                }
                let shown: Vec<_> = fragments(rec, &hits)
                    .into_iter()
                    .map(|f| {
                        let who: Vec<String> = f.speakers.iter().map(|&s| rec.speaker(s)).collect();
                        minijinja::context! { start => format_time(f.start), end => format_time(f.end), who, text => &f.text }
                    })
                    .collect();
                let text = prompts::render(
                    "results/recording/search.j2",
                    minijinja::context! { hits => shown },
                );
                let seen = hits
                    .iter()
                    .map(|h| format!("at@{}", h.chunk.start as i64))
                    .collect();
                Ok(Outcome {
                    text: bounded(text, self.max_chars, "search"),
                    seen: Some(seen),
                })
            }
            "read" => {
                let (from, to) = (arg("from"), arg("to"));
                let a = parse_time(&from)
                    .ok_or_else(|| error("not_a_time", minijinja::context! { value => &from }))?;
                let b = parse_time(&to)
                    .ok_or_else(|| error("not_a_time", minijinja::context! { value => &to }))?;
                let (a, b) = (a.min(b), a.max(b));
                anyhow::ensure!(
                    a <= rec.duration,
                    error(
                        "recording_length",
                        minijinja::context! { length => format_time(rec.duration) }
                    )
                );
                let mut text = String::new();
                let mut seen = Vec::new();
                for r in rec.replies.iter().filter(|r| r.end >= a && r.start <= b) {
                    let l = line(rec, r);
                    if text.chars().count() + l.chars().count() > self.max_chars {
                        text.push_str(&prompts::render(
                            "results/recording/stopped.j2",
                            minijinja::context! { time => format_time(r.start) },
                        ));
                        break;
                    }
                    text.push_str(&l);
                    text.push('\n');
                    seen.push(format!("at@{}", r.start as i64));
                }
                if seen.is_empty() {
                    return Ok(Outcome {
                        text: prompts::render(
                            "results/recording/silence.j2",
                            minijinja::context! { start => format_time(a), end => format_time(b) },
                        ),
                        seen: Some(vec![]),
                    });
                }
                Ok(Outcome {
                    text,
                    seen: Some(seen),
                })
            }
            "mentions" => self.mentions(&arg("query")),
            "rename_speakers" => {
                let rename = self.rename.ok_or_else(|| error("cannot_rename", ()))?;
                let mut done = Vec::new();
                for item in args["names"].as_array().cloned().unwrap_or_default() {
                    let (label, name) = (
                        item["speaker"].as_str().unwrap_or_default(),
                        item["name"].as_str().unwrap_or_default().trim(),
                    );
                    let s = speaker_of(rec, label)
                        .ok_or_else(|| error("no_speaker", minijinja::context! { label }))?;
                    anyhow::ensure!(
                        !name.is_empty(),
                        error("empty_name", minijinja::context! { label })
                    );
                    rename(s, name)?;
                    done.push(minijinja::context! { speaker => rec.speaker(s), name });
                }
                anyhow::ensure!(!done.is_empty(), needs("rename_speakers", "names"));
                Ok(Outcome {
                    text: prompts::render(
                        "results/recording/named.j2",
                        minijinja::context! { done },
                    ),
                    seen: None,
                })
            }
            "scan" => {
                let instruction = arg("instruction");
                anyhow::ensure!(!instruction.is_empty(), needs("scan", "instruction"));
                let scan = self.scan.ok_or_else(|| error("no_scan", ()))?;
                let found = scan(&instruction)?;
                let seen = found
                    .facts
                    .iter()
                    .map(|f| format!("fact@{}:{}", f.time as i64, f.text))
                    .collect();
                Ok(Outcome {
                    text: bounded(found.text(&instruction), self.max_chars, "scan"),
                    seen: Some(seen),
                })
            }
            "speakers" => {
                if !rec.diarized {
                    return Ok(Outcome::plain(prompts::render(
                        "results/recording/no_speakers.j2",
                        (),
                    )));
                }
                let mut stats: BTreeMap<u32, (f64, usize, f64)> = BTreeMap::new();
                for r in &rec.replies {
                    if let Some(s) = r.speaker {
                        let e = stats.entry(s).or_insert((0.0, 0, r.start));
                        e.0 += r.end - r.start;
                        e.1 += 1;
                    }
                }
                let speakers: Vec<_> = stats
                    .iter()
                    .map(|(&s, &(talk, replies, first))| minijinja::context! { name => rec.speaker(s), talk => format_time(talk), replies, first => format_time(first) })
                    .collect();
                Ok(Outcome::plain(prompts::render(
                    "results/recording/speakers.j2",
                    minijinja::context! { speakers },
                )))
            }
            "calculator" => Ok(Outcome::plain(calculate(&arg("expression"))?)),
            "time" => {
                let (secs, clock) = between(&arg("from"), &arg("to"))?;
                Ok(Outcome::plain(prompts::render(
                    "results/recording/time.j2",
                    minijinja::context! { clock, seconds => secs.to_string() },
                )))
            }
            other => Err(error("no_tool", minijinja::context! { tool => other })),
        }
    }
}

const MENTIONS_LISTED: usize = 15;
const MENTION_CONTEXT: usize = 6;
const SHORT_HIT: usize = 80;

const EMPTY_WORDS: &[&str] = &[
    "она",
    "он",
    "они",
    "оно",
    "это",
    "этот",
    "эта",
    "эти",
    "тот",
    "та",
    "те",
    "того",
    "то",
    "тех",
    "тем",
    "такой",
    "такая",
    "такое",
    "такие",
    "его",
    "ее",
    "их",
    "ему",
    "ей",
    "им",
    "ими",
    "меня",
    "тебя",
    "нас",
    "вас",
    "себя",
    "себе",
    "собой",
    "весь",
    "все",
    "который",
    "которая",
    "которое",
    "которые",
    "которого",
    "которую",
    "которых",
    "что",
    "кто",
    "чей",
    "чья",
    "чье",
    "чьи",
    "там",
    "тут",
    "тогда",
    "так",
    "поэтому",
    "же",
    "ли",
    "бы",
    "и",
    "а",
    "но",
    "или",
    "да",
    "нет",
    "не",
    "ни",
    "на",
    "в",
    "во",
    "с",
    "со",
    "к",
    "ко",
    "о",
    "об",
    "обо",
    "от",
    "до",
    "по",
    "за",
    "из",
    "у",
    "для",
    "при",
    "про",
    "без",
    "через",
    "над",
    "под",
    "между",
    "чем",
    "кем",
    "ком",
    "когда",
    "где",
    "куда",
    "откуда",
    "почему",
    "зачем",
    "как",
    "сколько",
    "сказал",
    "сказала",
    "сказали",
    "сказать",
    "говорит",
    "говорил",
    "говорила",
    "говорили",
    "говорить",
    "she",
    "he",
    "it",
    "they",
    "them",
    "their",
    "his",
    "her",
    "hers",
    "this",
    "that",
    "these",
    "those",
    "what",
    "who",
    "whom",
    "whose",
    "which",
    "the",
    "a",
    "an",
    "and",
    "or",
    "but",
    "of",
    "to",
    "in",
    "on",
    "at",
    "about",
    "with",
    "for",
    "from",
    "is",
    "was",
    "were",
    "be",
    "been",
    "being",
    "do",
    "does",
    "did",
    "there",
    "here",
    "then",
    "so",
    "say",
    "says",
    "said",
    "tell",
    "tells",
    "told",
    "mention",
    "mentions",
    "mentioned",
    "talk",
    "talks",
    "talked",
    "discuss",
    "discusses",
    "discussed",
    "me",
    "my",
    "you",
    "your",
    "we",
    "us",
    "our",
    "i",
];

pub(super) fn vague(query: &str) -> bool {
    let words: Vec<String> = query
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase().replace('ё', "е"))
        .collect();
    !words.is_empty() && words.iter().all(|w| EMPTY_WORDS.contains(&w.as_str()))
}

fn span_of(rec: &Recording, from: &str, to: &str) -> anyhow::Result<Option<(f64, f64)>> {
    if from.is_empty() && to.is_empty() {
        return Ok(None);
    }
    let a = if from.is_empty() {
        0.0
    } else {
        parse_time(from)
            .ok_or_else(|| error("not_a_time", minijinja::context! { value => from }))?
    };
    let b = if to.is_empty() {
        rec.duration
    } else {
        parse_time(to).ok_or_else(|| error("not_a_time", minijinja::context! { value => to }))?
    };
    let (a, b) = (a.min(b), a.max(b));
    anyhow::ensure!(
        a <= rec.duration,
        error(
            "recording_length",
            minijinja::context! { length => format_time(rec.duration) }
        )
    );
    Ok(Some((a, b)))
}

#[derive(Debug, Clone, PartialEq)]
pub(super) struct Fragment {
    pub start: f64,
    pub end: f64,
    pub speakers: Vec<u32>,
    pub text: String,
}

struct Piece {
    start: f64,
    end: f64,
    speakers: Vec<u32>,
    text: String,
    replies: Option<(usize, usize)>,
}

fn reply_text(rec: &Recording, first: usize, last: usize) -> (Vec<u32>, String) {
    let mut speakers = Vec::new();
    let mut text = String::new();
    for r in &rec.replies[first..=last] {
        if let Some(s) = r.speaker {
            if !speakers.contains(&s) {
                speakers.push(s);
            }
        }
        let t = r.text();
        if t.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text.push_str(&t);
    }
    (speakers, text)
}

pub(super) fn fragments(rec: &Recording, hits: &[Hit]) -> Vec<Fragment> {
    let touches = |r: &Reply, h: &Hit| r.end >= h.chunk.start && r.start <= h.chunk.end;
    let mut pieces: Vec<Piece> = Vec::new();
    for h in hits {
        let short = h.chunk.text.chars().count() < SHORT_HIT;
        let first = rec.replies.iter().position(|r| touches(r, h));
        match (short, first) {
            (true, Some(first)) => {
                let last = rec
                    .replies
                    .iter()
                    .rposition(|r| touches(r, h))
                    .unwrap_or(first);
                let first = first.saturating_sub(1);
                let last = (last + 1).min(rec.replies.len() - 1);
                let (speakers, text) = reply_text(rec, first, last);
                pieces.push(Piece {
                    start: rec.replies[first].start,
                    end: rec.replies[last].end,
                    speakers,
                    text,
                    replies: Some((first, last)),
                });
            }
            _ => pieces.push(Piece {
                start: h.chunk.start,
                end: h.chunk.end,
                speakers: h.chunk.speakers.clone(),
                text: h.chunk.text.clone(),
                replies: None,
            }),
        }
    }
    pieces.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut out: Vec<Piece> = Vec::new();
    for p in pieces {
        let Some(cur) = out.last_mut().filter(|cur| p.start <= cur.end) else {
            out.push(p);
            continue;
        };
        cur.end = cur.end.max(p.end);
        match (cur.replies, p.replies) {
            (Some(a), Some(b)) => {
                let (first, last) = (a.0.min(b.0), a.1.max(b.1));
                let (speakers, text) = reply_text(rec, first, last);
                cur.start = cur.start.min(p.start);
                cur.speakers = speakers;
                cur.text = text;
                cur.replies = Some((first, last));
            }
            _ => {
                let (start, end) = (cur.start.min(p.start), cur.end.max(p.end));
                let first = rec
                    .replies
                    .iter()
                    .position(|r| r.end >= start && r.start <= end);
                let last = rec
                    .replies
                    .iter()
                    .rposition(|r| r.end >= start && r.start <= end);
                if let (Some(first), Some(last)) = (first, last) {
                    let (speakers, text) = reply_text(rec, first, last);
                    cur.start = rec.replies[first].start;
                    cur.end = rec.replies[last].end;
                    cur.speakers = speakers;
                    cur.text = text;
                    cur.replies = Some((first, last));
                    continue;
                }
                for s in p.speakers {
                    if !cur.speakers.contains(&s) {
                        cur.speakers.push(s);
                    }
                }
                cur.end = end;
                if !p.text.is_empty() && !cur.text.contains(&p.text) {
                    if !cur.text.is_empty() {
                        cur.text.push(' ');
                    }
                    cur.text.push_str(&p.text);
                }
                cur.replies = None;
            }
        }
    }
    out.into_iter()
        .map(|p| Fragment {
            start: p.start,
            end: p.end,
            speakers: p.speakers,
            text: p.text,
        })
        .collect()
}

fn same_stem(word: &str, query: &str) -> bool {
    word == query
        || (word.chars().count() >= 5
            && query.chars().count() >= 5
            && (word.starts_with(query) || query.starts_with(word)))
}

impl Tools<'_> {
    fn mentions(&self, query: &str) -> anyhow::Result<Outcome> {
        let q = stems(query);
        anyhow::ensure!(!q.is_empty(), needs("mentions", "word"));
        let rec = self.recording;
        let mut found = Vec::new();
        let mut times: Vec<f64> = Vec::new();
        for r in &rec.replies {
            let words: Vec<String> = r
                .words
                .iter()
                .map(|w| stems(&w.text).into_iter().next().unwrap_or_default())
                .collect();
            for i in 0..words.len() {
                if q.iter()
                    .enumerate()
                    .all(|(k, s)| words.get(i + k).is_some_and(|w| same_stem(w, s)))
                {
                    let t = r.words[i].start;
                    times.push(t);
                    if found.len() < MENTIONS_LISTED {
                        let from = i.saturating_sub(MENTION_CONTEXT);
                        let to = (i + q.len() + MENTION_CONTEXT).min(r.words.len());
                        let context = r.words[from..to]
                            .iter()
                            .map(|w| w.text.as_str())
                            .collect::<Vec<_>>()
                            .join(" ");
                        found.push(minijinja::context! { time => format_time(t), speaker => r.speaker.map(|s| rec.speaker(s)), context });
                    }
                }
            }
        }
        let (Some(first), Some(last)) = (times.first(), times.last()) else {
            return Ok(Outcome {
                text: prompts::render(
                    "results/recording/never_said.j2",
                    minijinja::context! { query },
                ),
                seen: Some(vec![]),
            });
        };
        let text = prompts::render(
            "results/recording/mentions.j2",
            minijinja::context! { query, count => times.len(), first => format_time(*first), last => format_time(*last), more => times.len() - found.len(), found },
        );
        let seen = times
            .iter()
            .map(|t| format!("mention@{}", *t as i64))
            .collect();
        Ok(Outcome {
            text: bounded(text, self.max_chars, "plain"),
            seen: Some(seen),
        })
    }
}

pub fn adds(known: &mut HashSet<String>, seen: &[String]) -> bool {
    let mut new = false;
    for s in seen {
        new |= known.insert(s.clone());
    }
    new
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use mediaa_core::outline::{Digest, Item, Quote, Section, SectionKind};
    use mediaa_core::retrieval::Chunk;
    use mediaa_core::transcript::Word;

    pub(crate) fn reply(speaker: u32, start: f64, text: &str) -> Reply {
        let words: Vec<Word> = text
            .split(' ')
            .enumerate()
            .map(|(i, w)| Word {
                text: w.into(),
                start: start + i as f64,
                end: start + i as f64 + 0.8,
            })
            .collect();
        Reply {
            start,
            end: words.last().unwrap().end,
            speaker: Some(speaker),
            words,
        }
    }

    pub(crate) fn recording() -> Recording {
        Recording {
            title: "Планёрка".into(),
            duration: 600.0,
            diarized: true,
            names: BTreeMap::from([(0, "Дина".into())]),
            replies: vec![
                reply(0, 10.0, "Отчёт отправляем в пятницу."),
                reply(1, 30.0, "Графики будут к четвергу."),
                reply(0, 300.0, "Бюджет на квартал утвердили."),
            ],
            chapters: vec![Chapter {
                start: 0.0,
                end: 600.0,
                digest: Digest {
                    title: "Отчёт и бюджет".into(),
                    summary: "Сроки отчёта, бюджет.".into(),
                    ..Digest::default()
                },
            }],
            position: Some(42.0),
        }
    }

    fn no_search(_: &str, _: Options, _: usize) -> anyhow::Result<Vec<Hit>> {
        Ok(vec![])
    }

    #[test]
    fn card_tells_what_is_there() {
        let card = recording().card();
        assert!(card.contains("«Планёрка», 10:00 long (medium)"), "{card}");
        assert!(
            card.contains("Speakers: @s1 (the user calls them Дина), @s2."),
            "{card}"
        );
        assert!(card.contains("1. [0:00–10:00] Отчёт и бюджет — Сроки отчёта, бюджет."));
        assert!(!card.contains("Outline:"));
        assert!(card.contains("at [0:42]"));
    }

    #[test]
    fn tools_answer_from_the_recording() {
        let rec = recording();
        let found = |q: &str, _: Options, _: usize| -> anyhow::Result<Vec<Hit>> {
            Ok(if q.contains("бюджет") {
                vec![Hit {
                    id: 1,
                    chunk: Chunk {
                        start: 300.0,
                        end: 330.0,
                        speakers: vec![0],
                        text: "Бюджет на квартал утвердили.".into(),
                    },
                    score: 1.0,
                }]
            } else {
                vec![]
            })
        };
        let tools = Tools::new(&rec, &found, 2000);
        let out = tools.run("outline", &json!({})).unwrap();
        assert!(out
            .text
            .contains("[0:00–10:00] Отчёт и бюджет — Сроки отчёта, бюджет."));
        let out = tools.run("search", &json!({"query": "бюджет"})).unwrap();
        assert!(
            out.text.starts_with(
                "[0:30–5:03] (@s2, @s1) Графики будут к четвергу. Бюджет на квартал утвердили."
            ),
            "{}",
            out.text
        );
        assert_eq!(out.seen, Some(vec!["at@300".into()]));
        assert!(tools
            .run("search", &json!({"query": "погода"}))
            .unwrap()
            .text
            .starts_with("Nothing found"));
        assert!(tools.run("search", &json!({})).is_err());
        let out = tools
            .run("read", &json!({"from": "0:00", "to": "1:00"}))
            .unwrap();
        assert_eq!(
            out.text,
            "[0:10] @s1: Отчёт отправляем в пятницу.\n[0:30] @s2: Графики будут к четвергу.\n"
        );
        assert!(tools
            .run("read", &json!({"from": "2:00", "to": "3:00"}))
            .unwrap()
            .text
            .starts_with("Nobody speaks"));
        assert!(tools
            .run("read", &json!({"from": "вчера", "to": "1:00"}))
            .is_err());
        assert!(
            tools
                .run("read", &json!({"from": "20:00", "to": "30:00"}))
                .is_err(),
            "past the end"
        );
        let out = tools.run("speakers", &json!({})).unwrap();
        assert!(
            out.text
                .starts_with("@s1: talks 0:07 in 2 replies, first at [0:10]"),
            "{}",
            out.text
        );
        assert_eq!(
            tools
                .run("calculator", &json!({"expression": "2+2*2"}))
                .unwrap()
                .text,
            "6"
        );
        assert_eq!(
            tools
                .run("time", &json!({"from": "0:10", "to": "5:00"}))
                .unwrap()
                .text,
            "4:50 (290 seconds)"
        );
        assert!(tools.run("dance", &json!({})).is_err());
    }

    #[test]
    fn a_short_hit_takes_the_replies_around_it() {
        let rec = recording();
        let one = |start: f64, end: f64, text: &str| Hit {
            id: 1,
            chunk: Chunk {
                start,
                end,
                speakers: vec![0],
                text: text.into(),
            },
            score: 1.0,
        };
        let out = fragments(&rec, &[one(300.0, 330.0, "Бюджет.")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].start, out[0].end), (30.0, 303.8));
        assert_eq!(out[0].speakers, [1, 0]);
        assert_eq!(
            out[0].text,
            "Графики будут к четвергу. Бюджет на квартал утвердили."
        );

        let long = "слово ".repeat(20);
        let out = fragments(&rec, &[one(100.0, 120.0, &long)]);
        assert_eq!(
            (out[0].start, out[0].end),
            (100.0, 120.0),
            "a long hit stays as it is"
        );
        assert_eq!(out[0].text, long);
    }

    #[test]
    fn overlapping_fragments_become_one() {
        let rec = recording();
        let one = |start: f64, end: f64, text: &str| Hit {
            id: 1,
            chunk: Chunk {
                start,
                end,
                speakers: vec![0],
                text: text.into(),
            },
            score: 1.0,
        };
        let out = fragments(&rec, &[one(10.0, 13.4, "Да."), one(30.0, 33.4, "Да.")]);
        assert_eq!(out.len(), 1, "{out:?}");
        assert_eq!((out[0].start, out[0].end), (10.0, 303.8));
        assert_eq!(out[0].speakers, [0, 1]);
        assert!(
            out[0]
                .text
                .starts_with("Отчёт отправляем в пятницу. Графики будут к четвергу.")
                && out[0].text.ends_with("Бюджет на квартал утвердили."),
            "{}",
            out[0].text
        );
    }

    #[test]
    fn a_vague_query_is_refused() {
        let rec = recording();
        let called = std::sync::Mutex::new(false);
        let search = |_: &str, _: Options, _: usize| {
            *called.lock().unwrap() = true;
            Ok(vec![])
        };
        let tools = Tools::new(&rec, &search, 4000);
        for q in ["она", "и это", "what about it"] {
            let e = tools
                .run("search", &json!({ "query": q }))
                .unwrap_err()
                .to_string();
            assert!(e.contains("does not say what to look for"), "{q}: {e}");
        }
        assert!(
            !*called.lock().unwrap(),
            "a vague query never reaches the index"
        );
        tools
            .run("search", &json!({ "query": "что решили по бюджету" }))
            .unwrap();
        assert!(*called.lock().unwrap());
    }

    #[test]
    fn search_options_reach_the_index_and_bad_ones_are_refused() {
        let rec = recording();
        let asked = std::sync::Mutex::new(Options::default());
        let search = |_: &str, o: Options, _: usize| {
            *asked.lock().unwrap() = o;
            Ok(vec![])
        };
        let tools = Tools::new(&rec, &search, 4000);
        tools
            .run(
                "search",
                &json!({ "query": "бюджет", "speaker": "@s2", "from": "0:30", "to": "5:00" }),
            )
            .unwrap();
        assert_eq!(
            asked.lock().unwrap().speaker,
            Some(Parsed {
                index: 1,
                recording: None
            })
        );
        assert_eq!(asked.lock().unwrap().span, Some((30.0, 300.0)));

        tools
            .run(
                "search",
                &json!({ "query": "бюджет", "from": "5:00", "to": "0:30" }),
            )
            .unwrap();
        assert_eq!(
            asked.lock().unwrap().span,
            Some((30.0, 300.0)),
            "from and to swap"
        );

        let e = tools
            .run(
                "search",
                &json!({ "query": "бюджет", "from": "20:00", "to": "21:00" }),
            )
            .unwrap_err()
            .to_string();
        assert!(e.contains("only 10:00"), "{e}");
        let e = tools
            .run("search", &json!({ "query": "бюджет", "speaker": "@s5" }))
            .unwrap_err()
            .to_string();
        assert!(e.contains("@s5"), "{e}");
        let e = tools
            .run("search", &json!({ "query": "бюджет", "from": "вчера" }))
            .unwrap_err()
            .to_string();
        assert!(e.contains("not a time"), "{e}");
    }

    #[test]
    fn outline_lists_section_titles_and_reads_one_topic() {
        let mut rec = recording();
        rec.chapters = vec![
            Chapter {
                start: 0.0,
                end: 300.0,
                digest: Digest {
                    title: "Отчёт".into(),
                    summary: "Сроки.".into(),
                    quotes: vec![Quote {
                        time: 10.0,
                        text: "В пятницу.".into(),
                    }],
                    sections: vec![
                        Section {
                            title: "Советы".into(),
                            kind: SectionKind::List,
                            items: vec![
                                Item {
                                    text: "Сдать в пятницу".into(),
                                    time: Some(10.0),
                                },
                                Item {
                                    text: "Без времени".into(),
                                    time: None,
                                },
                            ],
                        },
                        Section {
                            title: "Порядок".into(),
                            kind: SectionKind::Steps,
                            items: vec![Item {
                                text: "Сначала черновик".into(),
                                time: Some(30.0),
                            }],
                        },
                    ],
                    ..Digest::default()
                },
            },
            Chapter {
                start: 300.0,
                end: 600.0,
                digest: Digest {
                    title: "Бюджет".into(),
                    summary: "Цифры.".into(),
                    ..Digest::default()
                },
            },
        ];
        let spec = specs().into_iter().find(|s| s.name == "outline").unwrap();
        assert_eq!(spec.params["properties"]["topic"]["type"], "integer");
        assert!(spec.params["required"].as_array().is_none());
        assert!(spec.description.contains("topic"), "{}", spec.description);
        let tools = Tools::new(&rec, &no_search, 4000);
        let map = tools.run("outline", &json!({})).unwrap();
        assert_eq!(
            map.text,
            "[0:00–5:00] Отчёт — Сроки. (Советы; Порядок)\n[5:00–10:00] Бюджет — Цифры."
        );
        assert_eq!(
            map.seen,
            Some(vec!["chapter@0".into(), "chapter@300".into()])
        );
        assert_eq!(
            tools.run("outline", &json!({"topic": null})).unwrap().text,
            map.text
        );
        let topic = tools.run("outline", &json!({"topic": 1})).unwrap();
        assert_eq!(
            topic.text,
            "Breakdown of topic\n1. [0:00–5:00] Отчёт — Сроки.\nСоветы (list)\n[0:10] Сдать в пятницу\nБез времени\nПорядок (steps)\n[0:30] Сначала черновик\n[0:10] В пятницу."
        );
        assert_eq!(topic.seen, Some(vec!["topic@1".into()]));
        let note = tools.run("outline", &json!({"topic": 2})).unwrap();
        assert_eq!(
            note.text.trim_end(),
            "Topic 2 «Бюджет» has no breakdown yet: use search and read within that topic."
        );
        assert_eq!(note.seen, Some(vec!["topic@2".into()]));
        let missing = tools
            .run("outline", &json!({"topic": 9}))
            .unwrap_err()
            .to_string();
        assert!(missing.contains('9') && missing.contains('2'), "{missing}");
        assert!(tools.run("outline", &json!({"topic": "нет"})).is_err());
    }

    #[test]
    fn mentions_are_counted_by_code() {
        let mut rec = recording();
        rec.replies.push(reply(
            1,
            400.0,
            "Отчёты сдаём по пятницам, а отчётов много.",
        ));
        let tools = Tools::new(&rec, &no_search, 4000);
        let out = tools.run("mentions", &json!({"query": "отчёт"})).unwrap();
        assert!(
            out.text
                .starts_with("«отчёт»: 3 mentions, first at [0:10], last at [6:45]."),
            "{}",
            out.text
        );
        assert!(
            out.text
                .contains("[0:10] @s1: …Отчёт отправляем в пятницу.…"),
            "{}",
            out.text
        );
        assert!(out.text.contains("[6:40] @s2: …Отчёты"), "{}", out.text);
        assert_eq!(out.seen.as_ref().map(Vec::len), Some(3));
        let out = tools
            .run("mentions", &json!({"query": "бюджет на квартал"}))
            .unwrap();
        assert!(
            out.text
                .starts_with("«бюджет на квартал»: 1 mentions, first at [5:00]"),
            "{}",
            out.text
        );
        assert!(tools
            .run("mentions", &json!({"query": "погода"}))
            .unwrap()
            .text
            .contains("never said"));
        assert!(tools.run("mentions", &json!({"query": "!!"})).is_err());
    }

    #[test]
    fn speakers_get_names() {
        let rec = recording();
        assert_eq!(speaker_of(&rec, "@s1"), Some(0));
        assert_eq!(speaker_of(&rec, "s2"), Some(1));
        assert_eq!(
            speaker_of(&rec, "@s2#3"),
            Some(1),
            "the recording number is ignored on one recording"
        );
        assert_eq!(speaker_of(&rec, "Дина"), None, "a name is not a label");
        assert_eq!(speaker_of(&rec, "@s9"), None, "no such speaker");
        let named = std::sync::Mutex::new(Vec::new());
        let rename = |s: u32, n: &str| {
            named.lock().unwrap().push((s, n.to_string()));
            Ok(())
        };
        let tools = Tools {
            rename: Some(&rename),
            ..Tools::new(&rec, &no_search, 4000)
        };
        let names: Vec<&str> = tools.specs().iter().map(|s| s.name).collect();
        assert!(names.contains(&"rename_speakers") && !names.contains(&"scan"));
        let out = tools
            .run(
                "rename_speakers",
                &json!({"names": [{"speaker": "@s2", "name": " Михаил "}]}),
            )
            .unwrap();
        assert_eq!(out.text, "Named: @s2 → Михаил.");
        assert_eq!(*named.lock().unwrap(), [(1, "Михаил".to_string())]);
        assert!(tools
            .run(
                "rename_speakers",
                &json!({"names": [{"speaker": "@s9", "name": "X"}]})
            )
            .is_err());
        let plain = Recording {
            diarized: false,
            ..recording()
        };
        let tools = Tools {
            rename: Some(&rename),
            ..Tools::new(&plain, &no_search, 4000)
        };
        assert!(!tools.specs().iter().any(|s| s.name == "rename_speakers"));
    }

    #[test]
    fn long_results_are_cut() {
        let rec = Recording {
            replies: (0..50)
                .map(|i| reply(0, i as f64 * 10.0, "слово слово слово слово"))
                .collect(),
            duration: 600.0,
            ..recording()
        };
        let tools = Tools::new(&rec, &no_search, 200);
        let out = tools
            .run("read", &json!({"from": "0:00", "to": "10:00"}))
            .unwrap();
        assert!(out.text.contains("read on from ["), "{}", out.text);
        assert!(out.text.chars().count() < 320);
    }

    #[test]
    fn novelty() {
        let mut known = HashSet::new();
        assert!(adds(&mut known, &["a".into(), "b".into()]));
        assert!(!adds(&mut known, &["b".into()]));
        assert!(adds(&mut known, &["b".into(), "c".into()]));
    }
}
