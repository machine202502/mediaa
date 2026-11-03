use super::Ctx;
use crate::ui::theme;
use crate::ui_state::TargetKind;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa2_core::formats::{AnimFormat, OutputKind, VideoContainer};
use mediaa2_core::library::human_duration;
use mediaa2_core::limits::{FPS, GIF_COLORS, VIDEO_QUALITY};
use mediaa2_core::recording::RecordTarget;
use mediaa2_core::screen::{MonitorInfo, WindowInfo};
use mediaa2_core::settings::{
    AnimationQuality, GifDither, MicChoice, RecordPreset, RecordProfile, OUTPUT_HEIGHTS,
};
use mediaa2_i18n::t;
use mediaa2_protocol::state::RecordingState;
use mediaa2_protocol::{ClientCommand, ScreenshotCommand};

const SOURCE_PANEL_H: f32 = 150.0;

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    header(ui, &t!("record.title"), "");
    command_error(ctx, ui);

    let busy = ctx.view.recording_active();

    let mut draft = ctx.ui.settings.draft.clone();
    let mut refresh = false;
    let mut pick_region = false;

    ui.add_enabled_ui(!busy, |ui| {
        let (pick, refresh_target) = target_tiles(ctx, ui);
        pick_region = pick;
        refresh |= refresh_target;
        ui.add_space(4.0);
        if draft.advanced_settings || ctx.ui.record.target != TargetKind::Screen {
            theme::card(ui).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.set_height(SOURCE_PANEL_H);
                refresh |= source_details(ctx, ui, draft.advanced_settings);
            });
            ui.add_space(4.0);
        }
        let sources_h = source_height(ctx);
        let profile_changed = profile_editor(
            ui,
            &mut draft,
            RecordProfile::App,
            ctx.microphones,
            sources_h,
            &mut refresh,
        );
        refresh |= profile_changed;
    });

    if pick_region {
        ctx.ui.record.picking_region = true;
        ctx.actions
            .send(ClientCommand::Screenshot(ScreenshotCommand::StartPicker));
    }
    if draft != ctx.ui.settings.draft {
        ctx.ui.settings.draft = draft.clone();
        ctx.ui.settings.dirty = true;
        ctx.actions.set_settings(draft.clone());
    }
    if refresh {
        ctx.actions.refresh_catalogs();
    }
}

pub(super) fn header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    let p = theme::palette(ui);
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).size(18.0).strong());
        if !subtitle.is_empty() {
            ui.label(RichText::new(subtitle).size(12.0).color(p.text_weak));
        }
    });
    ui.add_space(8.0);
}

pub(super) fn tile_width(ui: &egui::Ui, n: usize) -> f32 {
    let gap = ui.spacing().item_spacing.x;
    let available = ui.available_width();
    let columns = ((available + gap) / (180.0 + gap))
        .floor()
        .clamp(1.0, n as f32);
    ((available - gap * (columns - 1.0)) / columns).floor()
}

pub(super) fn tile(
    ui: &mut egui::Ui,
    icon: &str,
    title: &str,
    subtitle: &str,
    width: f32,
    selected: bool,
) -> egui::Response {
    let p = theme::palette(ui);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 62.0), egui::Sense::click());
    let hovered = response.hovered();
    let fill = if selected {
        p.accent_soft
    } else if hovered {
        p.card_hover
    } else {
        p.card
    };
    let stroke = egui::Stroke::new(
        if selected { 1.5 } else { 1.0 },
        if hovered || selected {
            p.accent
        } else {
            p.border
        },
    );
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(8),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    let badge =
        egui::Rect::from_min_size(rect.min + egui::vec2(12.0, 13.0), egui::vec2(36.0, 36.0));
    ui.painter()
        .rect_filled(badge, egui::CornerRadius::same(7), p.accent_soft);
    ui.painter().text(
        badge.center(),
        egui::Align2::CENTER_CENTER,
        icon,
        egui::FontId::proportional(18.0),
        p.accent,
    );
    let text_x = badge.max.x + 10.0;
    let title_y = if subtitle.is_empty() {
        rect.center().y - 9.0
    } else {
        rect.min.y + 14.0
    };
    ui.painter().text(
        egui::pos2(text_x, title_y),
        egui::Align2::LEFT_TOP,
        title,
        egui::FontId::proportional(14.0),
        p.text,
    );
    ui.painter().text(
        egui::pos2(text_x, rect.min.y + 34.0),
        egui::Align2::LEFT_TOP,
        subtitle,
        egui::FontId::proportional(11.5),
        p.text_weak,
    );
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, title)
    });
    response
}

