use crate::cases::{Case, Suite, TurnSpec};
use crate::report::{
    save_summary, write_repeat, ModelSteps, RepeatArtifacts, StatsFile, Summary, TurnStats,
};
use crate::score::{
    judge_schema, parse_judge, repeat_from_turns, score_turn, JudgeVerdict, RepeatScores,
};
use crate::{locate, repo_root, visible, Judge, RunArgs};
use anyhow::Context;
use mediaa_agent::assistant::brain::split_thinking;
use mediaa_agent::assistant::title::task_title;
use mediaa_agent::assistant::{ask, Event, Memory, Recording, StepRecord, Turn};
use mediaa_agent::scope::Scope;
use mediaa_agent::slots::Slots;
use mediaa_core::agent::AgentSettings;
use mediaa_core::catalogs::ai_dir;
use mediaa_core::transcript::{self, Transcript};
use mediaa_engine::{ChatMessage, ChatRequest, ChatRole, Connections, Engine};
use mediaa_queue::TaskId;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

pub fn execute(args: &RunArgs) -> anyhow::Result<PathBuf> {
    let _ = env_logger::Builder::from_env(env_logger::Env::default()).try_init();
    let root = repo_root();
    let cases_path = locate(&root, &args.cases);
    let library = locate(&root, &args.library);
    let file = locate(&root, &args.file);
    anyhow::ensure!(args.repeat >= 1, "--repeat must be at least 1");
    let suite = crate::cases::load_cases(&cases_path)?;
    let cases = select(&suite, args.case.as_deref())?;
    let ai = ai_dir(&library);
    anyhow::ensure!(
        ai.is_dir(),
        "library has no .ai data: {}",
        library.display()
    );
    anyhow::ensure!(file.is_file(), "recording not found: {}", file.display());

    let mut settings = AgentSettings::default();
    if let Some(text) = std::env::var("MEDIAA_LIVE_CONNECTIONS")
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok())
    {
        if let Ok(connections) = serde_json::from_str(&text) {
            settings.connections = connections;
        }
    }
    let judge_model = judge_model(&settings, &args.judge)?;
    let run_dir = make_run_dir(&root.join("target").join("evals"), args.label.as_deref())?;
    let queue_dir = run_dir.join("queue");
    std::fs::create_dir_all(&queue_dir)?;
    eprintln!(
        "eval: {} cases, repeat {}, judge {}",
        cases.len(),
        args.repeat,
        args.judge.as_str()
    );
    eprintln!("out: {}", run_dir.display());

    let engine: Arc<dyn Engine> = Arc::new(Connections::new(settings.connections.clone()));
    let slots = Slots::new(settings.budget.clone());
    let (file_id, recording) = load_recording(&ai, &file)?;
    let ctx = Ctx {
        engine,
        slots,
        settings,
        ai,
        file_id,
        recording,
        queue_dir,
        judge_model,
    };

    let mut summary = Summary {
        label: args.label.clone().unwrap_or_else(|| "run".into()),
        cases_file: cases_path.display().to_string(),
        repeat: args.repeat,
        judge: args.judge.as_str().to_string(),
        rows: Vec::new(),
        model_calls: 0,
        prompt_tokens: 0,
        completion_tokens: 0,
        errors: Vec::new(),
    };
    for (index, case) in cases.iter().enumerate() {
        let mut repeats = Vec::new();
        for n in 1..=args.repeat {
            eprintln!(
                "\n[{}/{}] {} repeat {n}/{}",
                index + 1,
                cases.len(),
                case.id,
                args.repeat
            );
            let outcome = run_repeat(case, n, &ctx, &run_dir)?;
            summary.model_calls += outcome.calls.model_calls;
            summary.prompt_tokens += outcome.calls.prompt_tokens;
            summary.completion_tokens += outcome.calls.completion_tokens;
            summary.errors.extend(outcome.errors);
            repeats.push(outcome.scores);
        }
        summary
            .rows
            .push(crate::report::row_from_repeats(case, &repeats));
        save_summary(&run_dir, &summary)?;
    }
    let report = std::fs::read_to_string(run_dir.join("report.md"))?;
    eprintln!("\n{report}");
    Ok(run_dir)
}

