use crate::prompts;
use super::brain::ToolSpec;
use super::tools::{self, Outcome, Recording, Tools};
use super::Toolbox;
use crate::search::Options;
use crate::tools::{format_time, parse_time};
use mediaa_core::retrieval::Hit;
use mediaa_core::speakers::Parsed;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;

const NEAR: f64 = 15.0;
const CARD_CHAPTERS: usize = 6;

#[derive(Debug, Clone)]
pub struct Item {
    pub recording: Recording,
    pub file_id: String,
    pub path: PathBuf,
}

pub type FolderSearch<'a> = dyn Fn(&str, Options, usize) -> anyhow::Result<Vec<(usize, Hit)>> + Sync + 'a;
pub type ItemSearch<'a> = dyn Fn(usize, &str, Options, usize) -> anyhow::Result<Vec<Hit>> + Sync + 'a;

pub struct FolderTools<'a> {
    pub items: &'a [Item],
    pub search: &'a FolderSearch<'a>,
    pub search_one: &'a ItemSearch<'a>,
    pub max_chars: usize,
    pub web: Option<&'a super::internet::Web<'a>>,
}

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
    fn item(&self, args: &Value) -> anyhow::Result<(usize, &Item)> {
        let raw = match &args["recording"] {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.trim().trim_start_matches('#').to_string(),
            _ => return Err(tools::error("which_recording", ())),
        };
        let i = match raw.parse::<usize>() {
            Ok(n) if (1..=self.items.len()).contains(&n) => n - 1,
            Ok(n) => return Err(tools::error("no_recording_number", minijinja::context! { number => n, count => self.items.len() })),
            Err(_) => self
                .items
                .iter()
                .position(|it| it.recording.title.to_lowercase().contains(&raw.to_lowercase()))
                .ok_or_else(|| tools::error("no_recording_title", minijinja::context! { title => &raw }))?,
        };
        Ok((i, &self.items[i]))
    }

    fn one(&self, i: usize, tool: &str, args: &Value) -> anyhow::Result<Outcome> {
        let search = |q: &str, o: Options, n: usize| (self.search_one)(i, q, o, n);
        let out = Tools::new(&self.items[i].recording, &search, self.max_chars).run(tool, args)?;
        let text = mediaa_core::speakers::tag_recording(&tag_times(&out.text, i + 1), (i + 1) as u32);
        Ok(Outcome { text, seen: out.seen.map(|s| s.into_iter().map(|k| format!("{}:{k}", i + 1)).collect()) })
    }

    fn speaker_arg(&self, args: &Value) -> anyhow::Result<Option<Parsed>> {
        let label = args["speaker"].as_str().unwrap_or_default().trim();
        if label.is_empty() {
            return Ok(None);
        }
        let parsed = mediaa_core::speakers::parse_label(label).ok_or_else(|| tools::error("no_speaker", minijinja::context! { label }))?;
        let found = match parsed.recording {
            Some(r) => r.checked_sub(1).and_then(|i| self.items.get(i as usize)).is_some_and(|it| tools::speaker_of(&it.recording, label).is_some()),
            None => self.items.iter().any(|it| tools::speaker_of(&it.recording, label).is_some()),
        };
        anyhow::ensure!(found, tools::error("no_speaker", minijinja::context! { label }));
        Ok(Some(parsed))
    }

    fn span_arg(&self, args: &Value) -> anyhow::Result<Option<(f64, f64)>> {
        let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
        let (from, to) = (arg("from"), arg("to"));
        if from.is_empty() && to.is_empty() {
            return Ok(None);
        }
        let a = if from.is_empty() { 0.0 } else { parse_time(&from).ok_or_else(|| tools::error("not_a_time", minijinja::context! { value => &from }))? };
        let b = if to.is_empty() { f64::MAX } else { parse_time(&to).ok_or_else(|| tools::error("not_a_time", minijinja::context! { value => &to }))? };
        Ok(Some((a.min(b), a.max(b))))
    }
}

