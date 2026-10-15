//! The agent: answers questions about a recording with tools, one step at a time (plan → act →
//! observe), reliable down to small local models:
//! * every step is structured (a tool call or the answer), never free text to parse;
//! * a call repeated with the same arguments is not run again (fingerprints), a result that only
//!   shows what was already seen counts as «nothing new», and several steps without anything new
//!   — or the step limit — make the agent answer with what it has;
//! * times cited in the answer (`[12:30]`) are checked against the transcript; wrong ones go
//!   back to the model once, then are dropped;
//! * old tool results are shortened when the context fills up, the question and the latest
//!   results stay whole.

pub mod brain;
pub mod folder;
pub mod internet;
pub mod memory;
pub mod provider;
pub mod scan;
pub mod tools;

pub use brain::{Brain, LocalBrain, Step, ToolSpec};
pub use memory::Memory;
pub use provider::ProviderBrain;
pub use tools::{Recording, Tools};

use crate::llm::{Message, Role};
use crate::tools::parse_time;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;

/// Steps before the agent must answer.
pub const MAX_STEPS: usize = 8;
/// Steps in a row without anything new before the agent must answer.
const STALL_STEPS: usize = 3;
/// A cited time is right when a reply is this close (seconds).
const NEAR: f64 = 15.0;
/// Share of the context the prompt may fill before old results are shortened.
const PROMPT_SHARE: f32 = 0.6;
/// Longest answer after sections were sent (longer: only its first paragraph is kept).
const CLOSING_CHARS: usize = 400;
/// Longest section sent with `say` (characters; ≈ a third of the step's tokens in Russian).
const SECTION_CHARS: usize = 2400;
/// Characters kept of a shortened result.
const STUB_CHARS: usize = 300;

/// How a step went (the honest trace: found / nothing / repeated / failed never look the same).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Found,
    Nothing,
    Repeated,
    Failed,
}

/// A tool step as the user sees it in the answer's details.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StepRecord {
    pub title: String,
    pub tool: String,
    pub args: Value,
    pub result: String,
    pub status: StepStatus,
}

/// Progress of an answer.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Step(StepRecord),
    /// More of the answer as the model writes it.
    Draft(String),
    /// The text streamed so far is not the answer after all (it goes back to the model).
    DraftDiscarded,
    /// A section of a long answer, sent before the rest.
    Say(String),
    /// The scan of the whole recording has read `done` of `total` parts.
    Scanning { done: usize, total: usize },
    Answer(String),
}

/// An earlier question and its answer in the chat.
#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    pub question: String,
    pub answer: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Answer {
    pub text: String,
    pub steps: Vec<StepRecord>,
    /// The agent had to answer before it was done (step limit or nothing new).
    pub forced: bool,
}

/// What the agent works with: one recording, or the recordings of a folder.
pub trait Toolbox {
    fn specs(&self) -> Vec<ToolSpec>;
    fn run(&self, tool: &str, args: &Value) -> anyhow::Result<tools::Outcome>;
    /// What the questions are about, for the system prompt.
    fn subject(&self) -> &'static str;
    /// How moments are cited.
    fn citation_form(&self) -> &'static str;
    /// What the agent knows up front (the card).
    fn card(&self) -> String;
    /// Citations in `text` that point at nothing said.
    fn bad_citations(&self, text: &str) -> Vec<String>;
    /// The web, when the agent may use it.
    fn web(&self) -> Option<&internet::Web<'_>> {
        None
    }
}

impl Toolbox for Tools<'_> {
    fn specs(&self) -> Vec<ToolSpec> {
        Tools::specs(self)
    }

    fn run(&self, tool: &str, args: &Value) -> anyhow::Result<tools::Outcome> {
        Tools::run(self, tool, args)
    }

    fn subject(&self) -> &'static str {
        "one recording (the transcript of a conversation)"
    }

    fn citation_form(&self) -> &'static str {
        "[m:ss]"
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
}

/// The system prompt: how to work, what is known up front and the tools.
pub fn system_prompt(toolbox: &dyn Toolbox, specs: &[ToolSpec]) -> String {
    let mut p = format!(
        "You answer questions about {} using tools.\n\
         Each turn write one JSON object: its action is a tool to call, or \"answer\" once you can answer.\n\
         Rules:\n\
         - Always look into the recordings first. Rely on them, not on your own knowledge; if they do not contain the answer, say so.\n\
         - Cite the moments you rely on as {}, with times taken from tool results.\n\
         - Answer in the language of the question, in Markdown (new lines for lists, never HTML), to the point.\n\
         - Never call a tool twice with the same arguments; if a search finds nothing, rephrase or read the outline.\n\
         - Use the calculator and time tools instead of computing in your head.\n\
         - If the question is ambiguous (who or which part is meant), answer with one short clarifying question instead of guessing.\n\n",
        toolbox.subject(),
        toolbox.citation_form(),
    );
    if toolbox.web().is_some() {
        p.push_str(
            "- When the user asks to check the internet, or about something the recordings mention but do not explain (a law, a form, a term), \
             call web_search, then web_read for details. The search tool only searches the recordings: never present them as web results. \
             Keep apart what the recordings say and what the web says; cite pages as [title](address), only addresses from tool results.\n\n",
        );
    }
    p.push_str(&toolbox.card());
    p.push_str("\nTools:\n");
    for t in specs {
        p.push_str(&format!("- {}: {} Example args: {}\n", t.name, t.description, t.example));
    }
    p
}