struct Ctx {
    engine: Arc<dyn Engine>,
    slots: Slots,
    settings: AgentSettings,
    ai: PathBuf,
    file_id: String,
    recording: Recording,
    queue_dir: PathBuf,
    judge_model: Option<String>,
}

struct Outcome {
    scores: RepeatScores,
    calls: TurnStats,
    errors: Vec<String>,
}

fn run_repeat(case: &Case, repeat: usize, ctx: &Ctx, run_dir: &Path) -> anyhow::Result<Outcome> {
    let mut history = Vec::new();
    let mut memory = Memory::default();
    let mut questions = Vec::new();
    let mut events = Vec::new();
    let mut answers = Vec::new();
    let mut shown_answers = Vec::new();
    let mut scores = Vec::new();
    let mut stats = Vec::new();
    let mut model_steps = Vec::new();
    let mut errors = Vec::new();
    for (i, spec) in case.turns.iter().enumerate() {
        eprintln!("Q: {}", spec.question);
        questions.push(spec.question.clone());
        let asked = ask_turn(ctx, spec, &history, &mut memory, i + 1);
        let raw = asked.raw;
        let shown = asked.shown;
        eprintln!(
            "A ({}, {} steps{}): {}",
            clock(asked.seconds),
            asked.steps,
            if asked.forced { ", forced" } else { "" },
            preview(&shown)
        );
        let mut score = score_turn(
            spec,
            &shown,
            &ctx.recording,
            asked.steps,
            asked.forced,
            asked.seconds,
        );
        if let Some(model) = &ctx.judge_model {
            match judge_turn(&*ctx.engine, &ctx.settings, model, spec, &shown) {
                Ok(verdict) => {
                    eprintln!("  judge {}", verdict.score);
                    score.judge = Some(verdict);
                }
                Err(error) => {
                    eprintln!("  judge failed: {error}");
                    score.judge_error = Some(error.clone());
                    errors.push(format!(
                        "{} repeat {repeat} turn {} judge: {error}",
                        case.id,
                        i + 1
                    ));
                }
            }
        }
        let citations = match score.precision() {
            Some(precision) => format!("{:.0}%", precision * 100.0),
            None => "—".into(),
        };
        eprintln!(
            "  facts {}/{} · forbidden {} · citations {citations}",
            score.facts_hit,
            score.facts_total,
            score.forbidden.len(),
        );
        if let Some(error) = &asked.error {
            let error = shorten(error);
            eprintln!("  error: {error}");
            errors.push(format!(
                "{} repeat {repeat} turn {}: {error}",
                case.id,
                i + 1
            ));
        } else {
            history.push(Turn {
                question: spec.question.clone(),
                answer: raw.clone(),
            });
        }
        events.extend(asked.events);
        answers.push(raw);
        shown_answers.push(shown);
        stats.push(asked.stats);
        model_steps.push(asked.model_steps);
        scores.push(score);
    }
    let calls = sum_stats(&stats);
    let file = StatsFile {
        model_calls: calls.model_calls,
        prompt_tokens: calls.prompt_tokens,
        completion_tokens: calls.completion_tokens,
        model_ms: calls.model_ms,
        models: calls.models.clone(),
        turns: stats,
    };
    let dir = run_dir.join(&case.id).join(repeat.to_string());
    write_repeat(
        &dir,
        &RepeatArtifacts {
            questions: &questions,
            events: &events,
            answers: &answers,
            shown: &shown_answers,
            turns: &scores,
            stats: &file,
            model_steps: &model_steps,
        },
    )?;
    if !errors.is_empty() {
        std::fs::write(dir.join("error.txt"), errors.join("\n") + "\n")?;
    }
    Ok(Outcome {
        scores: repeat_from_turns(&scores),
        calls,
        errors,
    })
}

struct Asked {
    events: Vec<Value>,
    raw: String,
    shown: String,
    steps: usize,
    forced: bool,
    seconds: f64,
    stats: TurnStats,
    model_steps: ModelSteps,
    error: Option<String>,
}

