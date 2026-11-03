use super::{kind_label, page_title, Ctx};
use crate::ui::theme;
use crate::ui_state::ConvertItemStatus;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_core::convert::{ConversionOptions, ConvertTarget, Effort, StillFormat, VideoFormat};
use mediaa2_core::formats::{AnimFormat, AudioFormat, MediaKind};
use mediaa2_core::library::human_size;
use mediaa2_core::limits::{GIF_COLORS, VIDEO_QUALITY};
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::settings::{GifDither, ANIM_HEIGHTS, AUDIO_BITRATES, OUTPUT_HEIGHTS};
use mediaa2_i18n::t;
use mediaa2_platform::dialogs::{pick_convert_files, pick_folder};
use mediaa2_platform::files::open;
use mediaa2_protocol::state::ConversionState;
use mediaa2_protocol::{ClientCommand, ConversionCommand};
use std::path::PathBuf;

const EFFORTS: [Effort; 4] = [Effort::Fastest, Effort::Fast, Effort::Balanced, Effort::Max];
const FPS_PRESETS: [u32; 4] = [60, 30, 24, 15];
const PNG_COLOR_PRESETS: [u32; 5] = [256, 128, 64, 32, 16];

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    sync_statuses(ctx);
    page_title(ui, &t!("convert.title"), "");
    let busy = ctx.view.conversion_busy();
    if !busy {
        let dropped: Vec<PathBuf> = ui.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            add_inputs(ctx, dropped);
        }
    }
    if let Some(error) = ctx.command_error {
        let p = theme::palette(ui);
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(icon::WARNING_CIRCLE).color(p.danger));
            ui.colored_label(p.danger, error);
        });
    }

    let progress = match ctx.view.conversion() {
        ConversionState::Running { done, total, .. } => Some((*done, *total)),
        _ => None,
    };
    inputs_card(ctx, ui, busy, progress);

    let allowed = allowed_targets(&ctx.ui.convert.inputs);
    let no_common = !ctx.ui.convert.inputs.is_empty() && allowed.is_empty();
    if no_common {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.colored_label(theme::palette(ui).danger, t!("convert.no_common"));
        });
    } else if !ctx.ui.convert.inputs.is_empty() {
        if !allowed.contains(&ctx.ui.convert.options.target) {
            ctx.ui.convert.options.target = allowed[0];
        }
        ui.add_space(4.0);
        target_card(ctx, ui, &allowed, busy);
        ui.add_space(4.0);
        settings_and_output(ctx, ui, busy);
    }
}

fn sync_statuses(ctx: &mut Ctx<'_>) {
    let count = ctx.ui.convert.inputs.len();
    ctx.ui
        .convert
        .statuses
        .resize(count, ConvertItemStatus::Pending);
    if let ConversionState::Running { done, .. } = ctx.view.conversion() {
        for status in ctx.ui.convert.statuses.iter_mut().take(*done as usize) {
            if *status == ConvertItemStatus::Pending {
                *status = ConvertItemStatus::Done;
            }
        }
    }
    let failed: Vec<String> = ctx
        .view
        .state
        .notices
        .iter()
        .filter(|notice| notice.level == NoticeLevel::Error)
        .filter_map(|notice| {
            notice
                .summary
                .strip_prefix("Conversion failed: ")
                .map(ToOwned::to_owned)
        })
        .collect();
    for name in failed {
        for (index, path) in ctx.ui.convert.inputs.iter().enumerate() {
            if path.file_name().and_then(|value| value.to_str()) == Some(name.as_str()) {
                ctx.ui.convert.statuses[index] = ConvertItemStatus::Failed;
            }
        }
    }
}

pub(super) fn status_card(ui: &egui::Ui, state: &ConversionState) -> Option<theme::StatusCard> {
    let palette = theme::palette(ui);
    match state {
        ConversionState::Running {
            done,
            total,
            progress,
            current,
            ..
        } => {
            let time = progress.map_or_else(
                || "\u{2026}".to_string(),
                |value| format!("{:.0}%", value * 100.0),
            );
            let batch = t!(
                "convert.batch_progress",
                done = (*done).min(*total),
                total = *total
            )
            .to_string();
            let detail = match current {
                Some(name) if *total > 1 => format!("{batch} · {name}"),
                Some(name) => name.to_string(),
                None => batch,
            };
            Some(theme::StatusCard {
                icon: icon::SHUFFLE,
                color: palette.accent,
                time,
                label: t!("convert.status_running").to_string(),
                detail: Some(detail),
                progress: match progress {
                    Some(progress) => theme::StatusProgress::Percent(*progress),
                    None => theme::StatusProgress::Indeterminate,
                },
                actions: vec![theme::StatusAction::secondary(
                    icon::X,
                    t!("common.cancel").to_string(),
                )],
            })
        }
        ConversionState::Cancelling => Some(theme::StatusCard {
            icon: icon::SPINNER,
            color: palette.accent,
            time: "\u{2026}".to_string(),
            label: format!("{}\u{2026}", t!("common.cancel")),
            detail: None,
            progress: theme::StatusProgress::Indeterminate,
            actions: Vec::new(),
        }),
        ConversionState::Idle => None,
    }
}

