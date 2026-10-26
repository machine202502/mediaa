use crate::jobs::Job;
use crate::pages::{activity, header};
use crate::{i18n, theme};
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

struct ConversionJob {
    output: OutputPath,
    progress: Arc<AtomicI32>,
    started: Instant,
    job: Job<ConversionTaskResult>,
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

pub enum ConversionUiAction {
    Open(PathBuf),
    Reveal(PathBuf),
    Error(String),
}

#[derive(Clone)]
struct ProbeInfo {
    kind: MediaKind,
    info: MediaInfo,
    size: Option<u64>,
}

type ProbeResult = Result<ProbeInfo, String>;

#[derive(Default)]
pub struct ConversionController {
    input: Option<PathBuf>,
    probed: Option<ProbeResult>,
    probe_job: Option<Job<ProbeResult>>,
    probe_cancel: Arc<AtomicBool>,
    target: Option<Target>,
    settings: ConvertSettings,
    max_height: u32,
    name: String,
    folder: Option<PathBuf>,
    job: Option<ConversionJob>,
    last: Option<PathBuf>,
    estimate: Option<Estimate>,
}

const ESTIMATE_DELAY: Duration = Duration::from_millis(500);
const ESTIMATE_TIMEOUT: Duration = Duration::from_secs(120);

struct Estimate {
    key: (PathBuf, Target, ConvertSettings),
    since: Instant,
    cancel: Arc<AtomicBool>,
    job: Option<Job<Result<u64, String>>>,
    result: Option<Result<u64, String>>,
    previous: Option<u64>,
    started: Option<Instant>,
}

impl Drop for Estimate {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl ConversionController {
    pub fn set_input(&mut self, path: PathBuf) {
        self.estimate = None;
        if let Some(job) = self.probe_job.take() {
            job.cancel();
        }
        self.probe_cancel = Arc::default();
        self.name.clear();
        self.input = Some(path.clone());
        self.last = None;
        if MediaKind::detect(&path).is_none() {
            self.probed = Some(Err(t!("convert.unsupported").into()));
        } else {
            self.probed = None;
        }
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn cancel_estimate(&mut self) {
        self.estimate = None;
    }

    fn poll_probe(&mut self, ctx: &egui::Context) {
        if self.probe_job.is_none() && self.probed.is_none() {
            let Some(path) = self.input.clone() else {
                return;
            };
            let Some(kind) = MediaKind::detect(&path) else {
                self.probed = Some(Err(t!("convert.unsupported").into()));
                return;
            };
            let cancel = self.probe_cancel.clone();
            let worker_cancel = cancel.clone();
            match Job::spawn("convert-probe", cancel, move || {
                mediaa_media::preview::details_cancellable(&path, kind, &worker_cancel)
                    .map(|info| ProbeInfo {
                        kind,
                        info,
                        size: std::fs::metadata(&path).ok().map(|metadata| metadata.len()),
                    })
                    .map_err(|error| format!("{error:#}"))
            }) {
                Ok(job) => self.probe_job = Some(job),
                Err(error) => self.probed = Some(Err(format!("probe: {error}"))),
            }
        }
        let Some(job) = self.probe_job.as_mut() else {
            return;
        };
        let Some(result) = job.poll() else {
            ctx.request_repaint_after(Duration::from_millis(50));
            return;
        };
        self.probe_job = None;
        self.probed = Some(result.unwrap_or_else(|_| Err(t!("convert.probe_stopped").into())));
        let targets = self.targets();
        if self.target.is_some_and(|target| !targets.contains(&target)) {
            self.target = None;
        }
    }

    pub(crate) fn poll_job(&mut self, ctx: &egui::Context) -> Option<ConversionOutcome> {
        let job = self.job.as_mut()?;
        let Some(result) = job.job.poll() else {
            ctx.request_repaint_after(Duration::from_millis(100));
            return None;
        };
        let job = self.job.take().expect("job checked above");
        let outcome = match result {
            Err(_) => ConversionOutcome::Failed {
                kind: MediaErrorKind::Internal,
                message: t!("convert.worker_stopped").into(),
            },
            Ok(ConversionTaskResult::Completed) => {
                self.last = Some(job.output.clone().into_path());
                ConversionOutcome::Finished(job.output)
            }
            Ok(ConversionTaskResult::Cancelled) => ConversionOutcome::Cancelled,
            Ok(ConversionTaskResult::Failed { kind, message }) => {
                ConversionOutcome::Failed { kind, message }
            }
        };
        Some(outcome)
    }

    fn start_job(
        &mut self,
        ctx: &egui::Context,
        target: Target,
        convert_dir: &std::path::Path,
    ) -> Option<String> {
        let input = self.input.clone()?;
        let folder = self
            .folder
            .clone()
            .unwrap_or_else(|| convert_dir.to_path_buf());
        let output = convert::output_path(
            &mediaa_core::paths::SourcePath::new(input.clone()),
            target,
            Some(&self.name),
            &mediaa_core::paths::MediaDir::new(folder),
        );
        let settings = self.effective();
        self.estimate = None;
        let progress = Arc::new(AtomicI32::new(-1));
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_output = output.clone();
        let worker_cancel = cancel.clone();
        let worker_progress = progress.clone();
        let worker_ctx = ctx.clone();
        let job = Job::spawn("convert", cancel, move || {
            let mut report = |value: Option<f32>| {
                worker_progress.store(
                    value.map_or(-1, |value| (value * 1000.0) as i32),
                    Ordering::Relaxed,
                );
                worker_ctx.request_repaint();
            };
            let source = mediaa_core::paths::SourcePath::new(input);
            match convert::convert(
                &source,
                &worker_output,
                target,
                &settings,
                &mut report,
                &worker_cancel,
            ) {
                Ok(()) => ConversionTaskResult::Completed,
                Err(error) if mediaa_media::job::is_cancelled(&error) => {
                    ConversionTaskResult::Cancelled
                }
                Err(error) => ConversionTaskResult::Failed {
                    kind: classify_media_error(&error),
                    message: format!("{error:#}"),
                },
            }
        });
        match job {
            Ok(job) => {
                self.job = Some(ConversionJob {
                    output,
                    progress,
                    started: Instant::now(),
                    job,
                });
                None
            }
            Err(error) => Some(t!("convert.start_failed", error = error).into()),
        }
    }

    fn update_estimate(&mut self, ctx: &egui::Context, target: Target) {
        let Some(input) = self.input.clone() else {
            return;
        };
        let key = (input, target, self.effective());
        if self
            .estimate
            .as_ref()
            .is_none_or(|estimate| estimate.key != key)
        {
            let previous = self.estimate.as_ref().and_then(|estimate| {
                estimate
                    .result
                    .as_ref()
                    .and_then(|result| result.as_ref().ok().copied())
                    .or(estimate.previous)
            });
            self.estimate = Some(Estimate {
                key,
                since: Instant::now(),
                cancel: Arc::default(),
                job: None,
                result: None,
                previous,
                started: None,
            });
        }
        let busy = self.is_busy();
        let Some(estimate) = self.estimate.as_mut() else {
            return;
        };
        if estimate.result.is_some() {
            return;
        }
        if estimate.job.is_none() && !busy && estimate.since.elapsed() >= ESTIMATE_DELAY {
            let (key, cancel, ctx) = (estimate.key.clone(), estimate.cancel.clone(), ctx.clone());
            let worker_cancel = cancel.clone();
            match Job::spawn("convert-estimate", cancel, move || {
                let (input, target, settings) = key;
                let input = mediaa_core::paths::SourcePath::new(input);
                let result = convert::estimate_size(&input, target, &settings, &worker_cancel)
                    .map_err(|error| format!("{error:#}"));
                ctx.request_repaint();
                result
            }) {
                Ok(job) => {
                    estimate.job = Some(job);
                    estimate.started = Some(Instant::now());
                }
                Err(error) => estimate.result = Some(Err(format!("estimate: {error}"))),
            }
        }
        if let Some(job) = estimate.job.as_mut() {
            if let Some(result) = job.poll() {
                estimate.job = None;
                estimate.started = None;
                estimate.result =
                    Some(result.unwrap_or_else(|_| Err(t!("convert.worker_stopped").into())));
            } else if estimate
                .started
                .is_some_and(|started| started.elapsed() >= ESTIMATE_TIMEOUT)
            {
                job.cancel();
                estimate.job = None;
                estimate.started = None;
                estimate.result = Some(Err(t!("convert.estimate_timeout").into()));
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        } else if estimate.result.is_none() {
            ctx.request_repaint_after(ESTIMATE_DELAY);
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
            Some(Ok(probe)) => Target::for_kind(probe.kind)
                .into_iter()
                .filter(|t| !matches!(t, Target::Audio(_)) || probe.info.has_audio())
                .collect(),
            _ => Vec::new(),
        }
    }
}

mod controls;
use controls::{
    effort_row, estimate_row, fps_combo, kbps_combo, mono_checkbox, pick_file_dialog,
    png_colors_combo, quality_row,
};

impl ConversionController {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        convert_dir: &std::path::Path,
    ) -> Option<ConversionUiAction> {
        let ctx = ui.ctx().clone();
        self.poll_probe(&ctx);
        header(ui, &t!("convert.title"), "");
        if self.input.is_none() {
            if let Some(p) = mediaa_core::dev::convert_input() {
                self.set_input(p);
            }
        }
        let busy = self.is_busy();
        if !busy {
            if let Some(p) = ctx.input(|i| {
                i.raw
                    .dropped_files
                    .iter()
                    .map(|f| f.path().to_path_buf())
                    .find(|p| !p.as_os_str().is_empty())
            }) {
                self.set_input(p);
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
                    pick_file_dialog(self, convert_dir);
                }
                if let Some(p) = &self.input {
                    ui.add(
                        egui::Label::new(
                            RichText::new(p.file_name().unwrap_or_default().to_string_lossy())
                                .strong(),
                        )
                        .truncate(),
                    )
                    .on_hover_text(p.to_string_lossy());
                }
            });
            match (&self.input, &self.probed) {
                (Some(_), Some(Ok(probe))) => {
                    let mut facts = vec![i18n::media_kind(probe.kind)];
                    facts.extend(probe.size.map(human_size));
                    if probe.info.width > 0 {
                        facts.push(format!("{}×{}", probe.info.width, probe.info.height));
                    }
                    if let Some(d) = probe.info.duration_secs() {
                        facts.push(human_duration(d));
                    }
                    if probe.info.fps > 0.0 && probe.kind == MediaKind::Video {
                        facts.push(format!("{:.0} fps", probe.info.fps));
                    }
                    if probe.kind == MediaKind::Video {
                        facts.push(if probe.info.has_audio() {
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

        let Some(Ok(probe)) = self.probed.clone() else {
            return None;
        };
        let info = probe.info;
        ui.add_space(4.0);

        let targets = self.targets();
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
        self.update_estimate(&ctx, target);
        ui.add_space(4.0);

        let mut action = None;
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
                            crate::pages::record::resolution_picker(
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
                            crate::pages::record::anim_options(
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
                    let default_title = self
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
                            egui::TextEdit::singleline(&mut self.name)
                                .hint_text(default_title)
                                .desired_width(f32::INFINITY),
                        );
                    });
                    let folder = self
                        .folder
                        .clone()
                        .unwrap_or_else(|| convert_dir.to_path_buf());
                    theme::row(ui, &t!("convert.folder"), |ui| {
                        if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("settings.choose_folder"))
                            .clicked()
                        {
                            if let Some(p) =
                                rfd::FileDialog::new().set_directory(&folder).pick_folder()
                            {
                                self.folder = Some(p);
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
                        }
                        ui.add(egui::Label::new(folder.to_string_lossy()).truncate());
                    });
                });
                estimate_row(ui, self.estimate.as_ref(), probe.size);
            },
        );
        ui.add_space(14.0);

        ui.vertical_centered(|ui| {
            if let Some(job) = &self.job {
                let value = job.progress.load(Ordering::Relaxed);
                let frac = value.max(0) as f32 / 1000.0;
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
                    job.job.cancel();
                }
                ctx.request_repaint_after(Duration::from_millis(200));
            } else {
                let btn = theme::primary(ui, icon::SHUFFLE, &t!("convert.start"))
                    .min_size(egui::vec2(240.0, 38.0));
                if ui.add(btn).clicked() {
                    if let Some(error) = self.start_job(&ctx, target, convert_dir) {
                        action = Some(ConversionUiAction::Error(error));
                    }
                }
                if let Some(last) = self.last.clone() {
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
                        if theme::icon_button(ui, icon::ARROW_SQUARE_OUT, &t!("lib.open")).clicked()
                        {
                            action = Some(ConversionUiAction::Open(last.clone()));
                        }
                        if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("lib.reveal")).clicked() {
                            action = Some(ConversionUiAction::Reveal(last));
                        }
                    });
                }
            }
        });
        action
    }
}

#[cfg(test)]
mod tests;
