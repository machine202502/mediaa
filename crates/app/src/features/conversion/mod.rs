use crate::jobs::Job;
use crate::pages::header;
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
    Error(String),
}

#[derive(Clone, Copy)]
pub(crate) struct ConversionStatus {
    pub progress: Option<f32>,
    pub elapsed: Duration,
    pub done: usize,
    pub total: usize,
}

#[derive(Clone)]
struct ProbeInfo {
    kind: MediaKind,
    info: MediaInfo,
    size: Option<u64>,
}

type ProbeResult = Result<ProbeInfo, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BatchStatus {
    Pending,
    Running,
    Done,
    Failed,
    Cancelled,
}

#[derive(Clone)]
struct BatchItem {
    path: PathBuf,
    kind: Option<MediaKind>,
    size: Option<u64>,
    duration: Option<f64>,
    status: BatchStatus,
    error: Option<String>,
}

#[derive(Default)]
pub struct ConversionController {
    input: Option<PathBuf>,
    queue: Vec<PathBuf>,
    batch_total: usize,
    batch_done: usize,
    batch_auto: bool,
    failed: Vec<(PathBuf, String)>,
    batch: Vec<BatchItem>,
    conflict_notified: bool,
    probed: Option<ProbeResult>,
    probe_job: Option<Job<ProbeResult>>,
    probe_cancel: Arc<AtomicBool>,
    probe_loading: bool,
    target: Option<Target>,
    settings: ConvertSettings,
    max_height: u32,
    folder: Option<PathBuf>,
    job: Option<ConversionJob>,
    run_started: Option<Instant>,
}

impl ConversionController {
    pub fn set_input(&mut self, path: PathBuf) {
        self.set_inputs(vec![path]);
    }

    pub fn set_inputs(&mut self, paths: Vec<PathBuf>) {
        let mut unique = Vec::new();
        for path in paths {
            if !unique.contains(&path) {
                unique.push(path);
            }
        }
        let Some(first) = unique.first().cloned() else {
            return;
        };
        self.batch_total = unique.len();
        self.batch_done = 0;
        self.batch_auto = false;
        self.failed.clear();
        self.conflict_notified = false;
        self.queue = unique.iter().skip(1).cloned().collect();
        self.batch = unique
            .iter()
            .map(|path| BatchItem {
                path: path.clone(),
                kind: MediaKind::detect(std::path::Path::new(path)),
                size: std::fs::metadata(path).ok().map(|metadata| metadata.len()),
                duration: None,
                status: BatchStatus::Pending,
                error: None,
            })
            .collect();
        self.target = None;
        self.set_current(first);
    }

    pub fn add_inputs(&mut self, paths: Vec<PathBuf>) {
        let mut unique = Vec::new();
        for path in paths {
            if !unique.contains(&path) {
                unique.push(path);
            }
        }
        if unique.is_empty() {
            return;
        }
        if self.batch.is_empty() {
            self.set_inputs(unique);
            return;
        }
        let first = self.input.is_none().then(|| unique[0].clone());
        for path in unique {
            if self.batch.iter().any(|item| item.path == path) {
                continue;
            }
            self.batch.push(BatchItem {
                kind: MediaKind::detect(&path),
                size: std::fs::metadata(&path).ok().map(|metadata| metadata.len()),
                duration: None,
                status: BatchStatus::Pending,
                error: None,
                path,
            });
        }
        self.batch_total = self.batch.len();
        self.conflict_notified = false;
        if let Some(path) = first {
            self.set_current(path);
        }
    }
    fn set_current(&mut self, path: PathBuf) {
        if let Some(job) = self.probe_job.take() {
            job.cancel();
        }
        self.probe_cancel = Arc::default();
        self.input = Some(path.clone());
        if self.batch_auto {
            self.probe_loading = true;
        } else {
            self.probe_loading = false;
            self.probed = None;
        }
        if MediaKind::detect(&path).is_none() {
            self.probed = Some(Err(t!("convert.unsupported").into()));
            self.probe_loading = false;
        }
    }

