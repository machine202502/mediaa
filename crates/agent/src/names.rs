use crate::prompts;
use crate::run::{Host, Stage, Work};
use crate::scope::Scope;
use mediaa_core::agent::AgentSettings;
use mediaa_core::speakers;
use mediaa_core::transcript::{self, NameEvidence, NameSource, Reply, Transcript};
use mediaa_engine::{ChatMessage, ChatRequest, Engine};
use mediaa_queue::{NewTask, Resource, Scheduler, Task, TaskId};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const KIND: &str = "speaker-names";

const WINDOW_SECS: f64 = 240.0;
const OVERLAP_SECS: f64 = 60.0;
const INTRO_SECS: f64 = 600.0;
const MAIN_SECS: f64 = 60.0;
const MAIN_SHARE: f64 = 0.1;
const MIN_SCORE: f64 = 3.0;
const LEAD: f64 = 2.0;
const SELF: f64 = 3.0;
const INTRODUCED: f64 = 2.0;
const ADDRESSED: f64 = 2.0;
const NICKNAME: f64 = 1.5;
const LOW: f64 = 0.5;
const MAX_TOKENS: u32 = 4096;
const NAME_CHARS: usize = 60;
const TIME_SLACK: f64 = 2.0;
const EPS: f64 = 1e-9;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub path: PathBuf,
}

pub fn model(settings: &AgentSettings) -> Option<mediaa_core::agent::ModelSource> {
    settings.models.agent.clone()
}

pub fn latest<'a>(tasks: &'a [Task], file_id: &str) -> Option<&'a Task> {
    tasks
        .iter()
        .filter(|task| {
            task.parent.is_none() && task.kind == KIND && task.scope.as_deref() == Some(file_id)
        })
        .max_by_key(|task| task.id)
}

