//! Convert section: pick a file → the formats it can become → their settings (plus an optional
//! name and the folder) → Convert. Video ⇄ video / animation / audio, animation → video /
//! animation, image → image, audio → audio. Results land in the conversions folder (the
//! library by default).

use super::header;
use crate::app::App;
use crate::{i18n, theme};
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::library::{human_duration, human_size};
use mediaa_core::settings::{AUDIO_BITRATES, OUTPUT_HEIGHTS};
use mediaa_core::MediaKind;
use mediaa_media::convert::{self, ConvertSettings, Effort, StillFormat, Target, PNG_COLORS, VIDEO_FPS};
use mediaa_media::info::MediaInfo;
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Job {
    output: PathBuf,
    progress: Arc<AtomicU32>,
    cancel: Arc<AtomicBool>,
    done: Receiver<Result<(), String>>,
}

#[derive(Default)]
pub struct ConvertUi {
    input: Option<PathBuf>,
    probed: Option<Result<(MediaKind, MediaInfo), String>>,
    target: Option<Target>,
    settings: ConvertSettings,
    /// Output height limit, 0 = original.
    max_height: u32,
    /// Output title; empty = after the input.
    name: String,
    /// Output folder; `None` = the conversions folder from Settings.
    folder: Option<PathBuf>,
    job: Option<Job>,
    /// Last finished output (shown with Open / Show buttons).
    last: Option<PathBuf>,
    /// Output size estimate for the current file, format and settings.
    estimate: Option<Estimate>,
}

/// Pause after the last settings change before a trial conversion starts.
const ESTIMATE_DELAY: Duration = Duration::from_millis(500);

/// A trial conversion (see `convert::estimate_size`) running in the background.
struct Estimate {
    key: (PathBuf, Target, ConvertSettings),
    /// When `key` last changed.
    since: Instant,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Result<u64, String>>>,
    result: Option<Result<u64, String>>,
    /// Previous estimate, shown while this one is being made.
    previous: Option<u64>,
}

impl Drop for Estimate {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl ConvertUi {
    pub fn set_input(&mut self, path: PathBuf) {
        self.probed = Some(match MediaKind::detect(&path) {
            None => Err(t!("convert.unsupported").into()),
            Some(kind) => mediaa_media::preview::details(&path, kind).map(|i| (kind, i)).map_err(|e| format!("{e:#}")),
        });
        // Keep the chosen format when it still applies to the new file.
        let targets = self.targets();
        if self.target.is_some_and(|t| !targets.contains(&t)) {
            self.target = None;
        }
        self.estimate = None;
        self.name.clear();
        self.input = Some(path);
        self.last = None;
    }

    /// The settings a conversion uses (the resolution picker keeps 0 for "original").
    fn effective(&self) -> ConvertSettings {
        ConvertSettings { max_height: (self.max_height > 0).then_some(self.max_height), ..self.settings.clone() }
    }

