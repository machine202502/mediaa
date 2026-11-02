use super::controls::{
    effort_row, fps_combo, kbps_combo, mono_checkbox, pick_file_dialog, png_colors_combo,
    quality_row,
};
use crate::features::conversion::{BatchStatus, ConversionController, ConversionUiAction};
use crate::i18n;
use crate::ui::pages::header;
use crate::ui::theme;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::library::{human_duration, human_size};
use mediaa_core::settings::OUTPUT_HEIGHTS;
use mediaa_media::convert::{StillFormat, Target};
use rust_i18n::t;
use std::path::PathBuf;
impl ConversionController {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        wake: &mediaa_core::wake::Wake,
        convert_dir: &std::path::Path,
        folder_override: &mut Option<PathBuf>,
    ) -> Option<ConversionUiAction> {
        let before = (
            self.target,
            self.settings.clone(),
            self.max_height,
            self.folder.clone(),
        );
        let action = self.ui_inner(ui, wake, convert_dir, folder_override);
        if !self.is_busy()
            && before
                != (
                    self.target,
                    self.settings.clone(),
                    self.max_height,
                    self.folder.clone(),
                )
        {
            self.reset_batch_statuses();
        }
        action
    }

    fn ui_inner(
        &mut self,
        ui: &mut egui::Ui,
        wake: &mediaa_core::wake::Wake,
        convert_dir: &std::path::Path,
        folder_override: &mut Option<PathBuf>,
    ) -> Option<ConversionUiAction> {
        let _ = wake;
        let ctx = ui.ctx().clone();
        self.folder = folder_override.clone();
        self.poll_probe(wake);
        #[cfg(test)]
        if self.batch_auto
            && self.job.is_none()
            && self.probed.as_ref().is_some_and(|probe| probe.is_ok())
        {
            if let Some(target) = self.target {
                if let Some(error) = self.start_job(wake, target, convert_dir) {
                    self.failed
                        .push((self.input.clone().unwrap_or_default(), error.clone()));
                }
            }
        }
        header(ui, &t!("convert.title"), "");
        if self.input.is_none() {
            if let Some(p) = crate::dev_mode::mode().convert.clone() {
                self.set_input(p);
            }
        }
        let busy = self.is_busy();
        let mut action = None;
        if !busy {
            let dropped: Vec<PathBuf> = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .filter(|p| !p.as_os_str().is_empty())
                    .collect()
            });
            if !dropped.is_empty() {
                self.set_inputs(dropped);
            }
        }

        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, icon::FILE, &t!("convert.input"));
            if !self.batch.is_empty() {
                theme::hint(
                    ui,
                    t!(
                        "convert.batch_progress",
                        done = self.batch_done.min(self.batch.len()),
                        total = self.batch.len()
                    ),
                );
                let mut retry = None;
                let mut remove = None;
                egui::Grid::new("convert-batch")
                    .num_columns(6)
                    .spacing(egui::vec2(12.0, 6.0))
                    .striped(true)
                    .show(ui, |ui| {
                        for heading in [
                            t!("convert.column_name"),
                            t!("convert.column_type"),
                            t!("convert.column_size"),
                            t!("convert.column_duration"),
                            String::new().into(),
                            String::new().into(),
                        ] {
                            ui.label(RichText::new(heading).strong().size(11.0));
                        }
                        ui.end_row();
                        for item in self.batch.clone() {
                            ui.add_sized(
                                [240.0, 20.0],
                                egui::Label::new(
                                    RichText::new(
                                        item.path.file_name().unwrap_or_default().to_string_lossy(),
                                    )
                                    .size(12.0),
                                )
                                .truncate(),
                            )
                            .on_hover_text(item.path.to_string_lossy());
                            let kind = item
                                .kind
                                .map(i18n::media_kind)
                                .unwrap_or_else(|| "\u{2014}".into());
                            ui.label(RichText::new(kind).size(11.5));
                            let size = item
                                .size
                                .map(human_size)
                                .unwrap_or_else(|| "\u{2014}".into());
                            ui.label(RichText::new(size).size(11.5));
                            let duration = item
                                .duration
                                .map(human_duration)
                                .unwrap_or_else(|| "\u{2014}".into());
                            ui.label(RichText::new(duration).size(11.5));
                            let palette = theme::palette(ui);
                            let status = match item.status {
                                BatchStatus::Done => Some((
                                    icon::CHECK,
                                    palette.success,
                                    t!("convert.status_done").into_owned(),
                                )),
                                BatchStatus::Failed => Some((
                                    icon::X,
                                    palette.danger,
                                    item.error.clone().unwrap_or_else(|| {
                                        t!("convert.status_failed").into_owned()
                                    }),
                                )),
                                _ => None,
                            };
                            if let Some((status_icon, color, tooltip)) = status {
                                ui.label(RichText::new(status_icon).size(14.0).color(color))
                                    .on_hover_text(tooltip);
                            } else {
                                ui.label("");
                            }
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled_ui(!busy, |ui| {
                                        theme::icon_button(ui, icon::TRASH, &t!("convert.remove"))
                                    })
                                    .inner
                                    .clicked()
                                {
                                    remove = Some(item.path.clone());
                                }
                                if item.status == BatchStatus::Failed
                                    && ui
                                        .add_enabled_ui(!busy, |ui| {
                                            theme::icon_button(
                                                ui,
                                                icon::ARROW_CLOCKWISE,
                                                &t!("convert.retry"),
                                            )
                                        })
                                        .inner
                                        .clicked()
                                {
                                    retry = Some(item.path.clone());
                                }
                            });
                            ui.end_row();
                        }
                    });
                if let Some(path) = retry {
                    self.retry_batch(&path);
                }
                if let Some(path) = remove {
                    self.remove_batch(&path);
                }
            }
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !busy,
                        theme::secondary(icon::FOLDER_OPEN, &t!("convert.choose")),
                    )
                    .on_hover_text(t!("convert.drop_hint"))
                    .clicked()
                {
                    pick_file_dialog(self, convert_dir);
                }
                if !self.batch.is_empty()
                    && ui
                        .add_enabled(!busy, theme::secondary(icon::TRASH, &t!("convert.clear")))
                        .clicked()
                {
                    self.clear_batch();
                }
            });
            if let Some(Err(e)) = &self.probed {
                ui.colored_label(theme::palette(ui).danger, e);
            }
        });

        let Some(Ok(probe)) = self.probed.clone() else {
            return None;
        };
        let info = probe.info;
        ui.add_space(4.0);

        let targets = self.targets();
        if targets.is_empty() {
            theme::card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.colored_label(theme::palette(ui).danger, t!("convert.no_common"));
            });
            if !self.conflict_notified {
                self.conflict_notified = true;
                action = Some(ConversionUiAction::Error(t!("convert.no_common").into()));
            }
            return action;
        }
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            theme::section(ui, icon::SHUFFLE, &t!("convert.format"));
            ui.add_enabled_ui(!busy, |ui| {
                for (label, group) in [
                    (
                        t!("convert.group_video"),
                        targets
                            .iter()
                            .filter(|t| matches!(t, Target::Video(_)))
                            .copied()
                            .collect::<Vec<_>>(),
                    ),
                    (
                        t!("convert.group_anim"),
                        targets
                            .iter()
                            .filter(|t| matches!(t, Target::Anim(_)))
                            .copied()
                            .collect(),
                    ),
                    (
                        t!("convert.group_image"),
                        targets
                            .iter()
                            .filter(|t| matches!(t, Target::Still(_)))
                            .copied()
                            .collect(),
                    ),
                    (
                        t!("convert.group_audio"),
                        targets
                            .iter()
                            .filter(|t| matches!(t, Target::Audio(_)))
                            .copied()
                            .collect(),
                    ),
                ] {
                    if group.is_empty() {
                        continue;
                    }
                    theme::row(ui, &label, |ui| {
                        let opts: Vec<(Option<Target>, String)> = group
                            .iter()
                            .map(|t| (Some(*t), t.label().to_string()))
                            .collect();
                        theme::segmented(ui, &mut self.target, &opts);
                    });
                }
            });
        });

        let target = self.target?;
        ui.add_space(4.0);

        theme::split_card(
            ui,
            |ui| {
                theme::section(ui, icon::SLIDERS_HORIZONTAL, &t!("convert.settings"));
                ui.add_enabled_ui(!busy, |ui| {
                    let s = &mut self.settings;
                    let source_h = (info.height > 0).then_some(info.height);
                    let max_height = &mut self.max_height;
                    let mut resolution_row = |ui: &mut egui::Ui| {
                        theme::row(ui, &t!("record.resolution"), |ui| {
                            crate::ui::pages::record::resolution_picker(
                                ui,
                                max_height,
                                &OUTPUT_HEIGHTS,
                                source_h,
                                egui::Id::new("convert-resolution"),
                            );
                        });
                    };
                    match target {
                        Target::Video(_) => {
                            quality_row(ui, &mut s.quality);
                            effort_row(ui, &mut s.effort);
                            resolution_row(ui);
                            theme::row(ui, &t!("convert.fps"), |ui| {
                                fps_combo(ui, &mut s.max_fps, info.fps)
                            });
                            if info.has_audio() {
                                theme::row(ui, "", |ui| {
                                    ui.checkbox(&mut s.remove_audio, t!("convert.remove_audio"))
                                });
                                theme::row(ui, &t!("convert.audio"), |ui| {
                                    ui.add_enabled_ui(!s.remove_audio, |ui| {
                                        kbps_combo(ui, &mut s.audio_kbps);
                                        mono_checkbox(ui, &mut s.mono);
                                    });
                                });
                            }
                        }
                        Target::Anim(fmt) => {
                            crate::ui::pages::record::anim_options(
                                ui,
                                &mut s.anim,
                                Some(fmt),
                                source_h,
                                true,
                            );
                            if fmt == mediaa_core::formats::AnimFormat::Webp {
                                effort_row(ui, &mut s.effort);
                            }
                        }
                        Target::Still(fmt) => {
                            match fmt {
                                StillFormat::Jpeg => quality_row(ui, &mut s.quality),
                                StillFormat::Webp => {
                                    theme::row(ui, "", |ui| {
                                        ui.checkbox(&mut s.webp_lossless, t!("convert.lossless"))
                                            .on_hover_text(t!("convert.lossless_hint"))
                                    });
                                    if !s.webp_lossless {
                                        quality_row(ui, &mut s.quality);
                                    }
                                    effort_row(ui, &mut s.effort);
                                }
                                StillFormat::Png => {
                                    theme::row(ui, &t!("convert.colors"), |ui| {
                                        png_colors_combo(ui, &mut s.png_colors)
                                    });
                                    effort_row(ui, &mut s.effort);
                                }
                                StillFormat::Bmp | StillFormat::Tiff => {}
                            }
                            resolution_row(ui);
                        }
                        Target::Audio(fmt) => {
                            if !fmt.lossless() {
                                theme::row(ui, &t!("audio.bitrate"), |ui| {
                                    kbps_combo(ui, &mut s.audio_kbps)
                                });
                            }
                            if fmt == mediaa_core::formats::AudioFormat::Flac {
                                effort_row(ui, &mut s.effort);
                            }
                            theme::row(ui, "", |ui| mono_checkbox(ui, &mut s.mono));
                        }
                    }
                });
            },
            |ui| {
                theme::section(ui, icon::FLOPPY_DISK, &t!("convert.output"));
                ui.add_enabled_ui(!busy, |ui| {
                    let folder = self
                        .folder
                        .clone()
                        .unwrap_or_else(|| convert_dir.to_path_buf());
                    theme::row(ui, &t!("convert.folder"), |ui| {
                        if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                            .clicked()
                        {
                            if let Some(p) = crate::infra::dialogs::pick_folder(&folder) {
                                self.folder = Some(p.clone());
                                *folder_override = Some(p);
                            }
                        }
                        if self.folder.is_some()
                            && theme::icon_button(
                                ui,
                                icon::ARROW_COUNTER_CLOCKWISE,
                                &t!("convert.folder_reset"),
                            )
                            .clicked()
                        {
                            self.folder = None;
                            *folder_override = None;
                        }
                        ui.add(egui::Label::new(folder.to_string_lossy()).truncate());
                    });
                });
            },
        );
        action
    }
}
