//! The agent over the recordings of a catalog folder: the list of recordings, one search across
//! them all, and the per-recording tools (outline, read, mentions, speakers) on the recording
//! named by its number. Moments are cited as `[#N m:ss]` — the recording's number and the time —
//! and checked against that recording.

use super::brain::ToolSpec;
use super::tools::{self, Outcome, Recording, Tools};
use super::Toolbox;
use crate::tools::{format_time, parse_time};
use mediaa_core::retrieval::Hit;
use serde_json::{json, Value};
use std::path::PathBuf;

/// A recording close enough to a cited time when a reply is this near (seconds).
const NEAR: f64 = 15.0;
/// Chapter titles of a recording shown on the card.
const CARD_CHAPTERS: usize = 6;

/// A recording of the folder.
#[derive(Debug, Clone)]
pub struct Item {
    pub recording: Recording,
    pub file_id: String,
    pub path: PathBuf,
}

/// Searches across the folder: (query, how many) → hits with the index of their recording.
pub type FolderSearch<'a> = dyn Fn(&str, usize) -> anyhow::Result<Vec<(usize, Hit)>> + 'a;
/// Searches one recording: (index, query, how many) → hits.
pub type ItemSearch<'a> = dyn Fn(usize, &str, usize) -> anyhow::Result<Vec<Hit>> + 'a;

pub struct FolderTools<'a> {
    pub items: &'a [Item],
    pub search: &'a FolderSearch<'a>,
    pub search_one: &'a ItemSearch<'a>,
    pub max_chars: usize,
    /// The web, when the agent may use it.
    pub web: Option<&'a super::internet::Web<'a>>,
}

/// `[m:ss` → `[#n m:ss` in a single recording's tool result.
pub fn tag_times(text: &str, n: usize) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    for (i, c) in text.char_indices() {
        out.push(c);
        if c == '[' && text[i + 1..].starts_with(|d: char| d.is_ascii_digit()) && text[i + 1..].split(']').next().is_some_and(|s| s.contains(':')) {
            out.push_str(&format!("#{n} "));
        }
    }
    out
}

/// Citations `[#N m:ss]` (also the start of a range): (as written, recording number, seconds).
pub fn folder_citations(text: &str) -> Vec<(String, usize, f64)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find("[#") {
        rest = &rest[open + 2..];
        let end = rest.find(']').unwrap_or(rest.len());
        let inside = &rest[..end];
        let mut parts = inside.splitn(2, ' ');
        let (Some(n), Some(time)) = (parts.next(), parts.next()) else { continue };
        let time = time.split(['–', '-']).next().unwrap_or_default();
        if let (Ok(n), Some(t)) = (n.parse::<usize>(), parse_time(time)) {
            out.push((format!("#{inside}"), n, t));
        }
    }
    out
}

impl FolderTools<'_> {
    /// The recording an argument names: its number (`2`, `#2`) or its title.
    fn item(&self, args: &Value) -> anyhow::Result<(usize, &Item)> {
        let raw = match &args["recording"] {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.trim().trim_start_matches('#').to_string(),
            _ => anyhow::bail!("say which recording (its number from the list)"),
        };
        let i = match raw.parse::<usize>() {
            Ok(n) if (1..=self.items.len()).contains(&n) => n - 1,
            Ok(n) => anyhow::bail!("there is no recording #{n} (there are {})", self.items.len()),
            Err(_) => self
                .items
                .iter()
                .position(|it| it.recording.title.to_lowercase().contains(&raw.to_lowercase()))
                .ok_or_else(|| anyhow::anyhow!("no recording is called «{raw}»"))?,
        };
        Ok((i, &self.items[i]))
    }

    fn one(&self, i: usize, tool: &str, args: &Value) -> anyhow::Result<Outcome> {
        let search = |q: &str, n: usize| (self.search_one)(i, q, n);
        let out = Tools::new(&self.items[i].recording, &search, self.max_chars).run(tool, args)?;
        Ok(Outcome { text: tag_times(&out.text, i + 1), seen: out.seen.map(|s| s.into_iter().map(|k| format!("{}:{k}", i + 1)).collect()) })
    }
}

