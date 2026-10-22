//! The agent's tools over one recording, run by code: the outline, search, reading a time range,
//! speakers (talk time computed, never guessed), and exact arithmetic and time math. Results are
//! text for the model, bounded by its context; each says what it adds, for the novelty check.

use crate::prompts;
use super::brain::ToolSpec;
use crate::tools::{between, calculate, format_time, parse_time};
use mediaa_core::outline::{Chapter, SectionKind};
use mediaa_core::retrieval::{stems, Hit};
use mediaa_core::transcript::Reply;
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashSet};

/// What the agent knows about the recording it talks about.
#[derive(Debug, Clone, Default)]
pub struct Recording {
    pub title: String,
    pub duration: f64,
    /// Speakers were told apart.
    pub diarized: bool,
    pub names: BTreeMap<u32, String>,
    pub replies: Vec<Reply>,
    pub chapters: Vec<Chapter>,
    /// Where the user is in playback (seconds).
    pub position: Option<f64>,
}

impl Recording {
    /// The label the model uses for a speaker. Names stay in `names` and are shown only in the UI.
    pub fn speaker(&self, s: u32) -> String {
        mediaa_core::speakers::label(s)
    }
}

/// Speakers the user can mention: one recording, or one recording of a folder (`number` is 1-based).
pub fn whos(rec: &Recording, number: Option<u32>) -> Vec<mediaa_core::speakers::Who> {
    let mut ids: Vec<u32> = rec.replies.iter().filter_map(|r| r.speaker).collect();
    ids.extend(rec.names.keys().copied());
    ids.sort_unstable();
    ids.dedup();
    let title = if number.is_some() { rec.title.clone() } else { String::new() };
    ids.into_iter()
        .map(|index| mediaa_core::speakers::Who { index, recording: number, name: rec.names.get(&index).cloned().unwrap_or_default(), title: title.clone() })
        .collect()
}

impl Recording {

    /// The card at the top of the context.
    pub fn card(&self) -> String {
        let mut ids: Vec<u32> = self.replies.iter().filter_map(|r| r.speaker).collect();
        ids.sort_unstable();
        ids.dedup();
        let speakers: Vec<String> = if self.diarized { ids.iter().map(|&s| self.speaker(s)).collect() } else { Vec::new() };
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

/// The tools the agent has for a recording.
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
            params: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
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

/// A tool's result: text for the model and the keys of what it showed (for novelty).
#[derive(Debug, Clone, PartialEq)]
pub struct Outcome {
    pub text: String,
    /// Places / facts shown (`None`: the tool has no notion of new information).
    pub seen: Option<Vec<String>>,
}

impl Outcome {
    fn plain(text: impl Into<String>) -> Outcome {
        Outcome { text: text.into(), seen: None }
    }
}

/// Text cut to `max` characters with a note (the template `results/cut/<note>.j2`).
pub(super) fn bounded(mut text: String, max: usize, note: &str) -> String {
    if text.chars().count() > max {
        text = text.chars().take(max).collect();
        text.push_str(&format!("\n… {}", prompts::render(&format!("results/cut/{note}.j2"), ())));
    }
    text
}

/// A tool's error text for the model: the template `errors/<name>.j2`.
pub(super) fn error(name: &str, ctx: impl serde::Serialize) -> anyhow::Error {
    anyhow::anyhow!(prompts::render(&format!("errors/{name}.j2"), ctx))
}

/// The error of a tool called without a required argument (`errors/needs_<what>.j2`).
pub(super) fn needs(tool: &str, what: &str) -> anyhow::Error {
    error(&format!("needs_{what}"), minijinja::context! { tool })
}

fn line(rec: &Recording, r: &Reply) -> String {
    prompts::render("results/recording/line.j2", minijinja::context! { time => format_time(r.start), speaker => r.speaker.map(|s| rec.speaker(s)), text => r.text() })
}

/// The focused scan of a recording: instruction → what matches, with coverage.
pub type Scanner<'a> = dyn Fn(&str) -> anyhow::Result<super::scan::Scan> + 'a;

fn section_kind(kind: SectionKind) -> &'static str {
    match kind {
        SectionKind::List => "list",
        SectionKind::Steps => "steps",
        SectionKind::Text => "text",
    }
}

/// The topic a call asks for (1-based). `Ok(None)` is the map. Anything else is an error the
/// model can answer by picking a number that exists.
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
        _ => Err(error("no_topic", minijinja::context! { topic => raw, count })),
    }
}

