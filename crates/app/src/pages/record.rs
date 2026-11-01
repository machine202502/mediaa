use super::{header, tile};
use crate::features::recording::TargetKind;
use crate::{i18n, theme};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::limits::{FPS, GIF_COLORS, VIDEO_QUALITY};
use mediaa_core::settings::{
    AnimSettings, AnimationQuality, GifDither, MicChoice, RecordProfile, ANIM_HEIGHTS,
    OUTPUT_HEIGHTS,
};
use mediaa_core::Settings;
use mediaa_core::{AnimFormat, OutputKind};
use mediaa_media::recorder::StopAction;
use mediaa_platform::screen::{MonitorInfo, WindowInfo};
use rust_i18n::t;
use std::time::Duration;

const SOURCE_PANEL_H: f32 = 150.0;

pub fn monitor_label(m: &MonitorInfo) -> String {
    if m.primary {
        format!("{} · {}", m.label(), t!("shot.primary"))
    } else {
        m.label()
    }
}

pub(super) mod status;

pub enum RecordUiAction {
    SettingsChanged,
    RefreshCatalogs,
    SelectRegion,
}

pub fn ui(
    recording: &mut crate::features::recording::RecordingController,
    settings: &mut Settings,
    monitors: &[MonitorInfo],
    windows: &[WindowInfo],
    microphones: &[String],
    ui: &mut egui::Ui,
) -> Vec<RecordUiAction> {
    let mut actions = Vec::new();
    header(ui, &t!("record.title"), "");
    let live = !recording.is_idle();
    if live {
        ui.disable();
    }

    let gap = ui.spacing().item_spacing.x;
    let avail = ui.available_width();
    let w = super::tile_width(ui, 3);
    let target = recording.ui.target;
    if w >= 340.0 {
        ui.horizontal(|ui| {
            if target_tile(
                ui,
                recording,
                TargetKind::Region,
                icon::SELECTION,
                &t!("shot.area"),
                w,
                target == TargetKind::Region,
            ) {
                actions.push(RecordUiAction::SelectRegion);
            }
            if target_tile(
                ui,
                recording,
                TargetKind::Screen,
                icon::MONITOR,
                &t!("shot.full"),
                w,
                target == TargetKind::Screen,
            ) {
                actions.push(RecordUiAction::RefreshCatalogs);
            }
            if target_tile(
                ui,
                recording,
                TargetKind::Windows,
                icon::APP_WINDOW,
                &t!("shot.window"),
                w,
                target == TargetKind::Windows,
            ) {
                actions.push(RecordUiAction::RefreshCatalogs);
            }
        });
    } else {
        let half = ((avail - gap) / 2.0).floor();
        ui.horizontal(|ui| {
            if target_tile(
                ui,
                recording,
                TargetKind::Region,
                icon::SELECTION,
                &t!("shot.area"),
                half,
                target == TargetKind::Region,
            ) {
                actions.push(RecordUiAction::SelectRegion);
            }
            if target_tile(
                ui,
                recording,
                TargetKind::Windows,
                icon::APP_WINDOW,
                &t!("shot.window"),
                half,
                target == TargetKind::Windows,
            ) {
                actions.push(RecordUiAction::RefreshCatalogs);
            }
        });
        ui.add_space(4.0);
        if target_tile(
            ui,
            recording,
            TargetKind::Screen,
            icon::MONITOR,
            &t!("shot.full"),
            avail,
            target == TargetKind::Screen,
        ) {
            actions.push(RecordUiAction::RefreshCatalogs);
        }
    }
    ui.add_space(4.0);

    if settings.advanced_settings || recording.ui.target != TargetKind::Screen {
        theme::card(ui).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height(SOURCE_PANEL_H);
            if source_details(recording, monitors, windows, settings.advanced_settings, ui) {
                actions.push(RecordUiAction::RefreshCatalogs);
            }
        });
        ui.add_space(4.0);
    }

    let sources_h = source_height(recording, monitors);
    let mut refresh_mics = false;
    if profile_editor(
        ui,
        settings,
        RecordProfile::App,
        microphones,
        sources_h,
        &mut refresh_mics,
    ) {
        actions.push(RecordUiAction::SettingsChanged);
    }
    if refresh_mics {
        actions.push(RecordUiAction::RefreshCatalogs);
    }
    actions
}

fn target_tile(
    ui: &mut egui::Ui,
    recording: &mut crate::features::recording::RecordingController,
    kind: TargetKind,
    icon: &str,
    label: &str,
    width: f32,
    selected: bool,
) -> bool {
    let clicked = tile(ui, icon, label, "", width, selected).clicked();
    if clicked {
        recording.ui.target = kind;
    }
    clicked
}