/// Times cited as `[m:ss]`, `[mm:ss]` or `[h:mm:ss]` (also the start of `[m:ss–m:ss]`).
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

/// Cited times that are not near anything said in the recording.
pub fn bad_citations(text: &str, recording: &Recording) -> Vec<String> {
    citations(text)
        .into_iter()
        .filter(|(_, t)| *t > recording.duration + 1.0 || !recording.replies.iter().any(|r| *t >= r.start - NEAR && *t <= r.end + NEAR))
        .map(|(s, _)| s)
        .collect()
}

/// `text` without the given citations.
fn without(text: &str, bad: &[String]) -> String {
    let mut t = text.to_string();
    for b in bad {
        t = t.replace(&format!(" [{b}]"), "").replace(&format!("[{b}]"), "");
    }
    t
}

/// The answer as Markdown: HTML line breaks some models write become new lines.
pub fn tidy(text: &str) -> String {
    text.replace("<br />", "\n").replace("<br/>", "\n").replace("<br>", "\n").trim().to_string()
}

/// Tool + normalised arguments: the same call twice has the same fingerprint.
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

/// Shortens the oldest tool results while the prompt is over its share of `context` tokens
/// (≈ 3 characters a token); the last two results stay whole.
pub fn compact(messages: &mut [Message], context: u32) {
    let limit = (context as f32 * PROMPT_SHARE) as usize * 3;
    let results: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| m.role == Role::User && m.content.starts_with(RESULT)).map(|(i, _)| i).collect();
    let keep = results.len().saturating_sub(2);
    for &i in &results[..keep] {
        let total: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        if total <= limit {
            break;
        }
        let m = &mut messages[i];
        if m.content.chars().count() > STUB_CHARS {
            m.content = format!("{}\n… (shortened to save room; call the tool again to see it all)", m.content.chars().take(STUB_CHARS).collect::<String>());
        }
    }
}

