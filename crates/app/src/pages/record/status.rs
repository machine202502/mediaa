use super::*;

pub(crate) fn live_status(app: &mut App, ui: &mut egui::Ui, ctx: &egui::Context) -> bool {
    let p = theme::palette(ui);
    match &app.recorder {
        RecState::Idle => false,
        RecState::Starting { started, .. } => {
            if starting_card(ui, *started) {
                app.dispatch_command(
                    ctx,
                    AppCommand::Recording(RecordingCommand::Stop {
                        action: StopAction::Discard,
                        source: crate::app::state::StopSource::Ui,
                    }),
                );
            }
            ctx.request_repaint_after(Duration::from_millis(100));
            true
        }
        RecState::Recording { session, .. } => {
            let paused = session.is_paused();
            let elapsed = human_duration(session.elapsed().as_secs_f64());
            let audio = if session.has_audio() {
                t!("record.with_audio")
            } else {
                t!("record.no_audio")
            };
            let detail = match session.output() {
                OutputKind::Audio(f) => f.label().to_string(),
                _ => format!(
                    "{} · {} · {audio}",
                    session.source_names().join(", "),
                    t!("record.frames", n = session.frames_captured())
                ),
            };
            theme::card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::RECORD).size(24.0).color(if paused {
                        p.warning
                    } else {
                        p.danger
                    }));
                    ui.label(RichText::new(elapsed).size(28.0).strong());
                    ui.label(
                        RichText::new(if paused {
                            t!("record.paused")
                        } else {
                            t!("record.recording")
                        })
                        .color(p.text_weak),
                    );
                });
                theme::hint(ui, detail);
                ui.horizontal(|ui| {
                    let (ic, label) = if paused {
                        (icon::PLAY, t!("record.resume"))
                    } else {
                        (icon::PAUSE, t!("record.pause"))
                    };
                    if ui.add(theme::secondary(ic, &label)).clicked() {
                        app.dispatch_command(
                            ctx,
                            AppCommand::Recording(RecordingCommand::TogglePause),
                        );
                    }
                    if ui
                        .add(theme::primary(ui, icon::STOP, &t!("record.stop_save")))
                        .clicked()
                    {
                        app.dispatch_command(
                            ctx,
                            AppCommand::Recording(RecordingCommand::Stop {
                                action: StopAction::Save,
                                source: crate::app::state::StopSource::Ui,
                            }),
                        );
                    }
                    if ui
                        .add(theme::danger(ui, icon::TRASH, &t!("record.cancel")))
                        .clicked()
                    {
                        app.dispatch_command(
                            ctx,
                            AppCommand::Recording(RecordingCommand::Stop {
                                action: StopAction::Discard,
                                source: crate::app::state::StopSource::Ui,
                            }),
                        );
                    }
                });
            });
            ctx.request_repaint_after(Duration::from_millis(250));
            true
        }
        RecState::Saving {
            progress,
            started,
            cancel,
            ..
        } => {
            let (progress, started, cancel) = (*progress, *started, cancel.clone());
            theme::card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(RichText::new(t!("record.saving_long")).strong());
                if let Some(progress) = progress {
                    ui.add(
                        egui::ProgressBar::new(progress)
                            .show_percentage()
                            .animate(true),
                    );
                } else {
                    activity(ui, "", started.elapsed());
                }
                if ui
                    .add(theme::secondary(icon::X, &t!("record.abort_saving")))
                    .clicked()
                {
                    cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                }
            });
            true
        }
    }
}

pub(crate) fn starting_card(ui: &mut egui::Ui, started: Instant) -> bool {
    let mut cancel = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            activity(ui, &t!("record.starting"), started.elapsed());
            cancel = ui
                .add(theme::secondary(icon::X, &t!("common.cancel")))
                .clicked();
        });
    });
    cancel
}
