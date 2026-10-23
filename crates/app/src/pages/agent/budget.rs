use super::super::header;
use super::engine_state::{gb, EngineState};
use crate::app::App;
use crate::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::agent::WorkKind;
use rust_i18n::t;

pub(super) fn kind_label(k: WorkKind) -> String {
    match k {
        WorkKind::Agent => t!("budget.kind_agent"),
        WorkKind::Transcription => t!("budget.kind_transcription"),
        WorkKind::Indexing => t!("budget.kind_indexing"),
        WorkKind::WebSearch => t!("budget.kind_web_search"),
        WorkKind::ProviderCalls => t!("budget.kind_provider_calls"),
    }
    .into()
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    header(ui, &t!("budget.title"), "");

    let mut changed = false;
    let budget = &mut app.settings.agent.budget;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::MEMORY, &t!("budget.memory"));
        theme::row(ui, &t!("budget.gpu_memory"), |ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut budget.gpu_memory_gb).range(0.0..=256.0).speed(0.25).max_decimals(1).suffix(" GB"))
                .on_hover_text(t!("budget.gpu_memory_hint"))
                .changed();
        });
        theme::row(ui, &t!("budget.ram_memory"), |ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut budget.ram_memory_gb).range(0.0..=1024.0).speed(0.25).max_decimals(1).suffix(" GB"))
                .on_hover_text(t!("budget.ram_memory_hint"))
                .changed();
        });
        theme::row(ui, &t!("budget.keep_alive"), |ui| {
            let mut minutes = minutes_of(budget.keep_alive_secs);
            if ui
                .add(egui::DragValue::new(&mut minutes).range(0..=MAX_KEEP_ALIVE_MINUTES).suffix(t!("budget.minutes")))
                .on_hover_text(t!("budget.keep_alive_hint"))
                .changed()
            {
                budget.keep_alive_secs = secs_of(minutes);
                changed = true;
            }
        });
    });
    ui.add_space(6.0);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.scope(|ui| theme::section(ui, icon::STACK, &t!("budget.at_once"))).response.on_hover_text(t!("budget.at_once_hint"));
        for kind in WorkKind::ALL {
            theme::row(ui, &kind_label(kind), |ui| {
                changed |= ui.add(egui::DragValue::new(budget.limit_mut(kind)).range(1..=16)).changed();
            });
        }
    });
    ui.add_space(6.0);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::GRAPHICS_CARD, &t!("budget.gpus"));
        gpus(ui, &app.agent.engine_state);
    });
    if changed {
        app.settings_changed();
    }
}

fn gpus(ui: &mut egui::Ui, engine: &EngineState) {
    if engine.loading() {
        ui.spinner();
        return;
    }
    let Some(health) = engine.health() else {
        engine.offline_hint(ui);
        return;
    };
    if health.gpus.is_empty() {
        theme::hint(ui, t!("budget.no_gpus"));
        return;
    }
    for g in &health.gpus {
        ui.horizontal(|ui| {
            ui.label(RichText::new(&g.name).strong());
            theme::hint(ui, t!("budget.gpu_line", backend = g.backend, total = gb(g.memory_total), free = gb(g.memory_free)));
        });
    }
}

const MAX_KEEP_ALIVE_MINUTES: u64 = 1440;

fn minutes_of(secs: u64) -> u64 {
    secs.div_ceil(60)
}

fn secs_of(minutes: u64) -> u64 {
    minutes * 60
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::agent::Budget;

    #[test]
    fn keep_alive_is_edited_in_whole_minutes() {
        assert_eq!(minutes_of(Budget::default().keep_alive_secs), 5, "the default is five minutes");
        assert_eq!(secs_of(0), 0, "zero unloads a model at once");
        assert_eq!(secs_of(minutes_of(300)), 300);
        assert_eq!(minutes_of(61), 2);
    }
}