pub(super) fn start_bar(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let busy = ctx.view.conversion_busy();
    let allowed = allowed_targets(&ctx.ui.convert.inputs);
    let no_common = !ctx.ui.convert.inputs.is_empty() && allowed.is_empty();
    let can_start = !ctx.ui.convert.inputs.is_empty() && !busy && !no_common;
    let output_dir = resolved_output_dir(ctx);
    ui.vertical_centered(|ui| {
        let start = theme::primary(ui, icon::SHUFFLE, &t!("convert.start"))
            .min_size(egui::vec2(240.0, 38.0));
        let mut response = ui.add_enabled(can_start, start);
        let hint = if busy {
            Some(t!("convert.busy").to_string())
        } else if no_common {
            Some(t!("convert.no_common").to_string())
        } else if ctx.ui.convert.inputs.is_empty() {
            Some(t!("convert.drop_hint").to_string())
        } else {
            None
        };
        if let Some(hint) = hint {
            response = response.on_disabled_hover_text(hint);
        }
        if response.clicked() {
            ctx.ui.convert.statuses.fill(ConvertItemStatus::Pending);
            ctx.ui
                .convert
                .statuses
                .resize(ctx.ui.convert.inputs.len(), ConvertItemStatus::Pending);
            ctx.actions
                .send(ClientCommand::Conversion(ConversionCommand::Start {
                    inputs: ctx.ui.convert.inputs.clone(),
                    target: ctx.ui.convert.options.clone(),
                    output_dir,
                }));
        }
    });
}