pub fn profile_editor(
    ui: &mut egui::Ui,
    settings: &mut Settings,
    profile: RecordProfile,
    mics: &[String],
    sources_h: Option<u32>,
    refresh_mics: &mut bool,
) -> bool {
    let changed = std::cell::Cell::new(false);
    let refresh = std::cell::Cell::new(false);
    let mark = |c: bool| changed.set(changed.get() | c);
    let salt = match profile {
        RecordProfile::App => 0,
        RecordProfile::Hotkey => 1,
        RecordProfile::Region => 2,
    };
    let advanced = settings.advanced_settings;
    let (r, anim) = settings.profile_mut(profile);
    if !advanced {
        match profile {
            RecordProfile::Hotkey | RecordProfile::App => {
                if !matches!(
                    r.output,
                    OutputKind::Video(
                        mediaa_core::VideoContainer::Mp4 | mediaa_core::VideoContainer::Mkv
                    )
                ) {
                    r.output = OutputKind::Video(mediaa_core::VideoContainer::Mkv);
                }
            }
            RecordProfile::Region => {
                if !matches!(
                    r.output,
                    OutputKind::Anim(AnimFormat::Gif | AnimFormat::Webp)
                ) {
                    r.output = OutputKind::Anim(AnimFormat::Gif);
                }
            }
        }
    }
    let kind = r.output;
    let presets = r.preset_options();
    let current_preset = r.preset(anim);
    let pending_preset = std::cell::Cell::new(None);
    let pending_output = std::cell::Cell::new(None);
    let (output, height, fps, quality, hide, cursor, no_signal, unavailable) = (
        &mut r.output,
        &mut r.output_height,
        &mut r.fps,
        &mut r.video_quality,
        &mut r.hide_to_tray,
        &mut r.record_cursor,
        &mut r.show_no_signal,
        &mut r.allow_unavailable_sources,
    );
    let (record_audio, microphone, system_audio) =
        (&mut r.record_audio, &mut r.microphone, &mut r.system_audio);
    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::FILM_STRIP, &t!("record.output"));
            theme::row(ui, &t!("record.format"), |ui| {
                let kinds = if !advanced {
                    match profile {
                        RecordProfile::Hotkey | RecordProfile::App => vec![
                            OutputKind::Video(mediaa_core::VideoContainer::Mkv),
                            OutputKind::Video(mediaa_core::VideoContainer::Mp4),
                        ],
                        RecordProfile::Region => vec![
                            OutputKind::Anim(AnimFormat::Gif),
                            OutputKind::Anim(AnimFormat::Webp),
                        ],
                    }
                } else {
                    output_kinds(profile)
                };
                let opts: Vec<(OutputKind, String)> = kinds
                    .into_iter()
                    .map(|k| (k, k.label().to_string()))
                    .collect();
                let mut chosen = *output;
                if theme::segmented(ui, &mut chosen, &opts) && chosen != *output {
                    pending_output.set(Some(chosen));
                    changed.set(true);
                }
            });
            if !presets.is_empty() {
                theme::row(ui, &t!("record.preset"), |ui| {
                    if let Some(preset) = super::preset_combo(
                        ui,
                        egui::Id::new(("rec-preset", salt)),
                        current_preset,
                        presets,
                    ) {
                        pending_preset.set(Some(preset));
                    }
                });
            }
            if advanced || profile == RecordProfile::App {
                theme::row(ui, &t!("record.resolution"), |ui| {
                    mark(resolution_picker(
                        ui,
                        height,
                        &OUTPUT_HEIGHTS,
                        sources_h,
                        egui::Id::new(("rec-resolution", salt)),
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
                        let (c, opened) =
                            mic_combo(ui, microphone, mics, egui::Id::new(("rec-mic", salt)));
                        mark(c);
                        refresh.set(opened);
                    });
                    theme::row(ui, "", |ui| {
                        let supported = mediaa_platform::audio::system_audio_supported();
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
            OutputKind::Anim(fmt) => {
                if advanced {
                    theme::section(ui, icon::GIF, &t!("record.animation"));
                    mark(quality_options(ui, anim, Some(fmt)));
                }
            }
        },
    );
    *refresh_mics = refresh.get();
    if let Some(output) = pending_output.get() {
        r.switch_output(output, anim);
        changed.set(true);
    }
    if let Some(preset) = pending_preset.get() {
        r.apply_preset(preset, anim);
        changed.set(true);
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

fn source_height(
    recording: &crate::features::recording::RecordingController,
    monitors: &[MonitorInfo],
) -> Option<u32> {
    match recording.ui.target {
        TargetKind::Screen => monitors
            .iter()
            .filter(|m| recording.ui.monitors.contains(&m.id))
            .map(|m| m.height)
            .min()
            .or_else(|| monitors.first().map(|m| m.height)),
        TargetKind::Region => recording.ui.region.as_ref().map(|(_, r)| r.h.max(0) as u32),
        TargetKind::Windows => None,
    }
}

pub fn resolution_picker(
    ui: &mut egui::Ui,
    value: &mut u32,
    presets: &[u32],
    source_h: Option<u32>,
    id: egui::Id,
) -> bool {
    let label = |h: u32| match h {
        0 => t!("record.res_original").to_string(),
        2160 => "2160p (4K)".into(),
        1440 => "1440p (QHD)".into(),
        1080 => "1080p (FHD)".into(),
        720 => "720p (HD)".into(),
        h => format!("{h}p"),
    };
    let custom_id = id.with("custom");
    let mut custom = ui
        .data(|d| d.get_temp::<bool>(custom_id))
        .unwrap_or(*value != 0 && !presets.contains(value));
    let mut changed = false;
    let selected = if custom {
        t!("record.res_custom").to_string()
    } else {
        label(*value)
    };
    crate::theme::combo_box(id)
        .selected_text(selected)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(!custom && *value == 0, label(0))
                .clicked()
            {
                (*value, custom, changed) = (0, false, true);
            }
            for &h in presets {
                if source_h.is_none_or(|s| h < s)
                    && ui
                        .selectable_label(!custom && *value == h, label(h))
                        .clicked()
                {
                    (*value, custom, changed) = (h, false, true);
                }
            }
            if ui
                .selectable_label(custom, t!("record.res_custom"))
                .clicked()
                && !custom
            {
                custom = true;
                if *value == 0 {
                    *value = source_h.map_or(480, |s| (s / 2).max(16));
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
    ui.data_mut(|d| d.insert_temp(custom_id, custom));
    changed
}

fn source_details(
    recording: &mut crate::features::recording::RecordingController,
    monitors: &[MonitorInfo],
    windows: &[mediaa_platform::screen::WindowInfo],
    advanced: bool,
    ui: &mut egui::Ui,
) -> bool {
    let mut refresh = false;
    if !advanced {
        if let Some(first) = recording.ui.monitors.iter().next().copied() {
            recording.ui.monitors.retain(|id| *id == first);
        }
        if let Some(first) = recording.ui.windows.iter().next().copied() {
            recording.ui.windows.retain(|id| *id == first);
        }
    }
    match recording.ui.target {
        TargetKind::Screen if advanced => {
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
            let primary = monitors.first().map(|m| m.id);
            for m in monitors {
                let ticked = recording.ui.monitors.contains(&m.id)
                    || (recording.ui.monitors.is_empty() && Some(m.id) == primary);
                if advanced {
                    let mut on = ticked;
                    if ui.checkbox(&mut on, monitor_label(m)).changed() {
                        if recording.ui.monitors.is_empty() {
                            recording.ui.monitors.extend(primary);
                        }
                        if on {
                            recording.ui.monitors.insert(m.id);
                        } else {
                            recording.ui.monitors.remove(&m.id);
                        }
                    }
                } else if ui.radio(ticked, monitor_label(m)).clicked() {
                    recording.ui.monitors.clear();
                    recording.ui.monitors.insert(m.id);
                }
            }
        }
        TargetKind::Screen => {}
        TargetKind::Region => {
            theme::section(ui, icon::SELECTION, &t!("record.area"));
            match &recording.ui.region {
                Some((m, r)) => ui.label(t!(
                    "record.area_value",
                    w = r.w,
                    h = r.h,
                    x = r.x,
                    y = r.y,
                    display = m.name
                )),
                None => ui.label(
                    RichText::new(t!("record.area_none")).color(theme::palette(ui).text_weak),
                ),
            };
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
            egui::ScrollArea::vertical()
                .id_salt("rec-windows")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for w in windows {
                        let on = recording.ui.windows.contains(&w.id);
                        let label = format!("{}  ({}×{})", w.label(), w.width, w.height);
                        if advanced {
                            let mut on = on;
                            if ui.checkbox(&mut on, label).changed() {
                                if on {
                                    recording.ui.windows.insert(w.id);
                                } else {
                                    recording.ui.windows.remove(&w.id);
                                }
                            }
                        } else if ui.radio(on, label).clicked() {
                            recording.ui.windows.clear();
                            recording.ui.windows.insert(w.id);
                        }
                    }
                });
        }
    }
    refresh
}

pub fn anim_options(
    ui: &mut egui::Ui,
    a: &mut AnimSettings,
    fmt: Option<AnimFormat>,
    source_h: Option<u32>,
    show_size_timing: bool,
) -> bool {
    let mut changed = false;
    if show_size_timing {
        theme::row(ui, &t!("anim.fps"), |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut a.fps, 1..=50).suffix(" fps"))
                .changed();
        });
        theme::row(ui, &t!("record.resolution"), |ui| {
            changed |= resolution_picker(
                ui,
                &mut a.max_height,
                &ANIM_HEIGHTS,
                source_h,
                egui::Id::new(("anim-resolution", fmt.is_some())),
            );
        });
    }
    changed |= quality_controls(
        ui,
        &mut a.gif_colors,
        &mut a.gif_dither,
        &mut a.webp_quality,
        fmt,
    );
    changed
}

fn quality_options(
    ui: &mut egui::Ui,
    quality: &mut AnimationQuality,
    fmt: Option<AnimFormat>,
) -> bool {
    quality_controls(
        ui,
        &mut quality.gif_colors,
        &mut quality.gif_dither,
        &mut quality.webp_quality,
        fmt,
    )
}

fn quality_controls(
    ui: &mut egui::Ui,
    gif_colors: &mut u32,
    gif_dither: &mut GifDither,
    webp_quality: &mut u8,
    fmt: Option<AnimFormat>,
) -> bool {
    let mut changed = false;
    if fmt != Some(AnimFormat::Webp) {
        theme::row(ui, &t!("anim.colors"), |ui| {
            changed |= ui
                .add(egui::Slider::new(
                    gif_colors,
                    GIF_COLORS.min..=GIF_COLORS.max,
                ))
                .changed();
        });
        theme::row(ui, &t!("anim.dither"), |ui| {
            crate::theme::combo_box(("dither", fmt.is_some()))
                .selected_text(i18n::dither(*gif_dither))
                .show_ui(ui, |ui| {
                    for d in GifDither::ALL {
                        changed |= ui
                            .selectable_value(gif_dither, d, i18n::dither(d))
                            .changed();
                    }
                });
        });
    }
    if fmt != Some(AnimFormat::Gif) {
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

pub fn start_bar(
    settings: &Settings,
    recovery_active: bool,
    target: &Result<(), String>,
    ui: &mut egui::Ui,
) -> bool {
    let mut start = false;
    ui.vertical_centered(|ui| {
        let btn =
            theme::primary(ui, icon::RECORD, &t!("record.start")).min_size(egui::vec2(220.0, 36.0));
        let available = target.is_ok() && !recovery_active;
        let mut resp = ui.add_enabled(available, btn);
        if let Err(why) = &target {
            if !why.is_empty() {
                resp = resp.on_disabled_hover_text(why.as_str());
            }
        }
        if recovery_active {
            resp = resp.on_disabled_hover_text(t!("record.recovery_wait"));
        }
        let hk = &settings.hotkeys;
        if hk.enabled && !hk.start_video.is_empty() {
            resp = resp.on_hover_text(t!("record.hotkey_hint", key = hk.start_video));
        }
        if resp.clicked() {
            start = true;
        }
    });
    start
}

pub fn mic_combo(
    ui: &mut egui::Ui,
    microphone: &mut MicChoice,
    mics: &[String],
    id: egui::Id,
) -> (bool, bool) {
    let label = |m: &MicChoice| match m {
        MicChoice::Auto => t!("record.mic_auto").to_string(),
        MicChoice::Off => t!("common.none").to_string(),
        MicChoice::Device(d) if mics.contains(d) => d.clone(),
        MicChoice::Device(d) => t!("record.mic_missing", device = d).to_string(),
    };
    let mut changed = false;
    let combo = crate::theme::combo_box(id)
        .selected_text(label(microphone))
        .show_ui(ui, |ui| {
            let mut options = vec![MicChoice::Auto, MicChoice::Off];
            options.extend(mics.iter().map(|d| MicChoice::Device(d.clone())));
            for o in options {
                let text = label(&o);
                changed |= ui.selectable_value(microphone, o, text).changed();
            }
        });
    (changed, combo.response.clicked())
}

#[cfg(test)]
mod tests;