    fn targets(&self) -> Vec<Target> {
        match &self.probed {
            Some(Ok((kind, info))) => Target::for_kind(*kind)
                .into_iter()
                // Sound only out of files that have sound.
                .filter(|t| !matches!(t, Target::Audio(_)) || info.has_audio())
                .collect(),
            _ => Vec::new(),
        }
    }
}

fn start(app: &mut App, ctx: &egui::Context, target: Target) {
    let Some(input) = app.conv.input.clone() else { return };
    let folder = app.conv.folder.clone().unwrap_or_else(|| app.settings.convert_dir());
    let output = convert::output_path(&input, target, Some(&app.conv.name), &folder);
    let settings = app.conv.effective();
    let progress = Arc::new(AtomicU32::new(0));
    let cancel = Arc::new(AtomicBool::new(false));
    let (tx, rx) = crossbeam_channel::bounded(1);
    let (p, c, out, ctx2) = (progress.clone(), cancel.clone(), output.clone(), ctx.clone());
    std::thread::Builder::new()
        .name("convert".into())
        .spawn(move || {
            let mut report = |f: f32| {
                p.store((f * 1000.0) as u32, Ordering::Relaxed);
                ctx2.request_repaint();
            };
            let r = convert::convert(&input, &out, target, &settings, &mut report, &c).map_err(|e| {
                if mediaa_media::job::is_cancelled(&e) {
                    String::new()
                } else {
                    format!("{e:#}")
                }
            });
            let _ = tx.send(r);
            ctx2.request_repaint();
        })
        .expect("spawn convert thread");
    app.conv.job = Some(Job { output, progress, cancel, done: rx });
}

/// Keeps the size estimate in step with the file, format and settings: a trial conversion
/// starts once they stay unchanged for [`ESTIMATE_DELAY`] (and no conversion is running).
fn update_estimate(app: &mut App, ctx: &egui::Context, target: Target) {
    let Some(input) = app.conv.input.clone() else { return };
    let key = (input, target, app.conv.effective());
    let conv = &mut app.conv;
    if conv.estimate.as_ref().is_none_or(|e| e.key != key) {
        let previous = conv.estimate.as_ref().and_then(|e| e.result.as_ref().and_then(|r| r.as_ref().ok().copied()).or(e.previous));
        conv.estimate = Some(Estimate { key, since: Instant::now(), cancel: Arc::default(), rx: None, result: None, previous });
    }
    let busy = conv.job.is_some();
    let Some(e) = conv.estimate.as_mut() else { return };
    if e.result.is_some() {
        return;
    }
    match &e.rx {
        Some(rx) => {
            if let Ok(r) = rx.try_recv() {
                e.result = Some(r);
            }
        }
        None if busy => {}
        None if e.since.elapsed() < ESTIMATE_DELAY => ctx.request_repaint_after(ESTIMATE_DELAY),
        None => {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let (key, cancel, ctx) = (e.key.clone(), e.cancel.clone(), ctx.clone());
            std::thread::Builder::new()
                .name("convert-estimate".into())
                .spawn(move || {
                    let (input, target, settings) = key;
                    let r = convert::estimate_size(&input, target, &settings, &cancel);
                    if !cancel.load(Ordering::Relaxed) {
                        let _ = tx.send(r.map_err(|e| format!("{e:#}")));
                        ctx.request_repaint();
                    }
                })
                .expect("spawn estimate thread");
            e.rx = Some(rx);
        }
    }
}

/// "≈ 4.2 MB (−58 %)" against the input size, a spinner while estimating.
fn estimate_row(ui: &mut egui::Ui, estimate: Option<&Estimate>, input: Option<&std::path::Path>) {
    let Some(e) = estimate else { return };
    let source = input.and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len());
    let text = |bytes: u64| {
        let mut s = format!("≈ {}", human_size(bytes));
        if let Some(src) = source.filter(|s| *s > 0) {
            let pct = (bytes as f64 / src as f64 - 1.0) * 100.0;
            s += &format!("  ({}{:.0} %)", if pct < 0.0 { "−" } else { "+" }, pct.abs());
        }
        s
    };
    theme::row(ui, &t!("convert.size"), |ui| {
        let resp = match &e.result {
            Some(Ok(bytes)) => ui.label(RichText::new(text(*bytes)).strong()),
            Some(Err(err)) => ui.label(RichText::new(t!("convert.size_failed")).color(theme::palette(ui).text_weak)).on_hover_text(err),
            None => {
                ui.add(egui::Spinner::new().size(12.0));
                match e.previous {
                    Some(bytes) => ui.label(RichText::new(text(bytes)).color(theme::palette(ui).text_weak)),
                    None => ui.label(RichText::new(t!("convert.size_estimating")).color(theme::palette(ui).text_weak)),
                }
            }
        };
        resp.on_hover_text(t!("convert.size_hint"));
    });
}

fn effort_row(ui: &mut egui::Ui, effort: &mut Effort) {
    theme::row(ui, &t!("convert.effort"), |ui| {
        let opts: Vec<(Effort, String)> = Effort::ALL
            .iter()
            .map(|e| {
                let label = match e {
                    Effort::Fast => t!("convert.effort_fast"),
                    Effort::Balanced => t!("convert.effort_balanced"),
                    Effort::Max => t!("convert.effort_max"),
                };
                (*e, label.to_string())
            })
            .collect();
        ui.scope(|ui| theme::segmented(ui, effort, &opts)).response.on_hover_text(t!("convert.effort_hint"));
    });
}

