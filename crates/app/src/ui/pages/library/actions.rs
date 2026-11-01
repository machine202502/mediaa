use crate::adapters::clipboard_worker::Purpose;
use crate::logic::library::cleanup::{CleanupPlan, CleanupReport};
use crate::logic::library::viewer::Viewer;
use crate::logic::library::LibraryAction;
use crate::ui::pages::ctx::Ctx;
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa_core::library::human_size;
use mediaa_core::naming;
use rust_i18n::t;
use std::path::PathBuf;

pub(crate) fn run_action(ctx: &mut Ctx<'_>, gui: &egui::Context, action: LibraryAction) {
    let res: anyhow::Result<Option<String>> = match action {
        LibraryAction::Open(p) => {
            ctx.open_path(&p);
            Ok(None)
        }
        LibraryAction::View(p, kind, info) => {
            ctx.previews.stop();
            if let Some(mut viewer) = ctx.lib.viewer.take() {
                viewer.cancel_pending();
            }
            ctx.lib.viewer = Some(Viewer::open(
                &p,
                kind,
                info.and_then(|d| d.duration_secs),
                info.is_some_and(|d| d.audio),
                ctx.settings.viewer_volume,
            ));
            Ok(None)
        }
        LibraryAction::Reveal(p) => {
            ctx.reveal_path(p);
            Ok(None)
        }
        LibraryAction::CopyFile(p) => ctx
            .clipboard
            .handle()
            .copy_files(vec![p], Purpose::Toast(t!("lib.copied_file").into_owned()))
            .map(|_| None),
        LibraryAction::CopyFiles(paths) => ctx
            .clipboard
            .handle()
            .copy_files(paths, Purpose::Toast(t!("lib.copied_file").into_owned()))
            .map(|_| None),
        LibraryAction::CopyPaths(paths) => ctx
            .clipboard
            .handle()
            .copy_text(
                paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("\n"),
                Purpose::Toast(t!("lib.copied_path").into_owned()),
            )
            .map(|_| None),
        LibraryAction::CopyImage(p) => {
            if ctx.image_io_busy() {
                ctx.warning(t!("toast.finish_current"));
                return;
            }
            let gui2 = gui.clone();
            let success = t!("lib.copied_image").into_owned();
            let clipboard = ctx.clipboard.handle();
            match ctx.capture.start_copy_image(move || {
                let result = image::image_dimensions(&p)
                    .map_err(anyhow::Error::from)
                    .and_then(|(width, height)| {
                        mediaa_core::formats::ensure_safe_dimensions(width, height)?;
                        image::open(&p).map_err(anyhow::Error::from)
                    })
                    .and_then(|image| {
                        clipboard
                            .copy_image_wait(mediaa_core::Frame::from_image(image.to_rgba8()))
                            .map_err(anyhow::Error::msg)
                    })
                    .map(|()| success)
                    .map_err(|error| format!("{error:#}"));
                gui2.request_repaint();
                result
            }) {
                Ok(()) => Ok(None),
                Err(error) => Err(error),
            }
        }
        LibraryAction::CopyPath(p) => ctx
            .clipboard
            .handle()
            .copy_text(
                p.to_string_lossy().into_owned(),
                Purpose::Toast(t!("lib.copied_path").into_owned()),
            )
            .map(|_| None),
        LibraryAction::CopyLink(url) => ctx
            .clipboard
            .handle()
            .copy_text(url, Purpose::Toast(t!("lib.copied_link").into_owned()))
            .map(|_| None),
        LibraryAction::Convert(p) => {
            ctx.open_in_convert(p);
            Ok(None)
        }
        LibraryAction::ConvertMany(paths) => {
            ctx.open_in_convert_many(paths);
            Ok(None)
        }
        LibraryAction::AskDelete(p) => {
            ctx.lib.confirm_delete = Some(p);
            Ok(None)
        }
        LibraryAction::AskDeleteMany(paths) => {
            ctx.lib.confirm_delete_many = Some(paths);
            Ok(None)
        }
        LibraryAction::AskRenameMany(paths) => {
            ctx.lib.renaming_many = Some((paths, String::new()));
            Ok(None)
        }
        LibraryAction::SetViewerVolume(volume) => {
            ctx.settings.viewer_volume = volume.clamp(0.0, 1.0);
            ctx.settings_changed();
            Ok(None)
        }
        LibraryAction::ToggleSelect(path) => {
            ctx.lib.toggle_selected(&path);
            Ok(None)
        }
        LibraryAction::AskRename(p, title) => {
            ctx.lib.renaming = Some((p, title));
            Ok(None)
        }
        LibraryAction::AskCleanup(plan) => {
            ctx.lib.confirm_cleanup = Some(plan);
            Ok(None)
        }
    };
    match res {
        Ok(Some(msg)) => ctx.success(msg),
        Ok(None) => {}
        Err(e) => ctx.error(format!("{e:#}")),
    }
}

