//! Queue: the neural-network tasks of the library (`.ai/queue.db`) with their status,
//! progress and the checkpoint they continue from after a crash or a restart; pause, resume,
//! start over, remove.

use super::super::header;
use super::super::library::library_dir;
use crate::app::App;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_agent::scheduler::{self, Pool};
use mediaa_core::agent::{Budget, WorkKind};
use mediaa_core::catalogs::ai_dir;
use mediaa_core::queue::{Queue, Task, TaskId, TaskStatus};
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How often the list is re-read while the page is open.
const REFRESH: Duration = Duration::from_secs(1);

#[derive(Default)]
pub struct QueueUi {
    queue: Option<(PathBuf, Result<Queue, String>)>,
    tasks: Vec<Task>,
    read: Option<Instant>,
}

enum Op {
    Pause(TaskId),
    Resume(TaskId),
    Restart(TaskId),
    Remove(TaskId),
    ClearDone,
}

fn status_label(s: TaskStatus) -> (&'static str, String) {
    match s {
        TaskStatus::Queued => (icon::CLOCK, t!("queue.queued").into()),
        TaskStatus::Running => (icon::SPINNER_GAP, t!("queue.running").into()),
        TaskStatus::Paused => (icon::PAUSE_CIRCLE, t!("queue.paused").into()),
        TaskStatus::Done => (icon::CHECK_CIRCLE, t!("queue.done").into()),
        TaskStatus::Failed => (icon::WARNING_CIRCLE, t!("queue.failed").into()),
    }
}

/// The checkpoint, short (`at 12:30`-style JSON, cut to one line).
fn cursor_text(task: &Task) -> Option<String> {
    if task.cursor.is_null() {
        return None;
    }
    let s = task.cursor.to_string();
    Some(if s.chars().count() > 80 { format!("{}…", s.chars().take(80).collect::<String>()) } else { s })
}

fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / f64::from(1u32 << 30))
}

/// Memory taken against its budget: the bar's fill and `4.1 / 8.0 GB`.
fn memory_bar(used: u64, max: u64) -> (f32, String) {
    let fill = if max == 0 { 1.0 } else { (used as f64 / max as f64).clamp(0.0, 1.0) as f32 };
    (fill, format!("{} / {} GB", gb(used), gb(max)))
}

/// Per kind of work: (kind, jobs running, its limit, jobs waiting).
fn slots(status: &scheduler::Status, budget: &Budget) -> Vec<(WorkKind, u32, u32, usize)> {
    WorkKind::ALL
        .iter()
        .map(|&k| {
            let running = status.running.iter().find(|(r, _)| *r == k).map_or(0, |(_, n)| *n);
            (k, running, budget.limit(k), status.waiting.iter().filter(|(_, w, _)| *w == k).count())
        })
        .collect()
}

/// A loaded model: its file name, where it runs, its size and how many jobs use it.
fn model_line(m: &scheduler::LoadedModel) -> String {
    let path = m.key.rsplit_once('#').map_or(m.key.as_str(), |(p, _)| p);
    let name = Path::new(path).file_name().map_or(path.into(), |n| n.to_string_lossy());
    let pool = match m.pool {
        Pool::Gpu => t!("queue.on_gpu"),
        Pool::Cpu => t!("queue.on_cpu"),
    };
    format!("{name} · {pool} · {} GB · {}", gb(m.bytes), t!("queue.jobs", n = m.leases))
}

