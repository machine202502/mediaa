use super::chat::linkify;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_agent::assistant::chat::Live;
use mediaa_agent::assistant::{Metrics, StepRecord, WaitReason};
use mediaa_core::chat::{ChatMessage, TimelineEntry, STEP_RUNNING};
use rust_i18n::t;
use std::hash::Hash;

pub const GAP: f32 = 8.0;
const ROW_TEXT: f32 = 12.5;
const METRICS_TEXT: f32 = 11.5;
const METRICS_BAR: f32 = 3.0;
const METRICS_GAP: f32 = 4.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Row<'a> {
    Step {
        title: &'a str,
        tool: &'a str,
        status: &'a str,
    },
    Thinking {
        text: &'a str,
    },
    Steer {
        text: &'a str,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block<'a> {
    Row(Row<'a>),
    Answer(&'a str),
    Folded(Vec<Row<'a>>),
}

pub fn blocks(entries: &[TimelineEntry], fold: bool) -> Vec<Block<'_>> {
    let mut out = Vec::new();
    let mut folded = Vec::new();
    for e in entries {
        let block = match e {
            TimelineEntry::Step {
                title,
                tool,
                status,
            } => Block::Row(Row::Step {
                title,
                tool,
                status,
            }),
            TimelineEntry::Thinking { text } if text.trim().is_empty() => continue,
            TimelineEntry::Thinking { text } => Block::Row(Row::Thinking { text }),
            TimelineEntry::Answer { text, .. } if text.trim().is_empty() => continue,
            TimelineEntry::Answer { text, .. } => Block::Answer(text),
            TimelineEntry::Steer { text } => {
                out.push(Block::Row(Row::Steer { text }));
                continue;
            }
        };
        match block {
            Block::Row(row) if fold => folded.push(row),
            block => out.push(block),
        }
    }
    if !folded.is_empty() {
        out.insert(0, Block::Folded(folded));
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    Waiting(WaitReason),
    Reasoning,
    Writing,
    Starting,
}

pub fn status_of(live: &Live) -> Option<Status> {
    if let Some(reason) = &live.waiting {
        return Some(Status::Waiting(reason.clone()));
    }
    if live.scanning.is_some() {
        return None;
    }
    match live.timeline.entries.last() {
        None => Some(Status::Starting),
        Some(TimelineEntry::Thinking { .. }) => Some(Status::Reasoning),
        Some(TimelineEntry::Answer { section: false, .. }) => Some(Status::Writing),
        Some(TimelineEntry::Step { status, .. }) if status == STEP_RUNNING => None,
        Some(_) => Some(Status::Starting),
    }
}

pub fn hidden_sections(messages: &[ChatMessage], running: bool) -> Vec<bool> {
    let mut hidden = vec![false; messages.len()];
    let mut end = messages.len();
    let mut last = true;
    for start in (0..messages.len()).rev() {
        if messages[start].author != mediaa_core::chat::Author::User {
            continue;
        }
        let turn = start + 1..end;
        let drawn_by_timeline = (last && running)
            || messages[turn.clone()]
                .iter()
                .any(|m| !is_section(m) && !m.timeline().is_empty());
        if drawn_by_timeline {
            for i in turn {
                hidden[i] = is_section(&messages[i]);
            }
        }
        end = start;
        last = false;
    }
    hidden
}

fn is_section(m: &ChatMessage) -> bool {
    m.details["section"].as_bool() == Some(true)
}

fn busy_names(tasks: &[mediaa_agent::scope::TaskRef]) -> String {
    tasks
        .iter()
        .map(|t| super::task_label::label(&t.kind, &t.title))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn wait_text(reason: &WaitReason) -> String {
    let name = |model: &str| {
        model
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(model)
            .to_string()
    };
    match reason {
        WaitReason::Loading { model } => t!("studio.wait_loading", model = name(model)).to_string(),
        WaitReason::Memory { model } => t!("studio.wait_memory", model = name(model)).to_string(),
        WaitReason::Downloading { model, done, total } => {
            let mb = |b: &u64| b / (1 << 20);
            t!(
                "studio.wait_downloading",
                model = name(model),
                done = mb(done),
                total = mb(total)
            )
            .to_string()
        }
        WaitReason::Busy { tasks } if tasks.is_empty() => t!("studio.wait_busy").to_string(),
        WaitReason::Busy { tasks } => {
            t!("studio.wait_busy_with", tasks = busy_names(tasks)).to_string()
        }
        WaitReason::ProviderSlot => t!("studio.wait_provider").to_string(),
    }
}

fn status_text(status: &Status) -> String {
    match status {
        Status::Waiting(reason) => wait_text(reason),
        Status::Reasoning => t!("studio.reasoning").to_string(),
        Status::Writing => t!("studio.writing").to_string(),
        Status::Starting => t!("studio.starting").to_string(),
    }
}

fn number(value: f64, digits: usize, locale: &str) -> String {
    let text = format!("{value:.digits$}");
    if locale == "ru" {
        text.replace('.', ",")
    } else {
        text
    }
}

pub fn metrics_text(metrics: &Metrics, locale: &str) -> (String, Option<String>) {
    let used = number(f64::from(metrics.context_tokens) / 1000.0, 1, locale);
    let total = number(f64::from(metrics.context) / 1000.0, 0, locale);
    let context = t!(
        "studio.context_fill",
        locale = locale,
        used = used,
        total = total
    )
    .to_string();
    let speed = metrics.speed.map(|s| {
        t!(
            "studio.tokens_per_second",
            locale = locale,
            speed = format!("{s:.0}")
        )
        .to_string()
    });
    (context, speed)
}

pub fn metrics_ui(ui: &mut egui::Ui, metrics: Option<&Metrics>, p: theme::Palette) {
    let locale = rust_i18n::locale();
    let (context, speed) = metrics.map_or_else(
        || (" ".into(), None),
        |metrics| metrics_text(metrics, &locale),
    );
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = METRICS_GAP;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.label(RichText::new(context).size(METRICS_TEXT).color(p.text_weak));
            if let Some(speed) = speed {
                ui.label(RichText::new("·").size(METRICS_TEXT).color(p.text_weak));
                ui.label(RichText::new(speed).size(METRICS_TEXT).color(p.text_weak));
            }
        });
        let (bar, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), METRICS_BAR),
            egui::Sense::hover(),
        );
        let Some(metrics) = metrics else { return };
        let filled =
            (metrics.context_tokens as f32 / metrics.context.max(1) as f32).clamp(0.0, 1.0);
        ui.painter()
            .rect_filled(bar, METRICS_BAR / 2.0, p.accent_soft);
        if filled > 0.0 {
            let width = bar.width() * filled;
            ui.painter().rect_filled(
                egui::Rect::from_min_size(bar.min, egui::vec2(width, METRICS_BAR)),
                METRICS_BAR / 2.0,
                p.accent,
            );
        }
    });
}