pub(super) fn delete_dialog(ctx: &mut Ctx<'_>, gui: &egui::Context) {
    let Some(path) = ctx.lib.confirm_delete.clone() else {
        return;
    };
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("confirm-delete")).show(gui, |ui| {
        ui.set_width(340.0);
        theme::popup_title(ui, &t!("lib.delete_title"));
        ui.label(t!("lib.delete_text", name = name));
        ui.add_space(6.0);
        let confirm = t!("lib.delete_confirm").to_string();
        let keep = t!("lib.delete_keep").to_string();
        let responses = theme::centered_buttons(
            ui,
            &[
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Danger,
                    icon: icon::TRASH,
                    label: &confirm,
                    enabled: true,
                },
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Secondary,
                    icon: icon::X,
                    label: &keep,
                    enabled: true,
                },
            ],
        );
        if responses[0].clicked() {
            decision = Some(true);
        }
        if responses[1].clicked() {
            decision = Some(false);
        }
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => {
            ctx.lib.confirm_delete = None;
            if let Err(error) = ctx.lib.start_delete(path) {
                ctx.error(format!("{error:#}"));
            }
        }
        Some(false) => ctx.lib.confirm_delete = None,
        None => {}
    }
}

pub(super) fn cleanup_dialog(ctx: &mut Ctx<'_>, gui: &egui::Context) {
    let Some(plan) = ctx.lib.confirm_cleanup.clone() else {
        return;
    };
    match cleanup_modal(gui, &plan) {
        Some(true) => {
            ctx.lib.confirm_cleanup = None;
            if let Err(error) = ctx.lib.start_cleanup(&plan) {
                ctx.error(format!("{error:#}"));
            }
        }
        Some(false) => ctx.lib.confirm_cleanup = None,
        None => {}
    }
}