/// A step as the model wrote it (keys in the grammar's order).
fn step_json(step: &Step) -> String {
    let q = |v: &str| Value::String(v.to_string()).to_string();
    match step {
        Step::Tool { thought, title, tool, args } => format!(r#"{{"thought":{},"action":{},"title":{},"args":{args}}}"#, q(thought), q(tool), q(title)),
        Step::Answer { thought, text } => format!(r#"{{"thought":{},"action":"{}","answer":{}}}"#, q(thought), brain::ANSWER, q(text)),
    }
}

/// Longest step of the model (tokens): a long answer fits.
const STEP_TOKENS: u32 = 2048;

/// Answers `question` about the transcript `file_id` of `ai_dir` with the agent's model of
/// `settings` (local through the runtime, or a provider's), searching with the runtime's models.
#[allow(clippy::too_many_arguments)]
pub fn ask(
    runtime: &crate::runtime::Runtime,
    settings: &mediaa_core::agent::AgentSettings,
    ai_dir: &std::path::Path,
    file_id: &str,
    recording: &Recording,
    history: &[Turn],
    memory: &mut Memory,
    question: &str,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(&Event),
) -> anyhow::Result<Answer> {
    use mediaa_core::agent::ModelSource;
    let find = |q: &str, n: usize| crate::search::search(runtime, settings, ai_dir, file_id, q, n, true).map(|(h, _)| h);
    // Events come from the loop and from a running scan (its progress), never at once.
    let events = std::cell::RefCell::new(on_event);
    let transcript = mediaa_core::transcript::Transcript::open(ai_dir, file_id).ok();
    let scan_with = |extract: &dyn scan::Extract, instruction: &str| {
        scan::scan(extract, recording, transcript.as_ref(), instruction, cancel, &mut |done, total| (events.borrow_mut())(&Event::Scanning { done, total }))
    };
    let rename = |speaker: u32, name: &str| transcript.as_ref().ok_or_else(|| anyhow::anyhow!("no transcript"))?.rename_speaker(speaker, name);
    // The sub-agent: a provider's model when one is chosen; otherwise the agent's own model
    // (loading a second local model next to it would compete for the same memory).
    let sub = match &settings.models.subagent {
        Some(ModelSource::Provider { provider, model }) => Some(provider_brain(runtime, settings, provider, model)?),
        _ => None,
    };
    let mut events_out = |e: &Event| (events.borrow_mut())(e);
    let (web_search, web_read) = web_calls(runtime, settings, cancel);
    let run = |brain: &dyn Brain, extract: &dyn scan::Extract| {
        let helper: &dyn Brain = sub.as_ref().map_or(brain, |s| s as &dyn Brain);
        let delegate = |task: &str, text: &str| {
            let messages = [
                Message::new(
                    Role::System,
                    "You do one bounded task on a stretch of a recorded conversation for the main assistant. \
                     Answer compactly, in the language of the conversation, citing times [m:ss] from the lines.",
                ),
                Message::new(Role::User, format!("Task: {task}\n\nThe stretch:\n{text}")),
            ];
            helper.complete(&messages, cancel)
        };
        let scanner = |instruction: &str| scan_with(extract, instruction);
        let max_chars = (brain.context() as usize) * 3 / 4;
        let web = settings.web.enabled.then(|| internet::Web::new(&*web_search, &*web_read, max_chars / 3));
        let tools = Tools { scan: Some(&scanner), rename: Some(&rename), delegate: Some(&delegate), web: web.as_ref(), ..Tools::new(recording, &find, max_chars) };
        answer(brain, &tools, history, memory, question, cancel, &mut events_out)
    };
    with_brain(runtime, settings, ai_dir, cancel, run)
}

/// Web search and page reading with `settings`, one call at a time (the scheduler's web slot).
fn web_calls<'a>(
    runtime: &'a crate::runtime::Runtime,
    settings: &'a mediaa_core::agent::AgentSettings,
    cancel: &'a AtomicBool,
) -> (Box<internet::WebSearch<'a>>, Box<internet::WebRead<'a>>) {
    use mediaa_core::agent::WorkKind;
    let lease = move || runtime.scheduler().request(WorkKind::WebSearch, None, true).wait(cancel);
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

/// A provider's model as a brain.
fn provider_brain(runtime: &crate::runtime::Runtime, settings: &mediaa_core::agent::AgentSettings, id: &str, model: &str) -> anyhow::Result<ProviderBrain> {
    let p = settings.provider(id).ok_or_else(|| anyhow::anyhow!("the provider of a model was removed"))?.clone();
    Ok(ProviderBrain { provider: p, model: model.to_string(), scheduler: runtime.scheduler().clone(), max_tokens: STEP_TOKENS })
}

/// Runs `f` with the agent's model of `settings` (a provider's, or a local one loaded through the
/// runtime) and how it reads parts of a recording for the scan.
fn with_brain<R>(
    runtime: &crate::runtime::Runtime,
    settings: &mediaa_core::agent::AgentSettings,
    ai_dir: &std::path::Path,
    cancel: &AtomicBool,
    f: impl FnOnce(&dyn Brain, &dyn scan::Extract) -> anyhow::Result<R>,
) -> anyhow::Result<R> {
    use mediaa_core::agent::{ModelRole, ModelSource, WorkKind};
    match settings.models.agent.as_ref().ok_or_else(|| anyhow::anyhow!("no agent model chosen (Agent → Models)"))? {
        ModelSource::Provider { provider, model } => {
            let brain = provider_brain(runtime, settings, provider, model)?;
            f(&brain, &scan::ProviderExtract(&brain))
        }
        local => {
            if let ModelSource::HuggingFace { repo } = local {
                crate::hub::download(ai_dir, repo, settings.hugging_face(), |_, _| {})?;
            }
            let how = crate::runtime::Use { kind: WorkKind::Agent, device: settings.devices.get(ModelRole::Agent), n_ctx: settings.agent_context.clamp(2048, 262_144), interactive: true };
            runtime.with_model(local, how, cancel, |model: &crate::llm::LocalModel, _| f(&LocalBrain { model, max_tokens: STEP_TOKENS }, &scan::LocalExtract(model)))
        }
    }
}

/// Answers `question` about the recordings of a folder (`items`), like [`ask`].
#[allow(clippy::too_many_arguments)]
pub fn ask_folder(
    runtime: &crate::runtime::Runtime,
    settings: &mediaa_core::agent::AgentSettings,
    ai_dir: &std::path::Path,
    items: &[folder::Item],
    history: &[Turn],
    memory: &mut Memory,
    question: &str,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(&Event),
) -> anyhow::Result<Answer> {
    let sources: Vec<crate::search::Source> = items.iter().map(|it| crate::search::Source { id: it.file_id.clone(), path: it.path.clone() }).collect();
    let across = |q: &str, n: usize| {
        let (hits, _) = crate::search::search_files(runtime, settings, ai_dir, &sources, q, n, true)?;
        Ok(hits.into_iter().filter_map(|h| items.iter().position(|it| it.path == h.path).map(|i| (i, h.hit))).collect())
    };
    let one = |i: usize, q: &str, n: usize| crate::search::search(runtime, settings, ai_dir, &items[i].file_id, q, n, true).map(|(h, _)| h);
    let (web_search, web_read) = web_calls(runtime, settings, cancel);
    with_brain(runtime, settings, ai_dir, cancel, |brain, _| {
        let max_chars = (brain.context() as usize) * 3 / 4;
        let web = settings.web.enabled.then(|| internet::Web::new(&*web_search, &*web_read, max_chars / 3));
        let tools = folder::FolderTools { items, search: &across, search_one: &one, max_chars, web: web.as_ref() };
        answer(brain, &tools, history, memory, question, cancel, on_event)
    })
}

/// Tools of the conversation itself (they act on the chat, not on the recording).
pub fn chat_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: "remember",
            description: "Keeps a fact for the rest of this chat: names, what the user prefers, conclusions to build on.",
            params: json!({"type": "object", "properties": {"fact": {"type": "string"}}, "required": ["fact"]}),
            example: json!({"fact": "<the fact>"}),
        },
        ToolSpec {
            name: "plan",
            description: "For a request that needs several steps: first a short checklist, then the same list again with items done as you go. Simple questions skip it.",
            params: json!({"type": "object", "properties": {"items": {"type": "array", "maxItems": 8, "items": {"type": "object", "properties": {"text": {"type": "string"}, "done": {"type": "boolean"}}, "required": ["text", "done"]}}}, "required": ["items"]}),
            example: json!({"items": [{"text": "<step>", "done": false}]}),
        },
        ToolSpec {
            name: "say",
            description: "Sends ONE section of a long answer to the user now (a few paragraphs). For long reports: section by section, each built from what you read, then answer briefly.",
            // Bounded: the grammar closes a section before the step runs out of tokens.
            params: json!({"type": "object", "properties": {"text": {"type": "string", "maxLength": SECTION_CHARS}}, "required": ["text"]}),
            example: json!({"text": "<a finished section, Markdown, with [m:ss] citations>"}),
        },
    ]
}

