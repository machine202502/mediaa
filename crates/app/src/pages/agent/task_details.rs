use super::analysis::clock;
use super::queue;
use super::task_label;
use crate::theme;
use chrono::NaiveDateTime;
use eframe::egui::{self, FontId, RichText};
use egui_phosphor::regular as icon;
use mediaa_platform::clipboard;
use mediaa_queue::{elapsed, Stats, Status as TaskStatus, Task, TaskId};
use rust_i18n::t;
use serde_json::Value;
use std::path::PathBuf;

const GAP: f32 = 8.0;
const LOG_BEFORE_SECS: i64 = 120;
const LOG_AFTER_SECS: i64 = 15;
const LOG_MAX_BYTES: usize = 12_000;
const LOG_FALLBACK_RECORDS: usize = 40;
const LOG_NEAR: usize = 4;
const LOG_RECORD_CHARS: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hint {
    ModelMissing,
    OutOfMemory,
    Network,
}

pub fn hint_for(error: &str) -> Option<Hint> {
    let e = error.to_lowercase();
    if model_missing(&e) {
        return Some(Hint::ModelMissing);
    }
    if out_of_memory(&e) {
        return Some(Hint::OutOfMemory);
    }
    if network_problem(&e) {
        return Some(Hint::Network);
    }
    None
}

fn model_missing(e: &str) -> bool {
    e.contains("does not load")
        || e.contains("not found")
        || e.contains("no such file")
        || e.contains("not downloaded")
        || e.contains("os error 2")
        || e.contains("не найден")
        || e.contains("не скачан")
}

fn out_of_memory(e: &str) -> bool {
    e.contains("out of memory")
        || e.contains("failed to allocate")
        || e.contains("cannot allocate")
        || e.contains("не хватает памяти")
        || e.contains("недостаточно памяти")
}