fn inputs_card(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, busy: bool, progress: Option<(u32, u32)>) {
    let mut choose = false;
    let mut clear = false;
    let mut remove = None;
    let mut retry = None;
    let hovering = ui.ctx().input(|input| !input.raw.hovered_files.is_empty());
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::FILE, &t!("convert.input"));
        let count = ctx.ui.convert.inputs.len();
        if count > 0 {
            let (done, total) = progress.unwrap_or((0, count as u32));
            theme::hint(
                ui,
                t!(
                    "convert.batch_progress",
                    done = done.min(total),
                    total = total
                ),
            );
            egui::Grid::new("convert-inputs")
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
                        ui.label(egui::RichText::new(heading).strong().size(11.0));
                    }
                    ui.end_row();
                    for index in 0..count {
                        let path = &ctx.ui.convert.inputs[index];
                        let name = path
                            .file_name()
                            .map(|value| value.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.to_string_lossy().into_owned());
                        let full = path.to_string_lossy().into_owned();
                        let kind = MediaKind::from_path(path);
                        let type_label =
                            kind.map_or_else(|| t!("convert.unsupported").to_string(), kind_label);
                        let size = std::fs::metadata(path)
                            .map(|meta| human_size(meta.len()))
                            .unwrap_or_else(|_| "\u{2014}".to_string());
                        ui.add_sized(
                            [260.0, 20.0],
                            egui::Label::new(egui::RichText::new(name).size(12.0)).truncate(),
                        )
                        .on_hover_text(full);
                        ui.label(egui::RichText::new(type_label).size(11.5));
                        ui.label(egui::RichText::new(size).size(11.5));
                        ui.label(egui::RichText::new("\u{2014}").size(11.5));
                        let status = ctx
                            .ui
                            .convert
                            .statuses
                            .get(index)
                            .copied()
                            .unwrap_or_default();
                        match status {
                            ConvertItemStatus::Done => {
                                ui.label(
                                    egui::RichText::new(icon::CHECK)
                                        .size(14.0)
                                        .color(theme::palette(ui).success),
                                );
                            }
                            ConvertItemStatus::Failed => {
                                ui.horizontal(|ui| {
                                    ui.label(
                                        egui::RichText::new(icon::X)
                                            .size(14.0)
                                            .color(theme::palette(ui).danger),
                                    );
                                    if ui
                                        .add_enabled_ui(!busy, |ui| {
                                            theme::icon_button(
                                                ui,
                                                icon::ARROWS_CLOCKWISE,
                                                &t!("convert.start"),
                                            )
                                        })
                                        .inner
                                        .clicked()
                                    {
                                        retry = Some(index);
                                    }
                                });
                            }
                            ConvertItemStatus::Cancelled => {
                                ui.label(
                                    egui::RichText::new("\u{2014}")
                                        .size(11.5)
                                        .color(theme::palette(ui).text_weak),
                                );
                            }
                            ConvertItemStatus::Pending => {
                                ui.label(String::new());
                            }
                        }
                        if ui
                            .add_enabled_ui(!busy, |ui| {
                                theme::icon_button(ui, icon::TRASH, &t!("convert.remove"))
                            })
                            .inner
                            .clicked()
                        {
                            remove = Some(index);
                        }
                        ui.end_row();
                    }
                });
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
                choose = true;
            }
            if count > 0
                && ui
                    .add_enabled(!busy, theme::secondary(icon::TRASH, &t!("convert.clear")))
                    .clicked()
            {
                clear = true;
            }
        });
        if hovering {
            theme::hint(ui, t!("convert.drop_hint"));
        }
    });
    if let Some(index) = remove {
        if index < ctx.ui.convert.inputs.len() {
            ctx.ui.convert.inputs.remove(index);
            if index < ctx.ui.convert.statuses.len() {
                ctx.ui.convert.statuses.remove(index);
            }
        }
    }
    if clear {
        ctx.ui.convert.inputs.clear();
        ctx.ui.convert.statuses.clear();
    }
    if let Some(index) = retry {
        if let Some(path) = ctx.ui.convert.inputs.get(index).cloned() {
            if index < ctx.ui.convert.statuses.len() {
                ctx.ui.convert.statuses[index] = ConvertItemStatus::Pending;
            }
            let output_dir = resolved_output_dir(ctx);
            ctx.actions
                .send(ClientCommand::Conversion(ConversionCommand::Start {
                    inputs: vec![path],
                    target: ctx.ui.convert.options.clone(),
                    output_dir,
                }));
        }
    }
    if choose {
        let directory = picker_dir(ctx);
        if let Some(paths) = pick_convert_files(&directory) {
            add_inputs(ctx, paths);
        }
    }
}

fn settings_and_output(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, busy: bool) {
    let mut options = ctx.ui.convert.options.clone();
    let output_dir = resolved_output_dir(ctx);
    let overridden = !ctx.ui.convert.output_dir.as_os_str().is_empty()
        && ctx.ui.convert.output_dir != ctx.view.settings().convert_dir();
    let mut choose_folder = false;
    let mut reset_folder = false;
    let mut open_folder = false;
    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::SLIDERS_HORIZONTAL, &t!("convert.settings"));
            ui.add_enabled_ui(!busy, |ui| options_controls(ui, &mut options));
        },
        |ui| {
            theme::section(ui, icon::FLOPPY_DISK, &t!("convert.output"));
            ui.add_enabled_ui(!busy, |ui| {
                theme::row(ui, &t!("convert.folder"), |ui| {
                    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                        .clicked()
                    {
                        choose_folder = true;
                    }
                    if overridden
                        && theme::icon_button(
                            ui,
                            icon::ARROW_COUNTER_CLOCKWISE,
                            &t!("convert.folder_reset"),
                        )
                        .clicked()
                    {
                        reset_folder = true;
                    }
                    if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, &t!("settings.open_folder"))
                        .clicked()
                    {
                        open_folder = true;
                    }
                    let path = output_dir.to_string_lossy().into_owned();
                    ui.add(egui::Label::new(path.clone()).truncate())
                        .on_hover_text(path);
                });
            });
        },
    );
    ctx.ui.convert.options = options;
    if choose_folder {
        if let Some(folder) = pick_folder(&output_dir) {
            ctx.ui.convert.output_dir = folder;
        }
    }
    if reset_folder {
        ctx.ui.convert.output_dir = PathBuf::new();
    }
    if open_folder {
        if let Err(error) = open(&output_dir) {
            ctx.actions.notice(
                NoticeLevel::Error,
                t!("settings.open_folder"),
                error.to_string(),
            );
        }
    }
}

