use super::*;

pub(super) fn run(app: &mut App, action: Action) {
    let res: anyhow::Result<Option<String>> = match action {
        Action::Open(p) => files::open(&p).map(|_| None),
        Action::View(p, kind, info) => {
            app.lib.previews.stop();
            app.lib.viewer = Some(Viewer::open(
                &p,
                kind,
                info.and_then(|d| d.duration_secs),
                info.is_some_and(|d| d.audio),
            ));
            Ok(None)
        }
        Action::Reveal(p) => files::reveal(&p).map(|_| None),
        Action::CopyFile(p) => {
            clipboard::copy_files(&[&p]).map(|_| Some(t!("lib.copied_file").into()))
        }
        Action::CopyImage(p) => image::image_dimensions(&p)
            .map_err(anyhow::Error::from)
            .and_then(|(width, height)| {
                mediaa_core::formats::ensure_safe_dimensions(width, height)?;
                image::open(&p).map_err(anyhow::Error::from)
            })
            .and_then(|img| clipboard::copy_image(&mediaa_core::Frame::from_image(img.to_rgba8())))
            .map(|_| Some(t!("lib.copied_image").into())),
        Action::CopyPath(p) => {
            clipboard::copy_text(&p.to_string_lossy()).map(|_| Some(t!("lib.copied_path").into()))
        }
        Action::CopyLink(url) => {
            clipboard::copy_text(&url).map(|_| Some(t!("lib.copied_link").into()))
        }
        Action::Convert(p) => {
            app.open_in_convert(p);
            Ok(None)
        }
        Action::AskDelete(p) => {
            app.lib.confirm_delete = Some(p);
            Ok(None)
        }
        Action::AskRename(p, title) => {
            app.lib.renaming = Some((p, title));
            Ok(None)
        }
    };
    match res {
        Ok(Some(msg)) => app.success(msg),
        Ok(None) => {}
        Err(e) => app.error(format!("{e:#}")),
    }
}

pub(super) fn delete_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(path) = app.lib.confirm_delete.clone() else {
        return;
    };
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("confirm-delete")).show(ctx, |ui| {
        ui.set_width(340.0);
        theme::popup_title(ui, &t!("lib.delete_title"));
        ui.label(t!("lib.delete_text", name = name));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
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
            app.lib.confirm_delete = None;
            let size = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or_default();
            let report = perform_cleanup(
                app,
                &[CleanupItem {
                    path: path.clone(),
                    size,
                }],
                None,
            );
            finish_cleanup(app, report, t!("lib.deleted", name = name).into());
        }
        Some(false) => app.lib.confirm_delete = None,
        None => {}
    }
}

pub(super) fn cleanup_dialog(app: &mut App, ctx: &egui::Context) {
    let Some(plan) = app.lib.confirm_cleanup.clone() else {
        return;
    };
    match cleanup_modal(ctx, &plan) {
        Some(true) => {
            app.lib.confirm_cleanup = None;
            if let Err(error) = app.lib.start_cleanup(&plan, &mut app.tasks) {
                app.error(format!("{error:#}"));
            }
        }
        Some(false) => app.lib.confirm_cleanup = None,
        None => {}
    }
}

pub(super) fn cleanup_modal(ctx: &egui::Context, plan: &CleanupPlan) -> Option<bool> {
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("confirm-cleanup")).show(ctx, |ui| {
        ui.set_width(410.0);
        theme::popup_title(ui, &plan.older.label());
        ui.label(t!(
            "lib.cleanup_text",
            count = plan.items.len(),
            size = human_size(plan.bytes())
        ));
        ui.add_space(6.0);
        ui.horizontal(|ui| {
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

fn perform_cleanup(
    app: &mut App,
    items: &[CleanupItem],
    older: Option<CleanupOlder>,
) -> CleanupReport {
    app.lib.prepare_for_removal(items);
    let report = match older {
        Some(older) => cleanup_old_selected(items, older, SystemTime::now(), files::move_to_trash),
        None => cleanup_selected(items, files::move_to_trash),
    };
    apply_cleanup_report(app, &report);
    report
}

pub(super) fn apply_cleanup_report(app: &mut App, report: &CleanupReport) {
    for item in &report.removed {
        app.thumbs.forget(&item.path, item.proxy.clone());
    }
    let removed: Vec<PathBuf> = report
        .removed
        .iter()
        .map(|item| item.path.clone())
        .collect();
    if !removed.is_empty() {
        app.lib.files_changed(&removed);
    }
}

pub(super) fn finish_cleanup(app: &mut App, report: CleanupReport, success: String) {
    let details = report
        .failures
        .iter()
        .map(|failure| format!("{}: {}", failure.path.display(), failure.error))
        .collect::<Vec<_>>()
        .join("\n");
    if report.removed.is_empty() && report.skipped > 0 && report.failures.is_empty() {
        app.info(t!("lib.cleanup_changed", count = report.skipped));
    } else if report.removed.is_empty() {
        app.error(t!(
            "lib.cleanup_failed",
            failed = report.failures.len(),
            details = details
        ));
    } else if report.skipped > 0 {
        app.warning(t!(
            "lib.cleanup_skipped",
            count = report.removed.len(),
            skipped = report.skipped,
            size = human_size(report.bytes()),
            failed = report.failures.len(),
            details = details
        ));
    } else if report.failures.is_empty() {
        app.success(success);
    } else {
        app.warning(t!(
            "lib.cleanup_partial",
            count = report.removed.len(),
            size = human_size(report.bytes()),
            failed = report.failures.len(),
            details = details
        ));
    }
}

pub(super) fn rename_dialog(app: &mut App, ctx: &egui::Context) {
    let Some((path, mut title)) = app.lib.renaming.clone() else {
        return;
    };
    let Some(current) = naming::parse_path(&path) else {
        app.lib.renaming = None;
        return;
    };
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_string();
    let mut decision = None;
    let modal = egui::Modal::new(egui::Id::new("rename-media")).show(ctx, |ui| {
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
        ui.horizontal(|ui| {
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
            app.lib.renaming = None;
            let proxy = mediaa_media::preview_cache::proxy_path(&path);
            match naming::rename(&path, &title) {
                Ok(new) if new != path => {
                    app.thumbs.forget(&path, proxy);
                    app.lib.files_changed(&[path.clone(), new.clone()]);
                    app.success(t!(
                        "lib.renamed",
                        name = new.file_name().unwrap_or_default().to_string_lossy()
                    ));
                }
                Ok(_) => {}
                Err(e) => app.error(format!("{e:#}")),
            }
        }
        Some(false) => app.lib.renaming = None,
        None => app.lib.renaming = Some((path, title)),
    }
}