/// Runs a tool of the conversation (`None`: not one of them).
fn chat_tool(tool: &str, args: &Value, toolbox: &dyn Toolbox, memory: &mut Memory, on_event: &mut dyn FnMut(&Event)) -> Option<anyhow::Result<tools::Outcome>> {
    let plain = |text: String| Ok(tools::Outcome { text, seen: None });
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or_default().trim().to_string();
    Some(match tool {
        "remember" => match memory.remember(&arg("fact")) {
            true => plain("Remembered for this chat.".into()),
            false => plain("Already remembered (or empty).".into()),
        },
        "plan" => {
            let items = args["items"].as_array().cloned().unwrap_or_default();
            let lines: Vec<String> = items
                .iter()
                .map(|i| format!("[{}] {}", if i["done"].as_bool() == Some(true) { "x" } else { " " }, i["text"].as_str().unwrap_or_default()))
                .collect();
            if lines.is_empty() {
                return Some(Err(anyhow::anyhow!("a plan needs items")));
            }
            plain(format!("Plan:\n{}", lines.join("\n")))
        }
        "say" => {
            let text = arg("text");
            if text.is_empty() {
                return Some(Err(anyhow::anyhow!("say needs a text")));
            }
            let text = tidy(&without(&text, &toolbox.bad_citations(&text)));
            let text = toolbox.web().map_or(text.clone(), |w| w.known_links(&text));
            // A section rests on the recording (or pages read): it cites what it is based on.
            if citations(&text).is_empty() && folder::folder_citations(&text).is_empty() && !text.contains("](http") {
                return Some(Err(anyhow::anyhow!(
                    "a section must cite the moments it is based on, as {}: read (or delegate) that part of the recording first",
                    toolbox.citation_form()
                )));
            }
            on_event(&Event::Say(text));
            plain("Sent to the user. Continue with the next section; once all are sent, answer in ONE short line (the sections are already with the user).".into())
        }
        _ => return toolbox.web().and_then(|w| w.run(tool, args)),
    })
}

