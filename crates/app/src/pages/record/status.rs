use super::*;
use crate::features::recording::{RecordingInfo, RecordingView};

const STATUS_MIN_H: f32 = 100.0;

fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::proportional(size),
                egui::Color32::WHITE,
            )
            .size()
            .x
    })
}

fn monospace_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.fonts_mut(|fonts| {
        fonts
            .layout_no_wrap(
                text.to_owned(),
                egui::FontId::monospace(size),
                egui::Color32::WHITE,
            )
            .size()
            .x
    })
}
fn saving_progress(ui: &mut egui::Ui, progress: Option<f32>, time: &str) {
    let p = theme::palette(ui);
    let width = 300.0;
    let height = 18.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    let painter = ui.painter();
    let radius = height / 2.0;
    let fill = progress.unwrap_or(1.0).clamp(0.0, 1.0);
    painter.rect_filled(
        rect,
        radius,
        if progress.is_some() {
            p.border
        } else {
            p.accent
        },
    );
    let filled = rect.width() * fill;
    if progress.is_some() {
        painter.rect_filled(
            egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + filled, rect.bottom())),
            radius,
            p.accent,
        );
    }
    let pad = 10.0;
    if let Some(progress) = progress {
        let color = if filled > 34.0 { p.on_accent } else { p.text };
        painter.text(
            egui::pos2(rect.left() + pad, rect.center().y),
            egui::Align2::LEFT_CENTER,
            format!("{:.0}%", progress * 100.0),
            egui::FontId::proportional(11.0),
            color,
        );
    }
    let time_width = monospace_width(ui, time, 11.0);
    let time_left = rect.right() - pad - time_width;
    let color = if progress.is_some() && filled >= time_left - rect.left() {
        p.on_accent
    } else if progress.is_some() {
        p.text
    } else {
        p.on_accent
    };
    painter.text(
        egui::pos2(rect.right() - pad, rect.center().y),
        egui::Align2::RIGHT_CENTER,
        time,
        egui::FontId::monospace(11.0),
        color,
    );
}
fn open_confirm(ctx: &egui::Context, id: &str) {
    ctx.memory_mut(|memory| {
        memory.data.insert_temp(egui::Id::new((id, "open")), true);
    });
}

fn confirm_modal(ctx: &egui::Context, id: &str, title: &str, text: &str, action: &str) -> bool {
    let open_id = egui::Id::new((id, "open"));
    let open = ctx
        .memory(|memory| memory.data.get_temp::<bool>(open_id))
        .unwrap_or(false);
    if !open {
        return false;
    }
    let mut confirmed = false;
    let mut dismiss = false;
    let modal = egui::Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(380.0);
        theme::popup_title(ui, title);
        ui.label(text);
        ui.add_space(8.0);
        let action_label = action.to_string();
        let cancel_label = t!("common.cancel").to_string();
        let responses = theme::centered_buttons(
            ui,
            &[
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Danger,
                    icon: icon::TRASH,
                    label: &action_label,
                    enabled: true,
                },
                theme::ModalButton {
                    kind: theme::ModalButtonKind::Secondary,
                    icon: icon::X,
                    label: &cancel_label,
                    enabled: true,
                },
            ],
        );
        confirmed = responses[0].clicked();
        dismiss = responses[1].clicked();
    });
    if modal.should_close() || dismiss || confirmed {
        ctx.memory_mut(|memory| memory.data.insert_temp(open_id, false));
    }
    confirmed
}
fn button_width(ui: &egui::Ui, icon: &str, text: &str) -> f32 {
    text_width(ui, &format!("{icon}  {text}"), 13.0) + 16.0
}
pub(crate) enum RecordingUiCommand {
    TogglePause,
    Stop(StopAction),
}

