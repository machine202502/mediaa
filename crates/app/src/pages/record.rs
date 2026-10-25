use super::{activity, header, tile};
use crate::app::command::{AppCommand, RecordingCommand};
use crate::app::state::RecState;
use crate::app::App;
use crate::overlay::Purpose;
use crate::{i18n, theme};
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::geometry::Rect;
use mediaa_core::library::human_duration;
use mediaa_core::settings::{
    AnimSettings, GifDither, MicChoice, RecordProfile, ANIM_HEIGHTS, OUTPUT_HEIGHTS,
};
use mediaa_core::Settings;
use mediaa_core::{AnimFormat, OutputKind};
use mediaa_media::recorder::StopAction;
use mediaa_platform::screen::MonitorInfo;
use rust_i18n::t;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const SOURCE_PANEL_H: f32 = 150.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetKind {
    #[default]
    Screen,
    Region,
    Windows,
}

#[derive(Default)]
pub struct RecordUi {
    pub target: TargetKind,
    pub monitors: BTreeSet<u32>,
    pub region: Option<(MonitorInfo, Rect)>,
    pub windows: BTreeSet<u32>,
}

pub fn monitor_label(m: &MonitorInfo) -> String {
    if m.primary {
        format!("{} · {}", m.label(), t!("shot.primary"))
    } else {
        m.label()
    }
}

pub(super) mod status;

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    header(ui, &t!("record.title"), "");
    if status::live_status(app, ui, &ctx) {
        return;
    }

    let w = super::tile_width(ui, 3);
    let target = app.rec.target;
    ui.horizontal_wrapped(|ui| {
        if tile(
            ui,
            icon::SELECTION,
            &t!("shot.area"),
            "",
            w,
            target == TargetKind::Region,
        )
        .clicked()
        {
            app.rec.target = TargetKind::Region;
            app.start_region_screenshot(&ctx, Purpose::RecordRegion);
        }
        if tile(
            ui,
            icon::MONITOR,
            &t!("shot.full"),
            "",
            w,
            target == TargetKind::Screen,
        )
        .clicked()
        {
            app.rec.target = TargetKind::Screen;
            app.refresh_monitors();
        }
        if tile(
            ui,
            icon::APP_WINDOW,
            &t!("shot.window"),
            "",
            w,
            target == TargetKind::Windows,
        )
        .clicked()
        {
            app.rec.target = TargetKind::Windows;
            app.refresh_windows();
        }
    });
    ui.add_space(4.0);

    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.set_height(SOURCE_PANEL_H);
        source_details(app, ui);
    });
    ui.add_space(4.0);

    let sources_h = source_height(app);
    let mut refresh_mics = false;
    if profile_editor(
        ui,
        &mut app.settings,
        RecordProfile::App,
        &app.mic_devices,
        sources_h,
        &mut refresh_mics,
    ) {
        app.settings_changed();
    }
    if refresh_mics {
        app.refresh_mics();
    }
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
    let (r, anim) = settings.profile_mut(profile);
    let kind = r.output;
    let (output, height, fps, quality, hide, cursor) = (
        &mut r.output,
        &mut r.output_height,
        &mut r.fps,
        &mut r.video_quality,
        &mut r.hide_to_tray,
        &mut r.record_cursor,
    );
    let (record_audio, microphone, system_audio, every_file) = (
        &mut r.record_audio,
        &mut r.microphone,
        &mut r.system_audio,
        &mut r.audio_in_every_file,
    );
    let (segmented_webp, parallel_webp) = (&mut r.segmented_webp, &mut r.parallel_webp);
    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::FILM_STRIP, &t!("record.output"));
            theme::row(ui, &t!("record.format"), |ui| {
                let opts: Vec<(OutputKind, String)> = output_kinds(profile)
                    .into_iter()
                    .map(|k| (k, k.label().to_string()))
                    .collect();
                mark(theme::segmented(ui, output, &opts));
            });
            theme::row(ui, &t!("record.resolution"), |ui| {
                mark(resolution_picker(
                    ui,
                    height,
                    &OUTPUT_HEIGHTS,
                    sources_h,
                    egui::Id::new(("rec-resolution", salt)),
                ));
            });
            theme::row(ui, &t!("record.fps"), |ui| {
                mark(
                    ui.add(egui::Slider::new(fps, 5..=60).suffix(" fps"))
                        .changed(),
                );
            });
            if let OutputKind::Video(_) = kind {
                theme::row(ui, &t!("record.quality"), |ui| {
                    mark(
                        ui.add(egui::Slider::new(quality, 1..=100).suffix("%"))
                            .changed(),
                    );
                });
            }
            theme::row(ui, "", |ui| {
                mark(ui.checkbox(cursor, t!("record.cursor")).changed());
            });
            theme::row(ui, "", |ui| {
                mark(ui.checkbox(hide, t!("record.hide_to_tray")).changed());
            });
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
                    if profile == RecordProfile::App {
                        theme::row(ui, "", |ui| {
                            mark(
                                ui.checkbox(every_file, t!("record.audio_every_file"))
                                    .on_hover_text(t!("record.audio_every_file_hint"))
                                    .changed(),
                            );
                        });
                    }
                });
            }
            OutputKind::Anim(fmt) => {
                theme::section(ui, icon::GIF, &t!("record.animation"));
                mark(anim_options(ui, anim, Some(fmt), sources_h, false));
                if fmt == AnimFormat::Webp {
                    theme::row(ui, "", |ui| {
                        mark(
                            ui.checkbox(segmented_webp, t!("record.segmented_webp"))
                                .changed(),
                        );
                    });
                    theme::row(ui, "", |ui| {
                        ui.add_enabled_ui(*segmented_webp, |ui| {
                            mark(
                                ui.checkbox(parallel_webp, t!("record.parallel_webp"))
                                    .changed(),
                            );
                        });
                    });
                }
            }
        },
    );
    *refresh_mics = refresh.get();
    changed.get()
}

