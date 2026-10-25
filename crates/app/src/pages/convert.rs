use super::{activity, header};
use crate::app::tasks::{TaskId, TaskKind, TaskSupervisor};
use crate::app::App;
use crate::{i18n, theme};
use crossbeam_channel::Receiver;
use eframe::egui::{self, RichText};
use egui_phosphor::regular as icon;
use mediaa_core::library::{human_duration, human_size};
use mediaa_core::paths::OutputPath;
use mediaa_core::settings::{AUDIO_BITRATES, OUTPUT_HEIGHTS};
use mediaa_core::MediaKind;
use mediaa_media::convert::{
    self, ConvertSettings, Effort, StillFormat, Target, PNG_COLORS, VIDEO_FPS,
};
use mediaa_media::error::{classify_media_error, MediaErrorKind};
use mediaa_media::info::MediaInfo;
use rust_i18n::t;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Job {
    output: OutputPath,
    progress: Arc<AtomicI32>,
    started: Instant,
    cancel: Arc<AtomicBool>,
    done: Receiver<ConversionTaskResult>,
    task: TaskId,
}

enum ConversionTaskResult {
    Completed,
    Failed {
        kind: MediaErrorKind,
        message: String,
    },
    Cancelled,
}

pub enum ConversionOutcome {
    Finished(OutputPath),
    Failed {
        kind: MediaErrorKind,
        message: String,
    },
    Cancelled,
}

type ProbeResult = Result<(MediaKind, MediaInfo), String>;

#[derive(Default)]
pub struct ConvertUi {
    input: Option<PathBuf>,
    probed: Option<ProbeResult>,
    probe_rx: Option<Receiver<ProbeResult>>,
    target: Option<Target>,
    settings: ConvertSettings,
    max_height: u32,
    name: String,
    folder: Option<PathBuf>,
    job: Option<Job>,
    last: Option<PathBuf>,
    estimate: Option<Estimate>,
}

const ESTIMATE_DELAY: Duration = Duration::from_millis(500);

struct Estimate {
    key: (PathBuf, Target, ConvertSettings),
    since: Instant,
    cancel: Arc<AtomicBool>,
    rx: Option<Receiver<Result<u64, String>>>,
    result: Option<Result<u64, String>>,
    previous: Option<u64>,
}

impl Drop for Estimate {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl ConvertUi {
    pub fn set_input(&mut self, path: PathBuf) {
        self.estimate = None;
        self.probe_rx = None;
        self.name.clear();
        self.input = Some(path.clone());
        self.last = None;
        match MediaKind::detect(&path) {
            None => {
                self.probed = Some(Err(t!("convert.unsupported").into()));
            }
            Some(kind) => {
                self.probed = None;
                let (tx, rx) = crossbeam_channel::bounded(1);
                let result = std::thread::Builder::new()
                    .name("convert-probe".into())
                    .spawn(move || {
                        let result = mediaa_media::preview::details(&path, kind)
                            .map(|info| (kind, info))
                            .map_err(|error| format!("{error:#}"));
                        let _ = tx.send(result);
                    });
                match result {
                    Ok(_) => self.probe_rx = Some(rx),
                    Err(error) => self.probed = Some(Err(format!("probe: {error}"))),
                }
            }
        }
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn cancel_estimate(&mut self) {
        self.estimate = None;
    }

    pub fn poll_probe(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.probe_rx else {
            return;
        };
        match rx.try_recv() {
            Ok(result) => {
                self.probed = Some(result);
                self.probe_rx = None;
                let targets = self.targets();
                if self.target.is_some_and(|target| !targets.contains(&target)) {
                    self.target = None;
                }
            }
            Err(crossbeam_channel::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(50))
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => {
                self.probed = Some(Err(t!("convert.probe_stopped").into()));
                self.probe_rx = None;
            }
        }
    }

    pub(crate) fn poll_job(
        &mut self,
        tasks: &mut TaskSupervisor,
        ctx: &egui::Context,
    ) -> Option<ConversionOutcome> {
        let job = self.job.as_ref()?;
        let received = match job.done.try_recv() {
            Ok(result) => result,
            Err(crossbeam_channel::TryRecvError::Empty) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                return None;
            }
            Err(crossbeam_channel::TryRecvError::Disconnected) => ConversionTaskResult::Failed {
                kind: MediaErrorKind::Internal,
                message: t!("convert.worker_stopped").into(),
            },
        };
        let job = self.job.take().expect("job checked above");
        let thread_failed = tasks.join(job.task).is_some_and(|exit| exit.panicked);
        let outcome = match received {
            ConversionTaskResult::Completed => {
                self.last = Some(job.output.clone().into_path());
                ConversionOutcome::Finished(job.output)
            }
            ConversionTaskResult::Cancelled => ConversionOutcome::Cancelled,
            ConversionTaskResult::Failed { kind, message } => {
                ConversionOutcome::Failed { kind, message }
            }
        };
        if thread_failed {
            Some(ConversionOutcome::Failed {
                kind: MediaErrorKind::Internal,
                message: t!("convert.worker_stopped").into(),
            })
        } else {
            Some(outcome)
        }
    }

    fn effective(&self) -> ConvertSettings {
        ConvertSettings {
            max_height: (self.max_height > 0).then_some(self.max_height),
            ..self.settings.clone()
        }
    }