fn step_row(
    ui: &mut egui::Ui,
    title: &str,
    status: &str,
    roster: &super::mentions::Roster,
    p: theme::Palette,
) {
    let (glyph, color) = match status {
        "found" => (icon::CHECK_CIRCLE, p.success),
        "nothing" => (icon::CIRCLE_DASHED, p.text_weak),
        "repeated" => (icon::ARROW_COUNTER_CLOCKWISE, p.text_weak),
        "failed" => (icon::WARNING_CIRCLE, p.danger),
        "stopped" => (icon::STOP_CIRCLE, p.text_weak),
        _ => (icon::DOTS_THREE_CIRCLE, p.accent),
    };
    ui.horizontal(|ui| {
        ui.label(RichText::new(glyph).color(color));
        let galley = super::mentions::galley(
            ui,
            title,
            roster,
            egui::FontId::proportional(ROW_TEXT),
            p.text,
            ui.available_width(),
            true,
            false,
            p,
        );
        ui.add(egui::Label::new(galley).truncate());
    });
}

fn thinking_row(
    ui: &mut egui::Ui,
    id: impl Hash + std::fmt::Debug,
    text: &str,
    roster: &super::mentions::Roster,
    p: theme::Palette,
) {
    egui::CollapsingHeader::new(
        RichText::new(t!("studio.thinking_block"))
            .size(ROW_TEXT)
            .color(p.text_weak),
    )
    .id_salt(id)
    .default_open(false)
    .show(ui, |ui| {
        super::mentions::show(
            ui,
            text.trim(),
            roster,
            egui::FontId::proportional(12.0),
            p.text_weak,
            false,
            p,
        );
    });
}