fn target_tiles(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) -> (bool, bool) {
    let mut pick_region = false;
    let mut refresh = false;
    let target = ctx.ui.record.target;
    let width = tile_width(ui, 3);
    let mut tile_clicked = |ui: &mut egui::Ui, kind, icon: &str, label: &str, width: f32| {
        if tile(ui, icon, label, "", width, target == kind).clicked() {
            ctx.ui.record.target = kind;
            match kind {
                TargetKind::Region => pick_region = true,
                TargetKind::Screen | TargetKind::Windows => refresh = true,
            }
        }
    };
    if width >= 340.0 {
        ui.horizontal(|ui| {
            tile_clicked(
                ui,
                TargetKind::Region,
                icon::SELECTION,
                &t!("shot.area"),
                width,
            );
            tile_clicked(
                ui,
                TargetKind::Screen,
                icon::MONITOR,
                &t!("shot.full"),
                width,
            );
            tile_clicked(
                ui,
                TargetKind::Windows,
                icon::APP_WINDOW,
                &t!("shot.window"),
                width,
            );
        });
    } else {
        let gap = ui.spacing().item_spacing.x;
        let available = ui.available_width();
        let half = ((available - gap) / 2.0).floor();
        ui.horizontal(|ui| {
            tile_clicked(
                ui,
                TargetKind::Region,
                icon::SELECTION,
                &t!("shot.area"),
                half,
            );
            tile_clicked(
                ui,
                TargetKind::Windows,
                icon::APP_WINDOW,
                &t!("shot.window"),
                half,
            );
        });
        ui.add_space(4.0);
        tile_clicked(
            ui,
            TargetKind::Screen,
            icon::MONITOR,
            &t!("shot.full"),
            available,
        );
    }
    (pick_region, refresh)
}

fn source_details(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, advanced: bool) -> bool {
    let mut refresh = false;
    if !advanced {
        if let Some(first) = ctx.ui.record.selected_monitors.iter().next().copied() {
            ctx.ui.record.selected_monitors.retain(|id| *id == first);
        }
        if let Some(first) = ctx.ui.record.selected_windows.iter().next().copied() {
            ctx.ui.record.selected_windows.retain(|id| *id == first);
        }
    }
    match ctx.ui.record.target {
        TargetKind::Screen => {
            ui.horizontal(|ui| {
                theme::section(ui, icon::MONITOR, &t!("record.displays_title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh"))
                        .clicked()
                    {
                        refresh = true;
                    }
                });
            });
            if ctx.monitors.is_empty() {
                theme::hint(ui, t!("err.no_display"));
                return refresh;
            }
            let primary = ctx.monitors.first().map(|monitor| monitor.id.get());
            for monitor in ctx.monitors {
                let id = monitor.id.get();
                let ticked = ctx.ui.record.selected_monitors.contains(&id)
                    || (ctx.ui.record.selected_monitors.is_empty() && Some(id) == primary);
                let mut on = ticked;
                if ui.checkbox(&mut on, monitor_label(monitor)).changed() {
                    if ctx.ui.record.selected_monitors.is_empty() {
                        if let Some(primary) = primary {
                            ctx.ui.record.selected_monitors.insert(primary);
                        }
                    }
                    if on {
                        ctx.ui.record.selected_monitors.insert(id);
                    } else {
                        ctx.ui.record.selected_monitors.remove(&id);
                    }
                }
            }
        }
        TargetKind::Region => {
            theme::section(ui, icon::SELECTION, &t!("record.area"));
            match &ctx.ui.record.region {
                Some((id, region)) => {
                    let display = ctx
                        .monitors
                        .iter()
                        .find(|monitor| monitor.id.get() == *id)
                        .map(MonitorInfo::label)
                        .unwrap_or_else(|| t!("err.no_display").to_string());
                    ui.label(t!(
                        "record.area_value",
                        w = region.w,
                        h = region.h,
                        x = region.x,
                        y = region.y,
                        display = display
                    ));
                }
                None => {
                    ui.label(
                        RichText::new(t!("record.area_none")).color(theme::palette(ui).text_weak),
                    );
                }
            }
        }
        TargetKind::Windows => {
            ui.horizontal(|ui| {
                theme::section(ui, icon::APP_WINDOW, &t!("record.windows_title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh"))
                        .clicked()
                    {
                        refresh = true;
                    }
                });
            });
            if ctx.windows.is_empty() {
                theme::hint(ui, t!("shot.no_windows"));
                return refresh;
            }
            egui::ScrollArea::vertical()
                .id_salt("record-windows")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for window in ctx.windows {
                        let id = window.id.get();
                        let label =
                            format!("{}  ({}Г—{})", window.label(), window.width, window.height);
                        if advanced {
                            let mut on = ctx.ui.record.selected_windows.contains(&id);
                            if ui.checkbox(&mut on, label).changed() {
                                if on {
                                    ctx.ui.record.selected_windows.insert(id);
                                } else {
                                    ctx.ui.record.selected_windows.remove(&id);
                                }
                            }
                        } else if ui
                            .radio(ctx.ui.record.selected_windows.contains(&id), label)
                            .clicked()
                        {
                            ctx.ui.record.selected_windows.clear();
                            ctx.ui.record.selected_windows.insert(id);
                        }
                    }
                });
        }
    }
    refresh
}

