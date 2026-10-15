//! The agent's tools over one recording, run by code: the outline, search, reading a time range,
//! speakers (talk time computed, never guessed), and exact arithmetic and time math. Results are
//! text for the model, bounded by its context; each says what it adds, for the novelty check.

use super::brain::ToolSpec;
use crate::tools::{between, calculate, format_time, parse_time};
use mediaa_core::outline::Chapter;
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
    /// The language of the recording (from its first replies).
    pub fn language(&self) -> &'static str {
        mediaa_core::outline::language(&self.replies.iter().take(20).map(Reply::text).collect::<Vec<_>>().join(" "))
    }

    pub fn speaker(&self, s: u32) -> String {
        self.names.get(&s).cloned().unwrap_or_else(|| mediaa_core::outline::unnamed_speaker(self.language(), s))
    }

    /// The card at the top of the context.
    pub fn card(&self) -> String {
        let minutes = self.duration / 60.0;
        let size = if minutes < 10.0 {
            "short"
        } else if minutes < 60.0 {
            "medium"
        } else {
            "long"
        };
        let speakers: Vec<u32> = {
            let mut s: Vec<u32> = self.replies.iter().filter_map(|r| r.speaker).collect();
            s.sort_unstable();
            s.dedup();
            s
        };
        let mut card = format!("Recording: «{}», {} long ({size}).\n", self.title, format_time(self.duration));
        if self.diarized && !speakers.is_empty() {
            card.push_str(&format!("Speakers: {}.\n", speakers.iter().map(|&s| self.speaker(s)).collect::<Vec<_>>().join(", ")));
        } else {
            card.push_str("Speakers were not told apart.\n");
        }
        card.push_str(&match self.chapters.len() {
            0 => "No outline yet: use search and read.\n".to_string(),
            n => format!("Outline: {n} chapters (tool outline).\n"),
        });
        if let Some(p) = self.position {
            card.push_str(&format!("The user is at [{}] in playback.\n", format_time(p)));
        }
        card
    }
}