fn target_card(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, allowed: &[ConvertTarget], busy: bool) {
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::SHUFFLE, &t!("convert.format"));
        let mut target = ctx.ui.convert.options.target;
        ui.add_enabled_ui(!busy, |ui| {
            for (label, group) in target_groups(allowed) {
                if group.is_empty() {
                    continue;
                }
                theme::row(ui, &label, |ui| {
                    let options: Vec<(ConvertTarget, String)> = group
                        .iter()
                        .map(|candidate| (*candidate, candidate.label().to_string()))
                        .collect();
                    theme::segmented(ui, &mut target, &options);
                });
            }
        });
        ctx.ui.convert.options.target = target;
    });
}

fn target_groups(allowed: &[ConvertTarget]) -> Vec<(String, Vec<ConvertTarget>)> {
    vec![
        (
            t!("convert.group_video").to_string(),
            present(
                allowed,
                VideoFormat::ALL.into_iter().map(ConvertTarget::Video),
            ),
        ),
        (
            t!("convert.group_anim").to_string(),
            present(
                allowed,
                AnimFormat::ALL.into_iter().map(ConvertTarget::Anim),
            ),
        ),
        (
            t!("convert.group_image").to_string(),
            present(
                allowed,
                StillFormat::ALL.into_iter().map(ConvertTarget::Still),
            ),
        ),
        (
            t!("convert.group_audio").to_string(),
            present(
                allowed,
                AudioFormat::ALL.into_iter().map(ConvertTarget::Audio),
            ),
        ),
    ]
}

fn present(
    allowed: &[ConvertTarget],
    candidates: impl IntoIterator<Item = ConvertTarget>,
) -> Vec<ConvertTarget> {
    candidates
        .into_iter()
        .filter(|target| allowed.contains(target))
        .collect()
}

fn allowed_targets(inputs: &[PathBuf]) -> Vec<ConvertTarget> {
    if inputs.is_empty() {
        return all_targets();
    }
    let mut allowed: Option<Vec<ConvertTarget>> = None;
    for path in inputs {
        let Some(kind) = MediaKind::from_path(path) else {
            continue;
        };
        let targets = ConvertTarget::for_kind(kind);
        allowed = Some(match allowed {
            Some(current) => current
                .into_iter()
                .filter(|target| targets.contains(target))
                .collect(),
            None => targets,
        });
    }
    allowed.unwrap_or_default()
}

fn all_targets() -> Vec<ConvertTarget> {
    let mut targets = Vec::new();
    for kind in MediaKind::ALL {
        for target in ConvertTarget::for_kind(kind) {
            if !targets.contains(&target) {
                targets.push(target);
            }
        }
    }
    targets
}

fn options_controls(ui: &mut egui::Ui, options: &mut ConversionOptions) {
    match options.target {
        ConvertTarget::Video(_) => {
            quality_row(ui, &mut options.quality);
            effort_row(ui, &mut options.effort);
            max_height_row(ui, &mut options.max_height, &OUTPUT_HEIGHTS);
            max_fps_row(ui, &mut options.max_fps);
            theme::row(ui, "", |ui| {
                ui.checkbox(&mut options.remove_audio, t!("convert.remove_audio"));
            });
            let audio_enabled = !options.remove_audio;
            theme::row(ui, &t!("convert.audio"), |ui| {
                ui.add_enabled_ui(audio_enabled, |ui| {
                    kbps_combo(ui, &mut options.audio_kbps);
                    mono_checkbox(ui, &mut options.mono);
                });
            });
        }
        ConvertTarget::Anim(format) => animation_options(ui, options, format),
        ConvertTarget::Still(format) => still_options(ui, options, format),
        ConvertTarget::Audio(format) => audio_options(ui, options, format),
    }
}

fn animation_options(ui: &mut egui::Ui, options: &mut ConversionOptions, format: AnimFormat) {
    theme::row(ui, &t!("anim.fps"), |ui| {
        ui.add(egui::Slider::new(&mut options.animation.fps, 1..=50).suffix(" fps"));
    });
    let mut height = (options.animation.max_height > 0).then_some(options.animation.max_height);
    max_height_row(ui, &mut height, &ANIM_HEIGHTS);
    options.animation.max_height = height.unwrap_or(0);
    match format {
        AnimFormat::Gif => {
            gif_colors_row(ui, &mut options.animation.gif_colors);
            dither_row(ui, &mut options.animation.gif_dither);
        }
        AnimFormat::Webp => {
            percent_slider(
                ui,
                &t!("anim.webp_quality"),
                &mut options.animation.webp_quality,
            );
            effort_row(ui, &mut options.effort);
        }
    }
}