/// Runs the tools of a recording.
pub struct Tools<'a> {
    pub recording: &'a Recording,
    /// Hybrid search over the recording: (query, how many) → hits.
    pub search: &'a dyn Fn(&str, usize) -> anyhow::Result<Vec<Hit>>,
    /// Longest result (characters), from the model's context.
    pub max_chars: usize,
    /// The focused scan of the whole recording for an instruction, when the model can do it.
    pub scan: Option<&'a Scanner<'a>>,
    /// Gives a speaker a name (kept with the transcript).
    pub rename: Option<&'a Renamer<'a>>,
    /// A bounded sub-task done by the sub-agent: (task, the text it works on) → its result.
    pub delegate: Option<&'a Delegate<'a>>,
    /// The web, when the agent may use it.
    pub web: Option<&'a super::internet::Web<'a>>,
}

/// Names a speaker of the recording.
pub type Renamer<'a> = dyn Fn(u32, &str) -> anyhow::Result<()> + 'a;
/// Does a sub-task on a piece of the recording.
pub type Delegate<'a> = dyn Fn(&str, &str) -> anyhow::Result<String> + 'a;

impl<'a> Tools<'a> {
    /// The tools of reading a recording (no scan, renaming or delegation).
    pub fn new(recording: &'a Recording, search: &'a dyn Fn(&str, usize) -> anyhow::Result<Vec<Hit>>, max_chars: usize) -> Tools<'a> {
        Tools { recording, search, max_chars, scan: None, rename: None, delegate: None, web: None }
    }
}

/// The speaker a model label means (`@sN`, `sN`, `@sN#R`). The recording number is ignored:
/// the tool runs on one recording. A person's name is not a label.
pub fn speaker_of(recording: &Recording, label: &str) -> Option<u32> {
    let s = mediaa_core::speakers::parse_label(label)?.index;
    (recording.replies.iter().any(|r| r.speaker == Some(s)) || recording.names.contains_key(&s)).then_some(s)
}