impl Toolbox for FolderTools<'_> {
    fn specs(&self) -> Vec<ToolSpec> {
        let recording = json!({"type": "integer", "description": "the recording's number from the list"});
        let mut specs = vec![
            ToolSpec {
                name: "recordings",
                description: "The recordings of the folder: number, title, length, speakers, chapters.",
                params: json!({"type": "object", "properties": {}}),
                example: json!({}),
            },
            ToolSpec {
                name: "search",
                description: "Finds the places that talk about something in ALL the recordings (by meaning and by words).",
                params: json!({"type": "object", "properties": {"query": {"type": "string"}}, "required": ["query"]}),
                example: json!({"query": "<what to find>"}),
            },
        ];
        for t in tools::specs().into_iter().filter(|t| matches!(t.name, "outline" | "read" | "mentions" | "speakers")) {
            let mut params = t.params.clone();
            params["properties"]["recording"] = recording.clone();
            let mut required = params["required"].as_array().cloned().unwrap_or_default();
            required.insert(0, json!("recording"));
            params["required"] = Value::Array(required);
            let mut example = t.example.clone();
            example["recording"] = json!(1);
            specs.push(ToolSpec { params, example, ..t });
        }
        specs.extend(tools::specs().into_iter().filter(|t| matches!(t.name, "calculator" | "time")));
        specs
    }

    fn run(&self, tool: &str, args: &Value) -> anyhow::Result<Outcome> {
        match tool {
            "recordings" => {
                let text = self
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let r = &it.recording;
                        let chapters = r.chapters.iter().map(|c| c.digest.title.as_str()).collect::<Vec<_>>().join("; ");
                        let state = if r.replies.is_empty() { " — not transcribed yet" } else { "" };
                        format!("#{} «{}», {}{state}{}", i + 1, r.title, format_time(r.duration), if chapters.is_empty() { String::new() } else { format!(". Chapters: {chapters}") })
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                Ok(Outcome { text, seen: None })
            }
            "search" => {
                let query = args["query"].as_str().unwrap_or_default().trim();
                anyhow::ensure!(!query.is_empty(), "search needs a query");
                let hits = (self.search)(query, 8)?;
                if hits.is_empty() {
                    return Ok(Outcome { text: format!("Nothing found for «{query}» in the folder."), seen: Some(vec![]) });
                }
                let text = hits
                    .iter()
                    .map(|(i, h)| {
                        let r = &self.items[*i].recording;
                        let who = h.chunk.speakers.iter().map(|&s| r.speaker(s)).collect::<Vec<_>>().join(", ");
                        format!("[#{} {}–{}] «{}» ({who}) {}", i + 1, format_time(h.chunk.start), format_time(h.chunk.end), r.title, h.chunk.text)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n");
                let seen = hits.iter().map(|(i, h)| format!("{}:at@{}", i + 1, h.chunk.start as i64)).collect();
                Ok(Outcome { text: tools_bounded(text, self.max_chars), seen: Some(seen) })
            }
            "mentions" if args["recording"].is_null() => {
                // Every recording: their counts together.
                let mut parts = Vec::new();
                let mut seen = Vec::new();
                for i in 0..self.items.len() {
                    let out = self.one(i, "mentions", args)?;
                    if !out.text.contains("never said") {
                        parts.push(format!("#{}: {}", i + 1, out.text));
                        seen.extend(out.seen.unwrap_or_default());
                    }
                }
                let text = if parts.is_empty() { "Never said in any recording.".to_string() } else { parts.join("\n\n") };
                Ok(Outcome { text: tools_bounded(text, self.max_chars), seen: Some(seen) })
            }
            "outline" | "read" | "mentions" | "speakers" => {
                let (i, _) = self.item(args)?;
                self.one(i, tool, args)
            }
            "calculator" | "time" => self.one(0, tool, args),
            other => anyhow::bail!("there is no tool «{other}»"),
        }
    }

    fn subject(&self) -> &'static str {
        "the recordings of a folder (transcripts of conversations), numbered #1, #2…"
    }

    fn citation_form(&self) -> &'static str {
        "[#2 12:30] (the recording's number, then the time)"
    }

    fn card(&self) -> String {
        // The map of the folder up front: what each recording is about, by its chapters.
        let mut card = format!("The folder has {} recordings:\n", self.items.len());
        for (i, it) in self.items.iter().enumerate() {
            let r = &it.recording;
            let about = if r.replies.is_empty() {
                " — not transcribed yet".to_string()
            } else if r.chapters.is_empty() {
                String::new()
            } else {
                let titles: Vec<&str> = r.chapters.iter().take(CARD_CHAPTERS).map(|c| c.digest.title.as_str()).collect();
                format!(". About: {}", titles.join("; "))
            };
            card.push_str(&format!("#{} «{}», {}{about}\n", i + 1, r.title, format_time(r.duration)));
        }
        tools_bounded(card, self.max_chars / 3)
    }

    fn web(&self) -> Option<&super::internet::Web<'_>> {
        self.web
    }

    fn bad_citations(&self, text: &str) -> Vec<String> {
        let mut bad: Vec<String> = folder_citations(text)
            .into_iter()
            .filter(|(_, n, t)| {
                let Some(it) = n.checked_sub(1).and_then(|i| self.items.get(i)) else { return true };
                let r = &it.recording;
                *t > r.duration + 1.0 || !r.replies.iter().any(|x| *t >= x.start - NEAR && *t <= x.end + NEAR)
            })
            .map(|(s, _, _)| s)
            .collect();
        // Written like a citation but not one (e.g. a placeholder copied from the rules).
        let mut rest = text;
        while let Some(open) = rest.find("[#") {
            rest = &rest[open + 1..];
            let inside = &rest[..rest.find(']').unwrap_or(rest.len())];
            if folder_citations(&format!("[{inside}]")).is_empty() {
                bad.push(inside.to_string());
            }
        }
        // A time without its recording cannot be followed.
        bad.extend(super::citations(text).into_iter().filter(|(s, _)| !s.starts_with('#')).map(|(s, _)| s));
        bad
    }
}