pub(super) fn profile_editor(
    ui: &mut egui::Ui,
    settings: &mut mediaa2_core::settings::Settings,
    profile: RecordProfile,
    microphones: &[String],
    sources_h: Option<u32>,
    refresh_mics: &mut bool,
) -> bool {
    let advanced = settings.advanced_settings;
    let (record, anim) = settings.profile_mut(profile);
    if !advanced
        && !matches!(
            record.output,
            OutputKind::Video(VideoContainer::Mp4 | VideoContainer::Mkv)
        )
    {
        record.output = OutputKind::Video(VideoContainer::Mkv);
    }
    let kind = record.output;
    let presets = record.preset_options();
    let current_preset = record.preset(anim);
    let changed = std::cell::Cell::new(false);
    let mark = |value: bool| changed.set(changed.get() | value);
    let mut pending_preset = None;
    let mut pending_output = None;
    let (output, height, fps, quality, hide, cursor, no_signal, unavailable) = (
        &mut record.output,
        &mut record.output_height,
        &mut record.fps,
        &mut record.video_quality,
        &mut record.hide_to_tray,
        &mut record.record_cursor,
        &mut record.show_no_signal,
        &mut record.allow_unavailable_sources,
    );
    let (record_audio, microphone, system_audio) = (
        &mut record.record_audio,
        &mut record.microphone,
        &mut record.system_audio,
    );
    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::FILM_STRIP, &t!("record.output"));
            theme::row(ui, &t!("record.format"), |ui| {
                let kinds = if advanced {
                    output_kinds(profile)
                } else {
                    vec![
                        OutputKind::Video(VideoContainer::Mkv),
                        OutputKind::Video(VideoContainer::Mp4),
                    ]
                };
                let options: Vec<(OutputKind, String)> = kinds
                    .into_iter()
                    .map(|kind| (kind, kind.label().to_string()))
                    .collect();
                let mut chosen = *output;
                if theme::segmented(ui, &mut chosen, &options) && chosen != *output {
                    pending_output = Some(chosen);
                    mark(true);
                }
            });
            if !presets.is_empty() {
                theme::row(ui, &t!("record.preset"), |ui| {
                    if let Some(preset) =
                        preset_combo(ui, egui::Id::new("record-preset"), current_preset, presets)
                    {
                        pending_preset = Some(preset);
                    }
                });
            }
            if advanced || matches!(profile, RecordProfile::App | RecordProfile::Hotkey) {
                theme::row(ui, &t!("record.resolution"), |ui| {
                    mark(resolution_picker(
                        ui,
                        height,
                        &OUTPUT_HEIGHTS,
                        sources_h,
                        egui::Id::new("record-resolution"),
                    ));
                });
                if advanced {
                    theme::row(ui, &t!("record.fps"), |ui| {
                        mark(
                            ui.add(egui::Slider::new(fps, 5..=FPS.max).suffix(" fps"))
                                .changed(),
                        );
                    });
                }
            }
            if advanced && matches!(kind, OutputKind::Video(_)) {
                theme::row(ui, &t!("record.quality"), |ui| {
                    mark(
                        ui.add(
                            egui::Slider::new(
                                quality,
                                VIDEO_QUALITY.min as u8..=VIDEO_QUALITY.max as u8,
                            )
                            .suffix("%"),
                        )
                        .changed(),
                    );
                });
            }
            theme::row(ui, "", |ui| {
                mark(ui.checkbox(cursor, t!("record.cursor")).changed());
            });
            if advanced {
                theme::row(ui, "", |ui| {
                    mark(
                        ui.checkbox(no_signal, t!("record.show_no_signal"))
                            .on_hover_text(t!("record.show_no_signal_hint"))
                            .changed(),
                    );
                });
            }
            if advanced && profile != RecordProfile::Region {
                theme::row(ui, "", |ui| {
                    mark(
                        ui.checkbox(unavailable, t!("record.allow_unavailable_sources"))
                            .on_hover_text(t!("record.allow_unavailable_sources_hint"))
                            .changed(),
                    );
                });
            }
            if advanced && profile == RecordProfile::App {
                theme::row(ui, "", |ui| {
                    mark(ui.checkbox(hide, t!("record.hide_to_tray")).changed());
                });
            }
        },
        |ui| match kind {
            OutputKind::Audio(_) => {}
            OutputKind::Video(_) => {
                theme::section(ui, icon::MICROPHONE, &t!("record.audio"));
                theme::row(ui, "", |ui| {
                    mark(
                        ui.checkbox(record_audio, t!("record.record_audio"))
                            .changed(),
                    );
                });
                ui.add_enabled_ui(*record_audio, |ui| {
                    theme::row(ui, &t!("record.microphone"), |ui| {
                        let (mic_changed, opened) = mic_combo_opened(
                            ui,
                            microphone,
                            microphones,
                            egui::Id::new("record-mic"),
                        );
                        mark(mic_changed);
                        *refresh_mics |= opened;
                    });
                    theme::row(ui, "", |ui| {
                        let supported = mediaa2_platform::audio::system_audio_supported();
                        ui.add_enabled_ui(supported, |ui| {
                            mark(
                                ui.checkbox(system_audio, t!("record.system_audio"))
                                    .changed(),
                            );
                        });
                        if !supported {
                            theme::hint(ui, t!("record.system_audio_linux"));
                        }
                    });
                });
            }
            OutputKind::Anim(format) => {
                if advanced {
                    theme::section(ui, icon::GIF, &t!("record.animation"));
                    mark(quality_options(ui, anim, Some(format)));
                }
            }
        },
    );
    if let Some(output) = pending_output {
        record.switch_output(output, anim);
        mark(true);
    }
    if let Some(preset) = pending_preset {
        record.apply_preset(preset, anim);
        mark(true);
    }
    changed.get()
}

