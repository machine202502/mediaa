use super::*;
use crate::features::capture::{CaptureKind, ScreenshotDone};
use crate::overlay::Overlay;
use crate::services::clipboard::ClipboardHandle;
use mediaa_core::geometry::Rect;

pub(super) struct CapturedImage {
    pub frame: Frame,
    pub selection: Rect,
    pub shapes: Vec<mediaa_render::annotate::Shape>,
    pub action: AfterCapture,
    pub format: Option<ImageFormat>,
    pub after_completion: bool,
}

impl CapturedImage {
    pub(super) fn whole(frame: Frame, action: AfterCapture) -> Self {
        let selection = Rect::new(0, 0, frame.width as i32, frame.height as i32);
        Self {
            frame,
            selection,
            shapes: Vec::new(),
            action,
            format: None,
            after_completion: false,
        }
    }
}

struct ScreenshotWork {
    image: CapturedImage,
    copy_image: bool,
    path: Option<std::path::PathBuf>,
    format: ImageFormat,
    copy_file: bool,
    clipboard: Option<ClipboardHandle>,
}

fn process_screenshot(work: ScreenshotWork) -> ScreenshotDone {
    let CapturedImage {
        frame,
        selection,
        shapes,
        ..
    } = work.image;
    let frame = mediaa_render::annotate::render_crop(&frame, selection, &shapes);
    let mut output = ScreenshotDone {
        copied: false,
        saved: None,
        copy_error: None,
        save_error: None,
        copy_file_error: None,
        size: format!("{}×{}", frame.width, frame.height),
    };
    if let Some(path) = work.path {
        match screenshot::save(&frame, work.format, &path) {
            Ok(()) => {
                let name = path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned();
                output.saved = Some((path, name));
            }
            Err(error) => output.save_error = Some(format!("{error:#}")),
        }
    }
    if work.copy_image {
        match work.clipboard.as_ref().map_or_else(
            || Err("clipboard service is unavailable".into()),
            |clipboard| clipboard.copy_image_wait(frame),
        ) {
            Ok(()) => output.copied = true,
            Err(error) => output.copy_error = Some(error),
        }
    }
    if work.copy_file {
        if let Some((path, _)) = &output.saved {
            let files = vec![path.clone()];
            if let Err(error) = work.clipboard.as_ref().map_or_else(
                || Err("clipboard service is unavailable".into()),
                |clipboard| clipboard.copy_files_wait(files),
            ) {
                output.copy_file_error = Some(error);
            }
        }
    }
    output
}

impl App {
    pub(crate) fn image_io_busy(&self) -> bool {
        self.capture.busy()
    }

    pub fn start_region_screenshot(&mut self, ctx: &egui::Context, purpose: overlay::Purpose) {
        self.start_region_screenshot_with_completion(ctx, purpose, false);
    }

    pub(super) fn start_region_screenshot_with_completion(
        &mut self,
        ctx: &egui::Context,
        purpose: overlay::Purpose,
        after_completion: bool,
    ) {
        if self.capture.overlay.is_some() {
            return;
        }
        if self.image_io_busy() {
            self.warning(t!("toast.finish_current"));
            return;
        }
        if self.monitors.is_empty() {
            self.refresh_monitors();
        }
        if ctx.input(|i| i.viewport().minimized == Some(true)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        }
        self.schedule(
            ctx,
            Pending::RegionScreenshot {
                purpose,
                after_completion,
            },
            false,
        );
    }

    pub fn start_fullscreen_screenshot(
        &mut self,
        ctx: &egui::Context,
        monitor: Option<MonitorInfo>,
    ) {
        self.start_fullscreen_screenshot_with_completion(ctx, monitor, false);
    }

    pub(super) fn start_fullscreen_screenshot_with_completion(
        &mut self,
        ctx: &egui::Context,
        monitor: Option<MonitorInfo>,
        after_completion: bool,
    ) {
        if self.image_io_busy() {
            self.warning(t!("toast.finish_current"));
            return;
        }
        if self.monitors.is_empty() {
            self.refresh_monitors();
        }
        let monitor = monitor
            .or_else(|| self.monitors.get(self.current_monitor_index(ctx)).cloned())
            .or_else(|| self.monitors.first().cloned());
        match monitor {
            Some(m) => {
                let hide = self.settings.screenshot.hide_window;
                self.schedule(
                    ctx,
                    Pending::FullscreenScreenshot {
                        monitor: m,
                        after_completion,
                    },
                    hide,
                );
            }
            None => self.error(t!("err.no_display")),
        }
    }

    pub fn start_window_screenshot(&mut self, ctx: &egui::Context, window_id: screen::WindowId) {
        if self.image_io_busy() {
            self.warning(t!("toast.finish_current"));
            return;
        }
        let hide = self.settings.screenshot.hide_window;
        self.schedule(ctx, Pending::WindowScreenshot(window_id), hide);
    }