/// Answers `question` about the recording of `tools`, after the chat's `history` (folded into
/// `memory`'s summary when it does not fit) with the chat's remembered facts.
pub fn answer(
    brain: &dyn Brain,
    tools: &dyn Toolbox,
    history: &[Turn],
    memory: &mut Memory,
    question: &str,
    cancel: &AtomicBool,
    on_event: &mut dyn FnMut(&Event),
) -> anyhow::Result<Answer> {
    memory::fold(brain, history, memory, cancel)?;
    let mut specs = tools.specs();
    specs.extend(chat_specs());
    if tools.web().is_some() {
        specs.extend(internet::Web::specs());
    }
    let mut messages = vec![Message::new(Role::System, system_prompt(tools, &specs) + &memory.prompt())];
    for t in &history[memory.summarized.min(history.len())..] {
        messages.push(Message::new(Role::User, t.question.clone()));
        messages.push(Message::new(Role::Assistant, t.answer.clone()));
    }
    messages.push(Message::new(Role::User, question.to_string()));

    let mut done: HashMap<String, usize> = HashMap::new();
    let mut known: HashSet<String> = HashSet::new();
    let mut steps: Vec<StepRecord> = Vec::new();
    let (mut stall, mut told_to_answer, mut checked_citations, mut cut_steps) = (0, false, false, 0);
    for n in 0..MAX_STEPS + 4 {
        let must_answer = steps.len() >= MAX_STEPS || stall >= STALL_STEPS;
        if must_answer && !told_to_answer {
            told_to_answer = true;
            messages.push(Message::new(Role::User, "Answer now with what you have found; say what is missing or uncertain."));
        }
        compact(&mut messages, brain.context());
        // Small models jump to conclusions: the first step always looks into the recording.
        let mut drafted = false;
        let step = brain.step(&messages, if must_answer { &[] } else { &specs }, !steps.is_empty(), cancel, &mut |text| {
            drafted = true;
            on_event(&Event::Draft(text.to_string()));
        });
        let step = match step {
            Ok(step) => step,
            // A step cut off at the token limit: the model is told (twice at most) and goes on.
            Err(e) if cut_steps < 2 && e.to_string().contains("not JSON") => {
                cut_steps += 1;
                stall += 1;
                if drafted {
                    on_event(&Event::DraftDiscarded);
                }
                messages.push(Message::new(Role::User, "Your last step was too long and got cut off. Keep each step short: send long text section by section."));
                continue;
            }
            Err(e) => return Err(e),
        };
        if drafted && !matches!(step, Step::Answer { .. }) {
            on_event(&Event::DraftDiscarded);
        }
        messages.push(Message::new(Role::Assistant, step_json(&step)));
        match step {
            Step::Tool { title, tool, args, .. } => {
                let fp = fingerprint(&tool, &args);
                let no = steps.len() + 1;
                let (status, text) = match done.get(&fp) {
                    Some(prev) => (StepStatus::Repeated, format!("Already done at step {prev} with the same arguments; its result is above. Do something else or answer.")),
                    None => match chat_tool(&tool, &args, tools, memory, on_event).unwrap_or_else(|| tools.run(&tool, &args)) {
                        Ok(out) => {
                            let new = out.seen.as_ref().is_none_or(|s| tools::adds(&mut known, s));
                            if new {
                                (StepStatus::Found, out.text)
                            } else if out.seen.as_ref().is_some_and(Vec::is_empty) {
                                (StepStatus::Nothing, out.text)
                            } else {
                                (StepStatus::Nothing, "Nothing new: everything this returned was already shown above.".to_string())
                            }
                        }
                        Err(e) => (StepStatus::Failed, format!("Error: {e:#}")),
                    },
                };
                done.entry(fp).or_insert(no);
                // A repeat means the model is going round in circles: it counts double.
                stall = match status {
                    StepStatus::Found => 0,
                    StepStatus::Repeated => stall + 2,
                    _ => stall + 1,
                };
                messages.push(Message::new(Role::User, format!("{RESULT}{no} ({tool}):\n{text}")));
                let record = StepRecord { title: if title.is_empty() { tool.clone() } else { title }, tool, args, result: text, status };
                on_event(&Event::Step(record.clone()));
                steps.push(record);
            }
            Step::Answer { text, .. } => {
                let bad = tools.bad_citations(&text);
                if !bad.is_empty() && !checked_citations && n + 1 < MAX_STEPS + 4 {
                    checked_citations = true;
                    on_event(&Event::DraftDiscarded);
                    let list = bad.iter().map(|b| format!("[{b}]")).collect::<Vec<_>>().join(", ");
                    messages.push(Message::new(
                        Role::User,
                        format!("These citations point at nothing said: {list}. Fix the answer: cite only times from the tool results, as {}.", tools.citation_form()),
                    ));
                    continue;
                }
                let mut text = tidy(&without(&text, &bad));
                if let Some(web) = tools.web() {
                    text = web.known_links(&text);
                }
                // Sections were sent: the answer only closes them (models tend to repeat them all).
                if steps.iter().any(|s| s.tool == "say" && s.status == StepStatus::Found) && text.chars().count() > CLOSING_CHARS {
                    text = text.split("\n\n").next().unwrap_or_default().trim().to_string();
                }
                on_event(&Event::Answer(text.clone()));
                return Ok(Answer { text, steps, forced: told_to_answer });
            }
        }
    }
    anyhow::bail!("the model did not answer in {} steps", MAX_STEPS + 4)
}