fn still_options(ui: &mut egui::Ui, options: &mut ConversionOptions, format: StillFormat) {
    match format {
        StillFormat::Jpeg => {
            quality_row(ui, &mut options.quality);
            max_height_row(ui, &mut options.max_height, &OUTPUT_HEIGHTS);
        }
        StillFormat::Png => {
            png_colors_row(ui, &mut options.png_colors);
            effort_row(ui, &mut options.effort);
            max_height_row(ui, &mut options.max_height, &OUTPUT_HEIGHTS);
        }
        StillFormat::Webp => {
            theme::row(ui, "", |ui| {
                ui.checkbox(&mut options.webp_lossless, t!("convert.lossless"))
                    .on_hover_text(t!("convert.lossless_hint"));
            });
            if !options.webp_lossless {
                quality_row(ui, &mut options.quality);
            }
            effort_row(ui, &mut options.effort);
            max_height_row(ui, &mut options.max_height, &OUTPUT_HEIGHTS);
        }
        StillFormat::Bmp | StillFormat::Tiff => {
            max_height_row(ui, &mut options.max_height, &OUTPUT_HEIGHTS);
        }
    }
}

fn audio_options(ui: &mut egui::Ui, options: &mut ConversionOptions, format: AudioFormat) {
    if !format.lossless() {
        theme::row(ui, &t!("audio.bitrate"), |ui| {
            kbps_combo(ui, &mut options.audio_kbps);
        });
    }
    if format == AudioFormat::Flac {
        effort_row(ui, &mut options.effort);
    }
    theme::row(ui, "", |ui| mono_checkbox(ui, &mut options.mono));
}

fn quality_row(ui: &mut egui::Ui, quality: &mut u8) {
    percent_slider(ui, &t!("record.quality"), quality);
}

fn percent_slider(ui: &mut egui::Ui, label: &str, quality: &mut u8) {
    theme::row(ui, label, |ui| {
        let mut value = u32::from(*quality);
        if ui
            .add(egui::Slider::new(&mut value, VIDEO_QUALITY.min..=VIDEO_QUALITY.max).suffix("%"))
            .changed()
        {
            *quality =
                u8::try_from(value.clamp(VIDEO_QUALITY.min, VIDEO_QUALITY.max)).unwrap_or(100);
        }
    });
}

fn effort_row(ui: &mut egui::Ui, effort: &mut Effort) {
    theme::row(ui, &t!("convert.effort"), |ui| {
        let options: Vec<(Effort, String)> = EFFORTS
            .iter()
            .map(|value| (*value, effort_label(*value)))
            .collect();
        ui.scope(|ui| {
            theme::segmented(ui, effort, &options);
        })
        .response
        .on_hover_text(t!("convert.effort_hint"));
    });
}

fn effort_label(effort: Effort) -> String {
    match effort {
        Effort::Fastest => t!("convert.effort_fastest").to_string(),
        Effort::Fast => t!("convert.effort_fast").to_string(),
        Effort::Balanced => t!("convert.effort_balanced").to_string(),
        Effort::Max => t!("convert.effort_max").to_string(),
    }
}

fn max_height_row(ui: &mut egui::Ui, height: &mut Option<u32>, presets: &[u32]) {
    let current = *height;
    theme::row(ui, &t!("record.resolution"), |ui| {
        theme::combo_box("convert-max-height")
            .selected_text(height_label(*height))
            .show_ui(ui, |ui| {
                ui.selectable_value(height, None, height_label(None));
                for value in presets {
                    ui.selectable_value(height, Some(*value), height_label(Some(*value)));
                }
                if let Some(value) = current {
                    if !presets.contains(&value) {
                        ui.selectable_value(height, Some(value), height_label(Some(value)));
                    }
                }
            })
            .response
            .on_hover_text(t!("record.resolution_hint"));
    });
}

fn height_label(height: Option<u32>) -> String {
    match height {
        None => t!("record.res_original").to_string(),
        Some(2160) => "2160p (4K)".to_string(),
        Some(1440) => "1440p (QHD)".to_string(),
        Some(1080) => "1080p (FHD)".to_string(),
        Some(720) => "720p (HD)".to_string(),
        Some(value) => format!("{value}p"),
    }
}