fn output_kinds(profile: RecordProfile) -> Vec<OutputKind> {
    OutputKind::ALL
        .iter()
        .copied()
        .filter(|kind| profile != RecordProfile::Hotkey || matches!(kind, OutputKind::Video(_)))
        .collect()
}

fn source_height(ctx: &Ctx<'_>) -> Option<u32> {
    match ctx.ui.record.target {
        TargetKind::Screen => ctx
            .monitors
            .iter()
            .filter(|monitor| ctx.ui.record.selected_monitors.contains(&monitor.id.get()))
            .map(|monitor| monitor.height)
            .min()
            .or_else(|| ctx.monitors.first().map(|monitor| monitor.height)),
        TargetKind::Region => ctx
            .ui
            .record
            .region
            .as_ref()
            .map(|(_, region)| region.h.max(0) as u32),
        TargetKind::Windows => None,
    }
}

pub(super) fn preset_combo(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: Option<RecordPreset>,
    presets: &[RecordPreset],
) -> Option<RecordPreset> {
    let selected = current
        .map(preset_label)
        .unwrap_or_else(|| t!("record.preset_custom").to_string());
    let mut picked = None;
    theme::combo_box(id)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            for preset in presets {
                if ui
                    .selectable_label(current == Some(*preset), preset_label(*preset))
                    .clicked()
                {
                    picked = Some(*preset);
                }
            }
        });
    picked
}