    pub(super) fn queue_monitor_capture(
        &mut self,
        ctx: &egui::Context,
        kind: CaptureKind,
        monitor: screen::MonitorId,
    ) {
        #[cfg(test)]
        let capture = self.monitor_capture.clone();
        self.queue_capture(ctx, kind, move || {
            #[cfg(test)]
            if let Some(capture) = &capture {
                return capture(monitor);
            }
            screen::capture_monitor(monitor).map_err(|error| format!("{error:#}"))
        });
    }

    fn queue_window_capture(
        &mut self,
        ctx: &egui::Context,
        kind: CaptureKind,
        window: screen::WindowId,
    ) {
        #[cfg(test)]
        let capture = self.window_capture.clone();
        self.queue_capture(ctx, kind, move || {
            #[cfg(test)]
            if let Some(capture) = &capture {
                return capture(window);
            }
            screen::capture_window(window).map_err(|error| format!("{error:#}"))
        });
    }

    fn queue_capture(
        &mut self,
        ctx: &egui::Context,
        kind: CaptureKind,
        worker: impl FnOnce() -> Result<Frame, String> + Send + 'static,
    ) {
        if self.image_io_busy() {
            self.restore_after_capture(ctx);
            self.warning(t!("toast.finish_current"));
            return;
        }
        match self.capture.start_capture(kind, worker) {
            Ok(()) => ctx.request_repaint(),
            Err(error) => {
                self.restore_after_capture(ctx);
                self.error(t!("err.capture_failed_detail", error = error));
            }
        }
    }

    pub(super) fn poll_capture(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.capture.poll_capture() else {
            if self.capture.capture_busy() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            return;
        };
        if outcome.timed_out {
            self.restore_after_capture(ctx);
            self.show_window(ctx);
            self.error(t!("err.capture_timeout"));
            return;
        }
        match (outcome.kind, outcome.result) {
            (
                CaptureKind::Region {
                    purpose,
                    after_completion,
                    start,
                },
                Ok(first),
            ) => {
                let after = self.settings.screenshot.after_capture;
                let purpose = match purpose {
                    overlay::Purpose::Area if !self.recording.is_idle() => {
                        overlay::Purpose::Screenshot
                    }
                    purpose => purpose,
                };
                if self.hidden_by_us {
                    self.ghosted = self
                        .native_window
                        .is_some_and(|window| window.set_ghost(true));
                    if !self.ghosted {
                        self.show_window(ctx);
                    }
                }
                self.capture.overlay = Some(Overlay::new(
                    ctx,
                    purpose,
                    self.monitors.clone(),
                    start,
                    first,
                    after,
                    after_completion,
                ));
                ctx.request_repaint();
            }
            (CaptureKind::Fullscreen { after_completion }, Ok(frame)) => {
                if !after_completion {
                    self.restore_after_capture(ctx);
                }
                let mut image = CapturedImage::whole(frame, self.settings.screenshot.after_capture);
                image.after_completion = after_completion;
                self.deliver_image(ctx, image);
            }
            (CaptureKind::Window, Ok(frame)) => {
                self.restore_after_capture(ctx);
                self.deliver_image(
                    ctx,
                    CapturedImage::whole(frame, self.settings.screenshot.after_capture),
                );
            }
            (CaptureKind::OverlayMonitor { index }, Ok(frame)) => {
                if let Some(overlay) = self.capture.overlay.as_ref() {
                    overlay.set_monitor_frame(index, frame);
                    ctx.request_repaint();
                }
            }
            (CaptureKind::Region { .. }, Err(error)) => {
                self.restore_after_capture(ctx);
                self.error(t!("err.capture_failed_detail", error = error));
            }
            (CaptureKind::Fullscreen { .. }, Err(error)) => {
                self.show_window(ctx);
                self.error(t!("err.capture_failed_detail", error = error));
            }
            (CaptureKind::Window, Err(error)) => {
                self.error(t!("err.window_capture_failed", error = error));
            }
            (CaptureKind::OverlayMonitor { .. }, Err(error)) => {
                self.error(t!("err.capture_failed_detail", error = error));
            }
        }
    }

    pub(super) fn deliver_image(&mut self, ctx: &egui::Context, image: CapturedImage) {
        let action = image.action;
        let format = image.format;
        let after_completion = image.after_completion;
        if self.image_io_busy() {
            self.warning(t!("toast.finish_current"));
            return;
        }
        let copies_file =
            after_completion && self.settings.after_completion == AfterCompletionAction::CopyFile;
        let save = action.saves() || after_completion;
        let format = format.unwrap_or(self.settings.screenshot.format);
        let path = save.then(|| {
            naming::media_path(
                &self.settings.media_dir,
                "Screenshot",
                chrono::Local::now(),
                format.extension(),
            )
        });
        let ctx2 = ctx.clone();
        let copy_image = action.copies() && !copies_file;
        let copy_file = copies_file;
        let clipboard = self.clipboard.handle();
        let screenshot = self
            .capture
            .start_screenshot(after_completion, copies_file, move || {
                let output = process_screenshot(ScreenshotWork {
                    image,
                    copy_image,
                    path,
                    format,
                    copy_file,
                    clipboard: Some(clipboard),
                });
                ctx2.request_repaint();
                output
            });
        match screenshot {
            Ok(()) => {}
            Err(error) => {
                self.show_window(ctx);
                self.error(t!("err.save_failed", error = error));
                return;
            }
        }
        ctx.request_repaint();
    }

