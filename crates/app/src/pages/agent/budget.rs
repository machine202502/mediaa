use super::super::header;
use crate::app::App;
use crate::theme;
use eframe::egui;
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
        ui.scope(|ui| theme::section(ui, icon::STACK, &t!("budget.at_once")))
            .response
            .on_hover_text(t!("budget.at_once_hint"));
        for kind in WorkKind::ALL {
            theme::row(ui, &kind_label(kind), |ui| {
                changed |= ui
                    .add(egui::DragValue::new(budget.limit_mut(kind)).range(1..=16))
                    .changed();
            });
        }
    });
    if changed {
        app.settings_changed();
    }
}