fn step_title(tool: &str, title: &str) -> String {
    shown_in(&rust_i18n::locale(), tool, title)
}

fn tool_label(locale: &str, key: &str) -> String {
    match key {
        "outline" => t!("chat.tool_outline", locale = locale),
        "topic" => t!("chat.tool_topic", locale = locale),
        "search" => t!("chat.tool_search", locale = locale),
        "read" => t!("chat.tool_read", locale = locale),
        "mentions" => t!("chat.tool_mentions", locale = locale),
        "speakers" => t!("chat.tool_speakers", locale = locale),
        "calculator" => t!("chat.tool_calculator", locale = locale),
        "time" => t!("chat.tool_time", locale = locale),
        "scan" => t!("chat.tool_scan", locale = locale),
        "rename_speakers" => t!("chat.tool_rename_speakers", locale = locale),
        "remember" => t!("chat.tool_remember", locale = locale),
        "plan" => t!("chat.tool_plan", locale = locale),
        "say" => t!("chat.tool_say", locale = locale),
        "web_search" => t!("chat.tool_web_search", locale = locale),
        "web_read" => t!("chat.tool_web_read", locale = locale),
        "recordings" => t!("chat.tool_recordings", locale = locale),
        _ => t!("chat.tool_step", locale = locale),
    }
    .into()
}

fn plain_label(tool: &str, title: &str) -> bool {
    if title.is_empty() || title == tool {
        return true;
    }
    for locale in ["ru", "en"] {
        if title == tool_label(locale, tool) {
            return true;
        }
        if tool == "outline" && title == tool_label(locale, "topic") {
            return true;
        }
    }
    false
}

fn shown_in(locale: &str, tool: &str, title: &str) -> String {
    if tool == "verify" {
        return t!("studio.verify_step", locale = locale).into();
    }
    if tool == "draft" {
        return t!("studio.draft_step", locale = locale).into();
    }
    if tool == "outline"
        && (title == tool_label("ru", "topic") || title == tool_label("en", "topic"))
    {
        return tool_label(locale, "topic");
    }
    if plain_label(tool, title) {
        tool_label(locale, tool)
    } else {
        title.to_string()
    }
}

fn draft_row(
    ui: &mut egui::Ui,
    id: impl Hash + std::fmt::Debug,
    body: &str,
    bare: bool,
    roster: &super::mentions::Roster,
    p: theme::Palette,
) -> Option<String> {
    let mut seek = None;
    egui::CollapsingHeader::new(
        RichText::new(t!("studio.draft_step"))
            .size(ROW_TEXT)
            .color(p.text_weak),
    )
    .id_salt(id)
    .default_open(false)
    .show(ui, |ui| {
        seek = super::markdown::show(ui, &linkify(body, bare), roster, p);
    });
    seek
}

#[allow(clippy::too_many_arguments)]
fn row(
    ui: &mut egui::Ui,
    id: impl Hash + std::fmt::Debug,
    row: &Row,
    steps: &[StepRecord],
    draft_at: usize,
    bare: bool,
    roster: &super::mentions::Roster,
    p: theme::Palette,
) -> Option<String> {
    match row {
        Row::Step { tool, .. } if *tool == "draft" => {
            let body = steps
                .iter()
                .filter(|s| s.tool == "draft")
                .nth(draft_at)
                .map(|s| s.result.as_str())
                .unwrap_or("");
            draft_row(ui, id, body, bare, roster, p)
        }
        Row::Step {
            title,
            tool,
            status,
        } => {
            step_row(ui, &step_title(tool, title), status, roster, p);
            None
        }
        Row::Thinking { text } => {
            thinking_row(ui, id, text, roster, p);
            None
        }
        Row::Steer { text } => {
            steer_row(ui, text, roster, p);
            None
        }
    }
}

fn steer_row(ui: &mut egui::Ui, text: &str, roster: &super::mentions::Roster, p: theme::Palette) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon::CHAT_TEXT).color(p.accent));
        let label = format!("{}: {}", t!("studio.steer_taken"), text.trim());
        let galley = super::mentions::galley(
            ui,
            &label,
            roster,
            egui::FontId::proportional(ROW_TEXT),
            p.text_weak,
            ui.available_width(),
            true,
            false,
            p,
        );
        ui.add(egui::Label::new(galley).truncate());
    });
}