fn network_problem(e: &str) -> bool {
    [
        "proxy",
        "timed out",
        "timeout",
        "connection refused",
        "connection reset",
        "dns",
        "network",
        "unreachable",
        "could not resolve",
        "нет сети",
        "прокси",
    ]
    .iter()
    .any(|needle| e.contains(needle))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Facts {
    pub label: String,
    pub status: String,
    pub error: String,
    pub kind: String,
    pub recording: String,
    pub scope: String,
    pub path: String,
    pub options: String,
    pub models: String,
    pub started: String,
    pub finished: String,
    pub duration: String,
    pub progress: String,
    pub stats: String,
    pub tool: String,
    pub cut: String,
    pub hang: String,
    pub names: String,
    pub evidence: String,
    pub read: String,
}

pub fn facts_of(task: &Task, stats: &Stats, now_ms: i64, locale: &str) -> Facts {
    let finished = if matches!(task.status, TaskStatus::Running | TaskStatus::Stopping) {
        None
    } else {
        task.finished_ms
    };
    Facts {
        label: task_label::label_in(locale, &task.kind, &task.title),
        status: status_text(task.status, locale),
        error: {
            let note = queue::note_text(task.note.as_ref(), locale);
            if note.is_empty() {
                task.error.clone().unwrap_or_default()
            } else {
                note
            }
        },
        kind: task.kind.clone(),
        recording: task.title.clone(),
        scope: task.scope.clone().unwrap_or_default(),
        path: json_str(task.input.get("path")),
        options: options_text(&task.input),
        models: stats.models.join(", "),
        started: stamp(task.started_ms),
        finished: stamp(finished),
        duration: if task.started_ms.is_some() {
            clock(elapsed(task, now_ms))
        } else {
            String::new()
        },
        progress: format!(
            "{} %",
            (task.progress.clamp(0.0, 1.0) * 100.0).round() as i32
        ),
        stats: queue::stats_text(stats),
        tool: queue::tool_text(&task.detail).unwrap_or_default(),
        cut: cut_text(task.result.as_ref(), locale),
        hang: task
            .detail
            .get("hang")
            .and_then(|value| serde_json::from_value::<mediaa_queue::Note>(value.clone()).ok())
            .map(|note| queue::note_text(Some(&note), locale))
            .unwrap_or_default(),
        names: names_text(task.result.as_ref()),
        evidence: evidence_text(task.result.as_ref(), locale),
        read: read_text(task.result.as_ref(), locale),
    }
}

fn names_text(result: Option<&Value>) -> String {
    let list = |key: &str| {
        result
            .and_then(|r| r.get(key))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    list("names")
        .iter()
        .filter_map(|item| {
            let speaker = item.get("speaker")?.as_str()?;
            let name = item.get("name")?.as_str()?;
            Some(format!("{speaker} — {name}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn evidence_text(result: Option<&Value>, locale: &str) -> String {
    let list = |key: &str| {
        result
            .and_then(|r| r.get(key))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    list("evidence")
        .iter()
        .filter_map(|item| {
            let speaker = item.get("speaker")?.as_str()?;
            let name = item.get("name")?.as_str()?;
            let how = how_text(
                item.get("how").and_then(Value::as_str).unwrap_or(""),
                locale,
            );
            let sure = sure_text(
                item.get("sure").and_then(Value::as_str).unwrap_or("high"),
                locale,
            );
            let time = item.get("time").and_then(Value::as_str).unwrap_or("");
            let text = item.get("text").and_then(Value::as_str).unwrap_or("");
            Some(format!(
                "[{time}] {speaker} — {name} ({how}, {sure}): {text}"
            ))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn how_text(how: &str, locale: &str) -> String {
    match how {
        "self" => t!("details.how_self", locale = locale).into(),
        "addressed" => t!("details.how_addressed", locale = locale).into(),
        "introduced" => t!("details.how_introduced", locale = locale).into(),
        "nickname" => t!("details.how_nickname", locale = locale).into(),
        other => other.to_string(),
    }
}

fn sure_text(sure: &str, locale: &str) -> String {
    match sure {
        "low" => t!("details.sure_low", locale = locale).into(),
        _ => t!("details.sure_high", locale = locale).into(),
    }
}

fn read_text(result: Option<&Value>, locale: &str) -> String {
    let Some(result) = result else {
        return String::new();
    };
    let (Some(read), Some(total)) = (
        result.get("read_secs").and_then(Value::as_f64),
        result.get("total_secs").and_then(Value::as_f64),
    ) else {
        return String::new();
    };
    let minutes = |secs: f64| format!("{}", (secs / 60.0).round() as i64);
    let text: String = t!(
        "details.read",
        locale = locale,
        read = minutes(read),
        total = minutes(total)
    )
    .into();
    if result.get("stopped_early").and_then(Value::as_bool) == Some(true) {
        format!("{text} · {}", t!("details.read_early", locale = locale))
    } else {
        text
    }
}

pub fn cut_text(result: Option<&Value>, locale: &str) -> String {
    let Some(note) = result.and_then(|r| r.get("cut")).and_then(Value::as_str) else {
        return String::new();
    };
    note.split(": ")
        .map(|part| cut_part(part.trim(), locale))
        .collect::<Vec<_>>()
        .join(": ")
}

fn cut_part(part: &str, locale: &str) -> String {
    let tokens =
        |value: &str| t!("details.cut_token_limit", tokens = value, locale = locale).to_string();
    if let Some(rest) = part.strip_prefix("cut at line ") {
        if let Some((line, kept)) = rest.split_once(" after ") {
            if let Some(kept) = kept.strip_suffix(" complete sections") {
                return t!(
                    "details.cut_at_line",
                    line = line,
                    kept = kept,
                    locale = locale
                )
                .into();
            }
        }
    }
    if let Some(line) = part.strip_prefix("the first answer was cut at line ") {
        return t!("details.cut_first", line = line, locale = locale).into();
    }
    if let Some(limit) = part
        .strip_prefix("hit the limit of ")
        .and_then(|r| r.strip_suffix(" tokens"))
    {
        return tokens(limit);
    }
    if let Some(limit) = part
        .strip_prefix("hit the context window of ")
        .and_then(|r| r.strip_suffix(" tokens"))
    {
        return t!("details.cut_context", tokens = limit, locale = locale).into();
    }
    if let Some(rest) = part.strip_prefix("the prompt (~") {
        if let Some((prompt, rest)) = rest.split_once(" tokens) and the answer (") {
            if let Some((answer, context)) = rest.split_once(" tokens) do not fit the context of ")
            {
                if let Some(context) = context.strip_suffix(" tokens") {
                    return t!(
                        "details.cut_does_not_fit",
                        prompt = prompt,
                        answer = answer,
                        context = context,
                        locale = locale
                    )
                    .into();
                }
            }
        }
    }
    part.to_string()
}

fn status_text(status: TaskStatus, locale: &str) -> String {
    match status {
        TaskStatus::Queued => t!("queue.queued", locale = locale),
        TaskStatus::Running => t!("queue.running", locale = locale),
        TaskStatus::Stopping => t!("queue.stopping", locale = locale),
        TaskStatus::Waiting => t!("queue.status_waiting", locale = locale),
        TaskStatus::Paused => t!("queue.paused", locale = locale),
        TaskStatus::Done => t!("queue.done", locale = locale),
        TaskStatus::Failed => t!("queue.failed", locale = locale),
        TaskStatus::Cancelled => t!("queue.cancelled", locale = locale),
    }
    .into()
}

fn json_str(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(other) if !other.is_null() => other.to_string(),
        _ => String::new(),
    }
}

fn options_text(input: &Value) -> String {
    if let Some(options) = input.get("options").filter(|v| !v.is_null()) {
        return serde_json::to_string(options).unwrap_or_default();
    }
    let Some(obj) = input.as_object() else {
        return String::new();
    };
    let mut rest = obj.clone();
    rest.remove("path");
    if rest.is_empty() {
        return String::new();
    }
    serde_json::to_string(&Value::Object(rest)).unwrap_or_default()
}

fn stamp(ms: Option<i64>) -> String {
    local_time(ms.unwrap_or(0))
        .filter(|_| ms.is_some())
        .map(|t| t.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_default()
}

pub fn local_time(ms: i64) -> Option<NaiveDateTime> {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|dt| dt.with_timezone(&chrono::Local).naive_local())
}

pub fn report_text(facts: &Facts, hint: &str, log: &str, locale: &str) -> String {
    let line = |key: &str, value: &str| {
        let label = field_label(key, locale);
        let value = if value.is_empty() { "—" } else { value };
        format!("{label}: {value}")
    };
    let mut out = vec![
        facts.label.clone(),
        line("status", &facts.status),
        line("kind", &facts.kind),
        line("recording", &facts.recording),
        line("scope", &facts.scope),
        line("path", &facts.path),
        line("options", &facts.options),
        line("models", &facts.models),
        line("started", &facts.started),
        line("finished", &facts.finished),
        line("duration", &facts.duration),
        line("progress", &facts.progress),
        line("stats", &facts.stats),
    ];
    if !facts.tool.is_empty() {
        out.push(line("tool", &facts.tool));
    }
    if !facts.cut.is_empty() {
        out.push(line("cut", &facts.cut));
    }
    if !facts.hang.is_empty() {
        out.push(line("hang", &facts.hang));
    }
    for (key, value) in [
        ("read", &facts.read),
        ("names", &facts.names),
        ("evidence", &facts.evidence),
    ] {
        if !value.is_empty() {
            out.push(format!("{}:\n{value}", field_label(key, locale)));
        }
    }
    if !hint.is_empty() {
        out.push(hint.to_string());
    }
    out.push(format!(
        "{}:\n{}",
        field_label("error", locale),
        if facts.error.is_empty() {
            "—"
        } else {
            facts.error.as_str()
        }
    ));
    out.push(format!(
        "{}:\n{}",
        field_label("log", locale),
        if log.is_empty() { "—" } else { log }
    ));
    out.join("\n")
}

fn field_label(key: &str, locale: &str) -> String {
    match key {
        "status" => t!("details.status", locale = locale).into(),
        "kind" => t!("details.kind", locale = locale).into(),
        "recording" => t!("details.recording", locale = locale).into(),
        "scope" => t!("details.scope", locale = locale).into(),
        "path" => t!("details.path", locale = locale).into(),
        "options" => t!("details.options", locale = locale).into(),
        "models" => t!("details.models", locale = locale).into(),
        "started" => t!("details.started", locale = locale).into(),
        "finished" => t!("details.finished", locale = locale).into(),
        "duration" => t!("details.duration", locale = locale).into(),
        "progress" => t!("details.progress", locale = locale).into(),
        "stats" => t!("details.stats", locale = locale).into(),
        "tool" => t!("details.tool", locale = locale).into(),
        "cut" => t!("details.cut", locale = locale).into(),
        "hang" => t!("details.hang", locale = locale).into(),
        "read" => t!("details.read_label", locale = locale).into(),
        "names" => t!("details.names", locale = locale).into(),
        "evidence" => t!("details.evidence", locale = locale).into(),
        "error" => t!("details.error", locale = locale).into(),
        "log" => t!("details.log", locale = locale).into(),
        _ => key.to_string(),
    }
}

pub fn hint_text(hint: Hint, locale: &str) -> String {
    match hint {
        Hint::ModelMissing => t!("details.hint_model", locale = locale),
        Hint::OutOfMemory => t!("details.hint_memory", locale = locale),
        Hint::Network => t!("details.hint_network", locale = locale),
    }
    .into()
}

pub fn program_log_path() -> PathBuf {
    mediaa_core::paths::logs_dir().join("mediaa.log")
}

pub fn read_program_log() -> String {
    let path = program_log_path();
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let previous = std::fs::read_to_string(path.with_file_name("mediaa.log.1")).unwrap_or_default();
    if previous.is_empty() {
        current
    } else {
        format!("{previous}{current}")
    }
}

struct Record {
    time: Option<NaiveDateTime>,
    text: String,
}

fn records_of(log: &str) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    for line in log.split_inclusive('\n') {
        if line.is_empty() {
            continue;
        }
        if line_time(line).is_some() || out.is_empty() {
            out.push(Record {
                time: line_time(line),
                text: line.to_string(),
            });
        } else if let Some(last) = out.last_mut() {
            last.text.push_str(line);
        }
    }
    out
}

fn line_time(line: &str) -> Option<NaiveDateTime> {
    let head = line.get(..23)?;
    NaiveDateTime::parse_from_str(head, "%Y-%m-%d %H:%M:%S%.3f").ok()
}

pub fn log_excerpt(log: &str, task_id: TaskId, around: Option<NaiveDateTime>) -> String {
    let records = records_of(log);
    let mark = format!("task {task_id} ");
    let picked: Vec<&Record> = if let Some(around) = around {
        let from = around - chrono::Duration::seconds(LOG_BEFORE_SECS);
        let to = around + chrono::Duration::seconds(LOG_AFTER_SECS);
        let window: Vec<&Record> = records
            .iter()
            .filter(|r| r.text.contains(&mark) || r.time.is_some_and(|t| t >= from && t <= to))
            .collect();
        if window.is_empty() {
            nearest(&records, around)
        } else {
            window
        }
    } else {
        let named: Vec<&Record> = records.iter().filter(|r| r.text.contains(&mark)).collect();
        if named.is_empty() {
            tail(&records)
        } else {
            named
        }
    };
    let text = picked
        .into_iter()
        .map(|r| clip_record(&r.text, r.text.contains(&mark)))
        .collect::<String>();
    tail_text(&text, LOG_MAX_BYTES)
}

fn nearest(records: &[Record], around: NaiveDateTime) -> Vec<&Record> {
    let Some(idx) = records
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            r.time
                .map(|t| (i, (t - around).num_milliseconds().unsigned_abs()))
        })
        .min_by_key(|(_, gap)| *gap)
        .map(|(i, _)| i)
    else {
        return tail(records);
    };
    let from = idx.saturating_sub(LOG_NEAR);
    let to = (idx + LOG_NEAR + 1).min(records.len());
    records[from..to].iter().collect()
}

fn clip_record(text: &str, keep: bool) -> String {
    if keep {
        return text.to_string();
    }
    let Some((end, _)) = text.char_indices().nth(LOG_RECORD_CHARS) else {
        return text.to_string();
    };
    format!("{}…\n", text[..end].trim_end())
}

fn tail(records: &[Record]) -> Vec<&Record> {
    let skip = records.len().saturating_sub(LOG_FALLBACK_RECORDS);
    records.iter().skip(skip).collect()
}

fn tail_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_string();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    let text = &text[start..];
    text[text.find('\n').map(|i| i + 1).unwrap_or(0)..].to_string()
}

pub enum Cmd {
    Close,
    CopyFailed(String),
}

pub fn show(
    ctx: &egui::Context,
    task: &Task,
    stats: &Stats,
    now_ms: i64,
    log: &str,
    copied: &mut bool,
    roster: &super::mentions::Roster,
) -> Option<Cmd> {
    let mut open = true;
    let mut cmd = None;
    let facts = facts_of(task, stats, now_ms, &rust_i18n::locale());
    let hint = hint_for(&facts.error);
    let hint_line = hint
        .map(|h| hint_text(h, &rust_i18n::locale()))
        .unwrap_or_default();
    let failed = task.status == TaskStatus::Failed || !facts.error.is_empty();
    theme::with_compact_title(ctx, || {
        theme::modal_backdrop(ctx);
        let response = egui::Window::new(t!("details.title"))
            .id(egui::Id::new(("task-details", task.id)))
            .order(egui::Order::Foreground)
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.set_max_width(640.0);
                ui.spacing_mut().item_spacing = egui::vec2(GAP, GAP);
                let p = theme::palette(ui);
                let locale = rust_i18n::locale();
                super::mentions::show(
                    ui,
                    &facts.label,
                    roster,
                    egui::FontId::proportional(14.0),
                    ui.visuals().strong_text_color(),
                    false,
                    p,
                );
                for (key, value) in [
                    ("kind", facts.kind.as_str()),
                    ("recording", facts.recording.as_str()),
                    ("scope", facts.scope.as_str()),
                    ("path", facts.path.as_str()),
                    ("options", facts.options.as_str()),
                    ("status", facts.status.as_str()),
                    ("started", facts.started.as_str()),
                    ("finished", facts.finished.as_str()),
                    ("duration", facts.duration.as_str()),
                    ("progress", facts.progress.as_str()),
                    ("stats", facts.stats.as_str()),
                    ("models", facts.models.as_str()),
                ] {
                    if key == "recording" {
                        field_value(ui, &field_label(key, &locale), value, Some(roster));
                    } else {
                        field(ui, &field_label(key, &locale), value);
                    }
                }
                if !facts.tool.is_empty() {
                    field(ui, &field_label("tool", &locale), &facts.tool);
                }
                if !facts.cut.is_empty() {
                    field(ui, &field_label("cut", &locale), &facts.cut);
                }
                if !facts.hang.is_empty() {
                    field(ui, &field_label("hang", &locale), &facts.hang);
                }
                if !facts.read.is_empty() {
                    field(ui, &field_label("read", &locale), &facts.read);
                }
                if !facts.names.is_empty() {
                    field(ui, &field_label("names", &locale), &facts.names);
                }
                if !facts.evidence.is_empty() {
                    field(ui, &field_label("evidence", &locale), &facts.evidence);
                }
                if failed {
                    if !hint_line.is_empty() {
                        ui.add(egui::Label::new(RichText::new(&hint_line).color(p.text)).wrap());
                    }
                    ui.label(RichText::new(field_label("error", &locale)).color(p.text_weak));
                    ui.add(
                        egui::Label::new(
                            RichText::new(if facts.error.is_empty() {
                                "—"
                            } else {
                                facts.error.as_str()
                            })
                            .color(p.danger),
                        )
                        .wrap()
                        .selectable(true),
                    );
                }
                ui.label(RichText::new(field_label("log", &locale)).color(p.text_weak));
                egui::ScrollArea::vertical()
                    .id_salt(("task-log", task.id))
                    .max_height(160.0)
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        let text = if log.is_empty() { "—" } else { log };
                        ui.add(
                            egui::Label::new(
                                RichText::new(text)
                                    .font(FontId::monospace(11.0))
                                    .color(p.text),
                            )
                            .wrap()
                            .selectable(true),
                        );
                    });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    if ui
                        .add(theme::secondary(icon::COPY, &t!("details.copy")))
                        .clicked()
                    {
                        let text = report_text(&facts, &hint_line, log, &rust_i18n::locale());
                        match clipboard::copy_text(&text) {
                            Ok(()) => *copied = true,
                            Err(e) => cmd = Some(Cmd::CopyFailed(format!("{e:#}"))),
                        }
                    }
                    let mark = t!("details.copied").to_string();
                    let width = ui.fonts_mut(|f| {
                        f.layout_no_wrap(mark.clone(), FontId::proportional(13.0), p.text)
                            .size()
                            .x
                    });
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::hover());
                    if *copied {
                        ui.painter().text(
                            rect.left_center(),
                            egui::Align2::LEFT_CENTER,
                            mark,
                            FontId::proportional(13.0),
                            p.success,
                        );
                    }
                });
            });
        theme::mark_modal(ctx, &response);
    });
    if !open {
        return Some(Cmd::Close);
    }
    cmd
}

