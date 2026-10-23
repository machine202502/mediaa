use crate::theme;
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_engine::{Engine, Health, ModelInfo, Status};
use rust_i18n::t;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub const POLL_EVERY: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub health: Health,
    pub status: Status,
    pub models: Vec<ModelInfo>,
}

#[derive(Default)]
pub struct EngineState {
    answer: Option<Result<Snapshot, String>>,
    run: Option<Receiver<Result<Snapshot, String>>>,
    next: Option<Instant>,
    refresh: bool,
}

impl EngineState {
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
                    self.answer = Some(Err(t!("agent.models_no_answer").to_string()));
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

    pub fn refresh_now(&mut self) {
        self.refresh = true;
    }

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

    pub fn models(&self) -> &[ModelInfo] {
        self.snapshot().map_or(&[], |s| s.models.as_slice())
    }

    pub fn error(&self) -> Option<&str> {
        match &self.answer {
            Some(Err(e)) => Some(e),
            _ => None,
        }
    }

    pub fn loading(&self) -> bool {
        self.answer.is_none()
    }

    pub fn online(&self) -> bool {
        self.snapshot().is_some()
    }

    pub fn offline_hint(&self, ui: &mut egui::Ui) -> bool {
        if self.online() {
            return false;
        }
        if self.loading() {
            ui.horizontal(|ui| {
                ui.spinner();
                theme::hint(ui, t!("agent.models_loading"));
            });
            return true;
        }
        let p = theme::palette(ui);
        let text = t!("agent.models_unavailable", error = self.error().unwrap_or_default());
        ui.add(egui::Label::new(RichText::new(format!("{}  {text}", icon::WARNING)).color(p.danger)).wrap());
        true
    }
}

pub fn gb(bytes: u64) -> String {
    format!("{:.1}", bytes as f64 / f64::from(1u32 << 30))
}

pub fn download_text(done: u64, total: u64) -> String {
    t!("agent.downloading", done = gb(done), total = gb(total)).into()
}

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