    fn mark_batch(&mut self, path: &std::path::Path, status: BatchStatus, error: Option<String>) {
        if let Some(item) = self.batch.iter_mut().find(|item| item.path == path) {
            item.status = status;
            item.error = error;
        }
    }

    fn retry_batch(&mut self, path: &std::path::Path) {
        if let Some(item) = self.batch.iter_mut().find(|item| item.path == path) {
            item.status = BatchStatus::Pending;
            item.error = None;
        }
        self.failed.retain(|(failed, _)| failed != path);
        self.queue.retain(|queued| queued != path);
        self.conflict_notified = false;
        self.batch_auto = false;
        self.target = None;
        self.set_current(path.to_path_buf());
    }

    fn remove_batch(&mut self, path: &std::path::Path) {
        self.batch.retain(|item| item.path != path);
        self.queue.retain(|queued| queued != path);
        self.failed.retain(|(failed, _)| failed != path);
        self.batch_total = self.batch.len();
        self.conflict_notified = false;
        if self.input.as_deref() == Some(path) {
            self.input = None;
            if let Some(next) = self.queue.first().cloned() {
                self.queue.remove(0);
                self.set_current(next);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn test_status(&mut self, progress: i32, elapsed: Duration) {
        if let Some(job) = &mut self.job {
            job.progress.store(progress, Ordering::Relaxed);
            job.started = Instant::now() - elapsed;
        }
    }
    #[cfg(test)]
    pub(crate) fn test_busy(&mut self, progress: i32, elapsed: Duration) -> Arc<AtomicBool> {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let job = Job::spawn("convert-test-busy", cancel.clone(), move || {
            while !worker_cancel.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            ConversionTaskResult::Cancelled
        })
        .unwrap();
        self.run_started = Some(Instant::now() - elapsed);
        self.job = Some(ConversionJob {
            output: OutputPath::new("convert-status.mp4"),
            progress: Arc::new(AtomicI32::new(progress)),
            started: Instant::now() - elapsed,
            job,
        });
        cancel
    }
    #[cfg(test)]
    pub(crate) fn test_batch_statuses(&self) -> Vec<BatchStatus> {
        self.batch.iter().map(|item| item.status).collect()
    }
    #[cfg(test)]
    pub(crate) fn test_batch_is_empty(&self) -> bool {
        self.batch.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn test_folder(&self) -> Option<&std::path::Path> {
        self.folder.as_deref()
    }

    #[cfg(test)]
    pub(crate) fn test_batch(
        &mut self,
        items: &[(&str, BatchStatus, Option<&str>)],
        target: Option<Target>,
    ) {
        let paths: Vec<PathBuf> = items
            .iter()
            .map(|(path, _, _)| PathBuf::from(path))
            .collect();
        self.batch = items
            .iter()
            .map(|(path, status, error)| BatchItem {
                path: PathBuf::from(path),
                kind: MediaKind::detect(std::path::Path::new(path)),
                size: Some(1024),
                duration: Some(12.0),
                status: *status,
                error: error.map(str::to_string),
            })
            .collect();
        self.batch_total = self.batch.len();
        self.batch_done = self
            .batch
            .iter()
            .filter(|item| matches!(item.status, BatchStatus::Done | BatchStatus::Failed))
            .count();
        self.input = paths.first().cloned();
        self.queue = paths.into_iter().skip(1).collect();
        self.target = target;
        self.probed = self.input.as_ref().map(|_| {
            Ok(ProbeInfo {
                kind: self
                    .batch
                    .first()
                    .and_then(|item| item.kind)
                    .unwrap_or(MediaKind::Video),
                info: MediaInfo {
                    duration_us: Some(12_000_000),
                    width: 1920,
                    height: 1080,
                    fps: 30.0,
                    video_codec: Some("h264".into()),
                    audio_codec: Some("aac".into()),
                    audio_streams: 1,
                },
                size: Some(1024),
            })
        });
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some() || self.batch_auto
    }

    pub(crate) fn can_start(&self) -> bool {
        !self.is_busy()
            && self.input.is_some()
            && self.target.is_some()
            && self
                .batch
                .iter()
                .any(|item| item.status != BatchStatus::Done)
    }

    pub(crate) fn status(&self) -> Option<ConversionStatus> {
        if self.job.is_none() && !self.batch_auto {
            return None;
        }
        let total = self.batch_total.max(1);
        let done = self.batch_done.min(total);
        let current = self.job.as_ref().map_or(0.0, |job| {
            let value = job.progress.load(Ordering::Relaxed);
            if value >= 0 {
                value as f32 / 1000.0
            } else {
                0.0
            }
        });
        let progress = ((done as f32 + current) / total as f32).clamp(0.0, 1.0);
        let elapsed = self
            .run_started
            .or_else(|| self.job.as_ref().map(|job| job.started))
            .map_or(Duration::ZERO, |started| started.elapsed());
        Some(ConversionStatus {
            progress: Some(progress),
            elapsed,
            done,
            total,
        })
    }

    pub(crate) fn cancel(&mut self) {
        self.batch_auto = false;
        self.queue.clear();
        if let Some(job) = &self.job {
            job.job.cancel();
        }
    }

    pub(crate) fn start_from_ui(
        &mut self,
        ctx: &egui::Context,
        convert_dir: &std::path::Path,
    ) -> Option<String> {
        if !self.can_start() {
            return None;
        }
        if !self.prepare_batch_run() {
            return None;
        }
        let target = self.target?;
        self.start_job(ctx, target, convert_dir)
    }

    fn prepare_batch_run(&mut self) -> bool {
        let mut paths = Vec::new();
        for item in &self.batch {
            if item.status != BatchStatus::Done {
                paths.push(item.path.clone());
            }
        }
        let Some(first) = paths.first().cloned() else {
            return false;
        };
        self.input = Some(first);
        self.queue = paths.into_iter().skip(1).collect();
        self.batch_done = self
            .batch
            .iter()
            .filter(|item| item.status == BatchStatus::Done)
            .count();
        self.failed.clear();
        self.batch_auto = false;
        self.conflict_notified = false;
        self.run_started = Some(Instant::now());
        true
    }

    fn reset_batch_statuses(&mut self) {
        for item in &mut self.batch {
            item.status = BatchStatus::Pending;
            item.error = None;
        }
        self.batch_done = 0;
        self.failed.clear();
        self.queue.clear();
        self.batch_auto = false;
        self.conflict_notified = false;
        self.run_started = None;
    }

    pub(crate) fn clear_batch(&mut self) {
        if self.is_busy() {
            return;
        }
        if let Some(job) = self.probe_job.take() {
            job.cancel();
        }
        self.probe_cancel = Arc::default();
        self.input = None;
        self.queue.clear();
        self.batch.clear();
        self.batch_total = 0;
        self.batch_done = 0;
        self.batch_auto = false;
        self.failed.clear();
        self.conflict_notified = false;
        self.probed = None;
        self.target = None;
        self.run_started = None;
    }

    fn poll_probe(&mut self, ctx: &egui::Context) {
        if self.probe_job.is_none() && (self.probed.is_none() || self.probe_loading) {
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
                Err(error) => {
                    self.probed = Some(Err(format!("probe: {error}")));
                    self.probe_loading = false;
                }
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
        self.probe_loading = false;
        self.probed = Some(result.unwrap_or_else(|_| Err(t!("convert.probe_stopped").into())));
        if let Some(Ok(probe)) = self.probed.clone() {
            if let Some(path) = self.input.clone() {
                if let Some(item) = self.batch.iter_mut().find(|item| item.path == path) {
                    item.kind = Some(probe.kind);
                    item.size = probe.size;
                    item.duration = probe.info.duration_secs();
                }
            }
        }
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
            Ok(ConversionTaskResult::Completed) => ConversionOutcome::Finished(job.output),
            Ok(ConversionTaskResult::Cancelled) => {
                let path = job.output.clone().into_path();
                let _ = std::fs::remove_file(path);
                ConversionOutcome::Cancelled
            }
            Ok(ConversionTaskResult::Failed { kind, message }) => {
                ConversionOutcome::Failed { kind, message }
            }
        };
        if let Some(path) = self.input.clone() {
            match &outcome {
                ConversionOutcome::Finished(_) => {
                    self.mark_batch(&path, BatchStatus::Done, None);
                }
                ConversionOutcome::Failed { message, .. } => {
                    self.failed.push((path.clone(), message.clone()));
                    self.mark_batch(&path, BatchStatus::Failed, Some(message.clone()));
                }
                ConversionOutcome::Cancelled => {
                    self.mark_batch(&path, BatchStatus::Cancelled, None);
                }
            }
        }
        self.batch_done += 1;
        if !matches!(outcome, ConversionOutcome::Cancelled) {
            if let Some(next) = self.queue.first().cloned() {
                self.queue.remove(0);
                self.batch_auto = true;
                self.set_current(next);
            }
        } else {
            self.queue.clear();
            self.batch_auto = false;
        }
        Some(outcome)
    }

    fn start_job(
        &mut self,
        ctx: &egui::Context,
        target: Target,
        convert_dir: &std::path::Path,
    ) -> Option<String> {
        let input = self.input.clone()?;
        self.mark_batch(&input, BatchStatus::Running, None);
        let folder = self
            .folder
            .clone()
            .unwrap_or_else(|| convert_dir.to_path_buf());
        let output = convert::output_path(
            &mediaa_core::paths::SourcePath::new(input.clone()),
            target,
            None,
            &mediaa_core::paths::MediaDir::new(folder),
        );
        let settings = self.effective();
        self.batch_auto = false;
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
                Err(error) if mediaa_ffmpeg::job::is_cancelled(&error) => {
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

    fn effective(&self) -> ConvertSettings {
        ConvertSettings {
            max_height: (self.max_height > 0).then_some(self.max_height),
            ..self.settings.clone()
        }
    }

    fn targets(&self) -> Vec<Target> {
        if !self.queue.is_empty() {
            let mut paths = self.queue.clone();
            if let Some(input) = &self.input {
                paths.insert(0, input.clone());
            }
            let mut common: Option<Vec<Target>> = None;
            for path in paths {
                let Some(kind) = MediaKind::detect(&path) else {
                    return Vec::new();
                };
                let targets = Target::for_kind(kind);
                common = Some(match common {
                    None => targets,
                    Some(previous) => previous
                        .into_iter()
                        .filter(|target| targets.contains(target))
                        .collect(),
                });
            }
            return common.unwrap_or_default();
        }
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
    effort_row, fps_combo, kbps_combo, mono_checkbox, pick_file_dialog, png_colors_combo,
    quality_row,
};

impl ConversionController {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        convert_dir: &std::path::Path,
        folder_override: &mut Option<PathBuf>,
    ) -> Option<ConversionUiAction> {
        let before = (
            self.target,
            self.settings.clone(),
            self.max_height,
            self.folder.clone(),
        );
        let action = self.ui_inner(ui, convert_dir, folder_override);
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
        convert_dir: &std::path::Path,
        folder_override: &mut Option<PathBuf>,
    ) -> Option<ConversionUiAction> {
        let ctx = ui.ctx().clone();
        self.folder = folder_override.clone();
        self.poll_probe(&ctx);
        if self.batch_auto
            && self.job.is_none()
            && self.probed.as_ref().is_some_and(|probe| probe.is_ok())
        {
            if let Some(target) = self.target {
                if let Some(error) = self.start_job(&ctx, target, convert_dir) {
                    self.failed
                        .push((self.input.clone().unwrap_or_default(), error.clone()));
                }
            }
        }
        header(ui, &t!("convert.title"), "");
        if self.input.is_none() {
            if let Some(p) = mediaa_core::dev::convert_input() {
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
                                BatchStatus::Done => {
                                    Some((icon::CHECK, palette.success, t!("convert.status_done")))
                                }
                                BatchStatus::Failed => {
                                    Some((icon::X, palette.danger, t!("convert.status_failed")))
                                }
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

#[cfg(test)]
mod tests;
