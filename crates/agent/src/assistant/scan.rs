//! The focused scan: for questions about the whole recording («every decision», «all the advice»,
//! «each time X comes up») the model reads every part against an instruction and picks out what
//! matches, with times (map), and the parts are collected in order (reduce by code). It is the
//! only reliable way for a small model to answer exhaustive questions. It reports its coverage —
//! parts it could not read are named, never silently skipped — and its results are kept per
//! instruction with the transcript.

use super::brain::Brain;
use super::provider::ProviderBrain;
use super::tools::Recording;
use crate::llm::{Generate, LocalModel, Message, Role};
use crate::prompts;
use crate::tools::{format_time, parse_time};
use mediaa_core::outline::{self, Window};
use mediaa_core::retrieval::stems;
use mediaa_core::transcript::{Reply, Transcript};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

/// Words of dialog per scanned part (smaller parts: a small model misses less).
pub const SCAN_WORDS: usize = 900;
/// Facts kept from one part.
const FACTS_PER_PART: u32 = 8;

/// JSON Schema of what a part yields.
pub fn facts_schema() -> String {
    serde_json::json!({
        "type": "object",
        "properties": {
            "facts": {
                "type": "array",
                "maxItems": FACTS_PER_PART,
                "items": {
                    "type": "object",
                    "properties": {"time": {"type": "string", "maxLength": 8}, "text": {"type": "string", "maxLength": 240}},
                    "required": ["time", "text"]
                }
            }
        },
        "required": ["facts"]
    })
    .to_string()
}

/// A fact found by the scan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Fact {
    pub time: f64,
    pub text: String,
}

/// What the scan found and how much of the recording it covered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scan {
    pub facts: Vec<Fact>,
    pub parts: usize,
    /// Parts not read: time range and why.
    pub failed: Vec<(f64, f64, String)>,
}

impl Scan {
    /// The result for the model.
    pub fn text(&self, instruction: &str) -> String {
        let failed: Vec<_> = self.failed.iter().map(|(a, b, why)| minijinja::context! { start => format_time(*a), end => format_time(*b), why }).collect();
        let facts: Vec<_> = self.facts.iter().map(|f| minijinja::context! { time => format_time(f.time), text => &f.text }).collect();
        let done = self.parts - self.failed.len();
        prompts::render("results/scan.j2", minijinja::context! { instruction, parts => self.parts, done, count => self.facts.len(), failed, facts })
    }
}

/// Reads one part against the instruction: (time as written, fact) pairs.
pub trait Extract {
    fn extract(&self, instruction: &str, part: &Window, i: usize, n: usize, cancel: &AtomicBool) -> anyhow::Result<Vec<(String, String)>>;
}

/// Where a finding was said: the word of the part's reply sharing the most words with it
/// (models misread times; the text tells), else the model's time inside the part, else the
/// part's start.
pub fn place(finding: &str, said_at: Option<f64>, part: &Window, replies: &[Reply]) -> f64 {
    let wanted: Vec<String> = {
        let mut s: Vec<String> = stems(finding).into_iter().filter(|s| s.chars().count() >= 3).collect();
        s.sort();
        s.dedup();
        s
    };
    let mut best: Option<(usize, f64)> = None;
    for r in replies.iter().filter(|r| r.start >= part.start - 0.01 && r.start <= part.end) {
        let words: Vec<(String, f64)> = r.words.iter().map(|w| (stems(&w.text).into_iter().next().unwrap_or_default(), w.start)).collect();
        let shared = wanted.iter().filter(|s| words.iter().any(|(w, _)| w == *s)).count();
        if shared > best.map_or(0, |b| b.0) {
            let first = words.iter().find(|(w, _)| wanted.contains(w)).map_or(r.start, |w| w.1);
            best = Some((shared, first));
        }
    }
    let enough = if wanted.len() <= 2 { 1 } else { 2 };
    match best {
        Some((shared, t)) if shared >= enough => t,
        _ => said_at.filter(|t| *t >= part.start - 5.0 && *t <= part.end + 5.0).unwrap_or(part.start),
    }
}

fn request(instruction: &str, part: &Window, i: usize, n: usize) -> String {
    let (start, end) = (format_time(part.start), format_time(part.end));
    prompts::render("scan/part.j2", minijinja::context! { instruction, number => i + 1, parts => n, start, end, text => &part.text })
}

