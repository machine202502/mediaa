//! One state of the engine for the whole Agent section: a background poller asks for its health,
//! status and downloaded models every [`POLL_EVERY`] and keeps the last answer or error. Every
//! page reads it; no page asks the engine on its own.

use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
use egui_phosphor::regular as icon;
use mediaa_engine::{Engine, Health, ModelInfo, Status};
use rust_i18n::t;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the engine is asked while it runs.
pub const POLL_EVERY: Duration = Duration::from_secs(3);

/// What the engine answered: its description, the loaded and waiting models, and the models it
/// has downloaded.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub health: Health,
    pub status: Status,
    pub models: Vec<ModelInfo>,
}

#[derive(Default)]
pub struct EngineState {
    /// The last answer, or why there is none.
    answer: Option<Result<Snapshot, String>>,
    run: Option<Receiver<Result<Snapshot, String>>>,
    next: Option<Instant>,
    /// Ask again as soon as the current request finishes.
    refresh: bool,
}

impl EngineState {
    /// Collects a finished answer and starts the next request when it is time. Called every
    /// frame; never waits for the engine.
    pub(super) fn tick(&mut self, engine: &Arc<dyn Engine>, ctx: &egui::Context) {
        if let Some(rx) = &self.run {
            match rx.try_recv() {
                Ok(answer) => {
                    self.answer = Some(answer);
                    self.run = None;
                    self.next = Some(Instant::now() + POLL_EVERY);
                }
                Err(crossbeam_channel::TryRecvError::Empty) => ctx.request_repaint_after(Duration::from_millis(200)),
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.answer = Some(Err(t!("engine.no_answer").to_string()));
                    self.run = None;
                    self.next = Some(Instant::now() + POLL_EVERY);
                }
            }
        }
        if self.run.is_none() && (self.refresh || self.next.is_none_or(|t| Instant::now() >= t)) {
            self.refresh = false;
            let engine = engine.clone();
            let (tx, rx) = crossbeam_channel::bounded(1);
            let _ = std::thread::Builder::new().name("engine-state".into()).spawn(move || {
                let answer = engine
                    .models()
                    .map(|models| Snapshot { health: engine.health(), status: engine.status(), models })
                    .map_err(|e| format!("{e:#}"));
                let _ = tx.send(answer);
            });
            self.run = Some(rx);
        }
        if self.run.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        } else if let Some(next) = self.next {
            ctx.request_repaint_after(next.saturating_duration_since(Instant::now()).max(Duration::from_millis(50)));
        }
    }

    /// Asks again now (after a download or a delete, for instance).
    pub fn refresh_now(&mut self) {
        self.refresh = true;
    }

    /// The last successful answer (kept while the next one is on its way).
    pub fn snapshot(&self) -> Option<&Snapshot> {
        match &self.answer {
            Some(Ok(s)) => Some(s),
            _ => None,
        }
    }

    pub fn health(&self) -> Option<&Health> {
        self.snapshot().map(|s| &s.health)
    }

    pub fn status(&self) -> Option<&Status> {
        self.snapshot().map(|s| &s.status)
    }

    /// Models the engine has downloaded; empty while it is not reachable.
    pub fn models(&self) -> &[ModelInfo] {
        self.snapshot().map_or(&[], |s| s.models.as_slice())
    }

    /// Why the engine did not answer (or did not answer correctly).
    pub fn error(&self) -> Option<&str> {
        match &self.answer {
            Some(Err(e)) => Some(e),
            _ => None,
        }
    }

    /// No answer yet: the first request is still on its way.
    pub fn loading(&self) -> bool {
        self.answer.is_none()
    }

    /// True when the engine has answered at least once and is reachable now.
    pub fn online(&self) -> bool {
        self.snapshot().is_some()
    }

    /// The engine chip of the Agent header: green when it works, red when it does not.
    pub fn chip(&self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        let (color, text, tip) = if self.loading() {
            (p.text_weak, t!("engine.checking").to_string(), None)
        } else if self.online() {
            (p.success, t!("engine.running").to_string(), Some(self.running_tooltip()))
        } else {
            let error = self.error().unwrap_or_default();
            (p.danger, t!("engine.not_running").to_string(), Some(t!("engine.failed", error = error).to_string()))
        };
        let resp = egui::Frame::new()
            .fill(p.card)
            .stroke(Stroke::new(1.0, p.border))
            .corner_radius(CornerRadius::same(10))
            .inner_margin(Margin::symmetric(8, 2))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter().circle_filled(rect.center(), 4.0, color);
                    ui.label(RichText::new(text).size(12.0).color(p.text));
                });
            })
            .response;
        if let Some(tip) = tip {
            resp.on_hover_text(tip);
        }
    }

    /// What every page shows while the engine does not work; `true` when it did not.
    pub fn offline_hint(&self, ui: &mut egui::Ui) -> bool {
        if self.online() {
            return false;
        }
        if self.loading() {
            ui.horizontal(|ui| {
                ui.spinner();
                theme::hint(ui, t!("engine.checking"));
            });
            return true;
        }
        let p = theme::palette(ui);
        let text = t!("engine.failed", error = self.error().unwrap_or_default());
        ui.add(egui::Label::new(RichText::new(format!("{}  {text}", icon::WARNING)).color(p.danger)).wrap());
        true
    }

    /// Version, GPUs and the models folder of the engine.
    fn running_tooltip(&self) -> String {
        let Some(s) = self.snapshot() else {
            return String::new();
        };
        let mut lines = vec![format!("{} · {}", s.health.name, t!("engine.version", version = s.health.version))];
        if s.health.gpus.is_empty() {
            lines.push(t!("engine.no_gpus").to_string());
        }
        for g in &s.health.gpus {
            lines.push(format!("{} · {} · {} GB", g.name, g.backend, gb(g.memory_total)));
        }
        if !s.health.models_dir.is_empty() {
            lines.push(format!("{}: {}", t!("engine.models_dir"), s.health.models_dir));
        }
        lines.join("\n")
    }
}

/// Gigabytes with one decimal (`1.5`), as the pages print sizes.
pub fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / f64::from(1u32 << 30))
}

/// «Скачивание N из M ГБ».
pub fn download_text(done: u64, total: u64) -> String {
    t!("agent.downloading", done = gb(done), total = gb(total)).into()
}

/// The download's bar: its fill (0…1) and its text. Nothing downloaded yet: an empty bar and
/// «скачивание…» instead of a size.
pub fn download_bar(done: u64, total: u64) -> (f32, String) {
    if total == 0 {
        return (0.0, t!("agent.downloading_wait").into());
    }
    let fill = (done as f64 / total as f64).clamp(0.0, 1.0) as f32;
    (fill, download_text(done, total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gigabytes_have_one_decimal() {
        assert_eq!(gb(0), "0.0");
        assert_eq!(gb(1 << 30), "1.0");
        assert_eq!(gb(3 << 29), "1.5");
    }

    #[test]
    fn download_progress_is_byte_based() {
        let (fill, text) = download_bar(1 << 29, 1 << 30);
        assert!((fill - 0.5).abs() < 1e-6, "{fill}");
        assert_eq!(text, t!("agent.downloading", done = "0.5", total = "1.0", locale = "en"));
        assert_eq!(text, "Downloading 0.5 of 1.0 GB", "the locale key is the one the UI uses");
        let (fill, text) = download_bar(10, 0);
        assert_eq!(fill, 0.0, "an unknown total is an empty bar");
        assert_eq!(text, t!("agent.downloading_wait", locale = "en"));
        assert_eq!(download_bar(2 << 30, 1 << 30).0, 1.0, "never more than full");
    }
}