fn preset_label(preset: RecordPreset) -> String {
    match preset {
        RecordPreset::Quality => t!("record.preset_quality").to_string(),
        RecordPreset::Medium => t!("record.preset_medium").to_string(),
        RecordPreset::Fast => t!("record.preset_fast").to_string(),
    }
}

fn resolution_picker(
    ui: &mut egui::Ui,
    value: &mut u32,
    presets: &[u32],
    source_h: Option<u32>,
    id: egui::Id,
) -> bool {
    let label = |height: u32| match height {
        0 => t!("record.res_original").to_string(),
        2160 => "2160p (4K)".into(),
        1440 => "1440p (QHD)".into(),
        1080 => "1080p (FHD)".into(),
        720 => "720p (HD)".into(),
        height => format!("{height}p"),
    };
    let custom_id = id.with("custom");
    let mut custom = ui
        .data(|data| data.get_temp::<bool>(custom_id))
        .unwrap_or(*value != 0 && !presets.contains(value));
    let mut changed = false;
    let selected = if custom {
        t!("record.res_custom").to_string()
    } else {
        label(*value)
    };
    theme::combo_box(id)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(!custom && *value == 0, label(0))
                .clicked()
            {
                (*value, custom, changed) = (0, false, true);
            }
            for &height in presets {
                if source_h.is_none_or(|source| height < source)
                    && ui
                        .selectable_label(!custom && *value == height, label(height))
                        .clicked()
                {
                    (*value, custom, changed) = (height, false, true);
                }
            }
            if ui
                .selectable_label(custom, t!("record.res_custom"))
                .clicked()
                && !custom
            {
                custom = true;
                if *value == 0 {
                    *value = source_h.map_or(480, |source| (source / 2).max(16));
                }
                changed = true;
            }
        })
        .response
        .on_hover_text(t!("record.resolution_hint"));
    if custom {
        changed |= ui
            .add(egui::DragValue::new(value).range(16..=4320).suffix(" px"))
            .changed();
    }
    ui.data_mut(|data| data.insert_temp(custom_id, custom));
    changed
}

fn quality_options(
    ui: &mut egui::Ui,
    quality: &mut AnimationQuality,
    format: Option<AnimFormat>,
) -> bool {
    quality_controls(
        ui,
        &mut quality.gif_colors,
        &mut quality.gif_dither,
        &mut quality.webp_quality,
        format,
    )
}

fn quality_controls(
    ui: &mut egui::Ui,
    gif_colors: &mut u32,
    gif_dither: &mut GifDither,
    webp_quality: &mut u8,
    format: Option<AnimFormat>,
) -> bool {
    let mut changed = false;
    if format != Some(AnimFormat::Webp) {
        theme::row(ui, &t!("anim.colors"), |ui| {
            changed |= ui
                .add(egui::Slider::new(
                    gif_colors,
                    GIF_COLORS.min..=GIF_COLORS.max,
                ))
                .changed();
        });
        theme::row(ui, &t!("anim.dither"), |ui| {
            theme::combo_box("record-dither")
                .selected_text(dither_label(*gif_dither))
                .show_ui(ui, |ui| {
                    for dither in GifDither::ALL {
                        changed |= ui
                            .selectable_value(gif_dither, dither, dither_label(dither))
                            .changed();
                    }
                });
        });
    }
    if format != Some(AnimFormat::Gif) {
        theme::row(ui, &t!("anim.webp_quality"), |ui| {
            changed |= ui
                .add(egui::Slider::new(
                    webp_quality,
                    VIDEO_QUALITY.min as u8..=VIDEO_QUALITY.max as u8,
                ))
                .changed();
        });
    }
    changed
}