#[allow(clippy::too_many_arguments)]
pub fn timeline_ui(
    ui: &mut egui::Ui,
    id: impl Hash + std::fmt::Debug,
    entries: &[TimelineEntry],
    steps: &[StepRecord],
    fold: bool,
    forced: bool,
    bare_times: bool,
    roster: &super::mentions::Roster,
    p: theme::Palette,
) -> Option<String> {
    let mut seek = None;
    let mut draft_at = 0usize;
    ui.push_id(id, |ui| {
        ui.spacing_mut().item_spacing.y = GAP;
        for (n, block) in blocks(entries, fold).iter().enumerate() {
            ui.push_id(n, |ui| match block {
                Block::Row(r) => {
                    let at = draft_at;
                    if matches!(r, Row::Step { tool: "draft", .. }) {
                        draft_at += 1;
                    }
                    if let Some(t) = row(ui, "row", r, steps, at, bare_times, roster, p) {
                        seek = Some(t);
                    }
                }
                Block::Folded(rows) => {
                    let header = RichText::new(t!("studio.steps_folded", n = rows.len()))
                        .size(11.5)
                        .color(p.text_weak);
                    egui::CollapsingHeader::new(header)
                        .id_salt("folded")
                        .default_open(false)
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.y = GAP / 2.0;
                            for (i, r) in rows.iter().enumerate() {
                                let at = draft_at;
                                if matches!(r, Row::Step { tool: "draft", .. }) {
                                    draft_at += 1;
                                }
                                if let Some(t) =
                                    row(ui, ("folded-row", i), r, steps, at, bare_times, roster, p)
                                {
                                    seek = Some(t);
                                }
                            }
                            if forced {
                                theme::hint(ui, t!("studio.forced"));
                            }
                        });
                }
                Block::Answer(text) => {
                    if let Some(t) =
                        super::markdown::show(ui, &linkify(text, bare_times), roster, p)
                    {
                        seek = Some(t);
                    }
                }
            });
        }
    });
    seek
}

pub fn live_ui(
    ui: &mut egui::Ui,
    live: &Live,
    bare_times: bool,
    roster: &super::mentions::Roster,
    p: theme::Palette,
    pending: &[String],
) -> Option<String> {
    let seek = timeline_ui(
        ui,
        "live",
        &live.timeline.entries,
        &live.steps,
        false,
        false,
        bare_times,
        roster,
        p,
    );
    ui.spacing_mut().item_spacing.y = GAP;
    if let Some((done, total)) = live.scanning {
        ui.label(
            RichText::new(t!("studio.scanning", done = done, total = total))
                .size(ROW_TEXT)
                .color(p.text_weak),
        );
        ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_height(5.0));
    }
    if let Some(status) = status_of(live) {
        ui.horizontal(|ui| {
            ui.spinner();
            ui.label(RichText::new(status_text(&status)).color(p.text_weak));
        });
    }
    for text in pending {
        theme::hint(ui, t!("studio.steer_pending", text = text));
    }
    metrics_ui(ui, live.metrics.as_ref(), p);
    seek
}

#[cfg(test)]
mod tests {
    use super::*;
    fn task(kind: &str, title: &str) -> mediaa_agent::scope::TaskRef {
        mediaa_agent::scope::TaskRef {
            kind: kind.into(),
            title: title.into(),
        }
    }

    use mediaa_agent::assistant::Event;
    use mediaa_core::chat::{Author, Timeline};
    use serde_json::json;

    fn step(title: &str, status: &str) -> TimelineEntry {
        TimelineEntry::Step {
            title: title.into(),
            tool: "search".into(),
            status: status.into(),
        }
    }
    fn thinking(text: &str) -> TimelineEntry {
        TimelineEntry::Thinking { text: text.into() }
    }
    fn answer(text: &str, section: bool) -> TimelineEntry {
        TimelineEntry::Answer {
            text: text.into(),
            section,
        }
    }