/// The tools the agent has for a recording.
pub fn specs() -> Vec<ToolSpec> {
    let none = json!({"type": "object", "properties": {}});
    vec![
        ToolSpec {
            name: "outline",
            description: "Chapters of the recording with times, titles and summaries: the map of the recording.",
            params: none.clone(),
            example: json!({}),
        },
        ToolSpec {
            name: "search",
            description: "Finds the places in the recording that talk about something (by meaning and by words; the recording only, not the internet). Returns fragments with times.",
            params: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            example: json!({"query": "сроки сдачи отчёта"}),
        },
        ToolSpec {
            name: "read",
            description: "The exact replies between two times (m:ss or h:mm:ss).",
            params: json!({"type": "object", "properties": {"from": {"type": "string"}, "to": {"type": "string"}}, "required": ["from", "to"]}),
            example: json!({"from": "12:00", "to": "15:30"}),
        },
        ToolSpec {
            name: "mentions",
            description: "Every time a word or phrase is said (any word form): how many times, exact times of the first and the last, each with its context. For «when first», «how often», «every time».",
            params: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
            example: json!({"query": "самозанятость"}),
        },
        ToolSpec {
            name: "speakers",
            description: "Who speaks: names, talk time, number of replies, when they first speak.",
            params: none,
            example: json!({}),
        },
        ToolSpec {
            name: "calculator",
            description: "Exact arithmetic: + - * / % ^, parentheses, sqrt, round, min, max.",
            params: json!({"type": "object", "properties": {"expression": {"type": "string"}}, "required": ["expression"]}),
            example: json!({"expression": "(45 - 12) / 3"}),
        },
        ToolSpec {
            name: "time",
            description: "The duration between two times of the recording.",
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

/// Text cut to `max` characters with a note.
fn bounded(mut text: String, max: usize, note: &str) -> String {
    if text.chars().count() > max {
        text = text.chars().take(max).collect();
        text.push_str(&format!("\n… {note}"));
    }
    text
}

fn line(rec: &Recording, r: &Reply) -> String {
    match r.speaker {
        Some(s) => format!("[{}] {}: {}", format_time(r.start), rec.speaker(s), r.text()),
        None => format!("[{}] {}", format_time(r.start), r.text()),
    }
}

/// The focused scan of a recording: instruction → what matches, with coverage.
pub type Scanner<'a> = dyn Fn(&str) -> anyhow::Result<super::scan::Scan> + 'a;

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

/// The speaker a label means: its name, «Speaker N» / «Говорящий N», or the number.
pub fn speaker_of(recording: &Recording, label: &str) -> Option<u32> {
    let label = label.trim();
    if let Some((&s, _)) = recording.names.iter().find(|(_, n)| n.eq_ignore_ascii_case(label) || n.to_lowercase() == label.to_lowercase()) {
        return Some(s);
    }
    let n: u32 = label.rsplit(' ').next()?.trim_start_matches('#').parse().ok()?;
    let s = n.checked_sub(1)?;
    recording.replies.iter().any(|r| r.speaker == Some(s)).then_some(s)
}

impl Tools<'_> {
    /// The tools offered: all of them, the scan when there is one.
    pub fn specs(&self) -> Vec<ToolSpec> {
        let mut s = specs();
        if self.scan.is_some() {
            s.push(ToolSpec {
                name: "scan",
                description: "Reads the WHOLE recording part by part and lists everything matching the instruction, with times. The instruction is what the user asks for, in their own words — not an example. Slow: only for exhaustive questions («all decisions», «every piece of advice», «each time X comes up»). Says which parts it could not read.",
                params: json!({"type": "object", "properties": {"instruction": {"type": "string"}}, "required": ["instruction"]}),
                example: json!({"instruction": "<what the user wants listed, in their words>"}),
            });
        }
        if self.rename.is_some() && self.recording.diarized {
            s.push(ToolSpec {
                name: "rename_speakers",
                description: "Gives speakers names, kept with the recording. Only when the user asks for it or confirms; names come from the recording (people introduce themselves or address each other — search for that first).",
                params: json!({"type": "object", "properties": {"names": {"type": "array", "maxItems": 12, "items": {"type": "object", "properties": {"speaker": {"type": "string"}, "name": {"type": "string"}}, "required": ["speaker", "name"]}}}, "required": ["names"]}),
                example: json!({"names": [{"speaker": "<a speaker as the tools show it>", "name": "<the name>"}]}),
            });
        }
        if self.delegate.is_some() {
            s.push(ToolSpec {
                name: "delegate",
                description: "Gives a bounded task on one stretch of the recording to a helper that reads it whole (e.g. «summarise», «list the arguments»): saves your context on long stretches. Returns its compact result with times.",
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
                    return Ok(Outcome::plain("No outline yet (it is being built). Use search and read."));
                }
                let text = rec
                    .chapters
                    .iter()
                    .map(|c| format!("[{}–{}] {} — {}", format_time(c.start), format_time(c.end), c.digest.title, c.digest.summary))
                    .collect::<Vec<_>>()
                    .join("\n");
                let seen = rec.chapters.iter().map(|c| format!("chapter@{}", c.start as i64)).collect();
                Ok(Outcome { text: bounded(text, self.max_chars, "(more chapters: read by time)"), seen: Some(seen) })
            }
            "search" => {
                let query = arg("query");
                anyhow::ensure!(!query.is_empty(), "search needs a query");
                let hits = (self.search)(&query, 6)?;
                if hits.is_empty() {
                    return Ok(Outcome { text: format!("Nothing found for «{query}». Try other words or read the outline."), seen: Some(vec![]) });
                }
                let text = hits
                    .iter()
                    .map(|h| {
                        let who = h.chunk.speakers.iter().map(|&s| rec.speaker(s)).collect::<Vec<_>>().join(", ");
                        format!("[{}–{}] ({who}) {}", format_time(h.chunk.start), format_time(h.chunk.end), h.chunk.text)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let seen = hits.iter().map(|h| format!("at@{}", h.chunk.start as i64)).collect();
                Ok(Outcome { text: bounded(text, self.max_chars, "(cut; read a range for the rest)"), seen: Some(seen) })
            }
            "read" => {
                let (from, to) = (arg("from"), arg("to"));
                let a = parse_time(&from).ok_or_else(|| anyhow::anyhow!("\"{from}\" is not a time (use m:ss)"))?;
                let b = parse_time(&to).ok_or_else(|| anyhow::anyhow!("\"{to}\" is not a time (use m:ss)"))?;
                let (a, b) = (a.min(b), a.max(b));
                anyhow::ensure!(a <= rec.duration, "the recording is only {} long", format_time(rec.duration));
                let mut text = String::new();
                let mut seen = Vec::new();
                for r in rec.replies.iter().filter(|r| r.end >= a && r.start <= b) {
                    let l = line(rec, r);
                    if text.chars().count() + l.chars().count() > self.max_chars {
                        text.push_str(&format!("… (stopped here; read on from [{}])", format_time(r.start)));
                        break;
                    }
                    text.push_str(&l);
                    text.push('\n');
                    seen.push(format!("at@{}", r.start as i64));
                }
                if seen.is_empty() {
                    return Ok(Outcome { text: format!("Nobody speaks between [{}] and [{}].", format_time(a), format_time(b)), seen: Some(vec![]) });
                }
                Ok(Outcome { text, seen: Some(seen) })
            }
            "mentions" => self.mentions(&arg("query")),
            "rename_speakers" => {
                let rename = self.rename.ok_or_else(|| anyhow::anyhow!("speakers cannot be named here"))?;
                let mut done = Vec::new();
                for item in args["names"].as_array().cloned().unwrap_or_default() {
                    let (label, name) = (item["speaker"].as_str().unwrap_or_default(), item["name"].as_str().unwrap_or_default().trim());
                    let s = speaker_of(rec, label).ok_or_else(|| anyhow::anyhow!("there is no speaker «{label}»"))?;
                    anyhow::ensure!(!name.is_empty(), "an empty name for «{label}»");
                    rename(s, name)?;
                    done.push(format!("{} → {name}", rec.speaker(s)));
                }
                anyhow::ensure!(!done.is_empty(), "rename_speakers needs names");
                Ok(Outcome { text: format!("Named: {}.", done.join(", ")), seen: None })
            }
            "delegate" => {
                let delegate = self.delegate.ok_or_else(|| anyhow::anyhow!("there is no helper"))?;
                let task = arg("task");
                anyhow::ensure!(!task.is_empty(), "delegate needs a task");
                // The helper reads the stretch whole (bounded by its own context, not yours).
                let stretch = Tools { max_chars: self.max_chars * 4, ..Tools::new(rec, self.search, self.max_chars) }.run("read", args)?;
                let result = delegate(&task, &stretch.text)?;
                Ok(Outcome { text: bounded(result, self.max_chars, "(cut)"), seen: None })
            }
            "scan" => {
                let instruction = arg("instruction");
                anyhow::ensure!(!instruction.is_empty(), "scan needs an instruction");
                let scan = self.scan.ok_or_else(|| anyhow::anyhow!("the scan is not available"))?;
                let found = scan(&instruction)?;
                let seen = found.facts.iter().map(|f| format!("fact@{}:{}", f.time as i64, f.text)).collect();
                Ok(Outcome { text: bounded(found.text(&instruction), self.max_chars, "(cut; ask a narrower instruction)"), seen: Some(seen) })
            }
            "speakers" => {
                if !rec.diarized {
                    return Ok(Outcome::plain("Speakers were not told apart in this recording."));
                }
                let mut stats: BTreeMap<u32, (f64, usize, f64)> = BTreeMap::new();
                for r in &rec.replies {
                    if let Some(s) = r.speaker {
                        let e = stats.entry(s).or_insert((0.0, 0, r.start));
                        e.0 += r.end - r.start;
                        e.1 += 1;
                    }
                }
                let text = stats
                    .iter()
                    .map(|(&s, &(talk, n, first))| format!("{}: talks {} in {n} replies, first at [{}]", rec.speaker(s), format_time(talk), format_time(first)))
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(Outcome::plain(text))
            }
            "calculator" => Ok(Outcome::plain(calculate(&arg("expression"))?)),
            "time" => {
                let (secs, clock) = between(&arg("from"), &arg("to"))?;
                Ok(Outcome::plain(format!("{clock} ({secs} seconds)")))
            }
            other => anyhow::bail!("there is no tool «{other}»"),
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
        anyhow::ensure!(!q.is_empty(), "mentions needs a word");
        let rec = self.recording;
        let mut found: Vec<String> = Vec::new();
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
                        let who = r.speaker.map(|s| format!(" {}:", rec.speaker(s))).unwrap_or_default();
                        found.push(format!("[{}]{who} …{context}…", format_time(t)));
                    }
                }
            }
        }
        let (Some(first), Some(last)) = (times.first(), times.last()) else {
            return Ok(Outcome { text: format!("«{query}» is never said (no word form of it). Try a synonym or search."), seen: Some(vec![]) });
        };
        let mut text = format!("«{query}»: {} mentions, first at [{}], last at [{}].\n", times.len(), format_time(*first), format_time(*last));
        text.push_str(&found.join("\n"));
        if times.len() > found.len() {
            text.push_str(&format!("\n… and {} more", times.len() - found.len()));
        }
        let seen = times.iter().map(|t| format!("mention@{}", *t as i64)).collect();
        Ok(Outcome { text: bounded(text, self.max_chars, "(cut)"), seen: Some(seen) })
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
    use mediaa_core::outline::Digest;
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
        assert!(card.contains("Speakers: Дана, Говорящий 2."));
        assert!(card.contains("Outline: 1 chapters"));
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
        assert!(out.text.starts_with("[5:00–5:30] (Дана) Бюджет"), "{}", out.text);
        assert_eq!(out.seen, Some(vec!["at@300".into()]));
        assert!(tools.run("search", &json!({"query": "погода"})).unwrap().text.starts_with("Nothing found"));
        assert!(tools.run("search", &json!({})).is_err());
        let out = tools.run("read", &json!({"from": "0:00", "to": "1:00"})).unwrap();
        assert_eq!(out.text, "[0:10] Дана: Отчёт отправляем в пятницу.\n[0:30] Говорящий 2: Графики будут к четвергу.\n");
        assert!(tools.run("read", &json!({"from": "2:00", "to": "3:00"})).unwrap().text.starts_with("Nobody speaks"));
        assert!(tools.run("read", &json!({"from": "вчера", "to": "1:00"})).is_err());
        assert!(tools.run("read", &json!({"from": "20:00", "to": "30:00"})).is_err(), "past the end");
        let out = tools.run("speakers", &json!({})).unwrap();
        assert!(out.text.starts_with("Дана: talks 0:07 in 2 replies, first at [0:10]"), "{}", out.text);
        assert_eq!(tools.run("calculator", &json!({"expression": "2+2*2"})).unwrap().text, "6");
        assert_eq!(tools.run("time", &json!({"from": "0:10", "to": "5:00"})).unwrap().text, "4:50 (290 seconds)");
        assert!(tools.run("dance", &json!({})).is_err());
    }

    #[test]
    fn mentions_are_counted_by_code() {
        let mut rec = recording();
        rec.replies.push(reply(1, 400.0, "Отчёты сдаём по пятницам, а отчётов много."));
        let tools = Tools::new(&rec, &no_search, 4000);
        let out = tools.run("mentions", &json!({"query": "отчёт"})).unwrap();
        assert!(out.text.starts_with("«отчёт»: 3 mentions, first at [0:10], last at [6:45]."), "{}", out.text);
        assert!(out.text.contains("[0:10] Дана: …Отчёт отправляем в пятницу.…"), "{}", out.text);
        assert!(out.text.contains("[6:40] Говорящий 2: …Отчёты"), "the label in the recording's language: {}", out.text);
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
        assert_eq!(speaker_of(&rec, "Дана"), Some(0));
        assert_eq!(speaker_of(&rec, "Говорящий 2"), Some(1));
        assert_eq!(speaker_of(&rec, "Speaker 2"), Some(1));
        assert_eq!(speaker_of(&rec, "Говорящий 7"), None, "no such speaker");
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
        let out = tools.run("rename_speakers", &json!({"names": [{"speaker": "Говорящий 2", "name": " Олег "}]})).unwrap();
        assert_eq!(out.text, "Named: Говорящий 2 → Олег.");
        assert_eq!(*named.borrow(), [(1, "Олег".to_string())]);
        assert!(tools.run("rename_speakers", &json!({"names": [{"speaker": "Говорящий 9", "name": "X"}]})).is_err());
        let out = tools.run("delegate", &json!({"task": "перескажи", "from": "0:00", "to": "1:00"})).unwrap();
        assert_eq!(out.text, "Итог: про пятницу [0:10].");
        assert!(seen.borrow().starts_with("перескажи|[0:10] Дана: Отчёт"), "{}", seen.borrow());
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
