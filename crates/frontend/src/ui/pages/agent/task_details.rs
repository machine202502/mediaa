use super::analysis::clock;
use super::queue;
use super::task_label;
use crate::features::agent::view::{
    elapsed, note_text, tool_text, Note, Stats, Status as TaskStatus, Task, TaskId,
};
use crate::ui::theme;
use chrono::NaiveDateTime;
use eframe::egui::{self, FontId, RichText};
use egui_phosphor::regular as icon;
use rust_i18n::t;
use serde_json::Value;

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
            let note = note_text(task.note.as_ref(), locale);
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
        tool: tool_text(&task.detail).unwrap_or_default(),
        cut: cut_text(task.result.as_ref(), locale),
        hang: task
            .detail
            .get("hang")
            .and_then(|value| serde_json::from_value::<Note>(value.clone()).ok())
            .map(|note| note_text(Some(&note), locale))
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

pub fn read_program_log() -> String {
    crate::infra::log_file::read_program_log()
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
    let mut cmd = None;
    let facts = facts_of(task, stats, now_ms, &rust_i18n::locale());
    let hint = hint_for(&facts.error);
    let hint_line = hint
        .map(|h| hint_text(h, &rust_i18n::locale()))
        .unwrap_or_default();
    let failed = task.status == TaskStatus::Failed || !facts.error.is_empty();
    let mut dismissed = false;
    theme::with_compact_title(ctx, || {
        let modal = egui::Modal::new(egui::Id::new(("task-details", task.id))).show(ctx, |ui| {
            ui.set_width(560.0);
            theme::popup_title(ui, &t!("details.title"));
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
            let copy_label = t!("details.copy").to_string();
            let mark = t!("details.copied").to_string();
            let mark_width = ui.fonts_mut(|f| {
                f.layout_no_wrap(mark.clone(), FontId::proportional(13.0), p.text)
                    .size()
                    .x
            });
            let width = theme::button_width(ui, icon::COPY, &copy_label) + GAP + mark_width;
            theme::centered_row_sized(ui, width, |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                if ui.add(theme::secondary(icon::COPY, &copy_label)).clicked() {
                    let text = report_text(&facts, &hint_line, log, &rust_i18n::locale());
                    match crate::infra::clipboard::copy_text(&text) {
                        Ok(()) => *copied = true,
                        Err(e) => cmd = Some(Cmd::CopyFailed(format!("{e:#}"))),
                    }
                }
                let (rect, _) =
                    ui.allocate_exact_size(egui::vec2(mark_width, 28.0), egui::Sense::hover());
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
        dismissed = modal.should_close();
    });
    if dismissed {
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