fn tools_bounded(text: String, max: usize) -> String {
    if text.chars().count() > max {
        format!("{}\n… (cut)", text.chars().take(max).collect::<String>())
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use super::super::tools::tests::recording;
    use super::*;
    use mediaa_core::retrieval::Chunk;

    fn items() -> Vec<Item> {
        let a = recording();
        let b = Recording { title: "Созвон".into(), duration: 120.0, ..recording() };
        let b = Recording { replies: b.replies.into_iter().take(2).collect(), chapters: vec![], ..b };
        vec![Item { recording: a, file_id: "a".into(), path: "/m/a.mp4".into() }, Item { recording: b, file_id: "b".into(), path: "/m/b.mp4".into() }]
    }

    #[test]
    fn times_are_tagged_and_citations_checked_per_recording() {
        assert_eq!(tag_times("[0:10] Дана: [см.] и [1:00–2:00]", 2), "[#2 0:10] Дана: [см.] и [#2 1:00–2:00]");
        let c = folder_citations("в [#1 0:10] и [#2 5:00–5:30], не [0:10]");
        assert_eq!(c.iter().map(|(_, n, t)| (*n, *t)).collect::<Vec<_>>(), [(1, 10.0), (2, 300.0)]);
        let items = items();
        let none = |_: &str, _: usize| -> anyhow::Result<Vec<(usize, Hit)>> { Ok(vec![]) };
        let one = |_: usize, _: &str, _: usize| -> anyhow::Result<Vec<Hit>> { Ok(vec![]) };
        let f = FolderTools { items: &items, search: &none, search_one: &one, max_chars: 4000, web: None };
        // #2 is two minutes long; #3 does not exist; a bare time names no recording.
        assert_eq!(f.bad_citations("[#1 5:00] [#2 5:00] [#3 0:10] [0:30] [#1 m:00]"), ["#2 5:00", "#3 0:10", "#1 m:00", "0:30"]);
    }

    #[test]
    fn tools_over_the_folder() {
        let items = items();
        let across = |q: &str, _: usize| -> anyhow::Result<Vec<(usize, Hit)>> {
            Ok(if q == "графики" { vec![(1, Hit { id: 1, chunk: Chunk { start: 30.0, end: 40.0, speakers: vec![1], text: "Графики будут к четвергу.".into() }, score: 1.0 })] } else { vec![] })
        };
        let one = |_: usize, _: &str, _: usize| -> anyhow::Result<Vec<Hit>> { Ok(vec![]) };
        let f = FolderTools { items: &items, search: &across, search_one: &one, max_chars: 4000, web: None };
        let names: Vec<&str> = f.specs().iter().map(|s| s.name).collect();
        assert_eq!(names, ["recordings", "search", "outline", "read", "mentions", "speakers", "calculator", "time"]);
        let read = f.specs().into_iter().find(|s| s.name == "read").unwrap();
        assert_eq!(read.params["required"], json!(["recording", "from", "to"]));
        let list = f.run("recordings", &json!({})).unwrap().text;
        assert!(list.starts_with("#1 «Планёрка», 10:00. Chapters: Отчёт и бюджет\n#2 «Созвон», 2:00"), "{list}");
        let found = f.run("search", &json!({"query": "графики"})).unwrap();
        assert!(found.text.starts_with("[#2 0:30–0:40] «Созвон» (Говорящий 2) Графики"), "{}", found.text);
        let read = f.run("read", &json!({"recording": 2, "from": "0:00", "to": "1:00"})).unwrap();
        assert!(read.text.starts_with("[#2 0:10] Дана: Отчёт"), "{}", read.text);
        assert_eq!(f.run("outline", &json!({"recording": "#1"})).unwrap().text.lines().next().unwrap(), "[#1 0:00–10:00] Отчёт и бюджет — Сроки отчёта, бюджет.");
        assert!(f.run("outline", &json!({"recording": "Созвон"})).unwrap().text.contains("No outline"));
        assert!(f.run("read", &json!({"recording": 5, "from": "0:00", "to": "1:00"})).is_err());
        assert!(f.run("read", &json!({"from": "0:00", "to": "1:00"})).is_err(), "which recording");
        let all = f.run("mentions", &json!({"query": "отчёт"})).unwrap().text;
        assert!(all.starts_with("#1: «отчёт»: 1 mentions") && all.contains("#2: «отчёт»: 1 mentions"), "{all}");
        assert_eq!(f.run("calculator", &json!({"expression": "1+1"})).unwrap().text, "2");
        assert_eq!(f.card(), "The folder has 2 recordings:\n#1 «Планёрка», 10:00. About: Отчёт и бюджет\n#2 «Созвон», 2:00\n");
    }
}