fn output_kinds(profile: RecordProfile) -> Vec<OutputKind> {
    OutputKind::ALL
        .iter()
        .copied()
        .filter(|kind| profile != RecordProfile::Hotkey || matches!(kind, OutputKind::Video(_)))
        .collect()
}

fn source_height(app: &App) -> Option<u32> {
    match app.rec.target {
        TargetKind::Screen => app
            .monitors
            .iter()
            .filter(|m| app.rec.monitors.contains(&m.id))
            .map(|m| m.height)
            .min()
            .or_else(|| app.monitors.first().map(|m| m.height)),
        TargetKind::Region => app.rec.region.as_ref().map(|(_, r)| r.h.max(0) as u32),
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
    egui::ComboBox::from_id_salt(id)
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

fn source_details(app: &mut App, ui: &mut egui::Ui) {
    match app.rec.target {
        TargetKind::Screen => {
            ui.horizontal(|ui| {
                theme::section(ui, icon::MONITOR, &t!("record.displays_title"));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh"))
                        .clicked()
                    {
                        app.refresh_monitors();
                    }
                });
            });
            let primary = app.monitors.first().map(|m| m.id);
            for m in &app.monitors {
                let ticked = app.rec.monitors.contains(&m.id)
                    || (app.rec.monitors.is_empty() && Some(m.id) == primary);
                let mut on = ticked;
                if ui.checkbox(&mut on, monitor_label(m)).changed() {
                    if app.rec.monitors.is_empty() {
                        app.rec.monitors.extend(primary);
                    }
                    if on {
                        app.rec.monitors.insert(m.id);
                    } else {
                        app.rec.monitors.remove(&m.id);
                    }
                }
            }
        }
        TargetKind::Region => {
            theme::section(ui, icon::SELECTION, &t!("record.area"));
            match &app.rec.region {
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
                        app.refresh_windows();
                    }
                });
            });
            egui::ScrollArea::vertical()
                .id_salt("rec-windows")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for w in &app.windows {
                        let mut on = app.rec.windows.contains(&w.id);
                        if ui
                            .checkbox(
                                &mut on,
                                format!("{}  ({}×{})", w.label(), w.width, w.height),
                            )
                            .changed()
                        {
                            if on {
                                app.rec.windows.insert(w.id);
                            } else {
                                app.rec.windows.remove(&w.id);
                            }
                        }
                    }
                });
        }
    }
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
    if fmt != Some(AnimFormat::Webp) {
        theme::row(ui, &t!("anim.colors"), |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut a.gif_colors, 4..=256))
                .changed();
        });
        theme::row(ui, &t!("anim.dither"), |ui| {
            egui::ComboBox::from_id_salt(("dither", fmt.is_some()))
                .selected_text(i18n::dither(a.gif_dither))
                .show_ui(ui, |ui| {
                    for d in GifDither::ALL {
                        changed |= ui
                            .selectable_value(&mut a.gif_dither, d, i18n::dither(d))
                            .changed();
                    }
                });
        });
    }
    if fmt != Some(AnimFormat::Gif) {
        theme::row(ui, &t!("anim.webp_quality"), |ui| {
            changed |= ui
                .add(egui::Slider::new(&mut a.webp_quality, 0..=100))
                .changed();
        });
    }
    changed
}

pub fn start_bar(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    let target = app.record_target_from_ui();
    ui.vertical_centered(|ui| {
        let btn =
            theme::primary(ui, icon::RECORD, &t!("record.start")).min_size(egui::vec2(220.0, 36.0));
        let available = target.is_ok() && !app.recovery_active();
        let mut resp = ui.add_enabled(available, btn);
        if app.recovery_active() {
            resp = resp.on_disabled_hover_text(t!("record.recovery_wait"));
        }
        let hk = &app.settings.hotkeys;
        if hk.enabled && !hk.start_video.is_empty() {
            resp = resp.on_hover_text(t!("record.hotkey_hint", key = hk.start_video));
        }
        if resp.clicked() {
            app.start_recording_from_ui(&ctx);
        }
        if let Err(why) = &target {
            if !why.is_empty() {
                theme::hint(ui, why.as_str());
            }
        }
    });
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
    let combo = egui::ComboBox::from_id_salt(id)
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
