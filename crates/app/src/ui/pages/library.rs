pub(crate) mod actions;
pub(crate) mod controller;
pub(crate) mod grid;
pub(crate) mod hover_preview;
pub(crate) mod thumbs;
pub(crate) mod viewer;

use crate::ui::pages::ctx::Ctx;
use eframe::egui;

pub(crate) fn dialogs(ctx: &mut Ctx<'_>, gui: &egui::Context) {
    controller::dialogs(ctx, gui);
}

pub(crate) fn apply_action(
    ctx: &mut Ctx<'_>,
    gui: &egui::Context,
    action: crate::logic::library::LibraryAction,
) {
    controller::apply_action(ctx, gui, action);
}

pub(crate) fn poll_cleanup(ctx: &mut Ctx<'_>) {
    controller::poll_cleanup(ctx);
}