#[cfg(test)]
mod tests {
    use super::tools::tests::recording;
    use super::*;
    use mediaa_core::retrieval::{Chunk, Hit};
    use std::cell::RefCell;

    /// A model that plays back scripted steps and records the prompts it saw.
    struct Scripted {
        steps: RefCell<Vec<Step>>,
        seen: RefCell<Vec<(Vec<Message>, usize)>>,
        context: u32,
    }

    impl Scripted {
        fn new(steps: Vec<Step>) -> Scripted {
            Scripted { steps: RefCell::new(steps.into_iter().rev().collect()), seen: RefCell::default(), context: 8192 }
        }
    }

    impl Brain for Scripted {
        fn step(&self, messages: &[Message], tools: &[ToolSpec], may_answer: bool, _: &AtomicBool, on_answer: &mut dyn FnMut(&str)) -> anyhow::Result<Step> {
            self.seen.borrow_mut().push((messages.to_vec(), tools.len()));
            if !may_answer && !tools.is_empty() {
                assert!(matches!(self.steps.borrow().last(), Some(Step::Tool { .. })), "an answer before looking");
            }
            if tools.is_empty() {
                // Forced to answer: skip scripted tool calls.
                while matches!(self.steps.borrow().last(), Some(Step::Tool { .. })) {
                    self.steps.borrow_mut().pop();
                }
            }
            let step = self.steps.borrow_mut().pop().ok_or_else(|| anyhow::anyhow!("script over"))?;
            if let Step::Answer { text, .. } = &step {
                // Streamed in two pieces.
                let half = text.char_indices().nth(text.chars().count() / 2).map_or(0, |(i, _)| i);
                on_answer(&text[..half]);
                on_answer(&text[half..]);
            }
            Ok(step)
        }

        fn complete(&self, messages: &[Message], _: &AtomicBool) -> anyhow::Result<String> {
            self.seen.borrow_mut().push((messages.to_vec(), 0));
            Ok(format!("сводка #{}", self.seen.borrow().len()))
        }

        fn context(&self) -> u32 {
            self.context
        }
    }

    fn call(tool: &str, args: Value) -> Step {
        Step::Tool { thought: String::new(), title: format!("{tool} step"), tool: tool.into(), args }
    }

    fn say(text: &str) -> Step {
        Step::Answer { thought: String::new(), text: text.into() }
    }

    fn search(q: &str, _: usize) -> anyhow::Result<Vec<Hit>> {
        Ok(if q.contains("отчёт") { vec![Hit { id: 1, chunk: Chunk { start: 10.0, end: 15.0, speakers: vec![0], text: "Отчёт отправляем в пятницу.".into() }, score: 1.0 }] } else { vec![] })
    }

    fn run(brain: &Scripted, question: &str) -> (Answer, Vec<Event>) {
        let rec = recording();
        let tools = Tools::new(&rec, &search, 4000);
        let mut events = Vec::new();
        let history = [Turn { question: "Привет".into(), answer: "Здравствуйте".into() }];
        let a = answer(brain, &tools, &history, &mut Memory::default(), question, &AtomicBool::new(false), &mut |e| events.push(e.clone())).unwrap();
        (a, events)
    }