fn kbps_combo(ui: &mut egui::Ui, kbps: &mut u32) {
    egui::ComboBox::from_id_salt("convert-kbps").selected_text(format!("{kbps} kbit/s")).show_ui(ui, |ui| {
        for k in AUDIO_BITRATES {
            ui.selectable_value(kbps, k, format!("{k} kbit/s"));
        }
    });
}

fn mono_checkbox(ui: &mut egui::Ui, mono: &mut bool) {
    ui.checkbox(mono, t!("convert.mono")).on_hover_text(t!("convert.mono_hint"));
}

/// Frame-rate cap: "Original" or the common rates below the source's.
fn fps_combo(ui: &mut egui::Ui, max_fps: &mut Option<u32>, source_fps: f64) {
    let label = |f: Option<u32>| f.map_or_else(|| t!("convert.fps_original").to_string(), |f| format!("{f} fps"));
    egui::ComboBox::from_id_salt("convert-fps")
        .selected_text(label(*max_fps))
        .show_ui(ui, |ui| {
            ui.selectable_value(max_fps, None, label(None));
            for f in VIDEO_FPS.into_iter().filter(|f| source_fps <= 0.0 || f64::from(*f) < source_fps - 0.5) {
                ui.selectable_value(max_fps, Some(f), label(Some(f)));
            }
        })
        .response
        .on_hover_text(t!("convert.fps_hint"));
}

fn png_colors_combo(ui: &mut egui::Ui, colors: &mut Option<u16>) {
    let label = |c: Option<u16>| c.map_or_else(|| t!("convert.colors_all").to_string(), |n| t!("convert.colors_n", n = n).to_string());
    egui::ComboBox::from_id_salt("convert-colors")
        .selected_text(label(*colors))
        .show_ui(ui, |ui| {
            ui.selectable_value(colors, None, label(None));
            for n in PNG_COLORS {
                ui.selectable_value(colors, Some(n), label(Some(n)));
            }
        })
        .response
        .on_hover_text(t!("convert.colors_hint"));
}

fn quality_row(ui: &mut egui::Ui, quality: &mut u8) {
    theme::row(ui, &t!("record.quality"), |ui| {
        ui.add(egui::Slider::new(quality, 1..=100).suffix("%"));
    });
}