    #[test]
    fn a_running_or_stopped_answer_is_drawn_in_order_and_a_final_one_is_folded() {
        let entries = [
            thinking("Надо искать."),
            step("Ищу сроки", "found"),
            thinking("  "),
            step("Читаю", "found"),
            answer("", false),
            answer("В пятницу.", false),
        ];
        let row = |i: usize| match &entries[i] {
            TimelineEntry::Step {
                title,
                tool,
                status,
            } => Row::Step {
                title,
                tool,
                status,
            },
            TimelineEntry::Thinking { text } => Row::Thinking { text },
            _ => unreachable!(),
        };
        assert_eq!(
            blocks(&entries, false),
            [
                Block::Row(row(0)),
                Block::Row(row(1)),
                Block::Row(row(3)),
                Block::Answer("В пятницу.")
            ]
        );
        let done = blocks(&entries, true);
        assert_eq!(
            done,
            [
                Block::Folded(vec![row(0), row(1), row(3)]),
                Block::Answer("В пятницу.")
            ]
        );
        assert_eq!(
            blocks(&[answer("Привет.", false)], true),
            [Block::Answer("Привет.")]
        );
        assert!(blocks(&[], true).is_empty());
        let parts = [
            step("А", "found"),
            answer("Часть 1.", true),
            step("Б", "found"),
            answer("Итог.", false),
        ];
        let folded = blocks(&parts, true);
        assert_eq!(folded.len(), 3);
        assert!(matches!(&folded[0], Block::Folded(rows) if rows.len() == 2));
        assert_eq!(
            (&folded[1], &folded[2]),
            (&Block::Answer("Часть 1."), &Block::Answer("Итог."))
        );
    }

    #[test]
    fn the_status_line_follows_what_the_model_does() {
        let mut live = Live::new();
        assert_eq!(status_of(&live), Some(Status::Starting));
        live.apply(&Event::Waiting {
            reason: WaitReason::Busy {
                tasks: vec![task("analysis", "a.mp4")],
            },
        });
        assert_eq!(
            status_of(&live),
            Some(Status::Waiting(WaitReason::Busy {
                tasks: vec![task("analysis", "a.mp4")]
            }))
        );
        live.apply(&Event::Thinking("Думаю".into()));
        assert_eq!(status_of(&live), Some(Status::Reasoning));
        live.apply(&Event::StepStarted {
            title: "Ищу".into(),
            tool: "search".into(),
        });
        assert_eq!(status_of(&live), None);
        live.apply(&Event::Scanning { done: 1, total: 4 });
        assert_eq!(status_of(&live), None);
        live.scanning = None;
        live.apply(&Event::AnswerText("В пят".into()));
        assert_eq!(status_of(&live), Some(Status::Writing));
        assert_eq!(live.timeline.draft(), "В пят");
    }

    #[test]
    fn the_waiting_reason_is_in_words() {
        let busy = WaitReason::Busy {
            tasks: vec![task("transcribe", "a.mp4")],
        };
        assert_eq!(
            wait_text(&busy),
            t!(
                "studio.wait_busy_with",
                tasks = crate::pages::agent::task_label::label("transcribe", "a.mp4")
            )
            .to_string()
        );
        assert_eq!(
            wait_text(&WaitReason::Loading {
                model: "owner/Qwen3-4B".into()
            }),
            t!("studio.wait_loading", model = "Qwen3-4B").to_string(),
            "the model, not its owner"
        );
    }

    #[test]
    fn the_numbers_are_the_context_and_the_speed_in_the_interface_language() {
        let metrics = Metrics {
            context_tokens: 6100,
            context: 8000,
            speed: Some(12.4),
        };
        assert_eq!(
            metrics_text(&metrics, "ru"),
            ("контекст: 6,1 из 8 тыс.".into(), Some("12 ток/с".into()))
        );
        assert_eq!(
            metrics_text(&metrics, "en"),
            ("context: 6.1 of 8k".into(), Some("12 tok/s".into()))
        );
        let provider = Metrics {
            context_tokens: 500,
            context: 4000,
            speed: None,
        };
        assert_eq!(
            metrics_text(&provider, "ru"),
            ("контекст: 0,5 из 4 тыс.".into(), None)
        );
    }

