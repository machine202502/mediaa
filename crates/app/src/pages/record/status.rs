use super::*;
use crate::features::recording::{RecordingInfo, RecordingView};

pub(crate) enum RecordingUiCommand {
    TogglePause,
    Stop(StopAction),
}

pub(crate) fn live_status(
    recording: &mut crate::features::recording::RecordingController,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
) -> Option<RecordingUiCommand> {
    let p = theme::palette(ui);
    match recording.view() {
        RecordingView::Idle => None,
        RecordingView::Starting(elapsed) => {
            if starting_card(ui, elapsed) {
                return Some(RecordingUiCommand::Stop(StopAction::Discard));
            }
            ctx.request_repaint_after(Duration::from_millis(100));
            None
        }
        RecordingView::Recording(RecordingInfo {
            output,
            paused,
            elapsed,
            has_audio,
            sources,
            frames,
        }) => {
            let mut command = None;
            let elapsed = human_duration(elapsed.as_secs_f64());
            let audio = if has_audio {
                t!("record.with_audio")
            } else {
                t!("record.no_audio")
            };
            let detail = match output {
                OutputKind::Audio(f) => f.label().to_string(),
                _ => format!(
                    "{} · {} · {audio}",
                    sources.join(", "),
                    t!("record.frames", n = frames)
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
                        command = Some(RecordingUiCommand::TogglePause);
                    }
                    if ui
                        .add(theme::primary(ui, icon::STOP, &t!("record.stop_save")))
                        .clicked()
                    {
                        command = Some(RecordingUiCommand::Stop(StopAction::Save));
                    }
                    if ui
                        .add(theme::danger(ui, icon::TRASH, &t!("record.cancel")))
                        .clicked()
                    {
                        command = Some(RecordingUiCommand::Stop(StopAction::Discard));
                    }
                });
            });
            ctx.request_repaint_after(Duration::from_millis(250));
            command
        }
        RecordingView::Saving { progress, elapsed } => {
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
                    activity(ui, "", elapsed);
                }
                if ui
                    .add(theme::secondary(icon::X, &t!("record.abort_saving")))
                    .clicked()
                {
                    recording.cancel_finalize();
                }
            });
            None
        }
    }
}

pub(crate) fn starting_card(ui: &mut egui::Ui, elapsed: Duration) -> bool {
    let mut cancel = false;
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            activity(ui, &t!("record.starting"), elapsed);
            cancel = ui
                .add(theme::secondary(icon::X, &t!("common.cancel")))
                .clicked();
        });
    });
    cancel
}