fn pick_file(app: &mut App) {
    let mut dialog = rfd::FileDialog::new()
        .add_filter(t!("convert.filter_media"), &mediaa_core::formats::all_media_extensions())
        .add_filter(t!("convert.filter_all"), &["*"]);
    if app.settings.media_dir.is_dir() {
        dialog = dialog.set_directory(&app.settings.media_dir);
    }
    if let Some(path) = dialog.pick_file() {
        app.conv.set_input(path);
    }
}

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    header(ui, &t!("convert.title"), "");
    // Dev: `MEDIAA_DEV_CONVERT=<file>` loads that file (and its first format) once, for UI checks.
    if app.conv.input.is_none() {
        if let Some(p) = std::env::var_os("MEDIAA_DEV_CONVERT") {
            app.conv.set_input(p.into());
            app.conv.target = app.conv.targets().first().copied();
        }
    }

    if let Some(job) = &app.conv.job {
        if let Ok(result) = job.done.try_recv() {
            let output = job.output.clone();
            app.conv.job = None;
            match result {
                Ok(()) => {
                    app.lib.files_changed(std::slice::from_ref(&output));
                    let name = output.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    app.success(t!("convert.done", name = name));
                    app.conv.last = Some(output);
                }
                Err(e) if e.is_empty() => app.info(t!("convert.cancelled")),
                Err(e) => app.error(t!("convert.failed", error = e)),
            }
        }
    }
    let busy = app.conv.job.is_some();
    // A file dropped onto the window is the new input.
    if !busy {
        if let Some(p) = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty())) {
            app.conv.set_input(p);
        }
    }

    // 1. File
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::FILE, &t!("convert.input"));
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, theme::secondary(icon::FOLDER_OPEN, &t!("convert.choose"))).on_hover_text(t!("convert.drop_hint")).clicked() {
                pick_file(app);
            }
            if let Some(p) = &app.conv.input {
                ui.add(egui::Label::new(RichText::new(p.file_name().unwrap_or_default().to_string_lossy()).strong()).truncate())
                    .on_hover_text(p.to_string_lossy());
            }
        });
        match (&app.conv.input, &app.conv.probed) {
            (Some(path), Some(Ok((kind, info)))) => {
                let mut facts = vec![i18n::media_kind(*kind)];
                facts.extend(std::fs::metadata(path).ok().map(|m| human_size(m.len())));
                if info.width > 0 {
                    facts.push(format!("{}×{}", info.width, info.height));
                }
                if let Some(d) = info.duration_secs() {
                    facts.push(human_duration(d));
                }
                if info.fps > 0.0 && *kind == MediaKind::Video {
                    facts.push(format!("{:.0} fps", info.fps));
                }
                // Sound only matters for videos (audio always has it, images and animations never).
                if *kind == MediaKind::Video {
                    facts.push(if info.has_audio() { t!("record.with_audio").into() } else { t!("record.no_audio").into() });
                }
                theme::hint(ui, facts.join(" · "));
            }
            (_, Some(Err(e))) => {
                ui.colored_label(theme::palette(ui).danger, e);
            }
            _ => {}
        }
    });

    let Some(Ok((_, info))) = app.conv.probed.clone() else { return };
    ui.add_space(4.0);

    // 2. Format
    let targets = app.conv.targets();
    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::SHUFFLE, &t!("convert.format"));
        ui.add_enabled_ui(!busy, |ui| {
            for (label, group) in [
                (t!("convert.group_video"), targets.iter().filter(|t| matches!(t, Target::Video(_))).copied().collect::<Vec<_>>()),
                (t!("convert.group_anim"), targets.iter().filter(|t| matches!(t, Target::Anim(_))).copied().collect()),
                (t!("convert.group_image"), targets.iter().filter(|t| matches!(t, Target::Still(_))).copied().collect()),
                (t!("convert.group_audio"), targets.iter().filter(|t| matches!(t, Target::Audio(_))).copied().collect()),
            ] {
                if group.is_empty() {
                    continue;
                }
                theme::row(ui, &label, |ui| {
                    let opts: Vec<(Option<Target>, String)> = group.iter().map(|t| (Some(*t), t.label().to_string())).collect();
                    theme::segmented(ui, &mut app.conv.target, &opts);
                });
            }
        });
    });

    let Some(target) = app.conv.target else { return };
    update_estimate(app, &ctx, target);
    ui.add_space(4.0);

    // 3. Settings, name and folder
    let mut changed = false;
    theme::split_card(
        ui,
        |ui| {
            theme::section(ui, icon::SLIDERS_HORIZONTAL, &t!("convert.settings"));
            ui.add_enabled_ui(!busy, |ui| {
                let s = &mut app.conv.settings;
                let source_h = (info.height > 0).then_some(info.height);
                let max_height = &mut app.conv.max_height;
                let mut resolution_row = |ui: &mut egui::Ui| {
                    theme::row(ui, &t!("record.resolution"), |ui| {
                        super::record::resolution_picker(ui, max_height, &OUTPUT_HEIGHTS, source_h, egui::Id::new("convert-resolution"));
                    });
                };
                match target {
                    Target::Video(_) => {
                        quality_row(ui, &mut s.quality);
                        effort_row(ui, &mut s.effort);
                        resolution_row(ui);
                        theme::row(ui, &t!("convert.fps"), |ui| fps_combo(ui, &mut s.max_fps, info.fps));
                        if info.has_audio() {
                            theme::row(ui, &t!("convert.audio"), |ui| {
                                ui.add_enabled_ui(!s.remove_audio, |ui| {
                                    kbps_combo(ui, &mut s.audio_kbps);
                                    mono_checkbox(ui, &mut s.mono);
                                });
                            });
                            theme::row(ui, "", |ui| ui.checkbox(&mut s.remove_audio, t!("convert.remove_audio")));
                        }
                    }
                    Target::Anim(fmt) => {
                        changed |= super::record::anim_options(ui, &mut s.anim, Some(fmt), source_h);
                        if fmt == mediaa_core::formats::AnimFormat::Webp {
                            effort_row(ui, &mut s.effort);
                        }
                    }
                    Target::Still(fmt) => {
                        match fmt {
                            StillFormat::Jpeg => quality_row(ui, &mut s.quality),
                            StillFormat::Webp => {
                                theme::row(ui, "", |ui| ui.checkbox(&mut s.webp_lossless, t!("convert.lossless")).on_hover_text(t!("convert.lossless_hint")));
                                if !s.webp_lossless {
                                    quality_row(ui, &mut s.quality);
                                }
                                effort_row(ui, &mut s.effort);
                            }
                            StillFormat::Png => {
                                theme::row(ui, &t!("convert.colors"), |ui| png_colors_combo(ui, &mut s.png_colors));
                                effort_row(ui, &mut s.effort);
                            }
                            StillFormat::Bmp | StillFormat::Tiff => {}
                        }
                        resolution_row(ui);
                    }
                    Target::Audio(fmt) => {
                        if !fmt.lossless() {
                            theme::row(ui, &t!("audio.bitrate"), |ui| kbps_combo(ui, &mut s.audio_kbps));
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
                let default_title = app
                    .conv
                    .input
                    .as_deref()
                    .and_then(|p| mediaa_core::naming::parse_path(p).map(|n| n.title).or_else(|| p.file_stem().map(|s| s.to_string_lossy().into_owned())))
                    .unwrap_or_default();
                theme::row(ui, &t!("convert.name"), |ui| {
                    ui.add(egui::TextEdit::singleline(&mut app.conv.name).hint_text(default_title).desired_width(f32::INFINITY));
                });
                let folder = app.conv.folder.clone().unwrap_or_else(|| app.settings.convert_dir());
                theme::row(ui, &t!("convert.folder"), |ui| {
                    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_directory(&folder).pick_folder() {
                            app.conv.folder = Some(p);
                        }
                    }
                    if app.conv.folder.is_some() && theme::icon_button(ui, icon::ARROW_COUNTER_CLOCKWISE, &t!("convert.folder_reset")).clicked() {
                        app.conv.folder = None;
                    }
                    ui.add(egui::Label::new(folder.to_string_lossy()).truncate());
                });
            });
            estimate_row(ui, app.conv.estimate.as_ref(), app.conv.input.as_deref());
        },
    );
    if changed {
        app.settings_changed();
    }
    ui.add_space(14.0);

    // 4. Convert (or its progress), centred.
    ui.vertical_centered(|ui| {
        if let Some(job) = &app.conv.job {
            let frac = job.progress.load(Ordering::Relaxed) as f32 / 1000.0;
            let cancel = job.cancel.clone();
            ui.add(egui::ProgressBar::new(frac).desired_width(420.0).show_percentage().animate(true));
            if ui.add(theme::secondary(icon::X, &t!("common.cancel"))).clicked() {
                cancel.store(true, Ordering::Relaxed);
            }
            ctx.request_repaint_after(Duration::from_millis(200));
        } else {
            let btn = theme::primary(ui, icon::SHUFFLE, &t!("convert.start")).min_size(egui::vec2(240.0, 38.0));
            if ui.add(btn).clicked() {
                start(app, &ctx, target);
            }
            if let Some(last) = app.conv.last.clone() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    // Centre the row: the label and its two buttons.
                    let name = last.file_name().unwrap_or_default().to_string_lossy().into_owned();
                    let w = ui.fonts_mut(|f| f.layout_no_wrap(name.clone(), egui::FontId::proportional(13.0), egui::Color32::WHITE).size().x) + 60.0;
                    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                    ui.label(RichText::new(format!("{}  {name}", icon::CHECK_CIRCLE)).color(theme::palette(ui).text_weak));
                    if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, &t!("lib.open")).clicked() {
                        app.open_path(&last);
                    }
                    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("lib.reveal")).clicked() {
                        let _ = mediaa_platform::files::reveal(&last);
                    }
                });
            }
        }
    });
}