    #[test]
    fn the_numbers_take_their_place_even_without_them() {
        fn height(metrics: Option<Metrics>) -> f32 {
            let mut harness = egui_kittest::Harness::new_ui_state(
                |ui: &mut egui::Ui, height: &mut Option<f32>| {
                    let before = ui.cursor().min.y;
                    metrics_ui(ui, metrics.as_ref(), crate::theme::DARK);
                    *height = Some(ui.cursor().min.y - before);
                },
                None,
            );
            harness.run();
            (*harness.state()).expect("the row is drawn")
        }
        let shown = height(Some(Metrics {
            context_tokens: 6100,
            context: 8000,
            speed: Some(12.4),
        }));
        let hidden = height(None);
        assert!(shown > 0.0);
        assert_eq!(shown, hidden, "the row is reserved");
    }

    fn message(author: Author, text: &str, details: serde_json::Value) -> ChatMessage {
        ChatMessage {
            author,
            text: text.into(),
            details,
        }
    }

    #[test]
    fn sections_are_not_drawn_twice() {
        let mut tl = Timeline::default();
        tl.step("Ищу", "search", "found");
        tl.section("Часть 1.");
        tl.finish_answer("Итог.");
        let section = || message(Author::Agent, "Часть 1.", json!({ "section": true }));
        let last = || {
            message(
                Author::Agent,
                "Итог.",
                json!({ "steps": [], "timeline": tl.to_value() }),
            )
        };
        let user = || message(Author::User, "в?", serde_json::Value::Null);

        let saved = [user(), section(), last(), user(), section(), last()];
        assert_eq!(
            hidden_sections(&saved, false),
            [false, true, false, false, true, false]
        );
        let running = [user(), section(), last(), user(), section()];
        assert_eq!(
            hidden_sections(&running, true),
            [false, true, false, false, true]
        );
        let failed = [user(), section()];
        assert_eq!(hidden_sections(&failed, false), [false, false]);
        let old = [
            user(),
            section(),
            message(Author::Agent, "Итог.", json!({ "steps": [] })),
        ];
        assert_eq!(hidden_sections(&old, false), [false, false, false]);
        let stopped = [
            user(),
            section(),
            message(
                Author::Agent,
                "",
                json!({ "steps": [], "timeline": tl.to_value(), "stopped": true }),
            ),
        ];
        assert_eq!(hidden_sections(&stopped, false), [false, true, false]);
    }

    #[test]
    fn a_step_shows_a_label_or_a_phrase_and_never_the_tool_name() {
        use mediaa_agent::assistant::step_title::caption;
        for tool in ["search", "read", "outline", "time", "say", "recordings"] {
            assert_eq!(
                shown_in("ru", tool, ""),
                caption(tool, false, false),
                "{tool}"
            );
            assert_eq!(
                shown_in("en", tool, ""),
                caption(tool, false, true),
                "{tool}"
            );
            assert_ne!(shown_in("en", tool, tool), tool, "{tool}");
            assert_ne!(shown_in("ru", tool, ""), tool);
        }
        assert_eq!(shown_in("en", "search", "Поиск"), "Search");
        assert_eq!(
            shown_in("en", "outline", "Разбор темы"),
            caption("outline", true, true)
        );
        assert_eq!(
            shown_in("ru", "outline", "Topic breakdown"),
            caption("outline", true, false)
        );
        assert_eq!(shown_in("ru", "search", "Ищу"), "Ищу");
        assert_eq!(shown_in("en", "search", "Ищу"), "Ищу");
        assert_eq!(shown_in("en", "nope", "nope"), caption("nope", false, true));
    }

    #[test]
    fn a_draft_and_its_check_fold_above_the_answer() {
        let draft = TimelineEntry::Step {
            title: "Черновик ответа".into(),
            tool: "draft".into(),
            status: "found".into(),
        };
        let verify = TimelineEntry::Step {
            title: "Проверка ответа".into(),
            tool: "verify".into(),
            status: "found".into(),
        };
        let entries = [draft, verify, answer("В пятницу.", false)];
        let done = blocks(&entries, true);
        assert!(
            matches!(&done[0], Block::Folded(rows) if rows.len() == 2 && matches!(rows[0], Row::Step { tool: "draft", .. }) && matches!(rows[1], Row::Step { tool: "verify", .. }))
        );
        assert_eq!(done[1], Block::Answer("В пятницу."));
    }
}