pub(crate) fn cleanup_modal(gui: &egui::Context, plan: &CleanupPlan) -> Option<bool> {
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("confirm-cleanup")).show(gui, |ui| {
        ui.set_width(410.0);
        theme::popup_title(ui, &plan.older.label());
        ui.label(t!(
            "lib.cleanup_text",
            count = plan.items.len(),
            size = human_size(plan.bytes())
        ));
        ui.add_space(6.0);
        theme::centered_row_sized(ui, 185.0, |ui| {
            if ui
                .add_enabled(
                    !plan.items.is_empty(),
                    theme::danger(ui, icon::CALENDAR_X, &t!("lib.cleanup_confirm")),
                )
                .clicked()
            {
                decision = Some(true);
            }
            if ui
                .add(theme::secondary(icon::X, &t!("common.cancel")))
                .clicked()
            {
                decision = Some(false);
            }
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    decision
}

pub(super) fn apply_cleanup_report(ctx: &mut Ctx<'_>, report: &CleanupReport) {
    for item in &report.removed {
        ctx.thumbs.forget_cached(&item.path);
    }
    let removed: Vec<PathBuf> = report
        .removed
        .iter()
        .map(|item| item.path.clone())
        .collect();
    if !removed.is_empty() {
        ctx.lib.remove_selected(&removed);
        ctx.lib.files_changed(&removed);
    }
}

pub(super) fn finish_cleanup(ctx: &mut Ctx<'_>, report: CleanupReport, success: String) {
    let details = report
        .failures
        .iter()
        .map(|failure| format!("{}: {}", failure.path.display(), failure.error))
        .collect::<Vec<_>>()
        .join("\n");
    if report.removed.is_empty() && report.skipped > 0 && report.failures.is_empty() {
        ctx.info(t!("lib.cleanup_changed", count = report.skipped));
    } else if report.removed.is_empty() {
        ctx.error(t!(
            "lib.cleanup_failed",
            failed = report.failures.len(),
            details = details
        ));
    } else if report.skipped > 0 {
        ctx.warning(t!(
            "lib.cleanup_skipped",
            count = report.removed.len(),
            skipped = report.skipped,
            size = human_size(report.bytes()),
            failed = report.failures.len(),
            details = details
        ));
    } else if report.failures.is_empty() {
        ctx.success(success);
    } else {
        ctx.warning(t!(
            "lib.cleanup_partial",
            count = report.removed.len(),
            size = human_size(report.bytes()),
            failed = report.failures.len(),
            details = details
        ));
    }
}

pub(super) fn rename_dialog(ctx: &mut Ctx<'_>, gui: &egui::Context) {
    let Some((path, mut title)) = ctx.lib.renaming.clone() else {
        return;
    };
    let Some(current) = naming::parse_path(&path) else {
        ctx.lib.renaming = None;
        return;
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_string();
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("rename-media")).show(gui, |ui| {
        ui.set_width(380.0);
        theme::popup_title(ui, &t!("lib.rename_title"));
        let edit = ui.add(egui::TextEdit::singleline(&mut title).desired_width(f32::INFINITY));
        if !edit.has_focus() && ui.memory(|m| m.focused().is_none()) {
            edit.request_focus();
        }
        let preview = naming::MediaName {
            title: naming::sanitize(&title, naming::MAX_TITLE),
            ..current.clone()
        };
        theme::hint(
            ui,
            t!(
                "lib.rename_file",
                name = format!("{}.{ext}", preview.stem())
            ),
        );
        ui.add_space(6.0);
        theme::centered_row_sized(ui, 158.0, |ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add(theme::primary(ui, icon::CHECK, &t!("lib.rename_confirm")))
                .clicked()
                || enter
            {
                decision = Some(true);
            }
            if ui
                .add(theme::secondary(icon::X, &t!("common.cancel")))
                .clicked()
            {
                decision = Some(false);
            }
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => {
            ctx.lib.renaming = None;
            ctx.rename_media(path, title);
        }
        Some(false) => ctx.lib.renaming = None,
        None => ctx.lib.renaming = Some((path, title)),
    }
}

pub(super) fn delete_many_dialog(ctx: &mut Ctx<'_>, gui: &egui::Context) {
    let Some(paths) = ctx.lib.confirm_delete_many.clone() else {
        return;
    };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("confirm-delete-many")).show(gui, |ui| {
        ui.set_width(360.0);
        theme::popup_title(ui, &t!("lib.delete_many_title"));
        ui.label(t!(
            "lib.delete_many_text",
            n = paths.len(),
            files = crate::i18n::files(paths.len())
        ));
        ui.add_space(6.0);
        theme::centered_row_sized(ui, 143.0, |ui| {
            if ui
                .add(theme::danger(ui, icon::TRASH, &t!("lib.delete_confirm")))
                .clicked()
            {
                decision = Some(true);
            }
            if ui
                .add(theme::secondary(icon::X, &t!("lib.delete_keep")))
                .clicked()
            {
                decision = Some(false);
            }
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => {
            ctx.lib.confirm_delete_many = None;
            if let Err(error) = ctx.lib.delete_many(paths) {
                ctx.error(format!("{error:#}"));
            }
        }
        Some(false) => ctx.lib.confirm_delete_many = None,
        None => ctx.lib.confirm_delete_many = Some(paths),
    }
}

pub(super) fn rename_many_dialog(ctx: &mut Ctx<'_>, gui: &egui::Context) {
    let Some((paths, mut title)) = ctx.lib.renaming_many.clone() else {
        return;
    };
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("rename-many-media")).show(gui, |ui| {
        ui.set_width(400.0);
        theme::popup_title(ui, &t!("lib.rename_many_title"));
        let edit = ui.add(egui::TextEdit::singleline(&mut title).desired_width(f32::INFINITY));
        if !edit.has_focus() && ui.memory(|m| m.focused().is_none()) {
            edit.request_focus();
        }
        theme::hint(
            ui,
            t!(
                "lib.rename_many_hint",
                n = paths.len(),
                files = crate::i18n::files(paths.len())
            ),
        );
        ui.add_space(6.0);
        theme::centered_row_sized(ui, 158.0, |ui| {
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add(theme::primary(ui, icon::CHECK, &t!("lib.rename_confirm")))
                .clicked()
                || enter
            {
                decision = Some(true);
            }
            if ui
                .add(theme::secondary(icon::X, &t!("common.cancel")))
                .clicked()
            {
                decision = Some(false);
            }
        });
    });
    if modal.should_close() && decision.is_none() {
        decision = Some(false);
    }
    match decision {
        Some(true) => {
            ctx.lib.renaming_many = None;
            let base = naming::sanitize(&title, naming::MAX_TITLE);
            for (index, path) in paths.into_iter().enumerate() {
                ctx.rename_media(path, format!("{base} {}", index + 1));
            }
        }
        Some(false) => ctx.lib.renaming_many = None,
        None => ctx.lib.renaming_many = Some((paths, title)),
    }
}

#[cfg(test)]
mod tests {
    use super::cleanup_modal;
    use crate::logic::library::cleanup::{plan_cleanup, CleanupOlder};
    use mediaa_core::library::LibraryEntry;
    use std::time::{Duration, SystemTime};

    fn entry(path: std::path::PathBuf, modified: SystemTime) -> LibraryEntry {
        LibraryEntry {
            name: path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
            title: "Recording".into(),
            path,
            kind: mediaa_core::MediaKind::Video,
            size: 1024,
            modified,
            taken: chrono::NaiveDateTime::default(),
            details: None,
        }
    }
    #[test]
    fn cleanup_modal_shows_the_scope_and_can_be_cancelled() {
        let _ui = crate::test_support::ui_test_guard();
        use egui_kittest::kittest::Queryable;
        if let Ok(locale) = std::env::var("MEDIAA_LIBRARY_MODAL_LOCALE") {
            rust_i18n::set_locale(&locale);
        }
        let plan = plan_cleanup(
            &[entry(
                "old.webp".into(),
                SystemTime::now()
                    .checked_sub(Duration::from_secs(31 * 24 * 60 * 60))
                    .unwrap(),
            )],
            CleanupOlder::Month,
            SystemTime::now(),
        );
        let mut harness = egui_kittest::Harness::new_ui_state(
            |ui: &mut eframe::egui::Ui, decision: &mut Option<bool>| {
                if let Some(value) = cleanup_modal(ui.ctx(), &plan) {
                    *decision = Some(value);
                }
            },
            None,
        );
        crate::ui::theme::install(&harness.ctx);
        harness.run();
        assert!(harness
            .query_by_label(&rust_i18n::t!("lib.cleanup_month"))
            .is_some());
        assert!(harness
            .query_by_label(&rust_i18n::t!(
                "lib.cleanup_text",
                count = 1,
                size = "1.0 KB"
            ))
            .is_some());
        if let Ok(path) = std::env::var("MEDIAA_LIBRARY_MODAL_SNAPSHOT") {
            harness.render().unwrap().save(path).unwrap();
        }
        harness
            .get_by_label_contains(&rust_i18n::t!("common.cancel"))
            .click();
        harness.run();
        assert_eq!(harness.state(), &Some(false));
    }
}