/// The budget in use: memory taken by models, the models, and the slots of each kind of work.
fn runtime_card(ui: &mut egui::Ui, status: &scheduler::Status, budget: &Budget) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::GAUGE, &t!("queue.budget"));
        let p = theme::palette(ui);
        egui::Grid::new("queue-budget").num_columns(3).spacing([12.0, 6.0]).show(ui, |ui| {
            for (label, used, max) in [(t!("budget.gpu_memory"), status.gpu_used, budget.gpu_bytes()), (t!("budget.ram_memory"), status.ram_used, budget.ram_bytes())] {
                let (fill, text) = memory_bar(used, max);
                ui.label(label);
                let color = if fill >= 0.95 { p.danger } else { p.accent };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(220.0, 8.0), egui::Sense::hover());
                ui.painter().rect_filled(rect, 4.0, p.accent_soft);
                if fill > 0.0 {
                    let done = egui::Rect::from_min_size(rect.min, egui::vec2((rect.width() * fill).max(rect.height()), rect.height()));
                    ui.painter().rect_filled(done, 4.0, color);
                }
                theme::hint(ui, text);
                ui.end_row();
            }
            for (kind, running, limit, waiting) in slots(status, budget) {
                ui.label(super::budget::kind_label(kind));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 3.0;
                    for i in 0..limit.max(running) {
                        let busy = i < running;
                        ui.label(RichText::new(if busy { icon::CIRCLE } else { icon::CIRCLE_DASHED }).color(if busy { p.accent } else { p.text_weak }));
                    }
                    theme::hint(ui, t!("queue.slots", busy = running, max = limit));
                });
                if waiting > 0 {
                    ui.label(RichText::new(t!("queue.waits", n = waiting)).color(p.warning));
                } else {
                    ui.label("");
                }
                ui.end_row();
            }
        });
        if !status.loaded.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(format!("{}  {}", icon::MEMORY, t!("queue.models"))).strong());
            for m in &status.loaded {
                theme::hint(ui, model_line(m));
            }
        }
    });
    ui.add_space(4.0);
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("queue.title"), "");
    let dir = ai_dir(&library_dir(&app.settings));
    let budget = app.settings.agent.budget.clone();
    let status = app.agent.runtime(&dir, &budget).scheduler().status();
    runtime_card(ui, &status, &budget);
    let state = &mut app.agent.queue;
    if state.queue.as_ref().is_none_or(|(d, _)| *d != dir) {
        state.queue = Some((dir.clone(), Queue::open(&dir).map_err(|e| format!("{e:#}"))));
        state.read = None;
    }
    let Some((_, opened)) = &state.queue else { return };
    let queue = match opened {
        Ok(q) => q,
        Err(e) => {
            ui.colored_label(theme::palette(ui).danger, e);
            return;
        }
    };
    if state.read.is_none_or(|r| r.elapsed() >= REFRESH) {
        match queue.list() {
            Ok(t) => state.tasks = t,
            Err(e) => log::warn!("agent queue: {e:#}"),
        }
        state.read = Some(Instant::now());
    }
    ui.ctx().request_repaint_after(REFRESH);

    let mut op = None;
    if state.tasks.iter().any(|t| t.status == TaskStatus::Done) && ui.add(theme::secondary(icon::BROOM, &t!("queue.clear_done"))).clicked() {
        op = Some(Op::ClearDone);
    }
    if state.tasks.is_empty() {
        theme::hint(ui, t!("queue.empty"));
    }
    ui.add_space(4.0);
    for task in &state.tasks {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let p = theme::palette(ui);
            ui.horizontal(|ui| {
                let (ic, label) = status_label(task.status);
                let color = match task.status {
                    TaskStatus::Failed => p.danger,
                    TaskStatus::Done => p.success,
                    TaskStatus::Running => p.accent,
                    _ => p.text_weak,
                };
                ui.label(RichText::new(format!("{ic}  {label}")).color(color));
                ui.label(RichText::new(&task.title).strong());
                theme::hint(ui, &task.kind);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::TRASH, &t!("queue.remove")).clicked() {
                        op = Some(Op::Remove(task.id));
                    }
                    if task.status != TaskStatus::Running && theme::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, &t!("queue.restart")).clicked() {
                        op = Some(Op::Restart(task.id));
                    }
                    match task.status {
                        TaskStatus::Queued | TaskStatus::Running => {
                            if theme::icon_button(ui, icon::PAUSE, &t!("queue.pause")).clicked() {
                                op = Some(Op::Pause(task.id));
                            }
                        }
                        TaskStatus::Paused | TaskStatus::Failed => {
                            if theme::icon_button(ui, icon::PLAY, &t!("queue.resume")).clicked() {
                                op = Some(Op::Resume(task.id));
                            }
                        }
                        TaskStatus::Done => {}
                    }
                });
            });
            if task.status != TaskStatus::Done && task.progress > 0.0 {
                ui.add(egui::ProgressBar::new(task.progress).desired_width(ui.available_width()).show_percentage());
            }
            let mut facts = vec![t!("queue.attempts", n = task.attempts).to_string()];
            if let Some(c) = cursor_text(task) {
                facts.push(t!("queue.checkpoint", at = c).to_string());
            }
            theme::hint(ui, facts.join(" · "));
            if let Some(e) = &task.error {
                ui.add(egui::Label::new(RichText::new(e).color(p.danger)).wrap());
            }
        });
        ui.add_space(4.0);
    }

    if let Some(op) = op {
        let r = match op {
            Op::Pause(id) => queue.pause(id),
            Op::Resume(id) => queue.resume(id),
            Op::Restart(id) => queue.restart(id),
            Op::Remove(id) => queue.remove(id),
            Op::ClearDone => queue.clear_done(),
        };
        state.read = None;
        if let Err(e) = r {
            app.error(format!("{e:#}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_in_use() {
        let status = scheduler::Status {
            loaded: vec![scheduler::LoadedModel { key: "/m/.ai/models/q/Qwen3-Q4_K_M.gguf#4096".into(), pool: Pool::Gpu, bytes: 3 << 30, leases: 2 }],
            running: vec![(WorkKind::Agent, 1), (WorkKind::WebSearch, 2)],
            waiting: vec![(7, WorkKind::WebSearch, scheduler::Wait::Slot(WorkKind::WebSearch)), (8, WorkKind::Transcription, scheduler::Wait::Turn)],
            gpu_used: 3 << 30,
            ram_used: 0,
        };
        let budget = Budget::default();
        assert_eq!(memory_bar(status.gpu_used, budget.gpu_bytes()), (0.375, "3.0 / 8.0 GB".to_string()));
        assert_eq!(memory_bar(5, 0).0, 1.0, "no budget: full");
        let s = slots(&status, &budget);
        assert_eq!(s.len(), WorkKind::ALL.len());
        assert!(s.contains(&(WorkKind::Agent, 1, 2, 0)) && s.contains(&(WorkKind::WebSearch, 2, 2, 1)) && s.contains(&(WorkKind::Transcription, 0, 1, 1)), "{s:?}");
        let line = model_line(&status.loaded[0]);
        assert!(line.starts_with("Qwen3-Q4_K_M.gguf · ") && line.contains(" · 3.0 GB · ") && line.ends_with('2'), "{line}");
    }

    #[test]
    fn checkpoints_are_shown_short() {
        let dir = tempfile::tempdir().unwrap();
        let q = Queue::open(dir.path()).unwrap();
        let id = q.enqueue("transcribe", "t", &serde_json::json!({}), 0).unwrap();
        assert_eq!(cursor_text(&q.get(id).unwrap().unwrap()), None, "not started");
        q.claim(&["transcribe"]).unwrap();
        q.checkpoint(id, &serde_json::json!({"second": 754.5}), &serde_json::Value::Null, 0.4).unwrap();
        assert_eq!(cursor_text(&q.get(id).unwrap().unwrap()).as_deref(), Some(r#"{"second":754.5}"#));
        q.checkpoint(id, &serde_json::json!("x".repeat(200)), &serde_json::Value::Null, 0.5).unwrap();
        assert!(cursor_text(&q.get(id).unwrap().unwrap()).unwrap().ends_with('…'));
    }
}