    pub(super) fn poll_screenshot(&mut self, ctx: &egui::Context) {
        let Some(outcome) = self.capture.poll_screenshot() else {
            if self.capture.busy() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            return;
        };
        let result = match outcome.result {
            Ok(result) => result,
            Err(_) => {
                self.show_window(ctx);
                self.error(t!("err.image_worker_stopped"));
                return;
            }
        };
        if let Some(error) = result.copy_error {
            self.error(t!("err.copy_failed", error = error));
        }
        if let Some(error) = result.save_error {
            self.error(t!("err.save_failed", error = error));
        }
        if let Some((path, _)) = &result.saved {
            self.lib.files_changed(std::slice::from_ref(path));
        }
        if outcome.after_completion {
            match self.settings.after_completion {
                AfterCompletionAction::OpenLibrary => {
                    self.show_window(ctx);
                    if !self.quit_when_idle {
                        self.page = Page::Library;
                    }
                }
                AfterCompletionAction::CopyFile => {
                    if let Some(error) = result.copy_file_error {
                        self.show_window(ctx);
                        self.error(t!("err.copy_failed", error = error));
                        if !self.quit_when_idle {
                            self.page = Page::Library;
                        }
                    } else if result.saved.is_none() {
                        self.show_window(ctx);
                        if !self.quit_when_idle {
                            self.page = Page::Library;
                        }
                    } else if !self.hidden_by_us {
                        self.success(t!("toast.shot_file_copied"));
                    }
                }
            }
        }
        let msg = match (
            result.copied,
            result.saved.as_ref().map(|(_, name)| name.as_str()),
        ) {
            (true, Some(name)) => t!("toast.shot_copied_saved", size = result.size, name = name),
            (false, Some(name)) => t!("toast.shot_saved", size = result.size, name = name),
            (true, None) => t!("toast.shot_copied", size = result.size),
            (false, None) => return,
        };
        if !outcome.copies_file {
            self.success(msg);
        }
    }

    pub(super) fn poll_copy_image(&mut self, ctx: &egui::Context) {
        let Some(result) = self.capture.poll_copy_image() else {
            if self.capture.busy() {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
            return;
        };
        match result {
            Ok(message) => self.success(message),
            Err(error) => self.error(t!("err.copy_failed", error = error)),
        }
    }

    pub(super) fn run_pending(&mut self, ctx: &egui::Context, pending: Pending) {
        log::info!("running {pending:?}");
        match pending {
            Pending::RegionScreenshot {
                purpose,
                after_completion,
            } => {
                let start = self
                    .current_monitor_index(ctx)
                    .min(self.monitors.len().saturating_sub(1));
                let Some(monitor) = self.monitors.get(start).cloned() else {
                    self.error(t!("err.no_display"));
                    return;
                };
                self.queue_monitor_capture(
                    ctx,
                    CaptureKind::Region {
                        purpose,
                        after_completion,
                        start,
                    },
                    monitor.id,
                );
            }
            Pending::FullscreenScreenshot {
                monitor,
                after_completion,
            } => {
                self.queue_monitor_capture(
                    ctx,
                    CaptureKind::Fullscreen { after_completion },
                    monitor.id,
                );
            }
            Pending::WindowScreenshot(id) => {
                self.queue_window_capture(ctx, CaptureKind::Window, id);
            }
            Pending::BeginRecording(target, profile) => self.begin_recording(ctx, target, profile),
        }
    }

    fn restore_after_capture(&mut self, ctx: &egui::Context) {
        if self.hidden_by_us && self.recording.is_idle() {
            self.show_window(ctx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{process_screenshot, CapturedImage, ScreenshotWork};
    use mediaa_core::settings::AfterCapture;
    use mediaa_core::{Frame, ImageFormat, Rect};

    #[test]
    fn screenshot_worker_crops_and_saves() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shot.png");
        let frame = Frame::filled(8, 6, [10, 20, 30, 255]);
        let output = process_screenshot(ScreenshotWork {
            image: CapturedImage {
                frame,
                selection: Rect::new(2, 1, 4, 3),
                shapes: Vec::new(),
                action: AfterCapture::Save,
                format: Some(ImageFormat::Png),
                after_completion: false,
            },
            copy_image: false,
            path: Some(path.clone()),
            format: ImageFormat::Png,
            copy_file: false,
            clipboard: None,
        });
        assert_eq!(output.size, "4×3");
        assert!(output.copy_error.is_none());
        assert!(output.save_error.is_none());
        assert_eq!(output.saved.unwrap().0, path);
        assert_eq!(image::image_dimensions(path).unwrap(), (4, 3));
    }
}