impl Tools<'_> {
    /// The tools offered: all of them, the scan when there is one.
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
        if self.delegate.is_some() {
            s.push(ToolSpec {
                name: "delegate",
                description: prompts::tool("delegate"),
                params: json!({"type": "object", "properties": {"task": {"type": "string"}, "from": {"type": "string"}, "to": {"type": "string"}}, "required": ["task", "from", "to"]}),
                example: json!({"task": "<what to do with this stretch>", "from": "40:00", "to": "55:00"}),
            });
        }
        s
    }

    pub fn run(&self, tool: &str, args: &Value) -> anyhow::Result<Outcome> {
        let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
        let rec = self.recording;
        match tool {
            "outline" => {
                if rec.chapters.is_empty() {
                    return Ok(Outcome::plain(prompts::render("results/recording/no_outline.j2", ())));
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
                    let text = prompts::render("results/recording/outline.j2", minijinja::context! { chapters });
                    let seen = rec.chapters.iter().map(|c| format!("chapter@{}", c.start as i64)).collect();
                    return Ok(Outcome { text: bounded(text, self.max_chars, "chapters"), seen: Some(seen) });
                };
                let c = &rec.chapters[n - 1];
                let seen = Some(vec![format!("topic@{n}")]);
                if c.digest.sections.is_empty() {
                    let text = prompts::render("results/recording/topic_empty.j2", minijinja::context! { number => n, title => &c.digest.title });
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
                let quotes: Vec<_> = c.digest.quotes.iter().map(|q| minijinja::context! { time => format_time(q.time), text => &q.text }).collect();
                let text = prompts::render(
                    "results/recording/topic.j2",
                    minijinja::context! { number => n, start => format_time(c.start), end => format_time(c.end), title => &c.digest.title, summary => &c.digest.summary, sections, quotes },
                );
                Ok(Outcome { text: bounded(text, self.max_chars, "plain"), seen })
            }
            "search" => {
                let query = arg("query");
                anyhow::ensure!(!query.is_empty(), needs("search", "query"));
                let hits = (self.search)(&query, 6)?;
                if hits.is_empty() {
                    return Ok(Outcome { text: prompts::render("results/recording/nothing.j2", minijinja::context! { query }), seen: Some(vec![]) });
                }
                let shown: Vec<_> = hits
                    .iter()
                    .map(|h| {
                        let who: Vec<String> = h.chunk.speakers.iter().map(|&s| rec.speaker(s)).collect();
                        minijinja::context! { start => format_time(h.chunk.start), end => format_time(h.chunk.end), who, text => &h.chunk.text }
                    })
                    .collect();
                let text = prompts::render("results/recording/search.j2", minijinja::context! { hits => shown });
                let seen = hits.iter().map(|h| format!("at@{}", h.chunk.start as i64)).collect();
                Ok(Outcome { text: bounded(text, self.max_chars, "search"), seen: Some(seen) })
            }
            "read" => {
                let (from, to) = (arg("from"), arg("to"));
                let a = parse_time(&from).ok_or_else(|| error("not_a_time", minijinja::context! { value => &from }))?;
                let b = parse_time(&to).ok_or_else(|| error("not_a_time", minijinja::context! { value => &to }))?;
                let (a, b) = (a.min(b), a.max(b));
                anyhow::ensure!(a <= rec.duration, error("recording_length", minijinja::context! { length => format_time(rec.duration) }));
                let mut text = String::new();
                let mut seen = Vec::new();
                for r in rec.replies.iter().filter(|r| r.end >= a && r.start <= b) {
                    let l = line(rec, r);
                    if text.chars().count() + l.chars().count() > self.max_chars {
                        text.push_str(&prompts::render("results/recording/stopped.j2", minijinja::context! { time => format_time(r.start) }));
                        break;
                    }
                    text.push_str(&l);
                    text.push('\n');
                    seen.push(format!("at@{}", r.start as i64));
                }
                if seen.is_empty() {
                    return Ok(Outcome { text: prompts::render("results/recording/silence.j2", minijinja::context! { start => format_time(a), end => format_time(b) }), seen: Some(vec![]) });
                }
                Ok(Outcome { text, seen: Some(seen) })
            }
            "mentions" => self.mentions(&arg("query")),
            "rename_speakers" => {
                let rename = self.rename.ok_or_else(|| error("cannot_rename", ()))?;
                let mut done = Vec::new();
                for item in args["names"].as_array().cloned().unwrap_or_default() {
                    let (label, name) = (item["speaker"].as_str().unwrap_or_default(), item["name"].as_str().unwrap_or_default().trim());
                    let s = speaker_of(rec, label).ok_or_else(|| error("no_speaker", minijinja::context! { label }))?;
                    anyhow::ensure!(!name.is_empty(), error("empty_name", minijinja::context! { label }));
                    rename(s, name)?;
                    done.push(minijinja::context! { speaker => rec.speaker(s), name });
                }
                anyhow::ensure!(!done.is_empty(), needs("rename_speakers", "names"));
                Ok(Outcome { text: prompts::render("results/recording/named.j2", minijinja::context! { done }), seen: None })
            }
            "delegate" => {
                let delegate = self.delegate.ok_or_else(|| error("no_helper", ()))?;
                let task = arg("task");
                anyhow::ensure!(!task.is_empty(), needs("delegate", "task"));
                // The helper reads the stretch whole (bounded by its own context, not yours).
                let stretch = Tools { max_chars: self.max_chars * 4, ..Tools::new(rec, self.search, self.max_chars) }.run("read", args)?;
                let result = delegate(&task, &stretch.text)?;
                Ok(Outcome { text: bounded(result, self.max_chars, "plain"), seen: None })
            }
            "scan" => {
                let instruction = arg("instruction");
                anyhow::ensure!(!instruction.is_empty(), needs("scan", "instruction"));
                let scan = self.scan.ok_or_else(|| error("no_scan", ()))?;
                let found = scan(&instruction)?;
                let seen = found.facts.iter().map(|f| format!("fact@{}:{}", f.time as i64, f.text)).collect();
                Ok(Outcome { text: bounded(found.text(&instruction), self.max_chars, "scan"), seen: Some(seen) })
            }
            "speakers" => {
                if !rec.diarized {
                    return Ok(Outcome::plain(prompts::render("results/recording/no_speakers.j2", ())));
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
                Ok(Outcome::plain(prompts::render("results/recording/speakers.j2", minijinja::context! { speakers })))
            }
            "calculator" => Ok(Outcome::plain(calculate(&arg("expression"))?)),
            "time" => {
                let (secs, clock) = between(&arg("from"), &arg("to"))?;
                Ok(Outcome::plain(prompts::render("results/recording/time.j2", minijinja::context! { clock, seconds => secs.to_string() })))
            }
            other => Err(error("no_tool", minijinja::context! { tool => other })),
        }
    }
}

/// Mentions listed with their context (the count covers all).
const MENTIONS_LISTED: usize = 15;
/// Words of context on each side of a mention.
const MENTION_CONTEXT: usize = 6;

/// Whether a word's stem matches a query stem (equal, or one begins the other when long).
fn same_stem(word: &str, query: &str) -> bool {
    word == query || (word.chars().count() >= 5 && query.chars().count() >= 5 && (word.starts_with(query) || query.starts_with(word)))
}

impl Tools<'_> {
    /// Every place the words of `query` are said in a row (by stems).
    fn mentions(&self, query: &str) -> anyhow::Result<Outcome> {
        let q = stems(query);
        anyhow::ensure!(!q.is_empty(), needs("mentions", "word"));
        let rec = self.recording;
        let mut found = Vec::new();
        let mut times: Vec<f64> = Vec::new();
        for r in &rec.replies {
            let words: Vec<String> = r.words.iter().map(|w| stems(&w.text).into_iter().next().unwrap_or_default()).collect();
            for i in 0..words.len() {
                if q.iter().enumerate().all(|(k, s)| words.get(i + k).is_some_and(|w| same_stem(w, s))) {
                    let t = r.words[i].start;
                    times.push(t);
                    if found.len() < MENTIONS_LISTED {
                        let from = i.saturating_sub(MENTION_CONTEXT);
                        let to = (i + q.len() + MENTION_CONTEXT).min(r.words.len());
                        let context = r.words[from..to].iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
                        found.push(minijinja::context! { time => format_time(t), speaker => r.speaker.map(|s| rec.speaker(s)), context });
                    }
                }
            }
        }
        let (Some(first), Some(last)) = (times.first(), times.last()) else {
            return Ok(Outcome { text: prompts::render("results/recording/never_said.j2", minijinja::context! { query }), seen: Some(vec![]) });
        };
        let text = prompts::render(
            "results/recording/mentions.j2",
            minijinja::context! { query, count => times.len(), first => format_time(*first), last => format_time(*last), more => times.len() - found.len(), found },
        );
        let seen = times.iter().map(|t| format!("mention@{}", *t as i64)).collect();
        Ok(Outcome { text: bounded(text, self.max_chars, "plain"), seen: Some(seen) })
    }
}