impl Toolbox for FolderTools<'_> {
    fn specs(&self) -> Vec<ToolSpec> {
        let recording = json!({"type": "integer", "description": prompts::tool("folder/recording_param")});
        let mut specs = vec![
            ToolSpec {
                name: "recordings",
                description: prompts::tool("folder/recordings"),
                params: json!({"type": "object", "properties": {}}),
                example: json!({}),
            },
            ToolSpec {
                name: "search",
                description: prompts::tool("folder/search"),
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
                let items: Vec<_> = self
                    .items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let r = &it.recording;
                        let chapters: Vec<&str> = r.chapters.iter().map(|c| c.digest.title.as_str()).collect();
                        minijinja::context! { number => i + 1, title => &r.title, length => format_time(r.duration), transcribed => !r.replies.is_empty(), chapters }
                    })
                    .collect();
                Ok(Outcome { text: prompts::render("results/folder/recordings.j2", minijinja::context! { items }), seen: None })
            }
            "search" => {
                let query = args["query"].as_str().unwrap_or_default().trim();
                anyhow::ensure!(!query.is_empty(), tools::needs("search", "query"));
                anyhow::ensure!(!tools::vague(query), tools::error("vague_query", minijinja::context! { query }));
                let speaker = self.speaker_arg(args)?;
                let span = self.span_arg(args)?;
                let hits = (self.search)(query, Options { speaker, span }, 8)?;
                if hits.is_empty() {
                    return Ok(Outcome { text: prompts::render("results/folder/nothing.j2", minijinja::context! { query }), seen: Some(vec![]) });
                }
                let mut shown = Vec::new();
                let mut order: Vec<usize> = Vec::new();
                let mut groups: HashMap<usize, Vec<Hit>> = HashMap::new();
                for (number, hit) in &hits {
                    if !groups.contains_key(number) {
                        order.push(*number);
                    }
                    groups.entry(*number).or_default().push(hit.clone());
                }
                for number in order {
                    let group = groups.remove(&number).unwrap_or_default();
                    let title = &self.items[number].recording.title;
                    for f in tools::fragments(&self.items[number].recording, &group) {
                        let who: Vec<String> = f.speakers.iter().map(|&s| mediaa_core::speakers::folder_label(s, (number + 1) as u32)).collect();
                        shown.push(minijinja::context! { number => number + 1, start => format_time(f.start), end => format_time(f.end), title, who, text => &f.text });
                    }
                }
                let text = prompts::render("results/folder/search.j2", minijinja::context! { hits => shown });
                let seen = hits.iter().map(|(i, h)| format!("{}:at@{}", i + 1, h.chunk.start as i64)).collect();
                Ok(Outcome { text: tools::bounded(text, self.max_chars, "plain"), seen: Some(seen) })
            }
            "mentions" if args["recording"].is_null() => {
                let mut parts = Vec::new();
                let mut seen = Vec::new();
                for i in 0..self.items.len() {
                    let out = self.one(i, "mentions", args)?;
                    if out.seen.as_ref().is_some_and(|s| !s.is_empty()) {
                        parts.push(minijinja::context! { number => i + 1, text => out.text });
                        seen.extend(out.seen.unwrap_or_default());
                    }
                }
                let text = if parts.is_empty() { prompts::render("results/folder/never_said.j2", ()) } else { prompts::render("results/folder/mentions.j2", minijinja::context! { parts }) };
                Ok(Outcome { text: tools::bounded(text, self.max_chars, "plain"), seen: Some(seen) })
            }
            "outline" | "read" | "mentions" | "speakers" => {
                let (i, _) = self.item(args)?;
                self.one(i, tool, args)
            }
            "calculator" | "time" => self.one(0, tool, args),
            other => Err(tools::error("no_tool", minijinja::context! { tool => other })),
        }
    }

    fn kind(&self) -> &'static str {
        "folder"
    }

    fn card(&self) -> String {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|it| {
                let r = &it.recording;
                let chapters: Vec<&str> = r.chapters.iter().take(CARD_CHAPTERS).map(|c| c.digest.title.as_str()).collect();
                json!({"title": r.title, "length": format_time(r.duration), "transcribed": !r.replies.is_empty(), "chapters": chapters})
            })
            .collect();
        tools::bounded(prompts::render("agent/card_folder.j2", minijinja::context! { items }), self.max_chars / 3, "plain")
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
        let mut rest = text;
        while let Some(open) = rest.find("[#") {
            rest = &rest[open + 1..];
            let inside = &rest[..rest.find(']').unwrap_or(rest.len())];
            if folder_citations(&format!("[{inside}]")).is_empty() {
                bad.push(inside.to_string());
            }
        }
        bad.extend(super::citations(text).into_iter().filter(|(s, _)| !s.starts_with('#')).map(|(s, _)| s));
        bad
    }

    fn cited_context(&self, text: &str) -> String {
        let cites = folder_citations(text);
        let mut parts = Vec::new();
        for (i, it) in self.items.iter().enumerate() {
            let n = i + 1;
            let times: Vec<f64> = cites.iter().filter(|(_, num, _)| *num == n).map(|(_, _, t)| *t).collect();
            if times.is_empty() {
                continue;
            }
            let lines: Vec<String> = super::cited_replies(&it.recording, &times).into_iter().map(|r| super::reply_line(&it.recording, r)).collect();
            if lines.is_empty() {
                continue;
            }
            parts.push(format!("#{n} {}\n{}", it.recording.title, lines.join("\n")));
        }
        parts.join("\n\n")
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
        let none = |_: &str, _: Options, _: usize| -> anyhow::Result<Vec<(usize, Hit)>> { Ok(vec![]) };
        let one = |_: usize, _: &str, _: Options, _: usize| -> anyhow::Result<Vec<Hit>> { Ok(vec![]) };
        let f = FolderTools { items: &items, search: &none, search_one: &one, max_chars: 4000, web: None };
        assert_eq!(f.bad_citations("[#1 5:00] [#2 5:00] [#3 0:10] [0:30] [#1 m:00]"), ["#2 5:00", "#3 0:10", "#1 m:00", "0:30"]);
        let around = f.cited_context("[#1 0:10] решили");
        assert!(around.contains("Отчёт отправляем в пятницу.") && around.contains("Графики будут к четвергу."), "{around}");
        assert!(!around.contains("Бюджет на квартал"), "{around}");
        assert!(around.starts_with("#1 Планёрка\n"), "{around}");
    }

    #[test]
    fn folder_search_options_are_checked_and_passed() {
        let items = items();
        let asked = std::sync::Mutex::new(Options::default());
        let across = |_: &str, o: Options, _: usize| {
            *asked.lock().unwrap() = o;
            Ok(vec![])
        };
        let one = |_: usize, _: &str, _: Options, _: usize| -> anyhow::Result<Vec<Hit>> { Ok(vec![]) };
        let f = FolderTools { items: &items, search: &across, search_one: &one, max_chars: 4000, web: None };
        f.run("search", &json!({"query": "графики", "speaker": "@s2#2", "from": "0:30", "to": "1:00"})).unwrap();
        assert_eq!(asked.lock().unwrap().speaker, Some(Parsed { index: 1, recording: Some(2) }));
        assert_eq!(asked.lock().unwrap().span, Some((30.0, 60.0)));
        f.run("search", &json!({"query": "графики", "speaker": "@s1"})).unwrap();
        assert_eq!(asked.lock().unwrap().speaker, Some(Parsed { index: 0, recording: None }), "a label without a recording is that speaker everywhere");
        let e = f.run("search", &json!({"query": "графики", "speaker": "@s1#9"})).unwrap_err().to_string();
        assert!(e.contains("@s1#9"), "{e}");
        let e = f.run("search", &json!({"query": "графики", "from": "вчера"})).unwrap_err().to_string();
        assert!(e.contains("not a time"), "{e}");
    }

    #[test]
    fn tools_over_the_folder() {
        let items = items();
        let across = |q: &str, _: Options, _: usize| -> anyhow::Result<Vec<(usize, Hit)>> {
            Ok(if q == "графики" { vec![(1, Hit { id: 1, chunk: Chunk { start: 30.0, end: 40.0, speakers: vec![1], text: "Графики будут к четвергу.".into() }, score: 1.0 })] } else { vec![] })
        };
        let one = |_: usize, _: &str, _: Options, _: usize| -> anyhow::Result<Vec<Hit>> { Ok(vec![]) };
        let f = FolderTools { items: &items, search: &across, search_one: &one, max_chars: 4000, web: None };
        let names: Vec<&str> = f.specs().iter().map(|s| s.name).collect();
        assert_eq!(names, ["recordings", "search", "outline", "read", "mentions", "speakers", "calculator", "time"]);
        let read = f.specs().into_iter().find(|s| s.name == "read").unwrap();
        assert_eq!(read.params["required"], json!(["recording", "from", "to"]));
        let list = f.run("recordings", &json!({})).unwrap().text;
        assert!(list.starts_with("#1 «Планёрка», 10:00. Chapters: Отчёт и бюджет\n#2 «Созвон», 2:00"), "{list}");
        let found = f.run("search", &json!({"query": "графики"})).unwrap();
        assert!(found.text.starts_with("[#2 0:10–0:33] «Созвон» (@s1#2, @s2#2) Отчёт отправляем в пятницу. Графики будут к четвергу."), "{}", found.text);
        let read = f.run("read", &json!({"recording": 2, "from": "0:00", "to": "1:00"})).unwrap();
        assert!(read.text.starts_with("[#2 0:10] @s1#2: Отчёт"), "{}", read.text);
        assert_eq!(f.run("outline", &json!({"recording": "#1"})).unwrap().text.lines().next().unwrap(), "[#1 0:00–10:00] Отчёт и бюджет — Сроки отчёта, бюджет.");
        assert!(f.run("outline", &json!({"recording": "Созвон"})).unwrap().text.contains("No outline"));
        assert!(f.run("read", &json!({"recording": 5, "from": "0:00", "to": "1:00"})).is_err());
        assert!(f.run("read", &json!({"from": "0:00", "to": "1:00"})).is_err(), "which recording");
        let all = f.run("mentions", &json!({"query": "отчёт"})).unwrap().text;
        assert!(all.starts_with("#1: «отчёт»: 1 mentions") && all.contains("#2: «отчёт»: 1 mentions"), "{all}");
        assert_eq!(f.run("calculator", &json!({"expression": "1+1"})).unwrap().text, "2");
        assert_eq!(f.card(), "The folder has 2 recordings:\n#1 «Планёрка», 10:00. About: Отчёт и бюджет\n#2 «Созвон», 2:00\n");
        let topic = f.run("outline", &json!({"recording": 1, "topic": 1})).unwrap().text;
        assert!(topic.contains("Topic 1 «Отчёт и бюджет» has no breakdown yet"), "{topic}");
        let missing = f.run("outline", &json!({"recording": 1, "topic": 4})).unwrap_err().to_string();
        assert!(missing.contains('4'), "{missing}");
    }
}