/// The scan's system prompt; `json_only` for models without a grammar.
fn system(json_only: bool) -> String {
    prompts::render("scan/system.j2", minijinja::context! { json_only })
}

/// Facts out of a model's JSON answer.
pub fn read_facts(text: &str) -> anyhow::Result<Vec<(String, String)>> {
    // Providers may wrap the JSON in prose or a code fence: the outermost object.
    let json = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => &text[a..=b],
        _ => anyhow::bail!("no JSON in the answer"),
    };
    let v: Value = serde_json::from_str(json)?;
    Ok(v["facts"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|f| Some((f["time"].as_str().unwrap_or_default().to_string(), f["text"].as_str()?.trim().to_string())))
                .filter(|(_, t)| !t.is_empty())
                .collect()
        })
        .unwrap_or_default())
}

/// A local model reads the parts (JSON constrained by a grammar).
pub struct LocalExtract<'a>(pub &'a LocalModel);

impl Extract for LocalExtract<'_> {
    fn extract(&self, instruction: &str, part: &Window, i: usize, n: usize, cancel: &AtomicBool) -> anyhow::Result<Vec<(String, String)>> {
        let messages = [Message::new(Role::System, system(false)), Message::new(Role::User, request(instruction, part, i, n))];
        let opts = Generate { max_tokens: 900, temperature: 0.0, json_schema: Some(facts_schema()), no_repeats: true };
        read_facts(&self.0.chat(&messages, &opts, cancel, &mut |_| true)?)
    }
}

/// A provider's model reads the parts (JSON asked for in the prompt).
pub struct ProviderExtract<'a>(pub &'a ProviderBrain);

impl Extract for ProviderExtract<'_> {
    fn extract(&self, instruction: &str, part: &Window, i: usize, n: usize, cancel: &AtomicBool) -> anyhow::Result<Vec<(String, String)>> {
        let messages = [Message::new(Role::System, system(true)), Message::new(Role::User, request(instruction, part, i, n))];
        read_facts(&self.0.complete(&messages, cancel)?)
    }
}

/// The cache key of an instruction (case and spacing do not matter).
fn cache_key(instruction: &str, parts: usize) -> String {
    format!("scan2:{parts}:{}", instruction.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" "))
}