fn ask_turn(
    ctx: &Ctx,
    spec: &TurnSpec,
    history: &[Turn],
    memory: &mut Memory,
    turn: usize,
) -> Asked {
    let started = Instant::now();
    let mut events = Vec::new();
    let scope = match begin_scope(&ctx.queue_dir, &ctx.engine, &ctx.file_id, &spec.question) {
        Ok(scope) => scope,
        Err(e) => {
            return Asked {
                events,
                raw: String::new(),
                shown: String::new(),
                steps: 0,
                forced: false,
                seconds: started.elapsed().as_secs_f64(),
                stats: TurnStats::default(),
                model_steps: ModelSteps::default(),
                error: Some(format!("{e:#}")),
            };
        }
    };
    let cancel = AtomicBool::new(false);
    let result = ask(
        &scope,
        &ctx.slots,
        &ctx.settings,
        &ctx.ai,
        &ctx.file_id,
        &ctx.recording,
        history,
        memory,
        &spec.question,
        &cancel,
        &mut |event| {
            if !matches!(event, Event::Metrics(_)) {
                events.push(event_json(turn, event));
            }
        },
    );
    let (stats, model_steps) = read_turn(&ctx.queue_dir, scope.task()).unwrap_or_else(|e| {
        eprintln!("queue stats: {e:#}");
        (TurnStats::default(), ModelSteps::default())
    });
    let seconds = started.elapsed().as_secs_f64();
    match result {
        Ok(answer) => {
            scope.finish(&json!({ "steps": answer.steps.len() }));
            let raw = shown_answer(&events, &answer.text);
            let shown = visible::shown(&raw, &ctx.recording);
            let steps = answer.steps.len();
            let forced = answer.forced;
            Asked {
                events,
                raw,
                shown,
                steps,
                forced,
                seconds,
                stats,
                model_steps,
                error: None,
            }
        }
        Err(e) => {
            scope.fail(&format!("{e:#}"));
            Asked {
                events,
                raw: String::new(),
                shown: String::new(),
                steps: 0,
                forced: false,
                seconds,
                stats,
                model_steps,
                error: Some(format!("{e:#}")),
            }
        }
    }
}

fn begin_scope(
    dir: &Path,
    engine: &Arc<dyn Engine>,
    file_id: &str,
    question: &str,
) -> anyhow::Result<Scope> {
    let mut last = None;
    for attempt in 0..5 {
        match Scope::begin(
            dir,
            engine.clone(),
            "chat",
            &task_title(question),
            &json!({ "question": question }),
            Some(file_id),
        ) {
            Ok(scope) => return Ok(scope),
            Err(e) => {
                last = Some(e);
                if attempt + 1 < 5 {
                    eprintln!("queue is busy, retrying");
                    std::thread::sleep(std::time::Duration::from_secs(2));
                }
            }
        }
    }
    Err(last.expect("the loop ran"))
}

fn read_turn(dir: &Path, id: TaskId) -> anyhow::Result<(TurnStats, ModelSteps)> {
    let store = mediaa_agent::run::open_store(dir)?;
    let mut tasks = store.subtree(id)?;
    tasks.sort_by_key(|task| task.id);
    let mut total = mediaa_queue::Stats::default();
    for task in &tasks {
        total.add(&task.stats);
    }
    let stats = TurnStats {
        model_calls: total.model_calls,
        prompt_tokens: total.prompt_tokens,
        completion_tokens: total.completion_tokens,
        model_ms: total.model_ms,
        models: total.models,
    };
    let model_steps = rounds(
        tasks
            .iter()
            .map(|t| (t.kind.as_str(), t.title.as_str(), t.created_ms)),
    );
    Ok((stats, model_steps))
}

fn rounds<'a>(tasks: impl IntoIterator<Item = (&'a str, &'a str, i64)>) -> ModelSteps {
    let mut tasks: Vec<(&str, &str, i64)> = tasks.into_iter().collect();
    tasks.sort_by_key(|(_, _, created_ms)| *created_ms);
    let mut rounds = ModelSteps::default();
    for (kind, title, _) in tasks {
        match kind {
            "model-step" if is_number(title) => {
                rounds.steps += 1;
                rounds.calls_per_step.push(0);
            }
            "tool" => {
                if let Some(calls) = rounds.calls_per_step.last_mut() {
                    *calls += 1;
                }
            }
            _ => {}
        }
    }
    rounds
}

fn is_number(title: &str) -> bool {
    !title.is_empty() && title.bytes().all(|b| b.is_ascii_digit())
}