    #[test]
    fn search_then_answer_with_a_citation() {
        let brain = Scripted::new(vec![call("search", json!({"query": "отчёт"})), say("В пятницу [0:10].")]);
        let (a, events) = run(&brain, "Когда отчёт?");
        assert_eq!(a.text, "В пятницу [0:10].");
        assert!(!a.forced);
        assert_eq!(a.steps.len(), 1);
        assert_eq!(a.steps[0].status, StepStatus::Found);
        // The step, the answer streamed in pieces, the final answer.
        assert!(matches!(events[0], Event::Step(_)));
        let draft: String = events.iter().filter_map(|e| if let Event::Draft(t) = e { Some(t.as_str()) } else { None }).collect();
        assert_eq!(draft, "В пятницу [0:10].");
        assert_eq!(events.last(), Some(&Event::Answer("В пятницу [0:10].".into())));
        assert!(!events.contains(&Event::DraftDiscarded));
        // The prompt: system with the card and tools, the history, the question; then the result.
        let (first, tools) = &brain.seen.borrow()[0];
        assert_eq!(*tools, tools::specs().len() + chat_specs().len());
        assert!(first[0].content.contains("«Планёрка»") && first[0].content.contains("- search: "));
        assert_eq!((first[1].content.as_str(), first[2].content.as_str(), first[3].content.as_str()), ("Привет", "Здравствуйте", "Когда отчёт?"));
        let (second, _) = &brain.seen.borrow()[1];
        assert!(second.last().unwrap().content.starts_with("Result of step 1 (search):\n[0:10–0:15] (Дана) Отчёт"));
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
        let a = answer(&brain, &tools, &[], &mut Memory::default(), "Что с трудовой?", &AtomicBool::new(false), &mut |e| events.push(e.clone())).unwrap();
        assert_eq!(a.text, "В записи: пятница [0:10]; см. [ТК РФ](https://example.org/tk) и блог.");
        assert!(a.steps.iter().all(|s| s.status == StepStatus::Found), "{:?}", a.steps);
        assert!(events.contains(&Event::Say("По закону — [ТК РФ](https://example.org/tk).".into())), "a section may rest on a page read");
        let (first, n) = &brain.seen.borrow()[0];
        assert_eq!(*n, tools::specs().len() + chat_specs().len() + 2);
        assert!(first[0].content.contains("- web_search: ") && first[0].content.contains("[title](address)"));

        // Without the web: no web tools, no rule.
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
        let a = answer(&brain, &tools, &[], &mut memory, "Отчёт частями", &AtomicBool::new(false), &mut |e| events.push(e.clone())).unwrap();
        assert_eq!(memory.facts, ["Пользователь просит кратко"]);
        assert_eq!(a.steps[1].result, "Plan:\n[x] найти сроки\n[ ] написать отчёт");
        assert_eq!(a.steps[2].status, StepStatus::Failed, "a section without citations is refused");
        assert!(a.steps[2].result.contains("must cite"));
        assert!(events.contains(&Event::Say("Часть 1: отчёт в пятницу [0:10], и ещё.".into())), "sent, a wrong time dropped: {events:?}");
        assert_eq!(a.text, "Всё отправлено.", "after sections the answer only closes them");
        // The next answer of the chat sees the fact.
        let brain = Scripted::new(vec![call("outline", json!({})), say("Кратко.")]);
        answer(&brain, &tools, &[], &mut memory, "Ещё", &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert!(brain.seen.borrow()[0].0[0].content.contains("Remembered in this chat:\n- Пользователь просит кратко"));
    }

    #[test]
    fn a_cut_off_step_is_told_and_the_answer_goes_on() {
        /// Cuts its first step off (as the token limit does), then plays the script.
        struct CutOnce(Scripted, std::cell::Cell<bool>);
        impl Brain for CutOnce {
            fn step(&self, m: &[Message], t: &[ToolSpec], may: bool, c: &AtomicBool, on: &mut dyn FnMut(&str)) -> anyhow::Result<Step> {
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
        let a = answer(&brain, &tools, &[], &mut Memory::default(), "Отчёт", &AtomicBool::new(false), &mut |_| {}).unwrap();
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
        answer(&brain, &tools, &history, &mut memory, "Новый вопрос", &AtomicBool::new(false), &mut |_| {}).unwrap();
        assert_eq!(memory.summarized, 3, "the last three stay");
        assert!(memory.summary.starts_with("сводка"));
        let seen = brain.seen.borrow();
        // The summary request, then the step: the summary in the system prompt, only the last
        // three exchanges word for word.
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
        // The read only shows the reply already found; nothing found; the repeat is caught and
        // counts double: the agent must answer.
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
        assert_eq!(events.iter().filter(|e| **e == Event::DraftDiscarded).count(), 1, "the first draft went back");
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

    /// The default agent model answers about the transcribed sample of `MEDIAA_TEST_AI` (run the
    /// `transcribes_through_the_queue` test first): it searches, finds the oak, cites the time.
    #[test]
    #[ignore]
    fn a_real_model_answers_from_the_recording() {
        use crate::runtime::Runtime;
        use mediaa_core::agent::{AgentSettings, Budget};
        use mediaa_core::transcript::{self, Transcript};
        let ai = std::path::PathBuf::from(std::env::var_os("MEDIAA_TEST_AI").expect("MEDIAA_TEST_AI"));
        let id = transcript::file_id(&ai.join("example.wav")).unwrap();
        let t = Transcript::open(&ai, &id).unwrap();
        let replies = t.dialog().unwrap();
        assert!(!replies.is_empty(), "transcribe the sample first");
        let rec = Recording { title: "Пушкин".into(), duration: 11.3, diarized: true, names: t.speaker_names().unwrap(), replies, chapters: t.chapters().unwrap(), position: None };
        let settings = AgentSettings::default();
        let runtime = Runtime::new(&ai, Budget::default());
        let question = "Какое дерево упоминается в записи и в какой момент?";
        let a = ask(&runtime, &settings, &ai, &id, &rec, &[], &mut Memory::default(), question, &AtomicBool::new(false), &mut |e| eprintln!("{e:?}")).unwrap();
        eprintln!("{}", a.text);
        assert!(a.text.to_lowercase().contains("дуб"), "{}", a.text);
        assert!(!citations(&a.text).is_empty(), "cites the moment: {}", a.text);
        assert!(bad_citations(&a.text, &rec).is_empty());
    }

    /// A bench over a real recording: `MEDIAA_BENCH_LIB` (a library with the file transcribed and
    /// indexed), `MEDIAA_BENCH_FILE`, questions in `MEDIAA_BENCH_QUESTIONS` (separated by `|`),
    /// a Tavily key in `MEDIAA_BENCH_TAVILY` and a proxy in `MEDIAA_BENCH_PROXY` for the web.
    /// Prints every answer with its steps and time; fails on wrong citations.
    #[test]
    #[ignore]
    fn bench_on_a_recording() {
        use crate::runtime::Runtime;
        use mediaa_core::agent::{AgentSettings, Budget};
        use mediaa_core::transcript::{self, Transcript};
        let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("{k}"));
        let ai = mediaa_core::catalogs::ai_dir(std::path::Path::new(&var("MEDIAA_BENCH_LIB")));
        let file = std::path::PathBuf::from(var("MEDIAA_BENCH_FILE"));
        let id = transcript::file_id(&file).unwrap();
        let mut settings = AgentSettings::default();
        // `MEDIAA_BENCH_TAVILY`: web search through Tavily with this key (else DuckDuckGo).
        if let Ok(key) = std::env::var("MEDIAA_BENCH_TAVILY") {
            settings.web.engine = mediaa_core::agent::WebEngine::Tavily;
            settings.web.api_key = key;
        }
        if let Ok(proxy) = std::env::var("MEDIAA_BENCH_PROXY") {
            settings.web.proxy = proxy;
        }
        let runtime = Runtime::new(&ai, Budget::default());
        if std::env::var_os("MEDIAA_BENCH_OUTLINE").is_some() {
            let started = std::time::Instant::now();
            let q = mediaa_core::queue::Queue::open(&ai).unwrap();
            crate::outline::enqueue(&ai, &file).unwrap();
            let s = settings.clone();
            let handlers: Vec<Box<dyn crate::worker::TaskHandler>> = vec![Box::new(crate::outline::Outline::new(&ai, runtime.clone(), std::sync::Arc::new(move || s.clone())))];
            assert!(crate::worker::run_next(&q, &handlers, &AtomicBool::new(false)).unwrap());
            eprintln!("outline rebuilt in {:.0} s", started.elapsed().as_secs_f32());
        }
        let t = Transcript::open(&ai, &id).unwrap();
        for c in t.chapters().unwrap() {
            eprintln!("chapter [{}] {} — {}", crate::tools::format_time(c.start), c.digest.title, c.digest.summary);
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
        // One chat: history and memory go on from question to question. `MEDIAA_BENCH_FOLDER`:
        // about every recording of the library's catalogs.
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
        for q in var("MEDIAA_BENCH_QUESTIONS").split('|') {
            let started = std::time::Instant::now();
            let mut print = |e: &Event| match e {
                Event::Step(s) => eprintln!("    · {:?} {} {} → {}", s.status, s.tool, s.args, s.result.chars().take(if s.tool == "scan" { 3000 } else { 160 }).collect::<String>().replace('\n', " / ")),
                Event::Say(t) => eprintln!("    » {t}"),
                _ => {}
            };
            let cancel = AtomicBool::new(false);
            let a = if items.is_empty() {
                ask(&runtime, &settings, &ai, &id, &rec, &history, &mut memory, q.trim(), &cancel, &mut print).unwrap()
            } else {
                ask_folder(&runtime, &settings, &ai, &items, &history, &mut memory, q.trim(), &cancel, &mut print).unwrap()
            };
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
    fn citations_and_fingerprints() {
        assert_eq!(citations("в [1:05] и [1:02:30–1:03:00], не [ссылка] и не [5]").iter().map(|(_, t)| *t).collect::<Vec<_>>(), [65.0, 3750.0]);
        assert_eq!(fingerprint("search", &json!({"query": " Бюджет  квартала"})), fingerprint("search", &json!({"query": "бюджет квартала"})));
        assert_ne!(fingerprint("search", &json!({"query": "a"})), fingerprint("read", &json!({"query": "a"})));
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
}