    fn targets(&self) -> Vec<Target> {
        match &self.probed {
            Some(Ok((kind, info))) => Target::for_kind(*kind)
                .into_iter()
                .filter(|t| !matches!(t, Target::Audio(_)) || info.has_audio())
                .collect(),
            _ => Vec::new(),
        }
    }
}

mod worker;
use worker::{start, update_estimate};

mod controls;
use controls::{
    effort_row, estimate_row, fps_combo, kbps_combo, mono_checkbox, pick_file, png_colors_combo,
    quality_row,
};

pub fn ui(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    app.conv.poll_probe(&ctx);
    header(ui, &t!("convert.title"), "");
    if app.conv.input.is_none() {
        if let Some(p) = std::env::var_os("MEDIAA_DEV_CONVERT") {
            app.conv.set_input(p.into());
        }
    }
    let busy = app.conv.is_busy();
    if !busy {
        if let Some(p) = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .find(|p| !p.as_os_str().is_empty())
        }) {
            app.conv.set_input(p);
        }
    }

    theme::card(ui).show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section(ui, icon::FILE, &t!("convert.input"));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !busy,
                    theme::secondary(icon::FOLDER_OPEN, &t!("convert.choose")),
                )
                .on_hover_text(t!("convert.drop_hint"))
                .clicked()
            {
                pick_file(app);
            }
            if let Some(p) = &app.conv.input {
                ui.add(
                    egui::Label::new(
                        RichText::new(p.file_name().unwrap_or_default().to_string_lossy()).strong(),
                    )
                    .truncate(),
                )
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
                if *kind == MediaKind::Video {
                    facts.push(if info.has_audio() {
                        t!("record.with_audio").into()
                    } else {
                        t!("record.no_audio").into()
                    });
                }
                theme::hint(ui, facts.join(" · "));
            }
            (_, Some(Err(e))) => {
                ui.colored_label(theme::palette(ui).danger, e);
            }
            _ => {}
        }
    });

    let Some(Ok((_, info))) = app.conv.probed.clone() else {
        return;
    };
    ui.add_space(4.0);

    let targets = app.conv.targets();
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
                    theme::segmented(ui, &mut app.conv.target, &opts);
                });
            }
        });
    });

    let Some(target) = app.conv.target else {
        return;
    };
    update_estimate(app, &ctx, target);
    ui.add_space(4.0);

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
                        super::record::resolution_picker(
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
                            theme::row(ui, &t!("convert.audio"), |ui| {
                                ui.add_enabled_ui(!s.remove_audio, |ui| {
                                    kbps_combo(ui, &mut s.audio_kbps);
                                    mono_checkbox(ui, &mut s.mono);
                                });
                            });
                            theme::row(ui, "", |ui| {
                                ui.checkbox(&mut s.remove_audio, t!("convert.remove_audio"))
                            });
                        }
                    }
                    Target::Anim(fmt) => {
                        changed |=
                            super::record::anim_options(ui, &mut s.anim, Some(fmt), source_h, true);
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
                let default_title = app
                    .conv
                    .input
                    .as_deref()
                    .and_then(|p| {
                        mediaa_core::naming::parse_path(p)
                            .map(|n| n.title)
                            .or_else(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                    })
                    .unwrap_or_default();
                theme::row(ui, &t!("convert.name"), |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut app.conv.name)
                            .hint_text(default_title)
                            .desired_width(f32::INFINITY),
                    );
                });
                let folder = app
                    .conv
                    .folder
                    .clone()
                    .unwrap_or_else(|| app.settings.convert_dir());
                theme::row(ui, &t!("convert.folder"), |ui| {
                    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                        .clicked()
                    {
                        if let Some(p) = rfd::FileDialog::new().set_directory(&folder).pick_folder()
                        {
                            app.conv.folder = Some(p);
                        }
                    }
                    if app.conv.folder.is_some()
                        && theme::icon_button(
                            ui,
                            icon::ARROW_COUNTER_CLOCKWISE,
                            &t!("convert.folder_reset"),
                        )
                        .clicked()
                    {
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

    ui.vertical_centered(|ui| {
        if let Some(job) = &app.conv.job {
            let value = job.progress.load(Ordering::Relaxed);
            let frac = value.max(0) as f32 / 1000.0;
            let cancel = job.cancel.clone();
            if value < 0 {
                activity(ui, &t!("convert.processing"), job.started.elapsed());
            } else {
                ui.add(
                    egui::ProgressBar::new(frac)
                        .desired_width(420.0)
                        .show_percentage()
                        .animate(true),
                );
            }
            if ui
                .add(theme::secondary(icon::X, &t!("common.cancel")))
                .clicked()
            {
                cancel.store(true, Ordering::Relaxed);
            }
            ctx.request_repaint_after(Duration::from_millis(200));
        } else {
            let btn = theme::primary(ui, icon::SHUFFLE, &t!("convert.start"))
                .min_size(egui::vec2(240.0, 38.0));
            if ui.add(btn).clicked() {
                start(app, &ctx, target);
            }
            if let Some(last) = app.conv.last.clone() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let name = last
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    let w = ui.fonts_mut(|f| {
                        f.layout_no_wrap(
                            name.clone(),
                            egui::FontId::proportional(13.0),
                            egui::Color32::WHITE,
                        )
                        .size()
                        .x
                    }) + 60.0;
                    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                    ui.label(
                        RichText::new(format!("{}  {name}", icon::CHECK_CIRCLE))
                            .color(theme::palette(ui).text_weak),
                    );
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

#[cfg(test)]
mod tests;