fn sum_stats(turns: &[TurnStats]) -> TurnStats {
    let mut total = TurnStats::default();
    for turn in turns {
        total.model_calls += turn.model_calls;
        total.prompt_tokens += turn.prompt_tokens;
        total.completion_tokens += turn.completion_tokens;
        total.model_ms += turn.model_ms;
        for model in &turn.models {
            if !total.models.iter().any(|have| have == model) {
                total.models.push(model.clone());
            }
        }
    }
    total
}

fn shown_answer(events: &[Value], final_text: &str) -> String {
    let mut parts = Vec::new();
    for event in events {
        if event.get("event").and_then(Value::as_str) == Some("say") {
            if let Some(text) = event.get("text").and_then(Value::as_str) {
                if !text.is_empty() {
                    parts.push(text.to_string());
                }
            }
        }
    }
    if !final_text.is_empty() {
        parts.push(final_text.to_string());
    }
    parts.join("\n\n")
}

fn event_json(turn: usize, event: &Event) -> Value {
    let mut value = match event {
        Event::Waiting { reason } => json!({"event": "waiting", "reason": format!("{reason:?}")}),
        Event::StepStarted { title, tool } => {
            json!({"event": "step_started", "title": title, "tool": tool})
        }
        Event::Step(step) => step_json(step),
        Event::Thinking(text) => json!({"event": "thinking", "text": text}),
        Event::AnswerText(text) => json!({"event": "answer_text", "text": text}),
        Event::AnswerDiscarded => json!({"event": "answer_discarded"}),
        Event::Say(text) => json!({"event": "say", "text": text}),
        Event::SteerTaken { text } => json!({"event": "steer_taken", "text": text}),
        Event::Scanning { done, total } => {
            json!({"event": "scanning", "done": done, "total": total})
        }
        Event::Metrics(metrics) => json!({
            "event": "metrics",
            "context_tokens": metrics.context_tokens,
            "context": metrics.context,
            "speed": metrics.speed
        }),
        Event::Done => json!({"event": "done"}),
        Event::Stopped => json!({"event": "stopped"}),
        Event::Failed(error) => json!({"event": "failed", "error": error}),
    };
    value["turn"] = json!(turn);
    value
}

fn step_json(step: &StepRecord) -> Value {
    json!({
        "event": "step",
        "title": step.title,
        "tool": step.tool,
        "args": step.args,
        "result": step.result,
        "status": step.status.as_str(),
    })
}

fn load_recording(ai: &Path, file: &Path) -> anyhow::Result<(String, Recording)> {
    let id = transcript::file_id(file).with_context(|| format!("reading {}", file.display()))?;
    let db = transcript::db_path(ai, &id);
    anyhow::ensure!(
        db.is_file(),
        "no transcript at {} — analyse this recording in the library first",
        db.display()
    );
    let stored = Transcript::open(ai, &id)?;
    let replies = stored.dialog().context("dialog")?;
    anyhow::ensure!(!replies.is_empty(), "the transcript has no replies");
    let duration = replies.last().map(|r| r.end).unwrap_or(0.0);
    let recording = Recording {
        title: file
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        duration,
        diarized: replies.iter().any(|r| r.speaker.is_some()),
        names: stored.speaker_names().context("speaker names")?,
        replies,
        chapters: stored.chapters().context("chapters")?,
        position: None,
    };
    Ok((id, recording))
}

fn judge_model(settings: &AgentSettings, judge: &Judge) -> anyhow::Result<Option<String>> {
    match judge {
        Judge::None => Ok(None),
        Judge::Agent => {
            let source = settings
                .models
                .agent
                .as_ref()
                .context("no agent model is set")?;
            Ok(Some(source.model.clone()))
        }
        Judge::Spec(spec) => Ok(Some(spec.clone())),
    }
}

#[derive(Serialize)]
struct JudgePrompt<'a> {
    question: &'a str,
    reference: &'a str,
    facts: Vec<&'a str>,
    answer: &'a str,
}