fn max_fps_row(ui: &mut egui::Ui, fps: &mut Option<u32>) {
    theme::row(ui, &t!("convert.fps"), |ui| {
        theme::combo_box("convert-max-fps")
            .selected_text(fps_label(*fps))
            .show_ui(ui, |ui| {
                ui.selectable_value(fps, None, fps_label(None));
                for value in FPS_PRESETS {
                    ui.selectable_value(fps, Some(value), fps_label(Some(value)));
                }
            })
            .response
            .on_hover_text(t!("convert.fps_hint"));
    });
}

fn fps_label(fps: Option<u32>) -> String {
    fps.map_or_else(
        || t!("convert.fps_original").to_string(),
        |value| format!("{value} fps"),
    )
}

fn kbps_combo(ui: &mut egui::Ui, kbps: &mut u32) {
    theme::combo_box("convert-audio-kbps")
        .selected_text(format!("{kbps} kbit/s"))
        .show_ui(ui, |ui| {
            for value in AUDIO_BITRATES {
                ui.selectable_value(kbps, value, format!("{value} kbit/s"));
            }
        });
}

fn mono_checkbox(ui: &mut egui::Ui, mono: &mut bool) {
    ui.checkbox(mono, t!("convert.mono"))
        .on_hover_text(t!("convert.mono_hint"));
}

fn gif_colors_row(ui: &mut egui::Ui, colors: &mut u32) {
    theme::row(ui, &t!("anim.colors"), |ui| {
        ui.add(egui::Slider::new(colors, GIF_COLORS.min..=GIF_COLORS.max));
    });
}

fn dither_row(ui: &mut egui::Ui, dither: &mut GifDither) {
    theme::row(ui, &t!("anim.dither"), |ui| {
        theme::combo_box("convert-gif-dither")
            .selected_text(dither_label(*dither))
            .show_ui(ui, |ui| {
                for value in GifDither::ALL {
                    ui.selectable_value(dither, value, dither_label(value));
                }
            });
    });
}

fn dither_label(dither: GifDither) -> String {
    match dither {
        GifDither::None => t!("dither.none").to_string(),
        value => value.label().to_string(),
    }
}

fn png_colors_row(ui: &mut egui::Ui, colors: &mut Option<u32>) {
    let current = *colors;
    theme::row(ui, &t!("convert.colors"), |ui| {
        theme::combo_box("convert-png-colors")
            .selected_text(png_colors_label(*colors))
            .show_ui(ui, |ui| {
                ui.selectable_value(colors, None, png_colors_label(None));
                for value in PNG_COLOR_PRESETS {
                    ui.selectable_value(colors, Some(value), png_colors_label(Some(value)));
                }
                if let Some(value) = current {
                    if !PNG_COLOR_PRESETS.contains(&value) {
                        ui.selectable_value(colors, Some(value), png_colors_label(Some(value)));
                    }
                }
            })
            .response
            .on_hover_text(t!("convert.colors_hint"));
    });
}

fn png_colors_label(colors: Option<u32>) -> String {
    colors.map_or_else(
        || t!("convert.colors_all").to_string(),
        |value| t!("convert.colors_n", n = value).to_string(),
    )
}

fn resolved_output_dir(ctx: &Ctx<'_>) -> PathBuf {
    if ctx.ui.convert.output_dir.as_os_str().is_empty() {
        ctx.view.settings().convert_dir()
    } else {
        ctx.ui.convert.output_dir.clone()
    }
}

fn picker_dir(ctx: &Ctx<'_>) -> PathBuf {
    ctx.ui
        .convert
        .inputs
        .first()
        .and_then(|path| path.parent())
        .map(|parent| parent.to_path_buf())
        .unwrap_or_else(|| ctx.view.settings().media_dir.clone())
}

fn add_inputs(ctx: &mut Ctx<'_>, paths: Vec<PathBuf>) {
    let mut rejected = Vec::new();
    for path in paths {
        if MediaKind::from_path(&path).is_some() {
            if !ctx.ui.convert.inputs.contains(&path) {
                ctx.ui.convert.inputs.push(path);
                ctx.ui.convert.statuses.push(ConvertItemStatus::Pending);
            }
        } else {
            rejected.push(
                path.file_name()
                    .map(|value| value.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.to_string_lossy().into_owned()),
            );
        }
    }
    if !rejected.is_empty() {
        ctx.actions.notice(
            NoticeLevel::Warning,
            t!("convert.unsupported"),
            rejected.join(", "),
        );
    }
}