fn dither_label(dither: GifDither) -> String {
    match dither {
        GifDither::None => t!("dither.none").to_string(),
        value => value.label().to_string(),
    }
}

pub(super) fn start_bar(ctx: &Ctx<'_>, ui: &mut egui::Ui) -> Option<RecordTarget> {
    let target = start_target(ctx);
    let recovery_active = ctx.view.recovery().running;
    let hotkeys = &ctx.ui.settings.draft.hotkeys;
    let mut start = None;
    ui.vertical_centered(|ui| {
        let button =
            theme::primary(ui, icon::RECORD, &t!("record.start")).min_size(egui::vec2(220.0, 36.0));
        let available = target.is_ok() && !recovery_active;
        let mut response = ui.add_enabled(available, button);
        if let Err(reason) = &target {
            if !reason.is_empty() {
                response = response.on_disabled_hover_text(reason.as_str());
            }
        }
        if recovery_active {
            response = response.on_disabled_hover_text(t!("record.recovery_wait"));
        }
        if hotkeys.enabled && !hotkeys.start_video.is_empty() {
            response = response.on_hover_text(t!("record.hotkey_hint", key = hotkeys.start_video));
        }
        if response.clicked() {
            start = target.ok();
        }
    });
    start
}

#[derive(Clone, Copy)]
pub(super) enum StatusCommand {
    TogglePause,
    StopAndSave,
    Discard,
    CancelStart,
    CancelFinalize,
}

pub(super) fn status_card(ui: &egui::Ui, recording: &RecordingState) -> Option<theme::StatusCard> {
    let palette = theme::palette(ui);
    match recording {
        RecordingState::Idle => None,
        RecordingState::Starting { .. } => Some(theme::StatusCard {
            icon: icon::RECORD,
            color: palette.danger,
            time: "00:00".to_string(),
            label: t!("record.starting").to_string(),
            detail: None,
            progress: theme::StatusProgress::Indeterminate,
            actions: vec![theme::StatusAction::danger(
                icon::TRASH,
                t!("common.cancel").to_string(),
            )],
        }),
        RecordingState::Active {
            paused,
            elapsed_ms,
            sources,
            has_audio,
            output,
            ..
        } => {
            let elapsed = human_duration(*elapsed_ms as f64 / 1000.0);
            let sound = if *has_audio {
                t!("record.with_audio").to_string()
            } else {
                t!("record.no_audio").to_string()
            };
            let detail = if sources.is_empty() {
                format!("{} · {}", output.label(), sound)
            } else {
                format!("{} · {} · {}", sources.join(", "), output.label(), sound)
            };
            let label = if *paused {
                t!("record.paused").to_string()
            } else {
                t!("record.recording").to_string()
            };
            let (pause_icon, pause_label) = if *paused {
                (icon::PLAY, t!("record.resume").to_string())
            } else {
                (icon::PAUSE, t!("record.pause").to_string())
            };
            Some(theme::StatusCard {
                icon: icon::RECORD,
                color: if *paused {
                    palette.warning
                } else {
                    palette.danger
                },
                time: elapsed,
                label,
                detail: Some(detail),
                progress: theme::StatusProgress::None,
                actions: vec![
                    theme::StatusAction::danger(icon::TRASH, t!("record.cancel").to_string()),
                    theme::StatusAction::primary(
                        icon::FLOPPY_DISK,
                        t!("record.stop_save").to_string(),
                    ),
                    theme::StatusAction::icon(pause_icon, pause_label),
                ],
            })
        }
        RecordingState::Finalizing {
            recorded_ms,
            progress,
            ..
        } => Some(theme::StatusCard {
            icon: icon::FLOPPY_DISK,
            color: palette.accent,
            time: human_duration(*recorded_ms as f64 / 1000.0),
            label: t!("record.saving_long").to_string(),
            detail: None,
            progress: match progress {
                Some(progress) => theme::StatusProgress::Percent(*progress),
                None => theme::StatusProgress::Indeterminate,
            },
            actions: vec![theme::StatusAction::danger(
                icon::TRASH,
                t!("record.abort_saving").to_string(),
            )],
        }),
    }
}