fn judge_turn(
    engine: &dyn Engine,
    settings: &AgentSettings,
    model: &str,
    spec: &TurnSpec,
    answer: &str,
) -> Result<JudgeVerdict, String> {
    let facts: Vec<&str> = spec.facts.iter().map(|f| f.text.as_str()).collect();
    let prompt = JudgePrompt {
        question: &spec.question,
        reference: &spec.reference,
        facts,
        answer,
    };
    let source = settings
        .models
        .agent
        .clone()
        .unwrap_or_else(|| mediaa_core::agent::ModelSource::new("", model));
    let request = ChatRequest {
        source,
        messages: vec![
            ChatMessage::new(
                ChatRole::System,
                mediaa_agent::prompts::render("eval/judge_system.j2", ()),
            ),
            ChatMessage::new(
                ChatRole::User,
                mediaa_agent::prompts::render("eval/judge.j2", &prompt),
            ),
        ],
        max_tokens: 512,
        temperature: 0.0,
        json_schema: Some(judge_schema()),
        ..ChatRequest::default()
    };
    let cancel = AtomicBool::new(false);
    let raw = engine
        .chat(&request, &cancel, &mut |_| true)
        .map_err(|e| format!("{e:#}"))?;
    let text = split_thinking(&raw.content).1;
    parse_judge(&text).map_err(|e| shorten(&format!("{e:#}")))
}

fn select(suite: &Suite, id: Option<&str>) -> anyhow::Result<Vec<Case>> {
    match id {
        None => Ok(suite.cases.clone()),
        Some(id) => {
            let found: Vec<Case> = suite.cases.iter().filter(|c| c.id == id).cloned().collect();
            anyhow::ensure!(!found.is_empty(), "no case {id}");
            Ok(found)
        }
    }
}

fn make_run_dir(root: &Path, label: Option<&str>) -> anyhow::Result<PathBuf> {
    std::fs::create_dir_all(root)?;
    let stamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
    let name = match label.map(str::trim).filter(|s| !s.is_empty()) {
        Some(label) => format!("{stamp}_{}", safe_label(label)),
        None => stamp,
    };
    let mut dir = root.join(&name);
    let mut n = 2;
    while dir.exists() {
        dir = root.join(format!("{name}-{n}"));
        n += 1;
    }
    std::fs::create_dir(&dir)?;
    Ok(dir)
}

fn safe_label(text: &str) -> String {
    text.chars()
        .map(|c| {
            if matches!(
                c,
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' | ' '
            ) {
                '_'
            } else {
                c
            }
        })
        .collect()
}

fn preview(text: &str) -> String {
    const MAX: usize = 1500;
    if text.chars().count() <= MAX {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(MAX).collect::<String>())
    }
}

fn shorten(text: &str) -> String {
    const MAX: usize = 500;
    if text.chars().count() <= MAX {
        text.to_string()
    } else {
        format!("{}…", text.chars().take(MAX).collect::<String>())
    }
}

fn clock(seconds: f64) -> String {
    let whole = seconds.round().max(0.0) as u64;
    if whole >= 60 {
        format!("{}m{:02}s", whole / 60, whole % 60)
    } else {
        format!("{whole}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_agent::assistant::{Event, StepStatus};

    #[test]
    fn say_sections_join_the_closing_text_and_a_step_keeps_its_fields() {
        let events = vec![
            event_json(1, &Event::Say("Раздел [1:40].".into())),
            event_json(
                1,
                &Event::Step(StepRecord {
                    title: "Ищу".into(),
                    tool: "search".into(),
                    args: json!({"query": "имя"}),
                    result: "Назаров".into(),
                    status: StepStatus::Found,
                }),
            ),
        ];
        assert_eq!(events[1]["turn"], 1);
        assert_eq!(events[1]["tool"], "search");
        assert_eq!(events[1]["args"]["query"], "имя");
        assert_eq!(events[1]["result"], "Назаров");
        assert_eq!(events[1]["status"], "found");
        assert_eq!(shown_answer(&events, "Итог."), "Раздел [1:40].\n\nИтог.");
        assert_eq!(shown_answer(&[], ""), "");
    }

    #[test]
    fn tool_calls_are_counted_into_the_model_step_that_asked_for_them() {
        let tasks = [
            ("chat", "как зовут", 1),
            ("tool", "search", 5),
            ("model-step", "1", 10),
            ("tool", "search", 11),
            ("tool", "search", 12),
            ("model-step", "Проверка ответа", 13),
            ("model-step", "2", 20),
            ("tool", "read", 21),
            ("model-step", "3", 30),
        ];
        assert_eq!(
            rounds(tasks),
            ModelSteps {
                steps: 3,
                calls_per_step: vec![2, 1, 0],
            }
        );
        let empty: [(&str, &str, i64); 0] = [];
        assert_eq!(rounds(empty), ModelSteps::default());
    }
}