pub(crate) fn live_status(
    recording: &mut crate::features::recording::RecordingController,
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    hide_preparation: bool,
) -> Option<RecordingUiCommand> {
    let p = theme::palette(ui);
    match recording.view() {
        RecordingView::Idle => None,
        RecordingView::Starting(elapsed) => {
            if hide_preparation {
                ctx.request_repaint_after(Duration::from_millis(100));
                return None;
            }
            if starting_card(ui, elapsed, ctx) {
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
            let status = if paused {
                t!("record.paused").to_string()
            } else {
                t!("record.recording").to_string()
            };
            let timer_w = text_width(ui, icon::RECORD, 24.0)
                + text_width(ui, &elapsed, 28.0)
                + text_width(ui, &status, 13.0)
                + 16.0;
            let (pause_icon, pause_label) = if paused {
                (icon::PLAY, t!("record.resume").to_string())
            } else {
                (icon::PAUSE, t!("record.pause").to_string())
            };
            let stop_label = t!("record.stop_save").to_string();
            let cancel_label = t!("record.cancel").to_string();
            let detail_w = text_width(ui, &detail, 11.5);
            let pause_w = 28.0;
            let stop_w = button_width(ui, icon::FLOPPY_DISK, &stop_label);
            let cancel_w = button_width(ui, icon::TRASH, &cancel_label);
            let buttons_w = pause_w + stop_w + cancel_w + 16.0;
            theme::card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(STATUS_MIN_H);
                ui.add_space(4.0);
                theme::centered_row_sized(ui, timer_w, |ui| {
                    ui.label(RichText::new(icon::RECORD).size(24.0).color(if paused {
                        p.warning
                    } else {
                        p.danger
                    }));
                    ui.label(RichText::new(elapsed).size(28.0).strong());
                    ui.label(RichText::new(&status).color(p.text_weak));
                });
                theme::centered_row_sized(ui, detail_w, |ui| {
                    theme::hint(ui, detail);
                });
                if buttons_w + 24.0 <= ui.available_width() {
                    theme::centered_row_sized(ui, buttons_w, |ui| {
                        if ui
                            .add(theme::danger(ui, icon::TRASH, &cancel_label))
                            .clicked()
                        {
                            open_confirm(ctx, "record-cancel-confirm");
                        }
                        if ui
                            .add(theme::primary(ui, icon::FLOPPY_DISK, &stop_label))
                            .clicked()
                        {
                            command = Some(RecordingUiCommand::Stop(StopAction::Save));
                        }
                        if theme::framed_icon_button(ui, pause_icon, &pause_label, pause_w)
                            .clicked()
                        {
                            command = Some(RecordingUiCommand::TogglePause);
                        }
                    });
                } else {
                    theme::centered_row_sized(ui, cancel_w, |ui| {
                        if ui
                            .add(theme::danger(ui, icon::TRASH, &cancel_label))
                            .clicked()
                        {
                            open_confirm(ctx, "record-cancel-confirm");
                        }
                    });
                    let width = ui.available_width();
                    if ui
                        .add_sized(
                            egui::vec2(width, 28.0),
                            theme::primary(ui, icon::FLOPPY_DISK, &stop_label),
                        )
                        .clicked()
                    {
                        command = Some(RecordingUiCommand::Stop(StopAction::Save));
                    }
                    theme::centered_widget(ui, pause_w, |ui| {
                        if theme::framed_icon_button(ui, pause_icon, &pause_label, pause_w)
                            .clicked()
                        {
                            command = Some(RecordingUiCommand::TogglePause);
                        }
                    });
                }
            });
            if confirm_modal(
                ctx,
                "record-cancel-confirm",
                &t!("record.cancel_title"),
                &t!("record.cancel_text"),
                &t!("record.cancel_confirm"),
            ) {
                command = Some(RecordingUiCommand::Stop(StopAction::Discard));
            }
            ctx.request_repaint_after(Duration::from_millis(250));
            command
        }
        RecordingView::Saving {
            discarding: true, ..
        } => None,
        RecordingView::Saving {
            progress,
            elapsed,
            recorded,
            ..
        } => {
            theme::card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_min_height(STATUS_MIN_H);
                ui.add_space(4.0);
                let title = t!("record.saving_long").to_string();
                let recorded = human_duration(recorded.as_secs_f64());
                let width = text_width(ui, icon::FLOPPY_DISK, 24.0)
                    + text_width(ui, &recorded, 28.0)
                    + text_width(ui, &title, 13.0)
                    + 16.0;
                theme::centered_widget(ui, width, |ui| {
                    ui.label(RichText::new(icon::FLOPPY_DISK).size(24.0).color(p.accent));
                    ui.label(RichText::new(&recorded).size(28.0).strong());
                    ui.label(RichText::new(&title).color(p.text_weak));
                });
                let duration = human_duration(elapsed.as_secs_f64());
                theme::centered_widget(ui, 300.0, |ui| {
                    saving_progress(ui, progress, &duration);
                });
                let abort = t!("record.abort_saving").to_string();
                theme::centered_widget(ui, button_width(ui, icon::TRASH, &abort), |ui| {
                    if ui.add(theme::danger(ui, icon::TRASH, &abort)).clicked() {
                        open_confirm(ctx, "record-abort-save");
                    }
                });
            });
            if confirm_modal(
                ctx,
                "record-abort-save",
                &t!("record.abort_title"),
                &t!("record.abort_text"),
                &t!("record.abort_saving"),
            ) {
                recording.cancel_finalize();
            }
            None
        }
    }
}

pub(crate) fn starting_card(ui: &mut egui::Ui, elapsed: Duration, ctx: &egui::Context) -> bool {
    let mut cancel = false;
    let p = theme::palette(ui);
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_min_height(STATUS_MIN_H);
        ui.add_space(4.0);
        let duration = human_duration(elapsed.as_secs_f64());
        let label = t!("record.starting").to_string();
        let width = text_width(ui, icon::RECORD, 24.0)
            + text_width(ui, &duration, 28.0)
            + text_width(ui, &label, 13.0)
            + 16.0;
        theme::centered_widget(ui, width, |ui| {
            ui.label(RichText::new(icon::RECORD).size(24.0).color(p.danger));
            ui.label(RichText::new(&duration).size(28.0).strong());
            ui.label(RichText::new(&label).color(p.text_weak));
        });
        theme::centered_widget(ui, 300.0, |ui| {
            saving_progress(ui, None, &duration);
        });
        let cancel_label = t!("common.cancel").to_string();
        theme::centered_widget(ui, button_width(ui, icon::TRASH, &cancel_label), |ui| {
            if ui
                .add(theme::danger(ui, icon::TRASH, &cancel_label))
                .clicked()
            {
                open_confirm(ctx, "record-start-cancel");
            }
        });
    });
    cancel = cancel
        || confirm_modal(
            ctx,
            "record-start-cancel",
            &t!("record.cancel_title"),
            &t!("record.cancel_text"),
            &t!("record.cancel_confirm"),
        );
    cancel
}