pub(super) fn status_command(recording: &RecordingState, index: usize) -> Option<StatusCommand> {
    match recording {
        RecordingState::Idle => None,
        RecordingState::Starting { .. } => (index == 0).then_some(StatusCommand::CancelStart),
        RecordingState::Active { .. } => [
            StatusCommand::Discard,
            StatusCommand::StopAndSave,
            StatusCommand::TogglePause,
        ]
        .get(index)
        .copied(),
        RecordingState::Finalizing { .. } => (index == 0).then_some(StatusCommand::CancelFinalize),
    }
}

fn start_target(ctx: &Ctx<'_>) -> Result<RecordTarget, String> {
    match ctx.ui.record.target {
        TargetKind::Screen => {
            let mut monitors: Vec<MonitorInfo> = ctx
                .monitors
                .iter()
                .filter(|monitor| ctx.ui.record.selected_monitors.contains(&monitor.id.get()))
                .cloned()
                .collect();
            if monitors.is_empty() {
                match ctx.monitors.iter().find(|monitor| monitor.primary) {
                    Some(primary) => monitors.push(primary.clone()),
                    None => monitors.extend(ctx.monitors.iter().cloned()),
                }
            }
            if monitors.is_empty() {
                Err(t!("err.no_display").to_string())
            } else {
                Ok(RecordTarget::Screens { monitors })
            }
        }
        TargetKind::Windows => {
            let windows: Vec<WindowInfo> = ctx
                .windows
                .iter()
                .filter(|window| ctx.ui.record.selected_windows.contains(&window.id.get()))
                .cloned()
                .collect();
            if windows.is_empty() {
                if ctx.windows.is_empty() {
                    Err(t!("shot.no_windows").to_string())
                } else {
                    Err(t!("record.windows_title").to_string())
                }
            } else {
                Ok(RecordTarget::Windows { windows })
            }
        }
        TargetKind::Region => match ctx.ui.record.region {
            Some((id, region)) => {
                match ctx.monitors.iter().find(|monitor| monitor.id.get() == id) {
                    Some(monitor) => Ok(RecordTarget::Region {
                        monitor: monitor.clone(),
                        region,
                    }),
                    None => Err(t!("err.no_display").to_string()),
                }
            }
            None => Err(t!("err.select_area").to_string()),
        },
    }
}

pub(super) fn monitor_label(monitor: &MonitorInfo) -> String {
    if monitor.primary {
        format!("{} В· {}", monitor.label(), t!("shot.primary"))
    } else {
        monitor.label()
    }
}

pub(super) fn mic_combo(
    ui: &mut egui::Ui,
    microphone: &mut MicChoice,
    microphones: &[String],
    id: egui::Id,
) -> bool {
    mic_combo_opened(ui, microphone, microphones, id).0
}

pub(super) fn mic_combo_opened(
    ui: &mut egui::Ui,
    microphone: &mut MicChoice,
    microphones: &[String],
    id: egui::Id,
) -> (bool, bool) {
    let label = |choice: &MicChoice| match choice {
        MicChoice::Auto => t!("record.mic_auto").to_string(),
        MicChoice::Off => t!("common.none").to_string(),
        MicChoice::Device(device) if microphones.contains(device) => device.clone(),
        MicChoice::Device(device) => t!("record.mic_missing", device = device).to_string(),
    };
    let mut changed = false;
    let combo = theme::combo_box(id)
        .selected_text(label(microphone))
        .show_ui(ui, |ui| {
            let mut options = vec![MicChoice::Auto, MicChoice::Off];
            options.extend(
                microphones
                    .iter()
                    .map(|device| MicChoice::Device(device.clone())),
            );
            for option in options {
                if ui
                    .selectable_value(microphone, option.clone(), label(&option))
                    .changed()
                {
                    changed = true;
                }
            }
        });
    (changed, combo.response.clicked())
}

fn command_error(ctx: &Ctx<'_>, ui: &mut egui::Ui) {
    let Some(error) = ctx.command_error else {
        return;
    };
    let palette = theme::palette(ui);
    ui.label(RichText::new(error).size(12.0).color(palette.danger));
}