pub fn enqueue(
    sched: &Scheduler,
    ai_dir: &Path,
    path: &Path,
    settings: &AgentSettings,
) -> anyhow::Result<TaskId> {
    let id = transcript::file_id(path)?;
    anyhow::ensure!(
        Transcript::exists(ai_dir, &id),
        "{} has no transcript",
        path.display()
    );
    let model =
        model(settings).ok_or_else(|| anyhow::anyhow!("the analysis model is not local"))?;
    let t = Transcript::open(ai_dir, &id)?;
    let dialog = t.dialog()?;
    anyhow::ensure!(
        dialog.iter().any(|reply| reply.speaker.is_some()),
        "{} has no speakers",
        path.display()
    );
    let minutes = dialog.last().map_or(0.0, |reply| reply.end) / 60.0;
    let title = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let task = NewTask::new(KIND, &title, &json!({ "path": path }))
        .resource(Resource::Llm)
        .scope(&id)
        .analysis(&id)
        .model(&model.model)
        .units(minutes);
    Ok(sched.enqueue_once(task)?)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Window {
    pub index: usize,
    pub start: f64,
    pub end: f64,
}

pub fn plan(total: f64) -> Vec<Window> {
    let step = WINDOW_SECS - OVERLAP_SECS;
    let mut starts = Vec::new();
    let mut start = 0.0;
    while start < total {
        starts.push(start);
        start += step;
    }
    let mut picks: Vec<f64> = starts.iter().copied().filter(|s| *s < INTRO_SECS).collect();
    picks.extend(starts.iter().copied().filter(|s| *s >= INTRO_SECS));
    picks
        .into_iter()
        .enumerate()
        .map(|(index, start)| Window {
            index,
            start,
            end: (start + WINDOW_SECS).min(total),
        })
        .collect()
}

pub fn covered(windows: &[Window]) -> f64 {
    let mut spans: Vec<(f64, f64)> = windows.iter().map(|w| (w.start, w.end)).collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut total = 0.0;
    let mut end = f64::NEG_INFINITY;
    for (start, stop) in spans {
        let from = start.max(end).max(0.0);
        total += (stop - from).max(0.0);
        end = end.max(stop);
    }
    total
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Cursor {
    #[serde(default)]
    pub done: Vec<usize>,
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

impl Cursor {
    fn from_value(value: &Value) -> Cursor {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum How {
    #[serde(rename = "self")]
    SelfName,
    Addressed,
    Introduced,
    Nickname,
}

impl How {
    pub fn as_str(self) -> &'static str {
        match self {
            How::SelfName => "self",
            How::Addressed => "addressed",
            How::Introduced => "introduced",
            How::Nickname => "nickname",
        }
    }

    fn weight(self) -> f64 {
        match self {
            How::SelfName => SELF,
            How::Addressed => ADDRESSED,
            How::Introduced => INTRODUCED,
            How::Nickname => NICKNAME,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evidence {
    pub speaker: u32,
    pub name: String,
    pub how: How,
    pub time: f64,
    pub text: String,
    pub sure: bool,
}

impl Evidence {
    pub fn score(&self) -> f64 {
        self.how.weight() * if self.sure { 1.0 } else { LOW }
    }

    fn to_name_evidence(&self) -> NameEvidence {
        NameEvidence {
            start: self.time,
            text: self.text.clone(),
            how: self.how.as_str().to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Accepted {
    pub speaker: u32,
    pub name: String,
    pub score: f64,
    pub evidence: Vec<Evidence>,
}

fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn aggregate(evidence: &[Evidence], manual: &BTreeMap<u32, String>) -> Vec<Accepted> {
    let taken: BTreeSet<String> = manual.values().map(|name| key(name)).collect();
    let mut per: BTreeMap<u32, BTreeMap<String, (f64, Vec<Evidence>)>> = BTreeMap::new();
    for e in evidence {
        let name = key(&e.name);
        if name.is_empty() || manual.contains_key(&e.speaker) || taken.contains(&name) {
            continue;
        }
        let (score, shown) = per
            .entry(e.speaker)
            .or_default()
            .entry(name)
            .or_insert_with(|| (0.0, Vec::new()));
        *score += e.score();
        shown.push(e.clone());
    }
    let mut best: Vec<Accepted> = Vec::new();
    for (speaker, names) in per {
        let mut ranked: Vec<(f64, Vec<Evidence>)> = names.into_values().collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        let Some((score, shown)) = ranked.first() else {
            continue;
        };
        let second = ranked.get(1).map_or(0.0, |(score, _)| *score);
        if *score + EPS < MIN_SCORE || *score + EPS < LEAD * second {
            continue;
        }
        let name = shown
            .iter()
            .max_by(|a, b| a.score().total_cmp(&b.score()))
            .map(|e| e.name.trim().to_string())
            .unwrap_or_default();
        best.push(Accepted {
            speaker,
            name,
            score: *score,
            evidence: shown.clone(),
        });
    }
    best.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.speaker.cmp(&b.speaker)));
    let mut used = BTreeSet::new();
    best.retain(|accepted| used.insert(key(&accepted.name)));
    best.sort_by_key(|accepted| accepted.speaker);
    best
}

pub fn main_speakers(dialog: &[Reply]) -> BTreeSet<u32> {
    let mut spoken: BTreeMap<u32, f64> = BTreeMap::new();
    let mut all = 0.0;
    for reply in dialog {
        let Some(speaker) = reply.speaker else {
            continue;
        };
        let spoken_secs = (reply.end - reply.start).max(0.0);
        all += spoken_secs;
        *spoken.entry(speaker).or_default() += spoken_secs;
    }
    spoken
        .into_iter()
        .filter(|(_, secs)| *secs >= MAIN_SECS || *secs + EPS >= MAIN_SHARE * all)
        .map(|(speaker, _)| speaker)
        .collect()
}

pub fn all_main_named(
    dialog: &[Reply],
    evidence: &[Evidence],
    manual: &BTreeMap<u32, String>,
) -> bool {
    let named: BTreeSet<u32> = manual
        .keys()
        .copied()
        .chain(aggregate(evidence, manual).into_iter().map(|a| a.speaker))
        .collect();
    main_speakers(dialog).is_subset(&named)
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawEvidence {
    #[serde(default)]
    speaker: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    how: String,
    #[serde(default)]
    time: String,
    #[serde(default)]
    sure: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct RawAnswer {
    #[serde(default)]
    evidence: Vec<RawEvidence>,
}

fn evidence_array_at(text: &str) -> Option<usize> {
    let key = text.find("\"evidence\"")?;
    let after = key + "\"evidence\"".len();
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

fn closed_evidence(text: &str) -> RawAnswer {
    let mut evidence = Vec::new();
    let Some(mut i) = evidence_array_at(text) else {
        return RawAnswer { evidence };
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
        let Ok(item) = serde_json::from_str::<RawEvidence>(&text[i..end]) else {
            break;
        };
        evidence.push(item);
        i = end;
    }
    RawAnswer { evidence }
}

pub fn parse_time(text: &str) -> Option<f64> {
    let parts: Vec<&str> = text.trim().split(':').map(str::trim).collect();
    let nums: Option<Vec<u32>> = parts.iter().map(|part| part.parse::<u32>().ok()).collect();
    match nums.as_deref() {
        Some([minutes, seconds]) if *seconds < 60 => {
            Some(f64::from(*minutes) * 60.0 + f64::from(*seconds))
        }
        Some([hours, minutes, seconds]) if *minutes < 60 && *seconds < 60 => {
            Some(f64::from(*hours) * 3600.0 + f64::from(*minutes) * 60.0 + f64::from(*seconds))
        }
        _ => None,
    }
}

fn parse_how(text: &str) -> Option<How> {
    match text.trim().to_lowercase().as_str() {
        "self" => Some(How::SelfName),
        "addressed" => Some(How::Addressed),
        "introduced" => Some(How::Introduced),
        "nickname" => Some(How::Nickname),
        _ => None,
    }
}

fn parse_speaker(text: &str) -> Option<u32> {
    let parsed = speakers::parse_label(text)?;
    if parsed.recording.is_some() {
        return None;
    }
    Some(parsed.index)
}

fn line_at(dialog: &[Reply], time: f64) -> Option<usize> {
    dialog
        .iter()
        .enumerate()
        .filter(|(_, reply)| (reply.start - time).abs() <= TIME_SLACK)
        .min_by(|(_, a), (_, b)| {
            (a.start - time)
                .abs()
                .total_cmp(&(b.start - time).abs())
                .then(a.start.total_cmp(&b.start))
        })
        .map(|(at, _)| at)
}

fn answers_within(dialog: &[Reply], at: usize, speaker: u32, within: usize) -> bool {
    let line = &dialog[at];
    if line.speaker.is_none_or(|s| s == speaker) {
        return false;
    }
    dialog
        .iter()
        .skip(at + 1)
        .take(within)
        .any(|reply| reply.speaker == Some(speaker))
}

fn check(raw: &RawEvidence, dialog: &[Reply]) -> Option<Evidence> {
    let how = parse_how(&raw.how)?;
    let speaker = parse_speaker(&raw.speaker)?;
    let name = raw.name.trim();
    if name.is_empty()
        || name.chars().count() > NAME_CHARS
        || name.contains(['\n', '\r'])
        || looks_like_label(name)
    {
        return None;
    }
    let time = parse_time(&raw.time)?;
    let at = line_at(dialog, time)?;
    let line = &dialog[at];
    let text = line.text();
    let found = name_in_line(&text, name);
    let holds = match how {
        How::SelfName => line.speaker == Some(speaker),
        How::Addressed => answers_within(dialog, at, speaker, 2),
        How::Introduced => answers_within(dialog, at, speaker, 3),
        How::Nickname => {
            found.is_some()
                && (line.speaker == Some(speaker) || answers_within(dialog, at, speaker, 2))
        }
    };
    holds.then(|| Evidence {
        speaker,
        name: found.unwrap_or_else(|| name.to_string()),
        how,
        time: line.start,
        text,
        sure: !raw.sure.trim().eq_ignore_ascii_case("low"),
    })
}

fn looks_like_label(name: &str) -> bool {
    let bare = name.trim().trim_start_matches('@');
    bare.len() > 1
        && matches!(bare.as_bytes()[0], b's' | b'S')
        && bare[1..].bytes().all(|byte| byte.is_ascii_digit())
}

fn comparable(word: &str) -> String {
    deunicode::deunicode(word.trim_matches(|c: char| !c.is_alphanumeric())).to_lowercase()
}

fn words(line: &str) -> Vec<(std::ops::Range<usize>, &str)> {
    let mut found = Vec::new();
    let mut start = None;
    for (at, c) in line.char_indices() {
        if c.is_whitespace() {
            if let Some(from) = start.take() {
                found.push((from..at, &line[from..at]));
            }
        } else if start.is_none() {
            start = Some(at);
        }
    }
    if let Some(from) = start {
        found.push((from..line.len(), &line[from..]));
    }
    found
}

fn name_in_line(line: &str, name: &str) -> Option<String> {
    let wanted: Vec<String> = name
        .split_whitespace()
        .map(comparable)
        .filter(|word| !word.is_empty())
        .collect();
    if wanted.is_empty() {
        return None;
    }
    if line.to_lowercase().contains(&name.to_lowercase()) {
        return Some(name.to_string());
    }
    let target = wanted.join(" ");
    let words = words(line);
    (0..words.len()).find_map(|at| {
        let shown: Vec<&str> = words[at..]
            .iter()
            .take(wanted.len())
            .map(|(_, word)| *word)
            .collect();
        if shown.len() < wanted.len()
            || shown
                .iter()
                .map(|word| comparable(word))
                .collect::<Vec<_>>()
                .join(" ")
                != target
        {
            return None;
        }
        Some(
            shown
                .iter()
                .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
                .collect::<Vec<_>>()
                .join(" "),
        )
    })
}

fn shown(item: &Evidence, window: &Window) -> bool {
    item.time >= window.start && item.time < window.end
}

fn stamp(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn line_text(reply: &Reply) -> String {
    match reply.speaker {
        Some(speaker) => format!(
            "[{}] {}: {}",
            stamp(reply.start),
            speakers::label(speaker),
            reply.text()
        ),
        None => format!("[{}] {}", stamp(reply.start), reply.text()),
    }
}

fn known_names(evidence: &[Evidence], manual: &BTreeMap<u32, String>) -> Vec<Value> {
    let mut known: BTreeMap<u32, String> = manual.clone();
    for accepted in aggregate(evidence, manual) {
        known.entry(accepted.speaker).or_insert(accepted.name);
    }
    known
        .into_iter()
        .map(|(speaker, name)| json!({ "label": speakers::label(speaker), "name": name }))
        .collect()
}

pub struct SpeakerNames {
    ai_dir: PathBuf,
    settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
}

struct Ask<'a> {
    window: &'a Window,
    dialog: &'a [Reply],
    picked: &'a [&'a Reply],
    known: &'a [Value],
    cancel: &'a AtomicBool,
}

impl SpeakerNames {
    pub fn new(
        ai_dir: &Path,
        settings: Arc<dyn Fn() -> AgentSettings + Send + Sync>,
    ) -> SpeakerNames {
        SpeakerNames {
            ai_dir: ai_dir.to_path_buf(),
            settings,
        }
    }

    fn read(
        &self,
        engine: &dyn Engine,
        model: &mediaa_core::agent::ModelSource,
        ask: Ask<'_>,
    ) -> anyhow::Result<Vec<Evidence>> {
        let Ask {
            window,
            dialog,
            picked,
            known,
            cancel,
        } = ask;
        let labels: Vec<String> = picked
            .iter()
            .filter_map(|reply| reply.speaker)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(speakers::label)
            .collect();
        let lines: Vec<String> = picked.iter().map(|reply| line_text(reply)).collect();
        let user = prompts::render(
            "speakers/names.j2",
            minijinja::context! {
                start => stamp(window.start),
                end => stamp(window.end),
                speakers => labels,
                known => known,
                lines => lines,
            },
        );
        let request = ChatRequest {
            source: model.clone(),
            messages: vec![
                ChatMessage::system(prompts::render("speakers/names_system.j2", ())),
                ChatMessage::user(user),
            ],
            max_tokens: MAX_TOKENS,
            temperature: 0.0,
            json_schema: Some(serde_json::from_str(EVIDENCE_SCHEMA)?),
            ..ChatRequest::default()
        };
        let answer = engine.chat(&request, cancel, &mut |_| true)?;
        let text = answer.content.trim();
        let raw: RawAnswer = match serde_json::from_str(text) {
            Ok(raw) => raw,
            Err(e) => {
                let raw = closed_evidence(text);
                log::warn!(
                    "names: the model's answer is not valid JSON ({e}); kept {} complete items",
                    raw.evidence.len()
                );
                raw
            }
        };
        Ok(raw
            .evidence
            .iter()
            .filter_map(|item| check(item, dialog))
            .filter(|item| shown(item, window))
            .collect())
    }
}

const EVIDENCE_SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "evidence": {
      "type": "array",
      "maxItems": 5,
      "items": {
        "type": "object",
        "properties": {
          "speaker": { "type": "string" },
          "name": { "type": "string" },
          "how": { "type": "string", "enum": ["self", "addressed", "introduced", "nickname"] },
          "time": { "type": "string" },
          "sure": { "type": "string", "enum": ["high", "low"] }
        },
        "required": ["speaker", "name", "how", "time", "sure"]
      }
    }
  },
  "required": ["evidence"]
}"#;

impl Stage for SpeakerNames {
    fn handles(&self, kind: &str) -> bool {
        kind == KIND
    }

    fn chunk(&self, task: &Task, cancel: &Arc<AtomicBool>, host: &Host) -> anyhow::Result<Work> {
        let input: Input = serde_json::from_value(task.input.clone())?;
        let settings = (self.settings)();
        let id = transcript::file_id(&input.path)?;
        anyhow::ensure!(
            Transcript::exists(&self.ai_dir, &id),
            "{} has no transcript",
            input.path.display()
        );
        let Some(model) = model(&settings) else {
            log::warn!(
                "{}: no local analysis model; the speakers stay unnamed",
                input.path.display()
            );
            return Ok(Work::Done(
                json!({ "file_id": id, "skipped": "the analysis model is not local" }),
            ));
        };
        let mut t = Transcript::open(&self.ai_dir, &id)?;
        let dialog = t.dialog()?;
        let total = dialog.last().map_or(0.0, |reply| reply.end);
        if !dialog.iter().any(|reply| reply.speaker.is_some()) || total <= 0.0 {
            return Ok(Work::Done(
                json!({ "file_id": id, "skipped": "there are no speakers" }),
            ));
        }
        let plan = plan(total);
        let mut cursor = Cursor::from_value(&task.cursor);
        let manual: BTreeMap<u32, String> = t
            .speaker_entries()?
            .into_iter()
            .filter(|(_, name)| name.source == NameSource::Manual)
            .map(|(speaker, name)| (speaker, name.name))
            .collect();
        let Some(window) = plan
            .iter()
            .find(|window| !cursor.done.contains(&window.index))
        else {
            return finish(&mut t, &id, &cursor, &plan, &manual, total, false);
        };
        let picked: Vec<&Reply> = dialog
            .iter()
            .filter(|reply| reply.start >= window.start && reply.start < window.end)
            .collect();
        let known = known_names(&cursor.evidence, &manual);
        let evidence = if picked.is_empty() || cancel.load(Ordering::Relaxed) {
            Vec::new()
        } else {
            let scope = Scope::attach(
                host.store.clone(),
                task.id,
                host.engine.clone(),
                task.scope.clone(),
            );
            let engine = scope.engine();
            match self.read(
                engine.as_ref(),
                &model,
                Ask {
                    window,
                    dialog: &dialog,
                    picked: &picked,
                    known: &known,
                    cancel,
                },
            ) {
                Ok(evidence) => evidence,
                Err(_) if cancel.load(Ordering::Relaxed) => return Ok(Work::Stopped),
                Err(e) => return Err(e),
            }
        };
        if cancel.load(Ordering::Relaxed) {
            return Ok(Work::Stopped);
        }
        for item in evidence {
            let seen = cursor.evidence.iter().any(|old| {
                old.speaker == item.speaker
                    && key(&old.name) == key(&item.name)
                    && (old.time - item.time).abs() < 1.0
            });
            if !seen {
                cursor.evidence.push(item);
            }
        }
        if !cursor.done.contains(&window.index) {
            cursor.done.push(window.index);
        }
        let read = covered(
            &plan
                .iter()
                .copied()
                .filter(|window| cursor.done.contains(&window.index))
                .collect::<Vec<_>>(),
        );
        let progress = if total > 0.0 {
            (read / total).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        if all_main_named(&dialog, &cursor.evidence, &manual) {
            return finish(&mut t, &id, &cursor, &plan, &manual, total, true);
        }
        Ok(Work::More {
            cursor: serde_json::to_value(&cursor)?,
            partial: Value::Null,
            progress,
        })
    }
}

fn finish(
    t: &mut Transcript,
    id: &str,
    cursor: &Cursor,
    plan: &[Window],
    manual: &BTreeMap<u32, String>,
    total: f64,
    stopped_early: bool,
) -> anyhow::Result<Work> {
    let accepted = aggregate(&cursor.evidence, manual);
    t.clear_auto_names()?;
    for name in &accepted {
        let evidence: Vec<NameEvidence> = name
            .evidence
            .iter()
            .map(Evidence::to_name_evidence)
            .collect();
        t.set_auto_name(name.speaker, &name.name, &evidence)?;
    }
    let read: Vec<Window> = plan
        .iter()
        .copied()
        .filter(|window| cursor.done.contains(&window.index))
        .collect();
    let read_secs = covered(&read);
    let names: Vec<Value> = accepted
        .iter()
        .map(|name| json!({ "speaker": speakers::label(name.speaker), "name": name.name, "score": name.score }))
        .collect();
    let evidence: Vec<Value> = cursor
        .evidence
        .iter()
        .map(|e| {
            json!({
                "speaker": speakers::label(e.speaker),
                "name": e.name,
                "how": e.how.as_str(),
                "time": stamp(e.time),
                "text": e.text,
                "sure": if e.sure { "high" } else { "low" },
            })
        })
        .collect();
    Ok(Work::Done(json!({
        "file_id": id,
        "names": names,
        "evidence": evidence,
        "read_secs": read_secs,
        "total_secs": total,
        "windows": { "done": read.len(), "all": plan.len() },
        "stopped_early": stopped_early,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::{AgentSettings, Budget};
    use mediaa_core::transcript::{Reply, Word};
    use mediaa_engine::testing::FakeEngine;
    use mediaa_engine::Engine;
    use mediaa_queue::Status as TaskStatus;
    use std::time::Duration;

    fn reply(start: f64, end: f64, speaker: u32, text: &str) -> Reply {
        Reply {
            start,
            end,
            speaker: Some(speaker),
            words: text
                .split_whitespace()
                .enumerate()
                .map(|(i, w)| Word {
                    text: w.into(),
                    start: start + i as f64 * 0.1,
                    end: start + i as f64 * 0.1 + 0.1,
                })
                .collect(),
        }
    }

    fn raw(speaker: &str, name: &str, how: &str, time: &str, sure: &str) -> RawEvidence {
        RawEvidence {
            speaker: speaker.into(),
            name: name.into(),
            how: how.into(),
            time: time.into(),
            sure: sure.into(),
        }
    }

    fn evidence(speaker: u32, name: &str, how: How, sure: bool) -> Evidence {
        Evidence {
            speaker,
            name: name.into(),
            how,
            time: 0.0,
            text: String::new(),
            sure,
        }
    }

    #[test]
    fn windows_start_with_the_first_ten_minutes() {
        let windows = plan(1620.0);
        let starts: Vec<f64> = windows.iter().map(|w| w.start).collect();
        assert_eq!(
            starts,
            [0.0, 180.0, 360.0, 540.0, 720.0, 900.0, 1080.0, 1260.0, 1440.0]
        );
        assert_eq!(windows[2].end, 600.0, "a window is four minutes");
        assert_eq!(
            windows.last().unwrap().end,
            1620.0,
            "the last window ends with the recording"
        );
        assert_eq!(
            windows.iter().map(|w| w.index).collect::<Vec<_>>(),
            (0..9).collect::<Vec<_>>()
        );
        assert!((covered(&windows) - 1620.0).abs() < 1e-6);
        assert!((covered(&windows[..3]) - 600.0).abs() < 1e-6);
        assert_eq!(
            plan(100.0),
            [Window {
                index: 0,
                start: 0.0,
                end: 100.0
            }]
        );
        assert_eq!(plan(0.0), Vec::<Window>::new());
    }

    #[test]
    fn a_time_is_minutes_and_seconds_or_hours() {
        assert_eq!(parse_time("0:05"), Some(5.0));
        assert_eq!(parse_time(" 12:34 "), Some(754.0));
        assert_eq!(parse_time("1:02:03"), Some(3723.0));
        assert_eq!(parse_time("1:60"), None);
        assert_eq!(parse_time("12"), None);
        assert_eq!(parse_time(""), None);
        assert_eq!(stamp(5.0), "0:05");
        assert_eq!(stamp(754.0), "12:34");
        assert_eq!(stamp(3723.0), "1:02:03");
    }

    #[test]
    fn every_way_must_hold_in_the_text() {
        let dialog = [
            reply(0.0, 2.0, 0, "Меня зовут Антон."),
            reply(10.0, 12.0, 1, "Антон, ты где?"),
            reply(13.0, 14.0, 0, "Я здесь."),
            reply(30.0, 32.0, 2, "Это Антон, наш новый коллега."),
            reply(34.0, 35.0, 0, "Спасибо."),
            reply(50.0, 52.0, 1, "Антон в отпуске, пишите @anton_dev."),
        ];
        assert_eq!(
            check(&raw("@s1", "Антон", "self", "0:00", "high"), &dialog),
            Some(Evidence {
                speaker: 0,
                name: "Антон".into(),
                how: How::SelfName,
                time: 0.0,
                text: "Меня зовут Антон.".into(),
                sure: true,
            })
        );
        let addressed = check(&raw("@s1", "Антон", "addressed", "0:10", "high"), &dialog).unwrap();
        assert_eq!(
            (addressed.speaker, addressed.how, addressed.time),
            (0, How::Addressed, 10.0)
        );
        let introduced =
            check(&raw("@s1", "Антон", "introduced", "0:30", "high"), &dialog).unwrap();
        assert_eq!((introduced.how, introduced.time), (How::Introduced, 30.0));
        let nicknamed = [
            reply(0.0, 2.0, 1, "Это @anton_dev, наш админ."),
            reply(3.0, 4.0, 0, "Да, это я."),
        ];
        let nickname = check(
            &raw("@s1", "anton_dev", "nickname", "0:00", "low"),
            &nicknamed,
        )
        .unwrap();
        assert_eq!(
            (nickname.name.as_str(), nickname.how, nickname.sure),
            ("anton_dev", How::Nickname, false)
        );
        let own_nickname = [reply(0.0, 2.0, 0, "Пишите мне в @anton_dev.")];
        assert!(check(
            &raw("@s1", "anton_dev", "nickname", "0:00", "high"),
            &own_nickname
        )
        .is_some());
        assert_eq!(
            check(&raw("@s1", "anton_dev", "nickname", "0:50", "low"), &dialog),
            None
        );

        assert_eq!(
            check(&raw("@s2", "Антон", "self", "0:00", "high"), &dialog),
            None
        );
        assert_eq!(
            check(&raw("@s1", "Антон", "self", "5:00", "high"), &dialog),
            None
        );
        let talked = [
            reply(0.0, 2.0, 1, "Антон в отпуске."),
            reply(3.0, 4.0, 2, "Понятно."),
        ];
        assert_eq!(
            check(&raw("@s1", "Антон", "addressed", "0:00", "high"), &talked),
            None
        );
        assert_eq!(
            check(&raw("@s1", "Антон", "introduced", "0:00", "high"), &talked),
            None
        );
        assert_eq!(
            check(
                &raw("@s1", "anton_dev", "nickname", "0:00", "high"),
                &dialog
            ),
            None
        );
        assert_eq!(
            check(&raw("Антон", "Антон", "self", "0:00", "high"), &dialog),
            None
        );
        assert_eq!(
            check(&raw("@s1", "Антон", "guessed", "0:00", "high"), &dialog),
            None
        );
        assert_eq!(
            check(&raw("@s1", "  ", "self", "0:00", "high"), &dialog),
            None
        );
        assert_eq!(
            check(
                &raw("@s1", "Антон\nПетров", "self", "0:00", "high"),
                &dialog
            ),
            None
        );
        assert_eq!(
            check(&raw("@s1", "s1", "self", "0:00", "high"), &dialog),
            None
        );
        assert_eq!(
            check(&raw("@s1", " @S12 ", "self", "0:00", "high"), &dialog),
            None
        );
        assert!(check(&raw("@s1", "Антон", "self", "0:02", "high"), &dialog).is_some());
    }

    #[test]
    fn a_report_about_another_part_is_not_evidence() {
        let dialog = [
            reply(0.0, 2.0, 0, "Меня зовут Антон."),
            reply(300.0, 302.0, 1, "Меня зовут Дана."),
        ];
        let window = Window {
            index: 0,
            start: 0.0,
            end: 240.0,
        };
        let near = check(&raw("@s1", "Антон", "self", "0:00", "high"), &dialog).unwrap();
        assert!(shown(&near, &window));
        let far = check(&raw("@s2", "Дана", "self", "5:00", "high"), &dialog).unwrap();
        assert!(
            !shown(&far, &window),
            "the model never saw this line in this window"
        );
    }

    #[test]
    fn a_name_in_another_alphabet_takes_the_spelling_of_the_line() {
        let dialog = [
            reply(0.0, 5.0, 0, "Так и скажите: Назаров, ты забанен. Это был Антон Назаров, и начните уже зарабатывать больше."),
            reply(10.0, 12.0, 1, "Антон Назаров, расскажи."),
            reply(13.0, 15.0, 0, "Рассказываю."),
        ];
        let self_name = check(
            &raw("@s1", "Anton Nazarov", "self", "0:00", "high"),
            &dialog,
        )
        .unwrap();
        assert_eq!(
            (self_name.name.as_str(), self_name.how),
            ("Антон Назаров", How::SelfName)
        );
        let addressed = check(
            &raw("@s1", "Anton Nazarov", "addressed", "0:10", "high"),
            &dialog,
        )
        .unwrap();
        assert_eq!(addressed.name, "Антон Назаров");
        let same = check(
            &raw("@s1", "Антон Назаров", "self", "0:00", "high"),
            &dialog,
        )
        .unwrap();
        assert_eq!(same.name, "Антон Назаров");
        let other = [reply(0.0, 2.0, 0, "Меня зовут Антон.")];
        assert_eq!(
            check(&raw("@s1", "Anton Nazarov", "self", "0:00", "high"), &other)
                .unwrap()
                .name,
            "Anton Nazarov"
        );
    }

    #[test]
    fn weights_threshold_and_the_two_times_rule() {
        let many = [
            evidence(0, "Антон", How::SelfName, true),
            evidence(0, "Антон", How::Addressed, true),
        ];
        assert_eq!(aggregate(&many, &BTreeMap::new()).len(), 1);
        assert_eq!(
            aggregate(
                &[evidence(0, "Антон", How::SelfName, true)],
                &BTreeMap::new()
            )
            .len(),
            1
        );
        assert_eq!(
            aggregate(
                &[evidence(0, "Антон", How::Nickname, true)],
                &BTreeMap::new()
            )
            .len(),
            0
        );
        assert_eq!(
            aggregate(
                &[evidence(0, "Антон", How::SelfName, false)],
                &BTreeMap::new()
            )
            .len(),
            0
        );
        assert_eq!(
            aggregate(
                &[
                    evidence(0, "Антон", How::SelfName, false),
                    evidence(0, "Антон", How::Introduced, true)
                ],
                &BTreeMap::new()
            )
            .len(),
            1
        );
        let two = [
            evidence(0, "Антон", How::SelfName, true),
            evidence(0, "Олег", How::Addressed, true),
        ];
        assert_eq!(
            aggregate(&two, &BTreeMap::new()).len(),
            0,
            "3 against 2 is not twice"
        );
        let lead = [
            evidence(0, "Антон", How::SelfName, true),
            evidence(0, "Олег", How::Nickname, true),
        ];
        let accepted = aggregate(&lead, &BTreeMap::new());
        assert_eq!(
            (accepted.len(), accepted[0].name.as_str(), accepted[0].score),
            (1, "Антон", 3.0)
        );
        let variants = [
            evidence(0, "Антон", How::SelfName, true),
            evidence(0, " антон ", How::SelfName, true),
        ];
        let accepted = aggregate(&variants, &BTreeMap::new());
        assert_eq!((accepted.len(), accepted[0].score), (1, 6.0));
        let both = [
            evidence(0, "Антон", How::SelfName, true),
            evidence(0, "Антон", How::Introduced, true),
            evidence(1, "Антон", How::SelfName, true),
        ];
        let accepted = aggregate(&both, &BTreeMap::new());
        assert_eq!(
            accepted
                .iter()
                .map(|a| (a.speaker, a.name.as_str()))
                .collect::<Vec<_>>(),
            [(0, "Антон")]
        );
        assert_eq!(accepted[0].evidence.len(), 2);
    }

    #[test]
    fn the_users_names_are_never_changed_or_reused() {
        let manual = BTreeMap::from([(0, "Антон".to_string())]);
        let evidence = [
            evidence(0, "Олег", How::SelfName, true),
            evidence(1, "Антон", How::SelfName, true),
            evidence(1, "Дана", How::SelfName, true),
        ];
        let accepted = aggregate(&evidence, &manual);
        assert_eq!(
            accepted
                .iter()
                .map(|a| (a.speaker, a.name.as_str()))
                .collect::<Vec<_>>(),
            [(1, "Дана")]
        );
    }

    #[test]
    fn the_run_stops_when_the_main_speakers_have_names() {
        let dialog = [
            reply(0.0, 70.0, 0, "долгая речь"),
            reply(70.0, 75.0, 1, "короткая"),
        ];
        assert_eq!(main_speakers(&dialog), BTreeSet::from([0]));
        let named = [evidence(0, "Антон", How::SelfName, true)];
        assert!(all_main_named(&dialog, &named, &BTreeMap::new()));
        assert!(!all_main_named(&dialog, &[], &BTreeMap::new()));
        let dialog = [reply(0.0, 90.0, 0, "речь"), reply(90.0, 100.0, 1, "ещё")];
        assert_eq!(main_speakers(&dialog), BTreeSet::from([0, 1]));
        assert!(!all_main_named(&dialog, &named, &BTreeMap::new()));
        let dialog = [
            reply(0.0, 100.0, 0, "речь"),
            reply(100.0, 165.0, 1, "ещё"),
            reply(165.0, 170.0, 2, "мало"),
        ];
        assert_eq!(main_speakers(&dialog), BTreeSet::from([0, 1]));
        assert!(all_main_named(
            &dialog,
            &named,
            &BTreeMap::from([(1, "Дана".to_string())])
        ));
    }

    #[test]
    fn the_prompt_lists_the_names_found_so_far() {
        let manual = BTreeMap::from([(1, "Дана".to_string())]);
        let known = known_names(&[evidence(0, "Антон", How::SelfName, true)], &manual);
        assert_eq!(
            known,
            [
                json!({ "label": "@s1", "name": "Антон" }),
                json!({ "label": "@s2", "name": "Дана" })
            ]
        );
    }

    fn test_settings() -> AgentSettings {
        let mut settings = AgentSettings::default();
        settings.models.agent = Some(mediaa_core::agent::ModelSource::new("c", "agent"));
        settings
    }

    fn current() -> Arc<dyn Fn() -> AgentSettings + Send + Sync> {
        Arc::new(test_settings)
    }

    fn recording(
        ai: &Path,
        path: &Path,
        minutes: f64,
        talk: (f64, f64),
        engine: Arc<dyn Engine>,
    ) -> Arc<mediaa_queue::Scheduler> {
        std::fs::write(path, b"x").unwrap();
        let id = transcript::file_id(path).unwrap();
        let mut t = Transcript::open(ai, &id).unwrap();
        let mut replies = Vec::new();
        let mut at = 0.0;
        let mut n = 0;
        while at < minutes * 60.0 {
            let end = (at + talk.0).min(minutes * 60.0);
            replies.push(reply(
                at,
                end,
                0,
                &format!("Реплика первого говорящего номер {n}."),
            ));
            at = end;
            let end = (at + talk.1).min(minutes * 60.0);
            replies.push(reply(
                at,
                end,
                1,
                &format!("Ответ второго говорящего номер {n}."),
            ));
            at = end;
            n += 1;
        }
        t.set_replies(&replies).unwrap();
        let sched = crate::run::open_scheduler(ai, &test_settings().budget).unwrap();
        crate::run::register_stages(
            &sched,
            ai,
            engine,
            crate::slots::Slots::new(Budget::default()),
            current(),
            Arc::new(crate::run::Streams::default()),
        );
        sched
    }

    fn drain(sched: &mediaa_queue::Scheduler) {
        crate::run::drain(sched, Duration::from_secs(60));
    }

    const NAMES_BOTH: &str = r#"{"evidence":[{"speaker":"@s1","name":"Антон","how":"self","time":"0:00","sure":"high"},{"speaker":"@s2","name":"Дана","how":"self","time":"0:20","sure":"high"}]}"#;
    const NAMES_FIRST: &str = r#"{"evidence":[{"speaker":"@s1","name":"Антон","how":"self","time":"0:00","sure":"high"}]}"#;

    #[test]
    fn names_land_in_the_transcript_as_automatic() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        let fake = FakeEngine::new();
        fake.chat_answers(&[NAMES_BOTH]);
        let sched = recording(&ai, &media, 1.0, (20.0, 5.0), Arc::new(fake.clone()));
        let id = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        drain(&sched);
        let task = sched.store().get(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        let result = task.result.clone().unwrap();
        assert_eq!(
            result["names"],
            json!([
                { "speaker": "@s1", "name": "Антон", "score": 3.0 },
                { "speaker": "@s2", "name": "Дана", "score": 3.0 },
            ])
        );
        assert_eq!(result["stopped_early"], json!(true));
        assert_eq!(result["windows"], json!({ "done": 1, "all": 1 }));

        let t = Transcript::open(&ai, &transcript::file_id(&media).unwrap()).unwrap();
        let entries = t.speaker_entries().unwrap();
        assert_eq!(entries[&0].name, "Антон");
        assert_eq!(entries[&0].source, NameSource::Auto);
        assert_eq!(entries[&0].evidence.len(), 1);
        assert_eq!(
            (
                entries[&0].evidence[0].start,
                entries[&0].evidence[0].how.as_str()
            ),
            (0.0, "self")
        );
        assert_eq!(entries[&1].name, "Дана");
        t.rename_speaker(0, "Антоша").unwrap();
        assert!(!t.set_auto_name(0, "Антон", &[]).unwrap());
        assert_eq!(t.speaker_names().unwrap()[&0], "Антоша");
    }

    #[test]
    fn one_run_per_recording() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        let sched = recording(&ai, &media, 1.0, (20.0, 5.0), Arc::new(FakeEngine::new()));
        let first = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        let second = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        assert_eq!(first, second, "the recording already has an unfinished run");
        let task = sched.store().get(first).unwrap().unwrap();
        let file = transcript::file_id(&media).unwrap();
        assert_eq!(
            (task.kind.as_str(), task.scope.as_deref()),
            (KIND, Some(file.as_str()))
        );
        assert!(task.model.is_some(), "the task keeps the model warm");
        let other = tmp.path().join("other.wav");
        std::fs::write(&other, b"x").unwrap();
        let mut t = Transcript::open(&ai, &transcript::file_id(&other).unwrap()).unwrap();
        t.set_replies(&[reply(0.0, 5.0, 0, "Привет.")]).unwrap();
        let third = enqueue(&sched, &ai, &other, &test_settings()).unwrap();
        assert_ne!(first, third);
    }

    #[test]
    fn the_run_reads_the_first_windows_and_stops() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        let fake = FakeEngine::new();
        fake.chat_answers(&[NAMES_FIRST]);
        let sched = recording(&ai, &media, 20.0, (20.0, 0.5), Arc::new(fake.clone()));
        let id = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        drain(&sched);
        let task = sched.store().get(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        let result = task.result.clone().unwrap();
        assert_eq!(result["stopped_early"], json!(true));
        assert_eq!(result["windows"]["done"], json!(1));
        assert!(result["windows"]["all"].as_u64().unwrap() > 4, "{result}");
        assert_eq!(result["read_secs"], json!(240.0));
        assert_eq!(fake.chats().len(), 1);
    }

    #[test]
    fn without_names_the_run_reads_to_the_end() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        let fake = FakeEngine::new();
        fake.chat_answers(&[r#"{"evidence":[]}"#; 8]);
        let sched = recording(&ai, &media, 20.0, (20.0, 5.0), Arc::new(fake.clone()));
        let id = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        drain(&sched);
        let task = sched.store().get(id).unwrap().unwrap();
        assert_eq!(task.status, TaskStatus::Done, "{:?}", task.error);
        let result = task.result.clone().unwrap();
        assert_eq!(result["stopped_early"], json!(false));
        assert_eq!(result["windows"]["done"], result["windows"]["all"]);
        assert_eq!(result["read_secs"], result["total_secs"]);
        assert_eq!(
            fake.chats().len() as u64,
            result["windows"]["all"].as_u64().unwrap()
        );
        assert!(Transcript::open(&ai, &transcript::file_id(&media).unwrap())
            .unwrap()
            .speaker_names()
            .unwrap()
            .is_empty());
    }

    #[test]
    #[ignore]
    fn speaker_names_live() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = std::env::var_os("MEDIAA_TEST_AI").map_or(tmp.path().to_path_buf(), PathBuf::from);
        let library = tempfile::tempdir().unwrap();
        let sample =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/data/pikhoya-1/audio.opus");
        let mut files = vec![sample];
        if let Some(extra) = std::env::var_os("MEDIAA_NAMES_FILE") {
            files.push(PathBuf::from(extra));
        }
        let defaults = AgentSettings::default();
        let engine = crate::live::engine();
        let sched = crate::run::open_scheduler(&ai, &defaults.budget).unwrap();
        for t in sched.store().list().unwrap() {
            let _ = sched.remove(t.id);
        }
        let settings: Arc<dyn Fn() -> AgentSettings + Send + Sync> =
            Arc::new(AgentSettings::default);
        crate::run::register_stages(
            &sched,
            &ai,
            engine,
            crate::slots::Slots::new(Budget::default()),
            settings,
            Arc::new(crate::run::Streams::default()),
        );
        for (n, source) in files.iter().enumerate() {
            let media = library.path().join(format!(
                "{n}-{}",
                source.file_name().unwrap().to_string_lossy()
            ));
            std::fs::copy(source, &media).unwrap();
            let id = transcript::file_id(&media).unwrap();
            let transcribed = Transcript::exists(&ai, &id)
                && Transcript::open(&ai, &id)
                    .ok()
                    .is_some_and(|t| t.dialog().is_ok_and(|dialog| !dialog.is_empty()));
            if !transcribed {
                let options = mediaa_core::agent::AnalysisOptions {
                    diarize: true,
                    speakers: None,
                    index: false,
                    topics: false,
                    details: false,
                };
                let task = crate::pipeline::start(&sched, &ai, &media, options).unwrap();
                crate::run::drain(&sched, Duration::from_secs(1800));
                let task = sched.store().get(task).unwrap().unwrap();
                assert_eq!(
                    task.status,
                    TaskStatus::Done,
                    "{}",
                    task.error.unwrap_or_default()
                );
            }
            let names_task = enqueue(&sched, &ai, &media, &defaults).unwrap();
            let started = std::time::Instant::now();
            crate::run::drain(&sched, Duration::from_secs(1800));
            let task = sched.store().get(names_task).unwrap().unwrap();
            assert_eq!(
                task.status,
                TaskStatus::Done,
                "{}",
                task.error.unwrap_or_default()
            );
            let result = task.result.clone().unwrap_or(Value::Null);
            eprintln!("== {}", media.display());
            eprintln!("took {:.0} s", started.elapsed().as_secs_f64());
            eprintln!(
                "read {} of {} s (stopped early: {})",
                result["read_secs"], result["total_secs"], result["stopped_early"]
            );
            eprintln!(
                "names: {}",
                serde_json::to_string_pretty(&result["names"]).unwrap_or_default()
            );
            eprintln!(
                "evidence: {}",
                serde_json::to_string_pretty(&result["evidence"]).unwrap_or_default()
            );
            let t = Transcript::open(&ai, &id).unwrap();
            eprintln!("transcript: {:?}", t.speaker_entries().unwrap());
            for reply in t.dialog().unwrap_or_default().iter().take(20) {
                let text: String = reply.text().chars().take(120).collect();
                eprintln!(
                    "[{}] {}: {text}",
                    stamp(reply.start),
                    reply
                        .speaker
                        .map_or_else(|| "-".to_string(), speakers::label)
                );
            }
            assert!(
                result["read_secs"].as_f64().is_some_and(|read| read > 0.0),
                "the run read the recording"
            );
        }
    }

    #[test]
    fn a_resumed_run_goes_on_with_the_next_window() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        let fake = FakeEngine::new();
        fake.chat_answers(&[r#"{"evidence":[]}"#, r#"{"evidence":[]}"#]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let sched = recording(&ai, &media, 10.0, (20.0, 5.0), engine.clone());
        let id = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        let stage = SpeakerNames::new(&ai, current());
        let host = Host {
            ai_dir: ai.clone(),
            engine,
            store: sched.store().clone(),
            slots: crate::slots::Slots::new(Budget::default()),
            settings: current(),
            streams: Arc::new(crate::run::Streams::default()),
        };
        let task = sched.store().get(id).unwrap().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let Work::More { cursor, .. } = stage.chunk(&task, &cancel, &host).unwrap() else {
            panic!("a window is left")
        };
        assert_eq!(cursor["done"], json!([0]));
        let mut next = task.clone();
        next.cursor = cursor;
        let Work::More { cursor, .. } = stage.chunk(&next, &cancel, &host).unwrap() else {
            panic!("a window is left")
        };
        assert_eq!(cursor["done"], json!([0, 1]));
        let chats = fake.chats();
        assert_eq!(chats.len(), 2);
        assert!(
            chats[1].texts[1].contains("Part of the recording 3:00 – 7:00."),
            "{}",
            chats[1].texts[1]
        );
        assert!(
            chats[1].texts[1].contains("[3:15] @s2: Ответ второго говорящего номер 7."),
            "{}",
            chats[1].texts[1]
        );
        assert!(
            !chats[1].texts[1].contains("[0:00]"),
            "the first window was read again"
        );
        assert!(chats[0].texts[1].contains("[0:00] @s1: Реплика первого говорящего номер 0."));
    }

    #[test]
    fn a_cut_answer_costs_one_window_not_the_run() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        let fake = FakeEngine::new();
        fake.chat_answers(&[
            r#"{"evidence":[{"speaker":"@s1","name":"Ан"#,
            r#"{"evidence":[{"speaker":"@s2","name":"Антон","how":"self","time":"3:15","sure":"high"}]}"#,
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let sched = recording(&ai, &media, 10.0, (20.0, 5.0), engine.clone());
        let id = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        let stage = SpeakerNames::new(&ai, current());
        let host = Host {
            ai_dir: ai.clone(),
            engine,
            store: sched.store().clone(),
            slots: crate::slots::Slots::new(Budget::default()),
            settings: current(),
            streams: Arc::new(crate::run::Streams::default()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut task = sched.store().get(id).unwrap().unwrap();
        let Work::More { cursor, .. } = stage.chunk(&task, &cancel, &host).unwrap() else {
            panic!("a window is left")
        };
        task.cursor = cursor;
        let first = Cursor::from_value(&task.cursor);
        assert_eq!(first.done, [0]);
        assert!(first.evidence.is_empty(), "a cut answer gives no evidence");
        let Work::More { cursor, .. } = stage.chunk(&task, &cancel, &host).unwrap() else {
            panic!("a window is left")
        };
        task.cursor = cursor;
        let second = Cursor::from_value(&task.cursor);
        assert_eq!(second.done, [0, 1]);
        assert_eq!(second.evidence.len(), 1);
        assert_eq!(
            (
                second.evidence[0].speaker,
                second.evidence[0].name.as_str(),
                second.evidence[0].time
            ),
            (1, "Антон", 195.0)
        );
        assert_eq!(fake.chats().len(), 2);
    }

    #[test]
    fn a_cut_answer_keeps_the_items_that_closed() {
        let cut = r#"{"evidence":[{"speaker":"@s1","name":"Антон","how":"self","time":"0:00","sure":"high"},{"speaker":"@s2","name":"Ан"#;
        let raw = closed_evidence(cut);
        assert_eq!(raw.evidence.len(), 1);
        assert_eq!(
            (
                raw.evidence[0].speaker.as_str(),
                raw.evidence[0].name.as_str()
            ),
            ("@s1", "Антон")
        );
        let whole = r#"{"evidence":[{"speaker":"@s1","name":"Антон","how":"self","time":"0:00","sure":"high"}]}"#;
        assert_eq!(closed_evidence(whole).evidence.len(), 1);
        assert!(closed_evidence(r#"{"evidence":[{"speaker""#)
            .evidence
            .is_empty());
    }

    #[test]
    fn a_cut_answer_keeps_its_complete_reports() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        let fake = FakeEngine::new();
        fake.chat_answers(&[
            r#"{"evidence":[{"speaker":"@s1","name":"Антон","how":"self","time":"0:00","sure":"high"},{"speaker":"@s2","name":"Ан"#,
            r#"{"evidence":[]}"#,
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let sched = recording(&ai, &media, 10.0, (20.0, 5.0), engine.clone());
        let id = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        let stage = SpeakerNames::new(&ai, current());
        let host = Host {
            ai_dir: ai.clone(),
            engine,
            store: sched.store().clone(),
            slots: crate::slots::Slots::new(Budget::default()),
            settings: current(),
            streams: Arc::new(crate::run::Streams::default()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let task = sched.store().get(id).unwrap().unwrap();
        let Work::More { cursor, .. } = stage.chunk(&task, &cancel, &host).unwrap() else {
            panic!("a window is left")
        };
        let cursor = Cursor::from_value(&cursor);
        assert_eq!(cursor.done, [0]);
        assert_eq!(cursor.evidence.len(), 1);
        assert_eq!(
            (cursor.evidence[0].speaker, cursor.evidence[0].name.as_str()),
            (0, "Антон")
        );
    }

    #[test]
    fn the_answer_grammar_asks_for_at_most_five_items() {
        let schema: Value = serde_json::from_str(EVIDENCE_SCHEMA).unwrap();
        assert_eq!(schema["properties"]["evidence"]["maxItems"], json!(5));
    }

    #[test]
    fn an_address_at_the_end_of_a_window_and_the_answer_in_the_next_are_seen() {
        let tmp = tempfile::tempdir().unwrap();
        let ai = tmp.path().join(".ai");
        let media = tmp.path().join("talk.wav");
        std::fs::write(&media, b"x").unwrap();
        let mut t = Transcript::open(&ai, &transcript::file_id(&media).unwrap()).unwrap();
        let address = reply(395.0, 400.0, 1, "Антон, расскажи, как ты подтвердил опыт.");
        let answer = reply(425.0, 435.0, 0, "Я работал в трёх компаниях.");
        t.set_replies(&[reply(0.0, 2.0, 1, "Начало."), address.clone(), answer])
            .unwrap();
        let plan = plan(435.0);
        assert!(
            395.0 >= plan[1].end - 30.0,
            "the address is in the last 30 s of window 1"
        );
        assert!(
            plan[1].start <= 395.0
                && 395.0 < plan[1].end
                && !(plan[1].start <= 425.0 && 425.0 < plan[1].end),
            "the answer is past the end of window 1"
        );
        assert!(
            plan[2].start <= 395.0 && 425.0 < plan[2].end,
            "window 2 shows both lines"
        );

        let fake = FakeEngine::new();
        fake.chat_answers(&[
            r#"{"evidence":[]}"#,
            r#"{"evidence":[]}"#,
            r#"{"evidence":[{"speaker":"@s1","name":"Антон","how":"addressed","time":"6:35","sure":"high"}]}"#,
        ]);
        let engine: Arc<dyn Engine> = Arc::new(fake.clone());
        let sched = crate::run::open_scheduler(&ai, &test_settings().budget).unwrap();
        crate::run::register_stages(
            &sched,
            &ai,
            engine.clone(),
            crate::slots::Slots::new(Budget::default()),
            current(),
            Arc::new(crate::run::Streams::default()),
        );
        let id = enqueue(&sched, &ai, &media, &test_settings()).unwrap();
        let stage = SpeakerNames::new(&ai, current());
        let host = Host {
            ai_dir: ai.clone(),
            engine,
            store: sched.store().clone(),
            slots: crate::slots::Slots::new(Budget::default()),
            settings: current(),
            streams: Arc::new(crate::run::Streams::default()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let mut task = sched.store().get(id).unwrap().unwrap();
        for _ in &plan {
            let Work::More { cursor, .. } = stage.chunk(&task, &cancel, &host).unwrap() else {
                panic!("the recording has {} windows", plan.len())
            };
            task.cursor = cursor;
        }
        let cursor = Cursor::from_value(&task.cursor);
        assert_eq!(cursor.done, [0, 1, 2]);
        let chats = fake.chats();
        assert_eq!(chats.len(), 3);
        assert!(
            chats[1].texts[1].contains("[6:35] @s2: Антон, расскажи"),
            "{}",
            chats[1].texts[1]
        );
        assert!(
            !chats[1].texts[1].contains("[7:05]"),
            "the answer is past the end of the window"
        );
        assert!(
            chats[2].texts[1].contains("[6:35] @s2: Антон, расскажи"),
            "{}",
            chats[2].texts[1]
        );
        assert!(
            chats[2].texts[1].contains("[7:05] @s1: Я работал"),
            "{}",
            chats[2].texts[1]
        );
        let found = cursor
            .evidence
            .first()
            .expect("the addressed report is evidence");
        assert_eq!(
            (found.speaker, found.name.as_str(), found.how, found.time),
            (0, "Антон", How::Addressed, 395.0)
        );
        assert_eq!(found.text, "Антон, расскажи, как ты подтвердил опыт.");
    }
}