fn field(ui: &mut egui::Ui, label: &str, value: &str) {
    field_value(ui, label, value, None);
}

fn field_value(
    ui: &mut egui::Ui,
    label: &str,
    value: &str,
    roster: Option<&super::mentions::Roster>,
) {
    let value = if value.is_empty() { "—" } else { value };
    let text = format!("{label}: {value}");
    match roster {
        Some(roster) => {
            let p = theme::palette(ui);
            super::mentions::show(
                ui,
                &text,
                roster,
                egui::FontId::proportional(13.0),
                p.text,
                false,
                p,
            );
        }
        None => {
            ui.add(egui::Label::new(text).wrap().selectable(true));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_queue::Role;
    use mediaa_queue::{Stats, Status as TaskStatus};
    use serde_json::json;

    fn task(error: &str) -> Task {
        Task {
            id: 7,
            kind: "index".into(),
            title: "interview.mp4".into(),
            input: json!({"path": "C:/rec/interview.mp4", "options": {"topics": true}}),
            priority: 0,
            status: TaskStatus::Failed,
            cursor: Value::Null,
            partial: Value::Null,
            result: None,
            progress: 0.4,
            error: Some(error.into()),
            note: None,
            attempts: 1,
            created_ms: 0,
            updated_ms: 4_000,
            parent: Some(1),
            scope: Some("file-1".into()),
            inline: false,
            started_ms: Some(1_000),
            finished_ms: Some(4_000),
            stats: Stats {
                model_calls: 3,
                prompt_tokens: 100,
                completion_tokens: 20,
                model_ms: 500,
                models: vec!["Qwen/Qwen3.5-4B".into()],
            },
            detail: Value::Null,
            units: 1.0,
            resource: None,
            memory: 0,
            model: None,
            role: Role::Work,
            gen: 0,
            yield_cont: false,
            heartbeat_ms: None,
            stop_intent: None,
        }
    }

    #[test]
    fn the_facts_name_the_task_its_input_and_its_models() {
        let facts = facts_of(&task("boom"), &task("boom").stats, 5_000, "ru");
        assert_eq!(facts.label, "Индекс: interview.mp4");
        assert_eq!(facts.status, "ошибка");
        assert_eq!(facts.kind, "index");
        assert_eq!(facts.recording, "interview.mp4");
        assert_eq!(facts.scope, "file-1");
        assert_eq!(facts.path, "C:/rec/interview.mp4");
        assert_eq!(facts.options, r#"{"topics":true}"#);
        assert_eq!(facts.models, "Qwen/Qwen3.5-4B");
        assert_eq!(facts.duration, "0:03");
        assert_eq!(facts.progress, "40 %");
        assert!(facts.stats.contains('3'), "{}", facts.stats);
    }

    #[test]
    fn a_running_task_counts_its_duration_up_to_now() {
        let mut t = task("");
        t.status = TaskStatus::Running;
        t.finished_ms = None;
        assert_eq!(facts_of(&t, &t.stats, 90_000, "ru").duration, "1:29");
        t.finished_ms = Some(2_000);
        let running = facts_of(&t, &t.stats, 90_000, "ru");
        assert_eq!(
            (running.duration.as_str(), running.finished.as_str()),
            ("1:29", "")
        );
        t.status = TaskStatus::Done;
        t.finished_ms = Some(4_000);
        assert_eq!(facts_of(&t, &t.stats, 90_000, "ru").duration, "0:03");
    }

    #[test]
    fn the_reason_the_watchdog_interrupted_the_task_is_shown() {
        let mut t = task("");
        t.status = TaskStatus::Queued;
        t.detail = json!({"hang": {"kind": "hang", "resource": "llm", "idle_s": 300}});
        let facts = facts_of(&t, &t.stats, 5_000, "ru");
        assert_eq!(
            facts.hang,
            "модель не отвечает дольше 5 мин — вызов прерван, задача продолжит с последней точки"
        );
        let report = report_text(&facts, "", "", "ru");
        assert!(
            report.contains("Прервано") && report.contains("не отвечает"),
            "{report}"
        );
        assert!(
            facts_of(&t, &t.stats, 5_000, "en")
                .hang
                .starts_with("the model has not responded"),
            "{}",
            facts_of(&t, &t.stats, 5_000, "en").hang
        );
        t.detail = Value::Null;
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").hang, "");
    }

    #[test]
    fn a_note_is_shown_instead_of_an_older_error_and_in_the_window_language() {
        let mut t = task("boom");
        t.note = Some(mediaa_queue::failure_note("Темы: rec"));
        assert_eq!(
            facts_of(&t, &t.stats, 5_000, "ru").error,
            "из-за ошибки в Темы: rec"
        );
        assert_eq!(
            facts_of(&t, &t.stats, 5_000, "en").error,
            "because of a failure in Темы: rec"
        );
        t.note = None;
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").error, "boom");
    }

    #[test]
    fn a_cut_topic_detail_says_why_it_stopped() {
        let mut t = task("");
        t.kind = "topic-detail".into();
        t.result = Some(json!({
            "cut": "cut at line 103 after 2 complete sections: hit the limit of 1819 tokens"
        }));
        let ru = facts_of(&t, &t.stats, 5_000, "ru");
        assert!(
            ru.cut.contains("строке 103") && ru.cut.contains("1819"),
            "{}",
            ru.cut
        );
        assert!(!ru.cut.contains("complete sections"), "{}", ru.cut);
        let en = facts_of(&t, &t.stats, 5_000, "en").cut;
        assert!(
            en.contains("line 103") && en.contains("limit") && en.contains("1819"),
            "{en}"
        );
        let report = report_text(&facts_of(&t, &t.stats, 5_000, "en"), "", "", "en");
        assert!(report.contains(&en), "{report}");

        t.result = Some(json!({
            "cut": "the first answer was cut at line 40: hit the context window of 8192 tokens"
        }));
        let text = facts_of(&t, &t.stats, 5_000, "en").cut;
        assert!(
            text.contains("first answer") && text.contains("40") && text.contains("8192"),
            "{text}"
        );
        t.result = Some(json!({
            "cut": "the prompt (~7000 tokens) and the answer (2000 tokens) do not fit the context of 8192 tokens"
        }));
        let text = facts_of(&t, &t.stats, 5_000, "ru").cut;
        assert!(
            text.contains("7000") && text.contains("2000") && text.contains("8192"),
            "{text}"
        );
        t.result = Some(json!({ "cut": "something new" }));
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").cut, "something new");
        t.result = None;
        assert_eq!(facts_of(&t, &t.stats, 5_000, "ru").cut, "");
    }

    #[test]
    fn a_speaker_names_run_lists_the_names_and_the_evidence() {
        let mut t = task("");
        t.kind = "speaker-names".into();
        t.status = TaskStatus::Done;
        t.result = Some(json!({
            "names": [{ "speaker": "@s1", "name": "Иван", "score": 3.0 }],
            "evidence": [{ "speaker": "@s1", "name": "Иван", "how": "self", "time": "0:05", "text": "Меня зовут Иван.", "sure": "high" }],
            "read_secs": 720.0,
            "total_secs": 1620.0,
            "stopped_early": true,
        }));
        let facts = facts_of(&t, &t.stats, 5_000, "ru");
        assert_eq!(facts.names, "@s1 — Иван");
        assert!(
            facts.evidence.contains("[0:05]") && facts.evidence.contains("Меня зовут Иван."),
            "{}",
            facts.evidence
        );
        assert!(
            facts.read.contains("12") && facts.read.contains("27"),
            "{}",
            facts.read
        );
        let report = report_text(&facts, "", "", "ru");
        assert!(
            report.contains(&facts.evidence) && report.contains(&facts.read),
            "{report}"
        );
        t.result.as_mut().unwrap()["stopped_early"] = json!(false);
        let en = facts_of(&t, &t.stats, 5_000, "en");
        assert!(
            en.read.contains("12") && en.read.contains("27"),
            "{}",
            en.read
        );
        assert!(
            !en.read
                .contains(&t!("details.read_early", locale = "en").to_string()),
            "{}",
            en.read
        );
        let plain = task("");
        let facts = facts_of(&plain, &plain.stats, 5_000, "ru");
        assert!(facts.names.is_empty() && facts.evidence.is_empty() && facts.read.is_empty());
    }

    #[test]
    fn speakers_count_as_the_input_when_there_are_no_options() {
        let mut t = task("x");
        t.input = json!({"path": "a.wav", "speakers": 2});
        assert_eq!(
            facts_of(&t, &t.stats, 5_000, "en").options,
            r#"{"speakers":2}"#
        );
    }

    #[test]
    fn known_failures_get_a_hint() {
        assert_eq!(
            hint_for(r"C:\models\silero_vad.onnx: the voice activity model does not load"),
            Some(Hint::ModelMissing)
        );
        assert_eq!(
            hint_for("models/x.gguf not found"),
            Some(Hint::ModelMissing)
        );
        assert_eq!(
            hint_for("the model ran out of memory"),
            Some(Hint::OutOfMemory)
        );
        assert_eq!(
            hint_for("proxy \"http://127.0.0.1:9\": connection timed out"),
            Some(Hint::Network)
        );
        assert_eq!(hint_for("the grammar was rejected"), None);
        assert_eq!(
            hint_for("proxy set, but model.onnx not found"),
            Some(Hint::ModelMissing)
        );
    }

    #[test]
    fn the_model_hint_tells_the_user_to_download() {
        let ru = hint_text(Hint::ModelMissing, "ru");
        assert!(ru.contains("Скачать"), "{ru}");
        assert!(!ru.contains("details."), "{ru}");
        let en = hint_text(Hint::ModelMissing, "en");
        assert!(en.to_lowercase().contains("download"), "{en}");
    }

    #[test]
    fn the_report_carries_the_error_the_hint_and_the_log() {
        let sample = task("model.onnx: the voice activity model does not load");
        let facts = facts_of(&sample, &sample.stats, 5_000, "en");
        let hint = hint_text(Hint::ModelMissing, "en");
        let log = "2026-10-21 21:07:01.123 [ERROR] mediaa_agent::worker: task 7 index (interview.mp4) failed: model.onnx: the voice activity model does not load\n";
        let text = report_text(&facts, &hint, log, "en");
        assert!(text.contains("interview.mp4"), "{text}");
        assert!(text.contains("does not load"), "{text}");
        assert!(text.contains(&hint), "{text}");
        assert!(text.contains("task 7 index"), "{text}");
        assert!(text.contains("Qwen/Qwen3.5-4B"), "{text}");
    }

    #[test]
    fn the_log_excerpt_keeps_the_failure_and_its_neighbours() {
        let log = "\
2026-10-21 21:00:00.000 [INFO] mediaa: started\n\
2026-10-21 21:06:50.000 [INFO] mediaa_engine: loading\n\
2026-10-21 21:07:01.100 [ERROR] mediaa_agent::worker: task 7 index (interview.mp4) failed: model.onnx: the voice activity model does not load\n\
caused by: file missing\n\
2026-10-21 21:07:02.000 [INFO] mediaa: still up\n\
2026-10-21 22:00:00.000 [INFO] mediaa: later\n";
        let around =
            NaiveDateTime::parse_from_str("2026-10-21 21:07:01.100", "%Y-%m-%d %H:%M:%S%.3f")
                .unwrap();
        let excerpt = log_excerpt(log, 7, Some(around));
        assert!(excerpt.contains("task 7 index"), "{excerpt}");
        assert!(excerpt.contains("caused by: file missing"), "{excerpt}");
        assert!(excerpt.contains("still up"), "{excerpt}");
        assert!(!excerpt.contains("later"), "{excerpt}");
        assert!(!excerpt.contains("started"), "{excerpt}");
    }

    #[test]
    fn an_empty_window_shows_the_records_nearest_the_failure() {
        let mut log = String::new();
        for i in 0..30 {
            log.push_str(&format!(
                "2026-10-21 18:39:{i:02}.000 [INFO] mediaa: startup {i}\n"
            ));
        }
        log.push_str("2026-10-21 18:58:00.000 [INFO] mediaa: last before the gap\n");
        log.push_str(&format!(
            "2026-10-21 21:40:58.000 [INFO] mediaa: nearest line {}\n",
            "A".repeat(2000)
        ));
        let around =
            NaiveDateTime::parse_from_str("2026-10-21 20:55:56.000", "%Y-%m-%d %H:%M:%S%.3f")
                .unwrap();
        let excerpt = log_excerpt(&log, 7, Some(around));
        assert!(excerpt.contains("nearest line"), "{excerpt}");
        assert!(excerpt.contains("last before the gap"), "{excerpt}");
        assert!(!excerpt.contains("startup 0"), "{excerpt}");
        assert!(excerpt.contains('…'), "{excerpt}");
        assert!(excerpt.len() < 2000, "{}", excerpt.len());
    }

    #[test]
    fn without_a_time_the_excerpt_is_the_task_or_the_tail() {
        let log = "2026-10-21 21:07:01.100 [ERROR] mediaa_agent::worker: task 7 index (interview.mp4) failed: boom\n";
        assert!(log_excerpt(log, 7, None).contains("task 7"));
        let other = "2026-10-21 21:07:01.100 [INFO] mediaa: hello\n";
        assert!(log_excerpt(other, 9, None).contains("hello"));
    }
}