/// Scans the recording for `instruction`; `progress(done, total)` after every part. A finished
/// scan is kept in `transcript` and reused.
pub fn scan(extract: &dyn Extract, recording: &Recording, transcript: Option<&Transcript>, instruction: &str, cancel: &AtomicBool, progress: &mut dyn FnMut(usize, usize)) -> anyhow::Result<Scan> {
    let language = recording.language();
    let parts = outline::windows(&recording.replies, SCAN_WORDS, &recording.names, |s| outline::unnamed_speaker(language, s));
    let key = cache_key(instruction, parts.len());
    if let Some(cached) = transcript.and_then(|t| t.meta(&key).ok().flatten()).and_then(|v| serde_json::from_str::<Scan>(&v).ok()) {
        return Ok(cached);
    }
    let mut result = Scan { facts: Vec::new(), parts: parts.len(), failed: Vec::new() };
    for (i, part) in parts.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            anyhow::bail!("stopped");
        }
        match extract.extract(instruction, part, i, parts.len(), cancel) {
            Ok(found) => {
                for (time, text) in found {
                    let t = place(&text, parse_time(&time), part, &recording.replies);
                    if !result.facts.iter().any(|f| f.text == text) {
                        result.facts.push(Fact { time: t, text });
                    }
                }
            }
            Err(e) => result.failed.push((part.start, part.end, format!("{e:#}").chars().take(80).collect())),
        }
        progress(i + 1, parts.len());
    }
    result.facts.sort_by(|a, b| a.time.total_cmp(&b.time));
    // Only a complete scan is reused.
    if let (Some(t), true) = (transcript, result.failed.is_empty()) {
        if let Err(e) = t.set_meta(&key, &serde_json::to_string(&result)?) {
            log::warn!("scan cache: {e:#}");
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::tools::tests::recording;
    use super::*;
    use std::cell::Cell;

    /// Finds «отчёт» in a part; fails on the part starting at 5:00.
    struct Fake {
        calls: Cell<usize>,
    }

    impl Extract for Fake {
        fn extract(&self, _: &str, part: &Window, _: usize, _: usize, _: &AtomicBool) -> anyhow::Result<Vec<(String, String)>> {
            self.calls.set(self.calls.get() + 1);
            anyhow::ensure!(part.start < 299.0, "the model broke");
            Ok(if part.text.contains("Отчёт") { vec![("0:10".into(), "Отчёт в пятницу".into()), ("59:00".into(), "Ещё раз про отчёт".into())] } else { vec![] })
        }
    }

    #[test]
    fn scans_every_part_reports_gaps_and_caches() {
        let mut rec = recording();
        // Three parts: small windows for the test.
        rec.replies = rec.replies.into_iter().map(|mut r| {
            r.words = (0..SCAN_WORDS).map(|i| mediaa_core::transcript::Word { text: if i == 0 { r.words[0].text.clone() } else { "слово".into() }, start: r.start, end: r.start + 1.0 }).collect();
            r
        }).collect();
        let dir = tempfile::tempdir().unwrap();
        let t = Transcript::open(dir.path(), "f").unwrap();
        let fake = Fake { calls: Cell::new(0) };
        let mut seen = Vec::new();
        let s = scan(&fake, &rec, Some(&t), "все сроки", &AtomicBool::new(false), &mut |d, n| seen.push((d, n))).unwrap();
        assert_eq!(seen, [(1, 3), (2, 3), (3, 3)]);
        assert_eq!(s.parts, 3);
        assert_eq!(s.failed.len(), 1, "the part at 5:00");
        assert_eq!(s.facts.iter().map(|f| (f.time, f.text.as_str())).collect::<Vec<_>>(), [(10.0, "Отчёт в пятницу"), (10.0, "Ещё раз про отчёт")], "a time outside its part is the part's start");
        let text = s.text("все сроки");
        assert!(text.starts_with("Scanned 2 of 3 parts for «все сроки»: 2 findings. NOT checked: [5:00–"), "{text}");
        // An incomplete scan is not kept; a complete one is.
        assert!(t.meta(&cache_key("все сроки", 3)).unwrap().is_none());
        rec.replies.truncate(2);
        let s = scan(&fake, &rec, Some(&t), "Все  СРОКИ", &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert!(s.failed.is_empty());
        let calls = fake.calls.get();
        let again = scan(&fake, &rec, Some(&t), "все сроки", &AtomicBool::new(false), &mut |_, _| {}).unwrap();
        assert_eq!((again, fake.calls.get()), (s, calls), "from the cache, no model calls");
        assert!(scan(&fake, &rec, None, "x", &AtomicBool::new(true), &mut |_, _| {}).is_err(), "stopped");
    }

    #[test]
    fn findings_are_placed_by_their_words() {
        let rec = recording();
        let part = Window { start: 0.0, end: 600.0, text: String::new() };
        // A paraphrase of the reply at 5:00 («Бюджет на квартал утвердили.»), the model said 0:11.
        assert_eq!(place("Утвердили бюджет на следующий квартал", Some(11.0), &part, &rec.replies), 300.0);
        // The word itself, not its reply's start: «пятницу» is the 4th word of the reply at 0:10.
        assert_eq!(place("Сдача в пятницу", None, &part, &rec.replies), 13.0);
        // Nothing shared: the model's time when inside the part, else the part's start.
        assert_eq!(place("Погода отличная", Some(42.0), &part, &rec.replies), 42.0);
        assert_eq!(place("Погода отличная", Some(4000.0), &part, &rec.replies), 0.0);
    }

    #[test]
    fn facts_out_of_answers() {
        assert_eq!(read_facts(r#"{"facts":[{"time":"1:46","text":" самозанятость "},{"time":"","text":""}]}"#).unwrap(), [("1:46".to_string(), "самозанятость".to_string())]);
        assert_eq!(read_facts("Вот:\n```json\n{\"facts\": []}\n```").unwrap(), []);
        assert!(read_facts("ничего").is_err());
        assert!(llama_cpp_2::json_schema_to_grammar(&facts_schema()).is_ok());
    }
}
