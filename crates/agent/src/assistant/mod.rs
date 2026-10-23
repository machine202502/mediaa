pub mod brain;
pub mod chat;
pub mod folder;
pub mod internet;
pub mod memory;
pub mod provider;
pub mod quote;
pub mod scan;
pub mod queued;
pub mod step_title;
pub mod title;
pub mod tools;

pub use brain::{Brain, Call, EngineBrain, Message, Piece, Role, Step, ToolSpec};
pub use memory::Memory;
pub use provider::ProviderBrain;
pub use tools::{Recording, Tools};

use crate::prompts;
use crate::slots::Slots;
use crate::tools::{format_time, parse_time};
use serde::{Deserialize, Serialize};
use crate::scope::Scope;
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const MAX_STEPS: usize = 8;
const STALL_STEPS: usize = 3;
const NEAR: f64 = 15.0;
const CITE_WINDOW: f64 = 30.0;
const DRAFT_TITLE: &str = "Черновик ответа";
const DRAFT_TOOL: &str = "draft";
const VERIFY_TITLE: &str = "Проверка ответа";
const VERIFY_TOOL: &str = "verify";
const PROMPT_SHARE: f32 = 0.6;
const ANSWER_SHARE: f32 = 0.2;
const CHARS_PER_TOKEN: usize = 3;
const METRICS_EVERY: Duration = Duration::from_millis(300);
const CLOSING_CHARS: usize = 400;
const SECTION_CHARS: usize = 2400;
const STUB_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Found,
    Nothing,
    Repeated,
    Failed,
}

impl StepStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            StepStatus::Found => "found",
            StepStatus::Nothing => "nothing",
            StepStatus::Repeated => "repeated",
            StepStatus::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub title: String,
    pub tool: String,
    pub args: Value,
    pub result: String,
    pub status: StepStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitReason {
    Loading { model: String },
    Memory { model: String },
    Downloading { model: String, done: u64, total: u64 },
    Busy { tasks: Vec<crate::scope::TaskRef> },
    ProviderSlot,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Waiting { reason: WaitReason },
    StepStarted { title: String, tool: String },
    Step(StepRecord),
    Thinking(String),
    AnswerText(String),
    AnswerDiscarded,
    Say(String),
    SteerTaken { text: String },
    Scanning { done: usize, total: usize },
    Metrics(Metrics),
    Done,
    Stopped,
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub context_tokens: u32,
    pub context: u32,
    pub speed: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Turn {
    pub question: String,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    pub steps: Vec<StepRecord>,
    pub forced: bool,
}

pub trait Toolbox: Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    fn run(&self, tool: &str, args: &Value) -> anyhow::Result<tools::Outcome>;
    fn kind(&self) -> &'static str;
    fn card(&self) -> String;
    fn bad_citations(&self, text: &str) -> Vec<String>;
    fn web(&self) -> Option<&internet::Web<'_>> {
        None
    }
    fn cited_context(&self, _text: &str) -> String {
        String::new()
    }
}

impl Toolbox for Tools<'_> {
    fn specs(&self) -> Vec<ToolSpec> {
        Tools::specs(self)
    }

    fn run(&self, tool: &str, args: &Value) -> anyhow::Result<tools::Outcome> {
        Tools::run(self, tool, args)
    }

    fn kind(&self) -> &'static str {
        "recording"
    }

    fn card(&self) -> String {
        self.recording.card()
    }

    fn bad_citations(&self, text: &str) -> Vec<String> {
        bad_citations(text, self.recording)
    }

    fn web(&self) -> Option<&internet::Web<'_>> {
        self.web
    }

    fn cited_context(&self, text: &str) -> String {
        let times: Vec<f64> = citations(text).into_iter().map(|(_, t)| t).collect();
        cited_replies(self.recording, &times).into_iter().map(|r| reply_line(self.recording, r)).collect::<Vec<_>>().join("\n")
    }
}

pub fn system_prompt(toolbox: &dyn Toolbox, specs: &[ToolSpec], memory: &Memory) -> String {
    let tools: Vec<Value> = specs.iter().map(|t| json!({"name": t.name, "description": t.description, "example": t.example.to_string()})).collect();
    let ctx = minijinja::context! { kind => toolbox.kind(), web => toolbox.web().is_some(), card => toolbox.card(), tools, facts => &memory.facts, summary => &memory.summary };
    prompts::render("agent/system.j2", ctx)
}

pub(crate) fn reply_line(rec: &Recording, reply: &mediaa_core::transcript::Reply) -> String {
    prompts::render("results/recording/line.j2", minijinja::context! { time => format_time(reply.start), speaker => reply.speaker.map(|s| rec.speaker(s)), text => reply.text() })
}

pub(crate) fn cited_replies<'a>(rec: &'a Recording, times: &[f64]) -> Vec<&'a mediaa_core::transcript::Reply> {
    rec.replies.iter().filter(|r| times.iter().any(|t| r.start <= t + CITE_WINDOW && r.end >= t - CITE_WINDOW)).collect()
}

pub fn citations(text: &str) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        rest = &rest[open + 1..];
        let end = rest.find([']', '–', '-']).unwrap_or(rest.len());
        let inside = &rest[..end];
        if inside.contains(':') {
            if let Some(t) = parse_time(inside) {
                out.push((inside.to_string(), t));
            }
        }
    }
    out
}

pub fn bad_citations(text: &str, recording: &Recording) -> Vec<String> {
    citations(text)
        .into_iter()
        .filter(|(_, t)| *t > recording.duration + 1.0 || !recording.replies.iter().any(|r| *t >= r.start - NEAR && *t <= r.end + NEAR))
        .map(|(s, _)| s)
        .collect()
}

fn without(text: &str, bad: &[String]) -> String {
    let mut t = text.to_string();
    for b in bad {
        t = t.replace(&format!(" [{b}]"), "").replace(&format!("[{b}]"), "");
    }
    t
}

pub fn tidy(text: &str) -> String {
    text.replace("<br />", "\n").replace("<br/>", "\n").replace("<br>", "\n").trim().to_string()
}

pub fn fingerprint(tool: &str, args: &Value) -> String {
    fn norm(v: &Value) -> Value {
        match v {
            Value::String(s) => Value::String(s.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")),
            Value::Object(o) => Value::Object(o.iter().map(|(k, v)| (k.clone(), norm(v))).collect()),
            Value::Array(a) => Value::Array(a.iter().map(norm).collect()),
            other => other.clone(),
        }
    }
    format!("{tool}:{}", norm(args))
}

const RESULT: &str = "Result of step ";

pub fn estimate_tokens(messages: &[Message]) -> usize {
    messages.iter().map(|m| m.content.chars().count()).sum::<usize>() / CHARS_PER_TOKEN
}

pub fn compact(messages: &mut [Message], context: u32) {
    compact_to(messages, (context as f32 * PROMPT_SHARE) as usize);
}

fn compact_to(messages: &mut [Message], limit: usize) {
    let results: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| m.role == Role::User && m.content.starts_with(RESULT)).map(|(i, _)| i).collect();
    let keep = results.len().saturating_sub(2);
    for &i in &results[..keep] {
        let total: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        if total <= limit * CHARS_PER_TOKEN {
            break;
        }
        let m = &mut messages[i];
        if m.content.chars().count() > STUB_CHARS {
            let head: String = m.content.chars().take(STUB_CHARS).collect();
            m.content = prompts::render("results/shortened.j2", minijinja::context! { head });
        }
    }
}

fn cut_middle(text: &str, keep: usize) -> String {
    let chars = text.chars().count();
    if chars <= keep {
        return text.to_string();
    }
    let marker = format!("\n… {}\n", prompts::render("results/cut/plain.j2", ()));
    let body = keep.saturating_sub(marker.chars().count());
    if body < 2 {
        return text.to_string();
    }
    let head: String = text.chars().take(body / 2).collect();
    let tail: String = text.chars().skip(chars - (body - body / 2)).collect();
    format!("{head}{marker}{tail}")
}

const MIN_KEEP_CHARS: usize = 200;

pub fn make_room(messages: &mut [Message], context: u32) {
    let budget = (context as f32 * (1.0 - ANSWER_SHARE)) as usize;
    if estimate_tokens(messages) <= budget {
        return;
    }
    compact(messages, context);
    compact_to(messages, budget);
    fn longest(messages: &[Message], skip: usize) -> Option<(usize, usize)> {
        messages.iter().enumerate().skip(skip).map(|(i, m)| (i, m.content.chars().count())).max_by_key(|(_, chars)| *chars)
    }
    loop {
        let total = estimate_tokens(messages);
        if total <= budget {
            return;
        }
        let Some((i, chars)) = longest(messages, 1).filter(|&(_, chars)| chars > MIN_KEEP_CHARS).or_else(|| longest(messages, 0)) else {
            return;
        };
        let keep = chars.saturating_sub((total - budget) * CHARS_PER_TOKEN).max(MIN_KEEP_CHARS);
        if keep >= chars {
            return;
        }
        messages[i].content = cut_middle(&messages[i].content, keep);
    }
}