/// Whether `seen` adds anything to `known` (and remembers it).
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
        let words: Vec<Word> = text.split(' ').enumerate().map(|(i, w)| Word { text: w.into(), start: start + i as f64, end: start + i as f64 + 0.8 }).collect();
        Reply { start, end: words.last().unwrap().end, speaker: Some(speaker), words }
    }

    pub(crate) fn recording() -> Recording {
        Recording {
            title: "Планёрка".into(),
            duration: 600.0,
            diarized: true,
            names: BTreeMap::from([(0, "Дана".into())]),
            replies: vec![
                reply(0, 10.0, "Отчёт отправляем в пятницу."),
                reply(1, 30.0, "Графики будут к четвергу."),
                reply(0, 300.0, "Бюджет на квартал утвердили."),
            ],
            chapters: vec![Chapter { start: 0.0, end: 600.0, digest: Digest { title: "Отчёт и бюджет".into(), summary: "Сроки отчёта, бюджет.".into(), ..Digest::default() } }],
            position: Some(42.0),
        }
    }

    fn no_search(_: &str, _: usize) -> anyhow::Result<Vec<Hit>> {
        Ok(vec![])
    }

    #[test]
    fn card_tells_what_is_there() {
        let card = recording().card();
        assert!(card.contains("«Планёрка», 10:00 long (medium)"), "{card}");
        assert!(card.contains("Speakers: @s1, @s2."));
        assert!(card.contains("1. [0:00–10:00] Отчёт и бюджет — Сроки отчёта, бюджет."));
        assert!(!card.contains("Outline:"));
        assert!(card.contains("at [0:42]"));
    }

    #[test]
    fn tools_answer_from_the_recording() {
        let rec = recording();
        let found = |q: &str, _: usize| -> anyhow::Result<Vec<Hit>> {
            Ok(if q.contains("бюджет") { vec![Hit { id: 1, chunk: Chunk { start: 300.0, end: 330.0, speakers: vec![0], text: "Бюджет на квартал утвердили.".into() }, score: 1.0 }] } else { vec![] })
        };
        let tools = Tools::new(&rec, &found, 2000);
        let out = tools.run("outline", &json!({})).unwrap();
        assert!(out.text.contains("[0:00–10:00] Отчёт и бюджет — Сроки отчёта, бюджет."));
        let out = tools.run("search", &json!({"query": "бюджет"})).unwrap();
        assert!(out.text.starts_with("[5:00–5:30] (@s1) Бюджет"), "{}", out.text);
        assert_eq!(out.seen, Some(vec!["at@300".into()]));
        assert!(tools.run("search", &json!({"query": "погода"})).unwrap().text.starts_with("Nothing found"));
        assert!(tools.run("search", &json!({})).is_err());
        let out = tools.run("read", &json!({"from": "0:00", "to": "1:00"})).unwrap();
        assert_eq!(out.text, "[0:10] @s1: Отчёт отправляем в пятницу.\n[0:30] @s2: Графики будут к четвергу.\n");
        assert!(tools.run("read", &json!({"from": "2:00", "to": "3:00"})).unwrap().text.starts_with("Nobody speaks"));
        assert!(tools.run("read", &json!({"from": "вчера", "to": "1:00"})).is_err());
        assert!(tools.run("read", &json!({"from": "20:00", "to": "30:00"})).is_err(), "past the end");
        let out = tools.run("speakers", &json!({})).unwrap();
        assert!(out.text.starts_with("@s1: talks 0:07 in 2 replies, first at [0:10]"), "{}", out.text);
        assert_eq!(tools.run("calculator", &json!({"expression": "2+2*2"})).unwrap().text, "6");
        assert_eq!(tools.run("time", &json!({"from": "0:10", "to": "5:00"})).unwrap().text, "4:50 (290 seconds)");
        assert!(tools.run("dance", &json!({})).is_err());
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
                    quotes: vec![Quote { time: 10.0, text: "В пятницу.".into() }],
                    sections: vec![
                        Section {
                            title: "Советы".into(),
                            kind: SectionKind::List,
                            items: vec![
                                Item { text: "Сдать в пятницу".into(), time: Some(10.0) },
                                Item { text: "Без времени".into(), time: None },
                            ],
                        },
                        Section {
                            title: "Порядок".into(),
                            kind: SectionKind::Steps,
                            items: vec![Item { text: "Сначала черновик".into(), time: Some(30.0) }],
                        },
                    ],
                    ..Digest::default()
                },
            },
            Chapter { start: 300.0, end: 600.0, digest: Digest { title: "Бюджет".into(), summary: "Цифры.".into(), ..Digest::default() } },
        ];
        let spec = specs().into_iter().find(|s| s.name == "outline").unwrap();
        assert_eq!(spec.params["properties"]["topic"]["type"], "integer");
        assert!(spec.params["required"].as_array().is_none());
        assert!(spec.description.contains("topic"), "{}", spec.description);
        let tools = Tools::new(&rec, &no_search, 4000);
        let map = tools.run("outline", &json!({})).unwrap();
        assert_eq!(map.text, "[0:00–5:00] Отчёт — Сроки. (Советы; Порядок)\n[5:00–10:00] Бюджет — Цифры.");
        assert_eq!(map.seen, Some(vec!["chapter@0".into(), "chapter@300".into()]));
        assert_eq!(tools.run("outline", &json!({"topic": null})).unwrap().text, map.text);
        let topic = tools.run("outline", &json!({"topic": 1})).unwrap();
        assert_eq!(
            topic.text,
            "Breakdown of topic\n1. [0:00–5:00] Отчёт — Сроки.\nСоветы (list)\n[0:10] Сдать в пятницу\nБез времени\nПорядок (steps)\n[0:30] Сначала черновик\n[0:10] В пятницу."
        );
        assert_eq!(topic.seen, Some(vec!["topic@1".into()]));
        let note = tools.run("outline", &json!({"topic": 2})).unwrap();
        assert_eq!(note.text.trim_end(), "Topic 2 «Бюджет» has no breakdown yet: use search and read within that topic.");
        assert_eq!(note.seen, Some(vec!["topic@2".into()]));
        let missing = tools.run("outline", &json!({"topic": 9})).unwrap_err().to_string();
        assert!(missing.contains('9') && missing.contains('2'), "{missing}");
        assert!(tools.run("outline", &json!({"topic": "нет"})).is_err());
    }

    #[test]
    fn mentions_are_counted_by_code() {
        let mut rec = recording();
        rec.replies.push(reply(1, 400.0, "Отчёты сдаём по пятницам, а отчётов много."));
        let tools = Tools::new(&rec, &no_search, 4000);
        let out = tools.run("mentions", &json!({"query": "отчёт"})).unwrap();
        assert!(out.text.starts_with("«отчёт»: 3 mentions, first at [0:10], last at [6:45]."), "{}", out.text);
        assert!(out.text.contains("[0:10] @s1: …Отчёт отправляем в пятницу.…"), "{}", out.text);
        assert!(out.text.contains("[6:40] @s2: …Отчёты"), "{}", out.text);
        assert_eq!(out.seen.as_ref().map(Vec::len), Some(3));
        // A phrase: its words in a row.
        let out = tools.run("mentions", &json!({"query": "бюджет на квартал"})).unwrap();
        assert!(out.text.starts_with("«бюджет на квартал»: 1 mentions, first at [5:00]"), "{}", out.text);
        assert!(tools.run("mentions", &json!({"query": "погода"})).unwrap().text.contains("never said"));
        assert!(tools.run("mentions", &json!({"query": "!!"})).is_err());
    }

    #[test]
    fn speakers_get_names_and_stretches_go_to_the_helper() {
        let rec = recording();
        assert_eq!(speaker_of(&rec, "@s1"), Some(0));
        assert_eq!(speaker_of(&rec, "s2"), Some(1));
        assert_eq!(speaker_of(&rec, "@s2#3"), Some(1), "the recording number is ignored on one recording");
        assert_eq!(speaker_of(&rec, "Дана"), None, "a name is not a label");
        assert_eq!(speaker_of(&rec, "@s9"), None, "no such speaker");
        let named = std::cell::RefCell::new(Vec::new());
        let rename = |s: u32, n: &str| {
            named.borrow_mut().push((s, n.to_string()));
            Ok(())
        };
        let seen = std::cell::RefCell::new(String::new());
        let delegate = |task: &str, text: &str| {
            *seen.borrow_mut() = format!("{task}|{text}");
            Ok("Итог: про пятницу [0:10].".to_string())
        };
        let tools = Tools { rename: Some(&rename), delegate: Some(&delegate), ..Tools::new(&rec, &no_search, 4000) };
        let names: Vec<&str> = tools.specs().iter().map(|s| s.name).collect();
        assert!(names.contains(&"rename_speakers") && names.contains(&"delegate") && !names.contains(&"scan"));
        let out = tools.run("rename_speakers", &json!({"names": [{"speaker": "@s2", "name": " Олег "}]})).unwrap();
        assert_eq!(out.text, "Named: @s2 → Олег.");
        assert_eq!(*named.borrow(), [(1, "Олег".to_string())]);
        assert!(tools.run("rename_speakers", &json!({"names": [{"speaker": "@s9", "name": "X"}]})).is_err());
        let out = tools.run("delegate", &json!({"task": "перескажи", "from": "0:00", "to": "1:00"})).unwrap();
        assert_eq!(out.text, "Итог: про пятницу [0:10].");
        assert!(seen.borrow().starts_with("перескажи|[0:10] @s1: Отчёт"), "{}", seen.borrow());
        // Without diarization there is nobody to name.
        let plain = Recording { diarized: false, ..recording() };
        let tools = Tools { rename: Some(&rename), ..Tools::new(&plain, &no_search, 4000) };
        assert!(!tools.specs().iter().any(|s| s.name == "rename_speakers"));
    }

    #[test]
    fn long_results_are_cut() {
        let rec = Recording { replies: (0..50).map(|i| reply(0, i as f64 * 10.0, "слово слово слово слово")).collect(), duration: 600.0, ..recording() };
        let tools = Tools::new(&rec, &no_search, 200);
        let out = tools.run("read", &json!({"from": "0:00", "to": "10:00"})).unwrap();
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