fn step_json(step: &Step, thinking: bool) -> String {
    let q = |v: &str| Value::String(v.to_string()).to_string();
    let thought = |t: &str| if thinking { String::new() } else { format!(r#""thought":{},"#, q(t)) };
    match step {
        Step::Tool { thought: t, calls } => {
            let calls: Vec<String> = calls.iter().map(|c| format!(r#"{{"action":{},"args":{}}}"#, q(&c.tool), c.args)).collect();
            format!(r#"{{{}"calls":[{}]}}"#, thought(t), calls.join(","))
        }
        Step::Answer { thought: t, text } => format!(r#"{{{}"action":"{}","answer":{}}}"#, thought(t), brain::ANSWER, q(text)),
    }
}

const STEP_TOKENS: u32 = 2048;

pub const WAIT_GRACE: Duration = Duration::from_millis(600);
const WAIT_POLL: Duration = Duration::from_millis(400);

pub struct Waiter<'a> {
    pub reason: &'a (dyn Fn() -> Option<WaitReason> + Sync),
    pub grace: Duration,
}

pub fn wait_reason(status: &mediaa_engine::Status, model: &str, busy: impl FnOnce() -> Vec<crate::scope::TaskRef>) -> Option<WaitReason> {
    let ours = |w: &&mediaa_engine::WaitingModel| w.model.as_deref() == Some(model);
    let waiting = |reason: &str| status.waiting.iter().filter(ours).find(|w| w.reason == reason);
    if let Some(w) = waiting("downloading") {
        let (done, total) = w.progress.as_ref().map_or((0, 0), |p| (p.done, p.total));
        return Some(WaitReason::Downloading { model: model.to_string(), done, total });
    }
    if waiting("memory").is_some() {
        return Some(WaitReason::Memory { model: model.to_string() });
    }
    if status.waiting.iter().any(|w| matches!(w.reason.as_str(), "slot" | "turn")) {
        return Some(WaitReason::Busy { tasks: busy() });
    }
    (!status.loaded.iter().any(|m| m.model == model)).then(|| WaitReason::Loading { model: model.to_string() })
}

pub fn provider_wait(status: &crate::slots::Status) -> Option<WaitReason> {
    status.waiting.iter().any(|(_, kind)| *kind == mediaa_core::agent::WorkKind::ProviderCalls).then_some(WaitReason::ProviderSlot)
}

fn waiter(scope: &Scope, slots: &Slots, settings: &mediaa_core::agent::AgentSettings) -> Box<dyn Fn() -> Option<WaitReason> + Send + Sync> {
    use mediaa_core::agent::ModelSource;
    match &settings.models.agent {
        Some(ModelSource::Provider { .. }) => {
            let slots = slots.clone();
            Box::new(move || provider_wait(&slots.status()))
        }
        Some(local) => {
            let model = crate::models::local_spec(local).unwrap_or_default();
            let (scope, engine) = (scope.clone(), scope.engine());
            Box::new(move || wait_reason(&engine.status(), &model, || scope.busy_with()))
        }
        None => Box::new(|| None),
    }
}

fn for_model(speakers: &[mediaa_core::speakers::Who], history: &[Turn], question: &str) -> (Vec<Turn>, String) {
    let one = |text: &str| mediaa_core::speakers::to_model(&quote::expand(text), speakers);
    let history = history.iter().map(|t| Turn { question: one(&t.question), answer: t.answer.clone() }).collect();
    (history, one(question))
}

#[allow(clippy::too_many_arguments)]
pub fn ask(
    scope: &Scope,
    slots: &Slots,
    settings: &mediaa_core::agent::AgentSettings,
    ai_dir: &std::path::Path,
    file_id: &str,
    recording: &Recording,
    history: &[Turn],
    memory: &mut Memory,
    question: &str,
    cancel: &AtomicBool,
    on_event: &mut (dyn FnMut(&Event) + Send),
) -> anyhow::Result<Answer> {
    let engine = scope.engine();
    let engine = engine.as_ref();
    let find = |q: &str, o: crate::search::Options, n: usize| {
        crate::search::search_with(engine, slots, settings, ai_dir, file_id, q, n, o, true).map(|(h, _)| h)
    };
    let events = Mutex::new(on_event);
    let scan_with = |extract: &dyn scan::Extract, instruction: &str| {
        let transcript = mediaa_core::transcript::Transcript::open(ai_dir, file_id).ok();
        scan::scan(extract, recording, transcript.as_ref(), instruction, cancel, &mut |done, total| (events.lock().expect("event sink"))(&Event::Scanning { done, total }))
    };
    let rename = |speaker: u32, name: &str| {
        let transcript = mediaa_core::transcript::Transcript::open(ai_dir, file_id).map_err(|e| anyhow::anyhow!("no transcript: {e:#}"))?;
        transcript.rename_speaker(speaker, name)
    };
    let mut events_out = |e: &Event| (events.lock().expect("event sink"))(e);
    let (web_search, web_read) = web_calls(slots, settings, cancel);
    let wait = waiter(scope, slots, settings);
    let waiter = Waiter { reason: &*wait, grace: WAIT_GRACE };
    let (history, question) = for_model(&tools::whos(recording, None), history, question);
    let run = |brain: &dyn Brain, extract: &dyn scan::Extract| {
        let scanner = |instruction: &str| scan_with(extract, instruction);
        let max_chars = (brain.context() as usize) * 3 / 4;
        let web = settings.web.enabled.then(|| internet::Web::new(&*web_search, &*web_read, max_chars / 3));
        let tools = Tools { scan: Some(&scanner), rename: Some(&rename), web: web.as_ref(), ..Tools::new(recording, &find, max_chars) };
        answer(brain, &tools, scope, &history, memory, &question, cancel, Some(&waiter), settings.verify_answers, &mut events_out)
    };
    with_brain(scope, slots, settings, run)
}

fn web_calls<'a>(
    slots: &'a Slots,
    settings: &'a mediaa_core::agent::AgentSettings,
    cancel: &'a AtomicBool,
) -> (Box<internet::WebSearch<'a>>, Box<internet::WebRead<'a>>) {
    use mediaa_core::agent::WorkKind;
    let lease = move || slots.request(WorkKind::WebSearch, true).wait(cancel);
    (
        Box::new(move |q: &str, n: usize| {
            let _lease = lease()?;
            crate::web::search(&settings.web, q, n)
        }),
        Box::new(move |url: &str, max: usize| {
            let _lease = lease()?;
            crate::web::read(&settings.web, url, max)
        }),
    )
}

fn provider_brain(scope: &Scope, slots: &Slots, settings: &mediaa_core::agent::AgentSettings, id: &str, model: &str, thinking: bool) -> anyhow::Result<ProviderBrain> {
    let p = settings.provider(id).ok_or_else(|| anyhow::anyhow!("the provider of a model was removed"))?.clone();
    Ok(ProviderBrain { provider: p, model: model.to_string(), slots: slots.clone(), max_tokens: STEP_TOKENS, scope: scope.clone(), thinking })
}

fn with_brain<R>(
    scope: &Scope,
    slots: &Slots,
    settings: &mediaa_core::agent::AgentSettings,
    f: impl FnOnce(&dyn Brain, &dyn scan::Extract) -> anyhow::Result<R>,
) -> anyhow::Result<R> {
    use mediaa_core::agent::{ModelRole, ModelSource};
    use mediaa_engine::Priority;
    let source = settings.models.agent.as_ref().ok_or_else(|| anyhow::anyhow!("no agent model chosen (Agent → Models)"))?;
    let thinking = settings.thinking_on(source);
    match source {
        ModelSource::Provider { provider, model } => {
            let brain = provider_brain(scope, slots, settings, provider, model, thinking)?;
            f(&brain, &scan::ProviderExtract(&brain))
        }
        local => {
            let model = crate::models::local_spec(local)?;
            let n_ctx = settings.agent_context.clamp(2048, 262_144);
            let device = settings.devices.get(ModelRole::Agent);
            let hf_token = hf_token(settings);
            let engine = scope.engine();
            let brain = EngineBrain { engine: engine.as_ref(), model: model.clone(), n_ctx, device, priority: Priority::Interactive, max_tokens: STEP_TOKENS, hf_token: hf_token.clone(), thinking, speed: std::sync::Mutex::new(None) };
            let extract = scan::EngineExtract { engine: engine.as_ref(), model, n_ctx, device, priority: Priority::Background, hf_token };
            f(&brain, &extract)
        }
    }
}

pub fn hf_token(settings: &mediaa_core::agent::AgentSettings) -> Option<String> {
    settings.hugging_face().map(|provider| provider.api_key.trim().to_string()).filter(|token| !token.is_empty())
}

#[allow(clippy::too_many_arguments)]
pub fn ask_folder(
    scope: &Scope,
    slots: &Slots,
    settings: &mediaa_core::agent::AgentSettings,
    ai_dir: &std::path::Path,
    items: &[folder::Item],
    history: &[Turn],
    memory: &mut Memory,
    question: &str,
    cancel: &AtomicBool,
    on_event: &mut (dyn FnMut(&Event) + Send),
) -> anyhow::Result<Answer> {
    let engine = scope.engine();
    let engine = engine.as_ref();
    let sources: Vec<crate::search::Source> = items.iter().map(|it| crate::search::Source { id: it.file_id.clone(), path: it.path.clone() }).collect();
    let across = |q: &str, o: crate::search::Options, n: usize| {
        let (hits, _) = crate::search::search_files_with(engine, slots, settings, ai_dir, &sources, q, n, o, true)?;
        Ok(hits.into_iter().filter_map(|h| items.iter().position(|it| it.path == h.path).map(|i| (i, h.hit))).collect())
    };
    let one = |i: usize, q: &str, o: crate::search::Options, n: usize| {
        crate::search::search_with(engine, slots, settings, ai_dir, &items[i].file_id, q, n, o, true).map(|(h, _)| h)
    };
    let (web_search, web_read) = web_calls(slots, settings, cancel);
    let wait = waiter(scope, slots, settings);
    let waiter = Waiter { reason: &*wait, grace: WAIT_GRACE };
    let speakers: Vec<_> = items.iter().enumerate().flat_map(|(i, it)| tools::whos(&it.recording, Some((i + 1) as u32))).collect();
    let (history, question) = for_model(&speakers, history, question);
    with_brain(scope, slots, settings, |brain, _| {
        let max_chars = (brain.context() as usize) * 3 / 4;
        let web = settings.web.enabled.then(|| internet::Web::new(&*web_search, &*web_read, max_chars / 3));
        let tools = folder::FolderTools { items, search: &across, search_one: &one, max_chars, web: web.as_ref() };
        answer(brain, &tools, scope, &history, memory, &question, cancel, Some(&waiter), settings.verify_answers, on_event)
    })
}

#[allow(clippy::too_many_arguments)]
pub fn answer_step(
    scope: &Scope,
    slots: &Slots,
    settings: &mediaa_core::agent::AgentSettings,
    ai_dir: &std::path::Path,
    file_id: &str,
    recording: &Recording,
    history: &[Turn],
    session: &mut Session,
    question: &str,
    steers: &[String],
    cancel: &AtomicBool,
    on_event: &mut (dyn FnMut(&Event) + Send),
) -> anyhow::Result<Advance> {
    let engine = scope.engine();
    let engine = engine.as_ref();
    let find = |q: &str, o: crate::search::Options, n: usize| {
        crate::search::search_with(engine, slots, settings, ai_dir, file_id, q, n, o, true).map(|(h, _)| h)
    };
    let events = Mutex::new(on_event);
    let scan_with = |extract: &dyn scan::Extract, instruction: &str| {
        let transcript = mediaa_core::transcript::Transcript::open(ai_dir, file_id).ok();
        scan::scan(extract, recording, transcript.as_ref(), instruction, cancel, &mut |done, total| (events.lock().expect("event sink"))(&Event::Scanning { done, total }))
    };
    let rename = |speaker: u32, name: &str| {
        let transcript = mediaa_core::transcript::Transcript::open(ai_dir, file_id).map_err(|e| anyhow::anyhow!("no transcript: {e:#}"))?;
        transcript.rename_speaker(speaker, name)
    };
    let (web_search, web_read) = web_calls(slots, settings, cancel);
    let wait = waiter(scope, slots, settings);
    let waiter = Waiter { reason: &*wait, grace: WAIT_GRACE };
    let (history, question) = for_model(&tools::whos(recording, None), history, question);
    let emit = |e: &Event| (events.lock().expect("event sink"))(e);
    let run = |brain: &dyn Brain, extract: &dyn scan::Extract| {
        let scanner = |instruction: &str| scan_with(extract, instruction);
        let max_chars = (brain.context() as usize) * 3 / 4;
        let web = settings.web.enabled.then(|| internet::Web::new(&*web_search, &*web_read, max_chars / 3));
        let tools = Tools { scan: Some(&scanner), rename: Some(&rename), web: web.as_ref(), ..Tools::new(recording, &find, max_chars) };
        advance(brain, &tools, scope, session, &history, &question, steers, cancel, Some(&waiter), settings.verify_answers, &emit)
    };
    with_brain(scope, slots, settings, run)
}

#[allow(clippy::too_many_arguments)]
pub fn answer_step_folder(
    scope: &Scope,
    slots: &Slots,
    settings: &mediaa_core::agent::AgentSettings,
    ai_dir: &std::path::Path,
    items: &[folder::Item],
    history: &[Turn],
    session: &mut Session,
    question: &str,
    steers: &[String],
    cancel: &AtomicBool,
    on_event: &mut (dyn FnMut(&Event) + Send),
) -> anyhow::Result<Advance> {
    let engine = scope.engine();
    let engine = engine.as_ref();
    let sources: Vec<crate::search::Source> = items.iter().map(|it| crate::search::Source { id: it.file_id.clone(), path: it.path.clone() }).collect();
    let across = |q: &str, o: crate::search::Options, n: usize| {
        let (hits, _) = crate::search::search_files_with(engine, slots, settings, ai_dir, &sources, q, n, o, true)?;
        Ok(hits.into_iter().filter_map(|h| items.iter().position(|it| it.path == h.path).map(|i| (i, h.hit))).collect())
    };
    let one = |i: usize, q: &str, o: crate::search::Options, n: usize| {
        crate::search::search_with(engine, slots, settings, ai_dir, &items[i].file_id, q, n, o, true).map(|(h, _)| h)
    };
    let events = Mutex::new(on_event);
    let (web_search, web_read) = web_calls(slots, settings, cancel);
    let wait = waiter(scope, slots, settings);
    let waiter = Waiter { reason: &*wait, grace: WAIT_GRACE };
    let speakers: Vec<_> = items.iter().enumerate().flat_map(|(i, it)| tools::whos(&it.recording, Some((i + 1) as u32))).collect();
    let (history, question) = for_model(&speakers, history, question);
    let emit = |e: &Event| (events.lock().expect("event sink"))(e);
    with_brain(scope, slots, settings, |brain, _| {
        let max_chars = (brain.context() as usize) * 3 / 4;
        let web = settings.web.enabled.then(|| internet::Web::new(&*web_search, &*web_read, max_chars / 3));
        let tools = folder::FolderTools { items, search: &across, search_one: &one, max_chars, web: web.as_ref() };
        advance(brain, &tools, scope, session, &history, &question, steers, cancel, Some(&waiter), settings.verify_answers, &emit)
    })
}

pub fn chat_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "remember",
            description: prompts::tool("remember"),
            params: json!({"type": "object", "properties": {"fact": {"type": "string"}}, "required": ["fact"]}),
            example: json!({"fact": "<the fact>"}),
        },
        ToolSpec {
            name: "plan",
            description: prompts::tool("plan"),
            params: json!({"type": "object", "properties": {"items": {"type": "array", "maxItems": 8, "items": {"type": "object", "properties": {"text": {"type": "string"}, "done": {"type": "boolean"}}, "required": ["text", "done"]}}}, "required": ["items"]}),
            example: json!({"items": [{"text": "<step>", "done": false}]}),
        },
        ToolSpec {
            name: "say",
            description: prompts::tool("say"),
            params: json!({"type": "object", "properties": {"text": {"type": "string", "maxLength": SECTION_CHARS}}, "required": ["text"]}),
            example: json!({"text": "<a finished section, Markdown, with [m:ss] citations>"}),
        },
    ]
}

fn chat_tool(tool: &str, args: &Value, toolbox: &dyn Toolbox, memory: &mut Memory, on_event: &mut dyn FnMut(&Event)) -> Option<anyhow::Result<tools::Outcome>> {
    let plain = |text: String| Ok(tools::Outcome { text, seen: None });
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    Some(match tool {
        "remember" => plain(prompts::render("results/remember.j2", minijinja::context! { new => memory.remember(&arg("fact")) })),
        "plan" => {
            let items: Vec<Value> = args["items"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|i| json!({"text": i["text"].as_str().unwrap_or_default(), "done": i["done"].as_bool() == Some(true)}))
                .collect();
            if items.is_empty() {
                return Some(Err(anyhow::anyhow!(prompts::render("results/plan_empty.j2", ()))));
            }
            plain(prompts::render("results/plan.j2", minijinja::context! { items }))
        }
        "say" => {
            let text = arg("text");
            if text.is_empty() {
                return Some(Err(anyhow::anyhow!(prompts::render("results/say_empty.j2", ()))));
            }
            let text = tidy(&without(&text, &toolbox.bad_citations(&text)));
            let text = toolbox.web().map_or(text.clone(), |w| w.known_links(&text));
            if citations(&text).is_empty() && folder::folder_citations(&text).is_empty() && !text.contains("](http") {
                return Some(Err(anyhow::anyhow!(prompts::render("results/say_uncited.j2", minijinja::context! { kind => toolbox.kind() }))));
            }
            on_event(&Event::Say(text));
            plain(prompts::render("results/say_sent.j2", ()))
        }
        _ => return toolbox.web().and_then(|w| w.run(tool, args)),
    })
}

fn is_chat(tool: &str) -> bool {
    matches!(tool, "remember" | "plan" | "say")
}

const ARGS_CHARS: usize = 200;

fn short_args(args: &Value) -> String {
    let text = args.to_string();
    if text.chars().count() <= ARGS_CHARS {
        return text;
    }
    let head: String = text.chars().take(ARGS_CHARS - 1).collect();
    format!("{head}…")
}

fn begin_child(scope: &Scope, kind: &str, title: &str, input: &Value) -> Option<Scope> {
    scope.inline_child(kind, title, input).map_err(|e| log::warn!("the queue does not show a {kind}: {e:#}")).ok()
}

fn watch(waiter: &Waiter, started: &AtomicBool, finished: &AtomicBool, emit: &(dyn Fn(&Event) + Sync)) {
    let mut last: Option<WaitReason> = None;
    let mut next = Instant::now() + waiter.grace;
    while !finished.load(Ordering::Relaxed) && !started.load(Ordering::Relaxed) {
        if Instant::now() >= next {
            next = Instant::now() + WAIT_POLL;
            if let Some(reason) = (waiter.reason)() {
                if last.as_ref() != Some(&reason) && !started.load(Ordering::Relaxed) {
                    emit(&Event::Waiting { reason: reason.clone() });
                    last = Some(reason);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn verify_schema() -> Value {
    json!({"type":"object","properties":{"changed":{"type":"boolean"},"answer":{"type":"string"}},"required":["changed","answer"],"additionalProperties":false})
}

fn millis(seconds: f64) -> i64 {
    (seconds * 1000.0).round() as i64
}

fn time_keys(text: &str) -> BTreeSet<(String, i64)> {
    citations(text).into_iter().map(|(written, t)| (written, millis(t))).collect()
}

fn folder_keys(text: &str) -> BTreeSet<(usize, i64)> {
    folder::folder_citations(text).into_iter().map(|(_, n, t)| (n, millis(t))).collect()
}

fn md_http(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut rest = text;
    while let Some(at) = rest.find("](http") {
        rest = &rest[at + 2..];
        let end = rest.find(')').unwrap_or(rest.len());
        out.insert(rest[..end].to_string());
    }
    out
}

fn http_urls(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        let https = rest.starts_with("https://");
        if https || rest.starts_with("http://") {
            let start = i;
            i += if https { 8 } else { 7 };
            while i < bytes.len() {
                let ch = text[i..].chars().next().unwrap();
                if ch.is_whitespace() || matches!(ch, ')' | ']' | '>' | '"' | '\'') {
                    break;
                }
                i += ch.len_utf8();
            }
            while i > start && matches!(bytes[i - 1], b'.' | b',' | b';') {
                i -= 1;
            }
            out.push(&text[start..i]);
        } else {
            i += text[i..].chars().next().map(char::len_utf8).unwrap_or(1);
        }
    }
    out
}

fn web_evidence(steps: &[StepRecord], draft: &str) -> String {
    steps
        .iter()
        .filter(|s| s.tool == "web_search" || s.tool == "web_read")
        .filter(|s| http_urls(&s.result).iter().any(|url| draft.contains(url)))
        .map(|s| s.result.as_str())
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn verdict(raw: &str) -> Option<(bool, String)> {
    let raw = raw.trim();
    let (Some(start), Some(end)) = (raw.find('{'), raw.rfind('}')) else { return None };
    if start >= end {
        return None;
    }
    let v: Value = serde_json::from_str(&raw[start..=end]).ok()?;
    Some((v.get("changed")?.as_bool()?, without_field_echo(v.get("answer")?.as_str()?)))
}

fn without_field_echo(answer: &str) -> String {
    let mut lines: Vec<&str> = answer.trim_end().lines().collect();
    while lines.last().is_some_and(|l| {
        let l = l.trim().trim_matches('"').to_lowercase();
        l.is_empty() || ["changed", "answer"].iter().any(|k| l.starts_with(k) && l[k.len()..].trim_start().starts_with(['—', '-', ':', '=']))
    }) {
        lines.pop();
    }
    lines.join("\n")
}

fn cancelled(error: &anyhow::Error, cancel: &AtomicBool) -> bool {
    cancel.load(Ordering::Relaxed) || error.to_string().to_lowercase().contains("cancel")
}

fn accept(draft: &str, raw: &str, tools: &dyn Toolbox) -> String {
    let Some((changed, answer)) = verdict(raw) else { return draft.to_string() };
    if !changed {
        return draft.to_string();
    }
    let mut answer = tidy(&answer);
    if let Some(web) = tools.web() {
        answer = web.known_links(&answer);
    }
    if answer.trim().is_empty() {
        return draft.to_string();
    }
    let kept = time_keys(&answer).is_subset(&time_keys(draft)) && folder_keys(&answer).is_subset(&folder_keys(draft)) && md_http(&answer).is_subset(&md_http(draft));
    if !kept || !tools.bad_citations(&answer).is_empty() {
        return draft.to_string();
    }
    answer
}

#[allow(clippy::too_many_arguments)]
fn check_answer(
    brain: &dyn Brain,
    tools: &dyn Toolbox,
    scope: &Scope,
    steps: &mut Vec<StepRecord>,
    question: &str,
    draft: String,
    cancel: &AtomicBool,
    waiter: Option<&Waiter>,
    emit: &(dyn Fn(&Event) + Sync),
) -> anyhow::Result<String> {
    emit(&Event::AnswerDiscarded);
    emit(&Event::StepStarted { title: DRAFT_TITLE.to_string(), tool: DRAFT_TOOL.to_string() });
    let draft_step = StepRecord { title: DRAFT_TITLE.to_string(), tool: DRAFT_TOOL.to_string(), args: json!({}), result: draft.clone(), status: StepStatus::Found };
    emit(&Event::Step(draft_step.clone()));
    steps.push(draft_step);

    let fragments = tools.cited_context(&draft);
    let web = web_evidence(steps, &draft);
    let messages = [
        Message::new(Role::System, prompts::render("agent/verify_system.j2", ())),
        Message::new(Role::User, prompts::render("agent/verify.j2", minijinja::context! { question, draft, fragments, web })),
    ];
    let child = begin_child(scope, "model-step", VERIFY_TITLE, &json!({ "step": VERIFY_TITLE }));
    scope.focus(child.as_ref().map(Scope::task));
    emit(&Event::StepStarted { title: VERIFY_TITLE.to_string(), tool: VERIFY_TOOL.to_string() });
    let (started, finished) = (AtomicBool::new(false), AtomicBool::new(false));
    let verdict = std::thread::scope(|s| {
        if let Some(w) = waiter {
            s.spawn(|| watch(w, &started, &finished, emit));
        }
        let result = brain.complete_json(&messages, &verify_schema(), cancel);
        started.store(true, Ordering::Relaxed);
        finished.store(true, Ordering::Relaxed);
        result
    });
    scope.focus(None);

    let text = match verdict {
        Err(e) if cancelled(&e, cancel) => {
            return Err(e);
        }
        Err(e) => {
            log::warn!("the answer check failed: {e:#}");
            if let Some(child) = &child {
                child.fail(&format!("{e:#}"));
            }
            let record = StepRecord { title: VERIFY_TITLE.to_string(), tool: VERIFY_TOOL.to_string(), args: json!({}), result: format!("Error: {e:#}"), status: StepStatus::Failed };
            emit(&Event::Step(record.clone()));
            steps.push(record);
            draft
        }
        Ok(raw) => {
            let text = accept(&draft, &raw, tools);
            let status = if text == draft { StepStatus::Nothing } else { StepStatus::Found };
            if let Some(child) = &child {
                child.finish(&json!({ "status": status.as_str() }));
            }
            let record = StepRecord { title: VERIFY_TITLE.to_string(), tool: VERIFY_TOOL.to_string(), args: json!({}), result: raw, status };
            emit(&Event::Step(record.clone()));
            steps.push(record);
            text
        }
    };
    if !text.is_empty() {
        emit(&Event::AnswerText(text.clone()));
    }
    Ok(text)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub messages: Vec<Message>,
    pub steps: Vec<StepRecord>,
    #[serde(default)]
    pub done: BTreeMap<String, usize>,
    #[serde(default)]
    pub known: Vec<String>,
    pub stall: usize,
    pub told_to_answer: bool,
    pub checked_citations: bool,
    pub cut_steps: usize,
    pub n: usize,
    pub memory: Memory,
    #[serde(default)]
    pub timeline: Value,
    #[serde(default)]
    pub gen_at_save: i64,
    #[serde(default)]
    pub noticed: bool,
    #[serde(default)]
    pub ui_english: bool,
    #[serde(default)]
    pub titles: bool,
    #[serde(default)]
    pub store: String,
    #[serde(default)]
    pub chat: i64,
    #[serde(default)]
    pub steers_taken: usize,
}

impl Session {
    pub fn new(memory: Memory) -> Session {
        Session {
            messages: Vec::new(),
            steps: Vec::new(),
            done: BTreeMap::new(),
            known: Vec::new(),
            stall: 0,
            told_to_answer: false,
            checked_citations: false,
            cut_steps: 0,
            n: 0,
            memory,
            timeline: Value::Null,
            gen_at_save: 0,
            noticed: false,
            ui_english: false,
            titles: false,
            store: String::new(),
            chat: 0,
            steers_taken: 0,
        }
    }
}

pub enum Advance {
    More,
    Done(Answer),
}

#[allow(clippy::too_many_arguments)]
pub fn answer(
    brain: &dyn Brain,
    tools: &dyn Toolbox,
    scope: &Scope,
    history: &[Turn],
    memory: &mut Memory,
    question: &str,
    cancel: &AtomicBool,
    waiter: Option<&Waiter>,
    verify: bool,
    on_event: &mut (dyn FnMut(&Event) + Send),
) -> anyhow::Result<Answer> {
    let sink = Mutex::new(on_event);
    let emit = |e: &Event| (sink.lock().expect("event sink"))(e);
    let mut session = Session::new(memory.clone());
    let result = (|| {
        loop {
            match advance(brain, tools, scope, &mut session, history, question, &[], cancel, waiter, verify, &emit)? {
                Advance::More => {}
                Advance::Done(answer) => return Ok(answer),
            }
        }
    })();
    *memory = session.memory;
    result
}

#[allow(clippy::too_many_arguments)]
pub fn advance(
    brain: &dyn Brain,
    tools: &dyn Toolbox,
    scope: &Scope,
    session: &mut Session,
    history: &[Turn],
    question: &str,
    steers: &[String],
    cancel: &AtomicBool,
    waiter: Option<&Waiter>,
    verify: bool,
    emit: &(dyn Fn(&Event) + Sync),
) -> anyhow::Result<Advance> {
    let running = || -> anyhow::Result<()> {
        anyhow::ensure!(!cancel.load(Ordering::Relaxed), "cancelled");
        Ok(())
    };
    if session.messages.is_empty() {
        running()?;
        memory::fold(brain, history, &mut session.memory, cancel)?;
        let mut specs = tools.specs();
        specs.extend(chat_specs());
        if tools.web().is_some() {
            specs.extend(internet::Web::specs());
        }
        let mut messages = vec![Message::new(Role::System, system_prompt(tools, &specs, &session.memory))];
        for t in &history[session.memory.summarized.min(history.len())..] {
            messages.push(Message::new(Role::User, t.question.clone()));
            messages.push(Message::new(Role::Assistant, t.answer.clone()));
        }
        messages.push(Message::new(Role::User, question.to_string()));
        session.messages = messages;
    }
    if !steers.is_empty() {
        for text in steers {
            session.messages.push(Message::new(Role::User, prompts::render("nudges/steer.j2", minijinja::context! { text })));
            emit(&Event::SteerTaken { text: text.clone() });
        }
        session.steers_taken += steers.len();
    }
    if session.n >= MAX_STEPS + 4 {
        anyhow::bail!("the model did not answer in {} steps", MAX_STEPS + 4);
    }
    let n = session.n;
    session.n += 1;
    running()?;
    let mut specs = tools.specs();
    specs.extend(chat_specs());
    if tools.web().is_some() {
        specs.extend(internet::Web::specs());
    }
    let must_answer = session.steps.len() >= MAX_STEPS || session.stall >= STALL_STEPS;
    if must_answer && !session.told_to_answer {
        session.told_to_answer = true;
        session.messages.push(Message::new(Role::User, prompts::render("nudges/answer_now.j2", ())));
    }
    let context = brain.context();
    make_room(&mut session.messages, context);
    emit(&Event::Metrics(Metrics { context_tokens: estimate_tokens(&session.messages) as u32, context, speed: brain.speed() }));
    let mut drafted = false;
    let mut streamed = 0usize;
    let model_step = begin_child(scope, "model-step", &(n + 1).to_string(), &json!({ "step": n + 1 }));
    scope.focus(model_step.as_ref().map(Scope::task));
    let step = {
        let (started, finished) = (AtomicBool::new(false), AtomicBool::new(false));
        let mut last_metrics = Instant::now();
        std::thread::scope(|s| {
            if let Some(w) = waiter {
                s.spawn(|| watch(w, &started, &finished, &emit));
            }
            let step = brain.step(&session.messages, if must_answer { &[] } else { &specs }, !session.steps.is_empty(), cancel, &mut |piece| {
                started.store(true, Ordering::Relaxed);
                match piece {
                    Piece::Thinking(text) => {
                        streamed += text.chars().count();
                        emit(&Event::Thinking(text.to_string()));
                    }
                    Piece::Answer(text) => {
                        drafted = true;
                        streamed += text.chars().count();
                        emit(&Event::AnswerText(text.to_string()));
                    }
                }
                if last_metrics.elapsed() >= METRICS_EVERY {
                    last_metrics = Instant::now();
                    let taken = estimate_tokens(&session.messages) + streamed / CHARS_PER_TOKEN;
                    emit(&Event::Metrics(Metrics { context_tokens: taken as u32, context, speed: brain.speed() }));
                }
            });
            finished.store(true, Ordering::Relaxed);
            step
        })
    };
    emit(&Event::Metrics(Metrics { context_tokens: (estimate_tokens(&session.messages) + streamed / CHARS_PER_TOKEN) as u32, context, speed: brain.speed() }));
    scope.focus(None);
    if let Some(child) = &model_step {
        match &step {
            Ok(_) => child.finish(&json!({})),
            Err(e) => child.fail(&format!("{e:#}")),
        }
    }
    running()?;
    let step = match step {
        Ok(step) => step,
        Err(e) if session.cut_steps < 2 && e.to_string().contains("not JSON") => {
            session.cut_steps += 1;
            session.stall += 1;
            if drafted {
                emit(&Event::AnswerDiscarded);
            }
            session.messages.push(Message::new(Role::User, prompts::render("nudges/cut_off.j2", ())));
            return Ok(Advance::More);
        }
        Err(e) => return Err(e),
    };
    if drafted && !matches!(step, Step::Answer { .. }) {
        emit(&Event::AnswerDiscarded);
    }
    session.messages.push(Message::new(Role::Assistant, step_json(&step, brain.thinking())));
    match step {
        Step::Tool { calls, .. } => {
            struct Planned {
                fp: String,
                no: usize,
                title: String,
                tool: String,
                args: Value,
                repeated: Option<usize>,
                child: Option<Scope>,
            }
            let mut plans: Vec<Planned> = Vec::new();
            let mut in_step: HashMap<String, usize> = HashMap::new();
            let room = MAX_STEPS.saturating_sub(session.steps.len()).max(1);
            if calls.len() > room {
                log::warn!("{} calls in one step, the limit leaves room for {room}", calls.len());
            }
            for call in calls.iter().take(room) {
                let fp = fingerprint(&call.tool, &call.args);
                let no = session.steps.len() + plans.len() + 1;
                let index = session.steps.len() + plans.len();
                let title = step_title::caption(&call.tool, step_title::is_topic(&call.args), session.ui_english);
                let purpose = tools.specs().into_iter().find(|s| s.name == call.tool).map(|s| s.description.lines().next().unwrap_or("").trim().to_string()).unwrap_or_default();
                step_title::enqueue(scope, session, index, question, &call.tool, &purpose, &call.args);
                emit(&Event::StepStarted { title: title.clone(), tool: call.tool.clone() });
                let child = begin_child(scope, "tool", &title, &json!({ "tool": &title, "args": &call.args }));
                let repeated = session.done.get(&fp).copied().or_else(|| in_step.get(&fp).copied());
                in_step.entry(fp.clone()).or_insert(no);
                plans.push(Planned { fp, no, title, tool: call.tool.clone(), args: call.args.clone(), repeated, child });
            }
            let mut outcomes: Vec<Option<anyhow::Result<tools::Outcome>>> = plans.iter().map(|_| None).collect();
            std::thread::scope(|s| {
                let mut threads = Vec::new();
                for (i, plan) in plans.iter().enumerate() {
                    if plan.repeated.is_some() || is_chat(&plan.tool) {
                        continue;
                    }
                    let child = plan.child.as_ref();
                    threads.push((i, s.spawn(move || {
                        if let Some(child) = child {
                            scope.focus(Some(child.task()));
                        }
                        let outcome = match tools.web() {
                            Some(web) => web.run(&plan.tool, &plan.args).unwrap_or_else(|| tools.run(&plan.tool, &plan.args)),
                            None => tools.run(&plan.tool, &plan.args),
                        };
                        scope.focus(None);
                        outcome
                    })));
                }
                for (i, thread) in threads {
                    let outcome = thread.join().unwrap_or_else(|e| std::panic::resume_unwind(e));
                    outcomes[i] = Some(outcome);
                }
            });
            for (i, plan) in plans.iter().enumerate() {
                if plan.repeated.is_none() && is_chat(&plan.tool) && outcomes[i].is_none() {
                    let outcome = chat_tool(&plan.tool, &plan.args, tools, &mut session.memory, &mut |e| emit(e)).unwrap_or_else(|| tools.run(&plan.tool, &plan.args));
                    outcomes[i] = Some(outcome);
                }
            }
            let mut known: HashSet<String> = session.known.iter().cloned().collect();
            let mut results: Vec<(StepStatus, String)> = Vec::new();
            for (i, plan) in plans.iter().enumerate() {
                let (status, text) = match plan.repeated {
                    Some(prev) => (StepStatus::Repeated, prompts::render("results/repeated.j2", minijinja::context! { step => prev })),
                    None => match outcomes[i].as_ref().expect("every call of the step was run") {
                        Ok(out) => {
                            let new = out.seen.as_ref().is_none_or(|s| tools::adds(&mut known, s));
                            if new {
                                (StepStatus::Found, out.text.clone())
                            } else if out.seen.as_ref().is_some_and(Vec::is_empty) {
                                (StepStatus::Nothing, out.text.clone())
                            } else {
                                (StepStatus::Nothing, prompts::render("results/nothing_new.j2", ()))
                            }
                        }
                        Err(e) => (StepStatus::Failed, format!("Error: {e:#}")),
                    },
                };
                if let Some(child) = &plan.child {
                    child.set_detail(&json!({ "tool": &plan.title, "args": short_args(&plan.args), "result_chars": text.chars().count() }));
                    match status {
                        StepStatus::Failed => child.fail(&text),
                        _ => child.finish(&json!({ "status": status.as_str() })),
                    }
                }
                results.push((status, text));
            }
            running()?;
            session.known = known.into_iter().collect();
            session.known.sort();
            for (plan, (status, text)) in plans.iter().zip(results) {
                session.done.entry(plan.fp.clone()).or_insert(plan.no);
                session.stall = match status {
                    StepStatus::Found => 0,
                    StepStatus::Repeated => session.stall + 2,
                    _ => session.stall + 1,
                };
                session.messages.push(Message::new(Role::User, prompts::render("results/step.j2", minijinja::context! { step => plan.no, tool => &plan.tool, text => &text })));
                let record = StepRecord { title: plan.title.clone(), tool: plan.tool.clone(), args: plan.args.clone(), result: text, status };
                emit(&Event::Step(record.clone()));
                session.steps.push(record);
            }
            Ok(Advance::More)
        }
        Step::Answer { text, .. } => {
            let bad = tools.bad_citations(&text);
            if !bad.is_empty() && !session.checked_citations && n + 1 < MAX_STEPS + 4 {
                session.checked_citations = true;
                emit(&Event::AnswerDiscarded);
                let bad: Vec<String> = bad.iter().map(|b| format!("[{b}]")).collect();
                let nudge = prompts::render("nudges/bad_citations.j2", minijinja::context! { bad, kind => tools.kind() });
                session.messages.push(Message::new(Role::User, nudge));
                return Ok(Advance::More);
            }
            let mut text = tidy(&without(&text, &bad));
            if let Some(web) = tools.web() {
                text = web.known_links(&text);
            }
            if session.steps.iter().any(|s| s.tool == "say" && s.status == StepStatus::Found) && text.chars().count() > CLOSING_CHARS {
                text = text.split("\n\n").next().unwrap_or_default().trim().to_string();
            }
            let text = if verify && !text.is_empty() {
                check_answer(brain, tools, scope, &mut session.steps, question, text, cancel, waiter, &emit)?
            } else {
                text
            };
            Ok(Advance::Done(Answer { text, steps: session.steps.clone(), forced: session.told_to_answer }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tools::tests::recording;
    use super::*;
    use mediaa_core::retrieval::{Chunk, Hit};
    use std::cell::RefCell;

    fn test_scope() -> (tempfile::TempDir, Scope) {
        let dir = tempfile::tempdir().unwrap();
        let scope = Scope::begin(dir.path(), std::sync::Arc::new(mediaa_engine::testing::FakeEngine::new()), "chat", "тест", &json!({}), Some("rec")).unwrap();
        (dir, scope)
    }

    struct Scripted {
        steps: RefCell<Vec<Step>>,
        seen: RefCell<Vec<(Vec<Message>, usize)>>,
        context: u32,
        thinking: Vec<(usize, &'static str)>,
        calls: std::cell::Cell<usize>,
        check: RefCell<Option<String>>,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Scripted {
            Scripted { steps: RefCell::new(steps.into_iter().rev().collect()), seen: RefCell::default(), context: 8192, thinking: Vec::new(), calls: std::cell::Cell::new(0), check: RefCell::new(None) }
        }

        fn checking(self, verdict: &str) -> Scripted {
            self.check.replace(Some(verdict.to_string()));
            self
        }

        fn thinking(mut self, before_step: usize, text: &'static str) -> Scripted {
            self.thinking.push((before_step, text));
            self
        }
    }

    impl Brain for Scripted {
        fn step(&self, messages: &[Message], tools: &[ToolSpec], may_answer: bool, _: &AtomicBool, on_piece: &mut dyn FnMut(Piece)) -> anyhow::Result<Step> {
            self.seen.borrow_mut().push((messages.to_vec(), tools.len()));
            let no = self.calls.replace(self.calls.get() + 1);
            for (_, text) in self.thinking.iter().filter(|(n, _)| *n == no) {
                let half = text.char_indices().nth(text.chars().count() / 2).map_or(0, |(i, _)| i);
                on_piece(Piece::Thinking(&text[..half]));
                on_piece(Piece::Thinking(&text[half..]));
            }
            if !may_answer && !tools.is_empty() {
                assert!(matches!(self.steps.borrow().last(), Some(Step::Tool { .. })), "an answer before looking");
            }
            if tools.is_empty() {
                while matches!(self.steps.borrow().last(), Some(Step::Tool { .. })) {
                    self.steps.borrow_mut().pop();
                }
            }
            let step = self.steps.borrow_mut().pop().ok_or_else(|| anyhow::anyhow!("script over"))?;
            if let Step::Answer { text, .. } = &step {
                let half = text.char_indices().nth(text.chars().count() / 2).map_or(0, |(i, _)| i);
                on_piece(Piece::Answer(&text[..half]));
                on_piece(Piece::Answer(&text[half..]));
            }
            Ok(step)
        }

        fn complete(&self, messages: &[Message], _: &AtomicBool) -> anyhow::Result<String> {
            self.seen.borrow_mut().push((messages.to_vec(), 0));
            if messages.iter().any(|m| m.content.contains("You check a draft answer")) {
                if let Some(verdict) = self.check.borrow_mut().take() {
                    return Ok(verdict);
                }
            }
            Ok(format!("сводка #{}", self.seen.borrow().len()))
        }

        fn context(&self) -> u32 {
            self.context
        }
    }

    fn call(tool: &str, args: Value) -> Step {
        Step::Tool { thought: String::new(), calls: vec![Call { tool: tool.into(), args }] }
    }

    fn batch(list: &[(&str, Value)]) -> Step {
        Step::Tool { thought: String::new(), calls: list.iter().map(|(tool, args)| Call { tool: (*tool).into(), args: args.clone() }).collect() }
    }

    fn say(text: &str) -> Step {
        Step::Answer { thought: String::new(), text: text.into() }
    }

    fn search(q: &str, _: crate::search::Options, _: usize) -> anyhow::Result<Vec<Hit>> {
        Ok(if q.contains("отчёт") { vec![Hit { id: 1, chunk: Chunk { start: 10.0, end: 15.0, speakers: vec![0], text: "Отчёт отправляем в пятницу.".into() }, score: 1.0 }] } else { vec![] })
    }

    fn run(brain: &Scripted, question: &str) -> (Answer, Vec<Event>) {
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let mut events = Vec::new();
        let history = [Turn { question: "Привет".into(), answer: "Здравствуйте".into() }];
        let (_dir, scope) = test_scope();
        let a = answer(brain, &tools, &scope, &history, &mut Memory::default(), question, &AtomicBool::new(false), None, false, &mut |e| events.push(e.clone())).unwrap();
        (a, events)
    }

    #[test]
    fn a_thinking_models_step_has_no_thought_in_the_history() {
        let tool = Step::Tool { thought: String::new(), calls: vec![Call { tool: "search".into(), args: json!({"q": "x"}) }] };
        let two = batch(&[("search", json!({"q": "x"})), ("read", json!({"from": 0}))]);
        let answer = Step::Answer { thought: "думаю".into(), text: "Да.".into() };
        assert_eq!(step_json(&tool, true), r#"{"calls":[{"action":"search","args":{"q":"x"}}]}"#);
        assert_eq!(step_json(&two, true), r#"{"calls":[{"action":"search","args":{"q":"x"}},{"action":"read","args":{"from":0}}]}"#);
        assert_eq!(step_json(&answer, true), r#"{"action":"answer","answer":"Да."}"#);
        assert_eq!(step_json(&tool, false), r#"{"thought":"","calls":[{"action":"search","args":{"q":"x"}}]}"#);
        assert!(serde_json::from_str::<Value>(&step_json(&answer, false)).is_ok());
    }

    #[test]
    fn search_then_answer_with_a_citation() {
        let brain = Scripted::new(vec![call("search", json!({"query": "отчёт"})), say("В пятницу [0:10].")]);
        let (a, events) = run(&brain, "Когда отчёт?");
        assert_eq!(a.text, "В пятницу [0:10].");
        assert!(!a.forced);
        assert_eq!(a.steps.len(), 1);
        assert_eq!(a.steps[0].status, StepStatus::Found);
        let story: Vec<&Event> = events.iter().filter(|e| !matches!(e, Event::Metrics(_))).collect();
        assert!(matches!(story[0], Event::StepStarted { title, tool } if title == "Поиск" && tool == "search"));
        assert!(matches!(story[1], Event::Step(s) if s.status == StepStatus::Found));
        let draft: String = events.iter().filter_map(|e| if let Event::AnswerText(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(draft, "В пятницу [0:10].");
        assert!(!events.contains(&Event::AnswerDiscarded));
        let (first, tools) = &brain.seen.borrow()[0];
        assert_eq!(*tools, tools::specs().len() + chat_specs().len());
        assert!(first[0].content.contains("«Планёрка»") && first[0].content.contains("- search: "));
        assert_eq!((first[1].content.as_str(), first[2].content.as_str(), first[3].content.as_str()), ("Привет", "Здравствуйте", "Когда отчёт?"));
        let (second, _) = &brain.seen.borrow()[1];
        assert!(second.last().unwrap().content.starts_with("Result of step 1 (search):\n[0:10–0:33] (@s1, @s2) Отчёт отправляем в пятницу."));
    }

    fn tag(e: &Event) -> String {
        match e {
            Event::Waiting { .. } => "wait".into(),
            Event::StepStarted { tool, .. } => format!("start:{tool}"),
            Event::Step(s) => format!("step:{}", s.tool),
            Event::Thinking(_) => "think".into(),
            Event::AnswerText(_) => "text".into(),
            Event::AnswerDiscarded => "discard".into(),
            Event::Say(_) => "say".into(),
            Event::SteerTaken { .. } => "steer".into(),
            Event::Scanning { .. } => "scan".into(),
            Event::Metrics(_) => "metrics".into(),
            Event::Done => "done".into(),
            Event::Stopped => "stopped".into(),
            Event::Failed(_) => "failed".into(),
        }
    }

    fn tags(events: &[Event]) -> Vec<String> {
        let mut out: Vec<String> = events.iter().filter(|e| !matches!(e, Event::Metrics(_))).map(tag).collect();
        out.dedup();
        out
    }

    #[test]
    fn corrections_of_the_user_are_taken_after_the_step_that_ran() {
        use chat::Live;
        use mediaa_core::chat::TimelineEntry;
        let brain = Scripted::new(vec![call("search", json!({"query": "отчёт"})), call("search", json!({"query": "погода"})), say("Учёл уточнения.")]);
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let (_dir, scope) = test_scope();
        let history = [Turn { question: "Привет".into(), answer: "Здравствуйте".into() }];
        let mut session = Session::new(Memory::default());
        let events = Mutex::new(Vec::new());
        let emit = |e: &Event| events.lock().expect("event sink").push(e.clone());
        let cancel = AtomicBool::new(false);
        let first = advance(&brain, &tools, &scope, &mut session, &history, "Когда отчёт?", &[], &cancel, None, false, &emit).unwrap();
        assert!(matches!(first, Advance::More));
        assert!(brain.seen.borrow()[0].0.iter().all(|m| !m.content.contains("Ещё про бюджет.")));
        let steers = ["Ещё про бюджет.".to_string(), "И про сроки.".to_string()];
        let next = advance(&brain, &tools, &scope, &mut session, &history, "Когда отчёт?", &steers, &cancel, None, false, &emit).unwrap();
        assert!(matches!(next, Advance::More));
        let seen = brain.seen.borrow();
        let texts: Vec<&str> = seen[1].0.iter().map(|m| m.content.as_str()).collect();
        let at = texts.iter().rposition(|t| t.starts_with("Result of step 1 (search):")).unwrap();
        assert_eq!(
            &texts[at + 1..],
            [
                "The user added while you were working: «Ещё про бюджет.». Take it into account from now on; if it changes what is asked, follow the new request.",
                "The user added while you were working: «И про сроки.». Take it into account from now on; if it changes what is asked, follow the new request.",
            ]
        );
        assert_eq!(session.steers_taken, 2);
        drop(seen);
        let mut live = Live::new();
        for e in events.lock().unwrap().iter() {
            live.apply(e);
        }
        let taken: Vec<&str> = live
            .timeline
            .entries
            .iter()
            .filter_map(|e| if let TimelineEntry::Steer { text } = e { Some(text.as_str()) } else { None })
            .collect();
        assert_eq!(taken, ["Ещё про бюджет.", "И про сроки."]);
        let at = live.timeline.entries.iter().position(|e| matches!(e, TimelineEntry::Step { .. })).unwrap();
        assert!(matches!(&live.timeline.entries[at + 1], TimelineEntry::Steer { text } if text == "Ещё про бюджет."));
    }

    #[test]
    fn steps_thinking_and_answer_parts_come_in_one_ordered_stream() {
        use chat::Live;
        use mediaa_core::chat::TimelineEntry;
        let brain = Scripted::new(vec![
            call("search", json!({"query": "отчёт"})),
            call("say", json!({"text": "Часть 1: отчёт в пятницу [0:10]."})),
            call("search", json!({"query": "погода"})),
            say("Остальное — позже."),
        ])
        .thinking(0, "Надо искать отчёт.")
        .thinking(1, "Нашёл, отправлю.")
        .thinking(3, "Теперь закрою.");
        let (a, events) = run(&brain, "Когда отчёт?");
        assert_eq!(a.text, "Остальное — позже.");
        assert_eq!(
            tags(&events),
            ["think", "start:search", "step:search", "think", "start:say", "say", "step:say", "start:search", "step:search", "think", "text"]
        );
        let thinking: String = events.iter().filter_map(|e| if let Event::Thinking(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(thinking, "Надо искать отчёт.Нашёл, отправлю.Теперь закрою.");
        let text: String = events.iter().filter_map(|e| if let Event::AnswerText(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(text, "Остальное — позже.");

        let mut live = Live::new();
        for e in &events {
            live.apply(e);
        }
        live.timeline.finish_answer(&a.text);
        assert_eq!(
            live.timeline.entries,
            [
                TimelineEntry::Thinking { text: "Надо искать отчёт.".into() },
                TimelineEntry::Step { title: "Поиск".into(), tool: "search".into(), status: "found".into() },
                TimelineEntry::Thinking { text: "Нашёл, отправлю.".into() },
                TimelineEntry::Step { title: "Ответ".into(), tool: "say".into(), status: "found".into() },
                TimelineEntry::Answer { text: "Часть 1: отчёт в пятницу [0:10].".into(), section: true },
                TimelineEntry::Step { title: "Поиск".into(), tool: "search".into(), status: "nothing".into() },
                TimelineEntry::Thinking { text: "Теперь закрою.".into() },
                TimelineEntry::Answer { text: "Остальное — позже.".into(), section: false },
            ]
        );
        assert_eq!(live.stored, 1);
        assert_eq!(live.steps.len(), 3);
    }

    #[test]
    fn a_discarded_draft_leaves_the_timeline() {
        use chat::Live;
        let brain = Scripted::new(vec![call("outline", json!({})), say("В пятницу [0:10], и ещё [42:00]."), say("В пятницу [0:10].")]);
        let (_, events) = run(&brain, "Когда?");
        assert_eq!(tags(&events), ["start:outline", "step:outline", "text", "discard", "text"]);
        let mut live = Live::new();
        events.iter().for_each(|e| live.apply(e));
        assert_eq!(live.timeline.draft(), "В пятницу [0:10].", "the first draft is gone, the second one is shown");
    }

    #[test]
    fn the_answer_says_what_the_model_waits_for() {
        struct Slow(Scripted, Duration);
        impl Brain for Slow {
            fn step(&self, m: &[Message], t: &[ToolSpec], may: bool, c: &AtomicBool, on: &mut dyn FnMut(Piece)) -> anyhow::Result<Step> {
                std::thread::sleep(self.1);
                self.0.step(m, t, may, c, on)
            }
            fn complete(&self, m: &[Message], c: &AtomicBool) -> anyhow::Result<String> {
                self.0.complete(m, c)
            }
            fn context(&self) -> u32 {
                self.0.context()
            }
        }
        let brain = Slow(Scripted::new(vec![call("outline", json!({})), say("Готово.")]), Duration::from_millis(300));
        let reason = || Some(WaitReason::Loading { model: "owner/model".into() });
        let waiter = Waiter { reason: &reason, grace: Duration::from_millis(50) };
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let (_dir, scope) = test_scope();
        let mut events = Vec::new();
        answer(&brain, &tools, &scope, &[], &mut Memory::default(), "Что?", &AtomicBool::new(false), Some(&waiter), false, &mut |e| events.push(e.clone())).unwrap();
        assert_eq!(tags(&events)[..3], ["wait", "start:outline", "step:outline"]);
        assert_eq!(events.iter().filter(|e| matches!(e, Event::Waiting { .. })).count(), 2, "once for each step that was silent for long: {:?}", tags(&events));
        let first = events.iter().find(|e| !matches!(e, Event::Metrics(_))).unwrap();
        assert_eq!(first, &Event::Waiting { reason: WaitReason::Loading { model: "owner/model".into() } });

        let none = || None;
        let waiter = Waiter { reason: &none, grace: Duration::from_millis(50) };
        let mut events = Vec::new();
        let brain = Slow(Scripted::new(vec![call("outline", json!({})), say("Готово.")]), Duration::from_millis(150));
        answer(&brain, &tools, &scope, &[], &mut Memory::default(), "Что?", &AtomicBool::new(false), Some(&waiter), false, &mut |e| events.push(e.clone())).unwrap();
        assert!(!events.iter().any(|e| matches!(e, Event::Waiting { .. })));
    }

    #[test]
    fn the_wait_comes_from_the_engines_status() {
        use mediaa_engine::{Budget as EngineBudget, LoadedModel, Pool, Progress, Status, Used, WaitingModel};
        let status = |loaded: &[&str], waiting: Vec<WaitingModel>| Status {
            loaded: loaded
                .iter()
                .map(|m| LoadedModel { model: (*m).to_string(), pool: Pool::Gpu, bytes: 1, users: 1, idle: Duration::ZERO, unloads_in: None })
                .collect(),
            waiting,
            budget: EngineBudget::default(),
            used: Used::default(),
        };
        let waiting = |model: Option<&str>, reason: &str, progress: Option<Progress>| WaitingModel { id: 1, model: model.map(String::from), reason: reason.into(), progress };
        let m = "owner/agent";
        let busy = || vec![crate::scope::TaskRef { kind: "analysis".into(), title: "интервью.mp4".into() }];
        assert_eq!(wait_reason(&status(&[m], vec![]), m, busy), None);
        assert_eq!(wait_reason(&status(&["owner/other"], vec![]), m, busy), Some(WaitReason::Loading { model: m.into() }));
        assert_eq!(wait_reason(&status(&[], vec![waiting(Some(m), "memory", None)]), m, busy), Some(WaitReason::Memory { model: m.into() }));
        let progress = Progress { done: 5, total: 10 };
        assert_eq!(wait_reason(&status(&[], vec![waiting(Some(m), "downloading", Some(progress))]), m, busy), Some(WaitReason::Downloading { model: m.into(), done: 5, total: 10 }));
        assert_eq!(wait_reason(&status(&[m], vec![waiting(None, "turn", None)]), m, busy), Some(WaitReason::Busy { tasks: busy() }));
        assert_eq!(wait_reason(&status(&[m], vec![waiting(Some("owner/other"), "downloading", None)]), m, busy), None);
        let taken = crate::slots::Status { running: vec![], waiting: vec![(1, mediaa_core::agent::WorkKind::ProviderCalls)] };
        assert_eq!(provider_wait(&taken), Some(WaitReason::ProviderSlot));
        assert_eq!(provider_wait(&crate::slots::Status::default()), None);
    }

    #[test]
    fn web_tools_when_enabled_and_links_only_to_pages_shown() {
        let found = |_: &str, _: usize| Ok(vec![crate::web::WebHit { title: "ТК РФ".into(), url: "https://example.org/tk".into(), snippet: "Статья 66.1".into() }]);
        let page = |url: &str, _: usize| Ok(crate::web::Page { title: "ТК РФ".into(), url: url.into(), text: "Сведения о трудовой деятельности. ".repeat(10) });
        let web = internet::Web::new(&found, &page, 4000);
        let brain = Scripted::new(vec![
            call("search", json!({"query": "отчёт"})),
            call("web_search", json!({"query": "электронная трудовая"})),
            call("web_read", json!({"url": "https://example.org/tk"})),
            call("say", json!({"text": "По закону — [ТК РФ](https://example.org/tk)."})),
            say("В записи: пятница [0:10]; см. [ТК РФ](https://example.org/tk) и [блог](https://made.up/x)."),
        ]);
        let rec = recording();
        let tools = Tools { web: Some(&web), ..Tools::new(&rec, &search, 4000) };
        let mut events = Vec::new();
        let (_dir, scope) = test_scope();
        let a = answer(&brain, &tools, &scope, &[], &mut Memory::default(), "Что с трудовой?", &AtomicBool::new(false), None, false, &mut |e| events.push(e.clone())).unwrap();
        assert_eq!(a.text, "В записи: пятница [0:10]; см. [ТК РФ](https://example.org/tk) и блог.");
        assert!(a.steps.iter().all(|s| s.status == StepStatus::Found), "{:?}", a.steps);
        assert!(events.contains(&Event::Say("По закону — [ТК РФ](https://example.org/tk).".into())), "a section may rest on a page read");
        let (first, n) = &brain.seen.borrow()[0];
        assert_eq!(*n, tools::specs().len() + chat_specs().len() + 2);
        assert!(first[0].content.contains("- web_search: ") && first[0].content.contains("[title](address)"));

        let brain = Scripted::new(vec![call("search", json!({"query": "отчёт"})), say("Пятница [0:10].")]);
        run(&brain, "Когда?");
        let (first, _) = &brain.seen.borrow()[0];
        assert!(!first[0].content.contains("web_search"));
    }

    #[test]
    fn chat_tools_remember_plan_and_send_parts() {
        let brain = Scripted::new(vec![
            call("remember", json!({"fact": "Пользователь просит кратко"})),
            call("plan", json!({"items": [{"text": "найти сроки", "done": true}, {"text": "написать отчёт", "done": false}]})),
            call("say", json!({"text": "Часть без ссылок."})),
            call("say", json!({"text": "Часть 1: отчёт в пятницу [0:10], и ещё [42:00]."})),
            say(&format!("Всё отправлено.\n\n{}", "Повтор всех разделов. ".repeat(30))),
        ]);
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let mut memory = Memory::default();
        let mut events = Vec::new();
        let (_dir, scope) = test_scope();
        let a = answer(&brain, &tools, &scope, &[], &mut memory, "Отчёт частями", &AtomicBool::new(false), None, false, &mut |e| events.push(e.clone())).unwrap();
        assert_eq!(memory.facts, ["Пользователь просит кратко"]);
        assert_eq!(a.steps[1].result, "Plan:\n[x] найти сроки\n[ ] написать отчёт");
        assert_eq!(a.steps[2].status, StepStatus::Failed, "a section without citations is refused");
        assert!(a.steps[2].result.contains("must cite"));
        assert!(events.contains(&Event::Say("Часть 1: отчёт в пятницу [0:10], и ещё.".into())), "sent, a wrong time dropped: {events:?}");
        assert_eq!(a.text, "Всё отправлено.", "after sections the answer only closes them");
        let brain = Scripted::new(vec![call("outline", json!({})), say("Кратко.")]);
        let (_dir, scope) = test_scope();
        answer(&brain, &tools, &scope, &[], &mut memory, "Ещё", &AtomicBool::new(false), None, false, &mut |_| {}).unwrap();
        assert!(brain.seen.borrow()[0].0[0].content.contains("Remembered in this chat:\n- Пользователь просит кратко"));
    }

    #[test]
    fn a_cut_off_step_is_told_and_the_answer_goes_on() {
        struct CutOnce(Scripted, std::cell::Cell<bool>);
        impl Brain for CutOnce {
            fn step(&self, m: &[Message], t: &[ToolSpec], may: bool, c: &AtomicBool, on: &mut dyn FnMut(Piece)) -> anyhow::Result<Step> {
                if !self.1.replace(true) {
                    return Err(anyhow::anyhow!("the step is not JSON (EOF while parsing a string): {{\"action\":\"say\""));
                }
                self.0.step(m, t, may, c, on)
            }
            fn complete(&self, m: &[Message], c: &AtomicBool) -> anyhow::Result<String> {
                self.0.complete(m, c)
            }
            fn context(&self) -> u32 {
                self.0.context()
            }
        }
        let brain = CutOnce(Scripted::new(vec![call("outline", json!({})), say("Готово [0:10].")]), std::cell::Cell::new(false));
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let (_dir, scope) = test_scope();
        let a = answer(&brain, &tools, &scope, &[], &mut Memory::default(), "Отчёт", &AtomicBool::new(false), None, false, &mut |_| {}).unwrap();
        assert_eq!(a.text, "Готово [0:10].");
        assert!(brain.0.seen.borrow()[0].0.iter().any(|m| m.content.contains("got cut off")));
    }

    #[test]
    fn a_long_history_is_folded_into_a_summary() {
        let mut brain = Scripted::new(vec![call("outline", json!({})), say("Ок.")]);
        brain.context = 2000;
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let history: Vec<Turn> = (0..6).map(|i| Turn { question: format!("вопрос {i}"), answer: "ответ ".repeat(150) }).collect();
        let mut memory = Memory::default();
        let (_dir, scope) = test_scope();
        answer(&brain, &tools, &scope, &history, &mut memory, "Новый вопрос", &AtomicBool::new(false), None, false, &mut |_| {}).unwrap();
        assert_eq!(memory.summarized, 3, "the last three stay");
        assert!(memory.summary.starts_with("сводка"));
        let seen = brain.seen.borrow();
        assert!(seen[0].0[1].content.contains("вопрос 0"));
        let prompt = &seen[1].0;
        assert!(prompt[0].content.contains("summarised:\nсводка"));
        assert!(!prompt.iter().any(|m| m.content == "вопрос 2"));
        assert!(prompt.iter().any(|m| m.content == "вопрос 3"));
    }

    #[test]
    fn repeats_are_not_run_and_stalls_end_in_an_answer() {
        let brain = Scripted::new(vec![
            call("search", json!({"query": "отчёт"})),
            call("read", json!({"from": "0:00", "to": "0:20"})),
            call("search", json!({"query": "погода"})),
            call("search", json!({"query": "  Отчёт "})),
            call("speakers", json!({})),
            say("Не знаю."),
        ]);
        let (a, _) = run(&brain, "Что с погодой?");
        let statuses: Vec<StepStatus> = a.steps.iter().map(|s| s.status).collect();
        assert_eq!(statuses, [StepStatus::Found, StepStatus::Nothing, StepStatus::Nothing, StepStatus::Repeated]);
        assert!(a.steps[1].result.starts_with("Nothing new"), "{}", a.steps[1].result);
        assert!(a.steps[2].result.starts_with("Nothing found"));
        assert!(a.steps[3].result.starts_with("Already done at step 1"));
        assert!(a.forced, "steps without anything new");
        assert_eq!(a.text, "Не знаю.");
        let last = brain.seen.borrow().last().unwrap().clone();
        assert_eq!(last.1, 0, "no tools offered when it must answer");
        assert!(last.0.iter().any(|m| m.content.starts_with("Answer now")));
    }

    #[test]
    fn wrong_times_go_back_once_then_are_dropped() {
        let brain = Scripted::new(vec![call("outline", json!({})), say("В пятницу [0:10], и ещё [42:00]."), say("В пятницу [0:10] и [9:59].")]);
        let (a, events) = run(&brain, "Когда?");
        assert_eq!(a.text, "В пятницу [0:10] и.", "the second wrong time is dropped");
        assert_eq!(events.iter().filter(|e| **e == Event::AnswerDiscarded).count(), 1, "the first draft went back");
        let fix = brain.seen.borrow()[2].0.last().unwrap().content.clone();
        assert!(fix.contains("[42:00]") && fix.contains("as [m:ss]"), "{fix}");
    }

    #[test]
    fn the_step_limit_forces_an_answer() {
        let mut script: Vec<Step> = (0..MAX_STEPS).map(|i| call("calculator", json!({"expression": format!("{i}+1")}))).collect();
        script.push(call("calculator", json!({"expression": "99"})));
        script.push(say("Готово."));
        let brain = Scripted::new(script);
        let (a, _) = run(&brain, "Посчитай");
        assert_eq!(a.steps.len(), MAX_STEPS);
        assert!(a.forced);
    }

    struct Together {
        state: Mutex<(usize, usize)>,
        cv: std::sync::Condvar,
    }

    impl Toolbox for Together {
        fn specs(&self) -> Vec<ToolSpec> {
            ["slow", "fast"].map(|name| ToolSpec { name, description: String::new(), params: json!({"type": "object", "properties": {}, "required": []}), example: json!({}) }).to_vec()
        }
        fn run(&self, tool: &str, _: &Value) -> anyhow::Result<tools::Outcome> {
            let mut state = self.state.lock().expect("state");
            state.0 += 1;
            state.1 = state.1.max(state.0);
            self.cv.notify_all();
            let until = Instant::now() + Duration::from_secs(2);
            while state.1 < 2 {
                let left = until.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    break;
                }
                state = self.cv.wait_timeout(state, left).expect("state").0;
            }
            state.0 -= 1;
            drop(state);
            if tool == "slow" {
                std::thread::sleep(Duration::from_millis(30));
            }
            Ok(tools::Outcome { text: format!("{tool} done"), seen: Some(vec![tool.to_string()]) })
        }
        fn kind(&self) -> &'static str {
            "recording"
        }
        fn card(&self) -> String {
            String::new()
        }
        fn bad_citations(&self, _: &str) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn calls_of_one_step_run_at_once_and_come_back_in_their_order() {
        let brain = Scripted::new(vec![batch(&[("slow", json!({})), ("fast", json!({}))]), say("Готово [0:10].")]);
        let tools = Together { state: Mutex::new((0, 0)), cv: std::sync::Condvar::new() };
        let mut events = Vec::new();
        let (_dir, scope) = test_scope();
        let a = answer(&brain, &tools, &scope, &[], &mut Memory::default(), "Что?", &AtomicBool::new(false), None, false, &mut |e| events.push(e.clone())).unwrap();
        assert_eq!(*tools.state.lock().unwrap(), (0, 2), "both calls were inside at once");
        assert_eq!(a.steps.iter().map(|s| (s.tool.as_str(), s.result.as_str())).collect::<Vec<_>>(), [("slow", "slow done"), ("fast", "fast done")]);
        let story: Vec<String> = events.iter().filter(|e| !matches!(e, Event::Metrics(_))).map(tag).collect();
        assert_eq!(story, ["start:slow", "start:fast", "step:slow", "step:fast", "text", "text"]);
    }

    #[test]
    fn the_same_call_twice_in_one_step_runs_once() {
        let searched = std::sync::atomic::AtomicUsize::new(0);
        let counted = |q: &str, o: crate::search::Options, n: usize| {
            searched.fetch_add(1, Ordering::Relaxed);
            search(q, o, n)
        };
        let rec = recording();
        let tools = Tools::new(&rec, &counted, 4000);
        let brain = Scripted::new(vec![
            batch(&[("search", json!({"query": "отчёт"})), ("search", json!({"query": "отчёт"}))]),
            say("В пятницу [0:10]."),
        ]);
        let mut events = Vec::new();
        let (_dir, scope) = test_scope();
        let a = answer(&brain, &tools, &scope, &[], &mut Memory::default(), "Когда отчёт?", &AtomicBool::new(false), None, false, &mut |e| events.push(e.clone())).unwrap();
        assert_eq!(searched.load(Ordering::Relaxed), 1, "the second copy of the call is not run");
        assert_eq!(a.steps.iter().map(|s| s.status).collect::<Vec<_>>(), [StepStatus::Found, StepStatus::Repeated]);
        assert!(a.steps[1].result.starts_with("Already done at step 1"), "{}", a.steps[1].result);
    }

    #[test]
    fn every_call_of_a_step_counts_as_a_step_of_its_own() {
        let three = |n: usize| {
            let expression = |k: usize| json!({ "expression": format!("{n}+{k}") });
            batch(&[("calculator", expression(1)), ("calculator", expression(2)), ("calculator", expression(3))])
        };
        let brain = Scripted::new(vec![three(1), three(2), three(3), say("Готово.")]);
        let (a, _) = run(&brain, "Посчитай");
        assert_eq!(a.steps.len(), MAX_STEPS, "the last batch has room for the rest of the limit only");
        assert!(a.forced);
    }

    fn checked(brain: &Scripted, question: &str) -> (Answer, Vec<Event>) {
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let mut events = Vec::new();
        let (_dir, scope) = test_scope();
        let a = answer(brain, &tools, &scope, &[], &mut Memory::default(), question, &AtomicBool::new(false), None, true, &mut |e| events.push(e.clone())).unwrap();
        (a, events)
    }

    #[test]
    fn a_check_may_drop_statements_but_never_brings_new_links() {
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let draft = "В пятницу [0:10]. См. [закон](https://example.org/tk).";
        let keep = |raw: &str| assert_eq!(accept(draft, raw, &tools), draft, "{raw}");
        keep("не json");
        keep(r#"{"changed":true}"#);
        keep(r#"{"changed":false,"answer":"Другой текст [0:10]. См. [закон](https://example.org/tk)."}"#);
        keep(r#"{"changed":true,"answer":"В пятницу [0:10] и [42:00]. См. [закон](https://example.org/tk)."}"#);
        keep(r#"{"changed":true,"answer":"  "}"#);
        keep(r#"{"changed":true,"answer":"В пятницу [0:10]. См. [закон](https://example.org/tk) и [ещё](https://made.up/x)."}"#);
        let better = "Отчёт — в пятницу [0:10]. См. [закон](https://example.org/tk).";
        assert_eq!(accept(draft, &format!("вот {{\"changed\":true,\"answer\":\"{better}\"}}"), &tools), better);
        let take = |raw: &str, expected: &str| assert_eq!(accept(draft, raw, &tools), expected, "{raw}");
        take(r#"{"changed":true,"answer":"В пятницу. См. [закон](https://example.org/tk)."}"#, "В пятницу. См. [закон](https://example.org/tk).");
        take(r#"{"changed":true,"answer":"В пятницу [0:10]."}"#, "В пятницу [0:10].");
        take(r#"{"changed":true,"answer":"В пятницу [0:10].\n\nchanged — true"}"#, "В пятницу [0:10].");
        assert_eq!(accept("В [#1 0:10] решили.", r#"{"changed":true,"answer":"Решили."}"#, &tools), "Решили.");

        let brain = Scripted::new(vec![call("outline", json!({})), say("В пятницу [0:10].")]).checking(r#"{"changed":true,"answer":"В [42:00]."}"#);
        let (a, events) = checked(&brain, "Когда?");
        assert_eq!(a.text, "В пятницу [0:10].", "a bad time keeps the draft");
        assert!(events.iter().any(|e| matches!(e, Event::Step(s) if s.tool == "verify" && s.status == StepStatus::Nothing)));

        let brain = Scripted::new(vec![call("outline", json!({})), say("В пятницу [0:10].")]).checking("не json");
        assert_eq!(checked(&brain, "Когда?").0.text, "В пятницу [0:10].", "an unreadable verdict keeps the draft");

        let brain = Scripted::new(vec![call("outline", json!({})), say("В пятницу [0:10].")]).checking(r#"{"changed":true,"answer":"Нет."}"#);
        assert_eq!(run(&brain, "Когда?").0.text, "В пятницу [0:10].");
        assert!(brain.check.borrow().is_some(), "the check is not called when it is off");
    }

    #[test]
    fn the_check_streams_the_answer_after_itself_and_sees_the_cited_window() {
        let brain = Scripted::new(vec![call("search", json!({"query": "отчёт"})), say("В пятницу [0:10] и потом.")])
            .checking(r#"{"changed":true,"answer":"Отчёт в пятницу [0:10]."}"#);
        let (a, events) = checked(&brain, "Когда отчёт?");
        assert_eq!(a.text, "Отчёт в пятницу [0:10].");
        assert_eq!(tags(&events), ["start:search", "step:search", "text", "discard", "start:draft", "step:draft", "start:verify", "step:verify", "text"]);
        let discard = events.iter().position(|e| matches!(e, Event::AnswerDiscarded)).unwrap();
        let verified = events.iter().rposition(|e| matches!(e, Event::Step(s) if s.tool == "verify")).unwrap();
        assert!(events[discard + 1..verified].iter().all(|e| !matches!(e, Event::AnswerText(_))), "nothing of the answer is written during the check");
        assert!(matches!(&events[verified + 1], Event::AnswerText(t) if t == "Отчёт в пятницу [0:10]."));
        assert!(a.steps.iter().any(|s| s.tool == "draft" && s.result == "В пятницу [0:10] и потом."));
        let prompt = brain.seen.borrow().last().unwrap().0.last().unwrap().content.clone();
        assert!(prompt.contains("Когда отчёт?") && prompt.contains("В пятницу [0:10] и потом."));
        assert!(prompt.contains("Отчёт отправляем в пятницу.") && prompt.contains("Графики будут к четвергу."), "{prompt}");
        assert!(!prompt.contains("Бюджет на квартал"), "{prompt}");
    }

    #[test]
    fn the_check_is_shown_the_web_result_only_when_the_draft_cites_it() {
        let found = |q: &str, _: usize| {
            Ok(if q.contains("закон") {
                vec![crate::web::WebHit { title: "ТК".into(), url: "https://example.org/tk".into(), snippet: "Статья.".into() }]
            } else {
                vec![]
            })
        };
        let page = |url: &str, _: usize| Ok(crate::web::Page { title: "Страница".into(), url: url.into(), text: format!("Текст {url}") });
        let web = internet::Web::new(&found, &page, 4000);
        let brain = Scripted::new(vec![
            call("search", json!({"query": "отчёт"})),
            call("web_search", json!({"query": "закон"})),
            call("web_read", json!({"url": "https://example.org/other"})),
            call("web_read", json!({"url": "https://example.org/tk"})),
            say("В пятницу [0:10]. См. [ТК](https://example.org/tk)."),
        ])
        .checking(r#"{"changed":true,"answer":"Отчёт в пятницу [0:10]. См. [ТК](https://example.org/tk)."}"#);
        let rec = recording();
        let tools = Tools { web: Some(&web), ..Tools::new(&rec, &search, 4000) };
        let (_dir, scope) = test_scope();
        let a = answer(&brain, &tools, &scope, &[], &mut Memory::default(), "Что с законом?", &AtomicBool::new(false), None, true, &mut |_| {}).unwrap();
        assert_eq!(a.text, "Отчёт в пятницу [0:10]. См. [ТК](https://example.org/tk).");
        let prompt = brain.seen.borrow().last().unwrap().0.last().unwrap().content.clone();
        assert!(prompt.contains("https://example.org/tk") && prompt.contains("Отчёт отправляем в пятницу."), "{prompt}");
        assert!(!prompt.contains("https://example.org/other"), "{prompt}");
    }

    #[test]
    #[ignore]
    fn a_real_model_answers_from_the_recording() {
        use mediaa_core::agent::{AgentSettings, Budget};
        use mediaa_core::transcript::{self, Transcript};
        let ai = std::path::PathBuf::from(std::env::var_os("MEDIAA_TEST_AI").expect("MEDIAA_TEST_AI"));
        let id = transcript::file_id(&ai.join("example.wav")).unwrap();
        let t = Transcript::open(&ai, &id).unwrap();
        let replies = t.dialog().unwrap();
        assert!(!replies.is_empty(), "transcribe the sample first");
        let rec = Recording { title: "Пушкин".into(), duration: 11.3, diarized: true, names: t.speaker_names().unwrap(), replies, chapters: t.chapters().unwrap(), position: None };
        let settings = AgentSettings::default();
        let engine = crate::live::engine();
        let slots = Slots::new(Budget::default());
        let question = "Какое дерево упоминается в записи и в какой момент?";
        let scope = Scope::begin(&ai, engine.clone(), "chat", "тест", &json!({}), Some(&id)).unwrap();
        let a = ask(&scope, &slots, &settings, &ai, &id, &rec, &[], &mut Memory::default(), question, &AtomicBool::new(false), &mut |e| eprintln!("{e:?}")).unwrap();
        eprintln!("{}", a.text);
        assert!(a.text.to_lowercase().contains("дуб"), "{}", a.text);
        assert!(!citations(&a.text).is_empty(), "cites the moment: {}", a.text);
        assert!(bad_citations(&a.text, &rec).is_empty());
    }

    #[test]
    #[ignore]
    fn bench_on_a_recording() {
        use mediaa_core::agent::{AgentSettings, Budget};
        use mediaa_core::transcript::{self, Transcript};
        let _ = env_logger::Builder::from_env(env_logger::Env::default()).try_init();
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
        let ai = mediaa_core::catalogs::ai_dir(std::path::Path::new(&var("MEDIAA_BENCH_LIB")));
        let file = std::path::PathBuf::from(var("MEDIAA_BENCH_FILE"));
        let id = transcript::file_id(&file).unwrap();
        let mut settings = AgentSettings::default();
        if let Ok(key) = std::env::var("MEDIAA_BENCH_TAVILY") {
            settings.web.engine = mediaa_core::agent::WebEngine::Tavily;
            settings.web.api_key = key;
        }
        if let Ok(proxy) = std::env::var("MEDIAA_BENCH_PROXY") {
            settings.web.proxy = proxy;
        }
        if std::env::var_os("MEDIAA_BENCH_THINKING").is_some() {
            if let Some(model) = settings.models.agent.clone() {
                settings.set_thinking(&model, true);
            }
        }
        let engine = crate::live::engine();
        let slots = Slots::new(Budget::default());
        let s = settings.clone();
        let current: std::sync::Arc<dyn Fn() -> AgentSettings + Send + Sync> = std::sync::Arc::new(move || s.clone());
        let sched = crate::run::open_scheduler(&ai, &settings.budget).unwrap();
        crate::run::register_stages(&sched, &ai, engine.clone(), slots.clone(), current.clone(), std::sync::Arc::new(crate::run::Streams::default()));
        let drain = || {
            crate::run::drain(&sched, std::time::Duration::from_secs(3600));
        };
        let print_tasks = || {
            let now = sched.store().now_ms();
            for t in sched.store().list().unwrap().iter().filter(|t| t.created_ms >= 0).take(40) {
                let time = t.started_ms.map_or(0.0, |start| (t.finished_ms.unwrap_or(now) - start) as f64 / 1000.0);
                eprintln!("task {} {} {:?} {time:.0} s {}", t.kind, t.title, t.status, t.error.clone().unwrap_or_default());
            }
        };
        if let Ok(from) = std::env::var("MEDIAA_BENCH_FROM") {
            use crate::pipeline::Stage;
            let stage = match from.as_str() {
                "transcribe" => Stage::Transcribe,
                "diarize" => Stage::Diarize,
                "index" => Stage::Index,
                "topics" => Stage::Topics,
                "details" => Stage::Details,
                other => panic!("MEDIAA_BENCH_FROM: no stage {other} (transcribe, diarize, index, topics, details)"),
            };
            let started = std::time::Instant::now();
            crate::pipeline::restart(&sched, &ai, &file, stage, settings.analysis).unwrap();
            drain();
            print_tasks();
            let ready = crate::pipeline::readiness(&ai, &id).unwrap();
            eprintln!("from {from} in {:.0} s, ready: {ready:?}", started.elapsed().as_secs_f32());
        } else if std::env::var_os("MEDIAA_BENCH_ANALYSIS").is_some() {
            let started = std::time::Instant::now();
            crate::pipeline::restart(&sched, &ai, &file, crate::pipeline::Stage::Transcribe, settings.analysis).unwrap();
            drain();
            print_tasks();
            let ready = crate::pipeline::readiness(&ai, &id).unwrap();
            eprintln!("analysis in {:.0} s, ready: {ready:?}", started.elapsed().as_secs_f32());
        } else if std::env::var_os("MEDIAA_BENCH_OUTLINE").is_some() {
            let started = std::time::Instant::now();
            crate::pipeline::restart(&sched, &ai, &file, crate::pipeline::Stage::Topics, settings.analysis).unwrap();
            drain();
            eprintln!("outline rebuilt in {:.0} s", started.elapsed().as_secs_f32());
        } else if std::env::var_os("MEDIAA_BENCH_MERGE").is_some() {
            let t = Transcript::open(&ai, &id).unwrap();
            let key = crate::outline::TOPICS_KEY;
            let json = t.meta(key).unwrap().unwrap_or_else(|| panic!("no {key} in the transcript: run with MEDIAA_BENCH_OUTLINE first"));
            let topics: Vec<mediaa_core::outline::Chapter> = serde_json::from_str(&json).unwrap();
            crate::outline::write_chapters(&t, &topics).unwrap();
            for key in [crate::outline::MERGE_KEY, crate::outline::MERGED_KEY, crate::outline::FINAL_KEY] {
                t.remove_meta(key).unwrap();
            }
            drop(t);
            let started = std::time::Instant::now();
            let input = serde_json::to_value(crate::outline::Input { path: file.clone(), options: settings.analysis }).unwrap();
            sched.enqueue(mediaa_queue::NewTask::new(crate::outline::MERGE_KIND, "rec.mp4", &input).resource(mediaa_queue::Resource::Llm).analysis(&id)).unwrap();
            drain();
            eprintln!("merge rebuilt in {:.0} s", started.elapsed().as_secs_f32());
        } else if std::env::var_os("MEDIAA_BENCH_DETAILS").is_some() {
            let started = std::time::Instant::now();
            crate::pipeline::restart(&sched, &ai, &file, crate::pipeline::Stage::Details, settings.analysis).unwrap();
            drain();
            eprintln!("details rebuilt in {:.0} s", started.elapsed().as_secs_f32());
        }
        let t = Transcript::open(&ai, &id).unwrap();
        for c in t.chapters().unwrap() {
            eprintln!("chapter [{}] {} — {}", crate::tools::format_time(c.start), c.digest.title, c.digest.summary);
            for section in &c.digest.sections {
                eprintln!("  {} ({:?})", section.title, section.kind);
                for item in &section.items {
                    let time = item.time.map_or_else(|| "—".to_string(), crate::tools::format_time);
                    eprintln!("    [{time}] {}", item.text);
                }
            }
        }
        let replies = t.dialog().unwrap();
        let duration = replies.last().map_or(0.0, |r| r.end);
        let rec = Recording {
            title: file.file_stem().unwrap().to_string_lossy().into_owned(),
            duration,
            diarized: replies.iter().any(|r| r.speaker.is_some()),
            names: t.speaker_names().unwrap(),
            replies,
            chapters: t.chapters().unwrap(),
            position: None,
        };
        let items: Vec<folder::Item> = if std::env::var_os("MEDIAA_BENCH_FOLDER").is_some() {
            let cat = mediaa_core::catalogs::Catalogs::open(&ai).unwrap();
            cat.nodes()
                .iter()
                .filter_map(|n| cat.file_path(n.id))
                .filter_map(|p| {
                    let id = transcript::file_id(p).ok()?;
                    let t = Transcript::open(&ai, &id).ok()?;
                    let replies = t.dialog().ok()?;
                    let recording = Recording {
                        title: p.file_stem()?.to_string_lossy().into_owned(),
                        duration: replies.last().map_or(0.0, |r| r.end),
                        diarized: replies.iter().any(|r| r.speaker.is_some()),
                        names: t.speaker_names().ok()?,
                        replies,
                        chapters: t.chapters().ok()?,
                        position: None,
                    };
                    Some(folder::Item { recording, file_id: id, path: p.to_path_buf() })
                })
                .collect()
        } else {
            Vec::new()
        };
        let mut history: Vec<Turn> = Vec::new();
        let mut memory = Memory::default();
        let questions: Vec<String> = std::env::var("MEDIAA_BENCH_QUESTIONS")
            .map(|s| s.split('|').map(|q| q.trim().to_string()).filter(|q| !q.is_empty()).collect())
            .unwrap_or_default();
        for q in questions {
            let started = std::time::Instant::now();
            let mut print = |e: &Event| match e {
                Event::Step(s) => eprintln!("    · {:?} {} {} → {}", s.status, s.tool, s.args, s.result.chars().take(if s.tool == "scan" { 3000 } else { 160 }).collect::<String>().replace('\n', " / ")),
                Event::Say(t) => eprintln!("    » {t}"),
                Event::Thinking(t) => eprint!("{t}"),
                Event::Waiting { reason } => eprintln!("    … {reason:?}"),
                _ => {}
            };
            let scope = Scope::begin(&ai, engine.clone(), "chat", q.trim(), &json!({}), Some(&id)).unwrap();
            let cancel = AtomicBool::new(false);
            let a = if items.is_empty() {
                ask(&scope, &slots, &settings, &ai, &id, &rec, &history, &mut memory, q.trim(), &cancel, &mut print).unwrap()
            } else {
                ask_folder(&scope, &slots, &settings, &ai, &items, &history, &mut memory, q.trim(), &cancel, &mut print).unwrap()
            };
            scope.finish(&json!({ "steps": a.steps.len() }));
            eprintln!("Q: {q}\nA ({:.0} s, {} steps{}): {}\n", started.elapsed().as_secs_f32(), a.steps.len(), if a.forced { ", forced" } else { "" }, a.text);
            if items.is_empty() {
                assert!(bad_citations(&a.text, &rec).is_empty(), "{}", a.text);
            }
            history.push(Turn { question: q.trim().to_string(), answer: a.text });
        }
        eprintln!("memory: {memory:?}");
    }

    #[test]
    fn answers_are_markdown() {
        assert_eq!(tidy(" Один<br>два<br />три<br/>четыре "), "Один\nдва\nтри\nчетыре");
    }

    #[test]
    fn a_user_mention_becomes_a_label_before_the_model_reads_it() {
        let rec = tools::tests::recording();
        let speakers = tools::whos(&rec, None);
        let history = [Turn { question: "а @Дана?".into(), answer: "уже @s1".into() }];
        let (history, question) = for_model(&speakers, &history, "и @Говорящий 2");
        assert_eq!(question, "и @s2");
        assert_eq!(history[0].question, "а @s1?");
        assert_eq!(history[0].answer, "уже @s1", "an answer already stores labels");
        let folder = tools::whos(&rec, Some(3));
        assert_eq!(mediaa_core::speakers::to_model("@Дана", &folder), "@s1#3");
        assert_eq!(mediaa_core::speakers::to_model("@s1", &speakers), "@s1");
    }

    #[test]
    fn citations_and_fingerprints() {
        assert_eq!(citations("в [1:05] и [1:02:30–1:03:00], не [ссылка] и не [5]").iter().map(|(_, t)| *t).collect::<Vec<_>>(), [65.0, 3750.0]);
        assert_eq!(fingerprint("search", &json!({"query": " Бюджет  квартала"})), fingerprint("search", &json!({"query": "бюджет квартала"})));
        assert_ne!(fingerprint("search", &json!({"query": "a"})), fingerprint("read", &json!({"query": "a"})));
    }

    #[test]
    fn a_tool_result_begins_the_way_compaction_finds_it() {
        let result = crate::prompts::render("results/step.j2", minijinja::context! { step => 3, tool => "search", text => "x" });
        assert!(result.starts_with(RESULT), "compaction finds old results by this prefix: {result}");
    }

    #[test]
    fn old_results_are_shortened_first() {
        let long = "x".repeat(3000);
        let mut m = vec![
            Message::new(Role::System, "s"),
            Message::new(Role::User, format!("{RESULT}1 (search):\n{long}")),
            Message::new(Role::User, format!("{RESULT}2 (search):\n{long}")),
            Message::new(Role::User, format!("{RESULT}3 (search):\n{long}")),
        ];
        compact(&mut m, 4000);
        assert!(m[1].content.contains("shortened"));
        assert!(!m[2].content.contains("shortened") && !m[3].content.contains("shortened"), "the last two stay");
        let mut roomy = m.clone();
        roomy[1] = Message::new(Role::User, format!("{RESULT}1 (search):\n{long}"));
        compact(&mut roomy, 100_000);
        assert!(!roomy[1].content.contains("shortened"), "only when needed");
    }

    #[test]
    fn a_long_result_is_cut_in_the_middle_and_a_fifth_of_the_context_stays_free() {
        let mut m = vec![Message::new(Role::System, "s".repeat(600)), Message::new(Role::User, format!("{RESULT}1 (search):\n{}", "x".repeat(12_000)))];
        make_room(&mut m, 1000);
        assert!(estimate_tokens(&m) <= 800, "{} tokens", estimate_tokens(&m));
        let cut = &m[1].content;
        assert!(cut.contains("(cut)"), "the cut is marked: {cut:.80}");
        assert!(cut.starts_with(&format!("{RESULT}1 (search):\nxx")), "the head stays");
        assert!(cut.ends_with('x'), "the tail stays: {cut:.80}");
        let mut small = vec![Message::new(Role::System, "system"), Message::new(Role::User, "question")];
        let before = small.clone();
        make_room(&mut small, 1000);
        assert_eq!(small, before);
    }

    struct Huge;

    impl Toolbox for Huge {
        fn specs(&self) -> Vec<ToolSpec> {
            vec![ToolSpec { name: "search", description: String::new(), params: json!({"type": "object", "properties": {}, "required": []}), example: json!({}) }]
        }
        fn run(&self, _: &str, _: &Value) -> anyhow::Result<tools::Outcome> {
            Ok(tools::Outcome { text: "x".repeat(20_000), seen: None })
        }
        fn kind(&self) -> &'static str {
            "recording"
        }
        fn card(&self) -> String {
            String::new()
        }
        fn bad_citations(&self, _: &str) -> Vec<String> {
            Vec::new()
        }
    }

    #[test]
    fn every_step_starts_with_room_for_the_answer() {
        let brain = Scripted { context: 2000, ..Scripted::new(vec![call("search", json!({})), call("search", json!({})), say("Готово.")]) };
        let (_dir, scope) = test_scope();
        let mut events = Vec::new();
        answer(&brain, &Huge, &scope, &[], &mut Memory::default(), "Что сказано?", &AtomicBool::new(false), None, false, &mut |e| events.push(e.clone())).unwrap();
        let seen = brain.seen.borrow();
        assert_eq!(seen.len(), 3, "two tools and the answer");
        for (messages, _) in seen.iter() {
            assert!(estimate_tokens(messages) <= 1600, "the answer has less than a fifth of the context: {} tokens", estimate_tokens(messages));
        }
        assert!(seen[1].0.iter().any(|m| m.content.contains("(cut)")), "the result was cut");
        let metrics: Vec<Metrics> = events.iter().filter_map(|e| if let Event::Metrics(m) = e { Some(*m) } else { None }).collect();
        assert!(!metrics.is_empty());
        assert!(metrics.iter().all(|m| m.context == 2000));
        assert!(metrics.iter().any(|m| m.context_tokens > 0));
    }
}
