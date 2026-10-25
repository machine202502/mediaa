use super::*;

impl App {
    pub fn start_region_screenshot(&mut self, ctx: &egui::Context, purpose: overlay::Purpose) {
        self.start_region_screenshot_with_completion(ctx, purpose, false);
    }

    pub(super) fn start_region_screenshot_with_completion(
        &mut self,
        ctx: &egui::Context,
        purpose: overlay::Purpose,
        after_completion: bool,
    ) {
        if self.overlay.is_some() {
            return;
        }
        self.refresh_monitors();
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
        self.refresh_monitors();
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

    pub fn start_window_screenshot(&mut self, ctx: &egui::Context, window_id: u32) {
        let hide = self.settings.screenshot.hide_window;
        self.schedule(ctx, Pending::WindowScreenshot(window_id), hide);
    }

    pub fn deliver_image(
        &mut self,
        ctx: &egui::Context,
        frame: Frame,
        action: AfterCapture,
        format: Option<ImageFormat>,
        after_completion: bool,
    ) {
        let mut copied = false;
        let mut saved = None;
        let copies_file =
            after_completion && self.settings.after_completion == AfterCompletionAction::CopyFile;
        if action.copies() && !copies_file {
            match clipboard::copy_image(&frame) {
                Ok(()) => copied = true,
                Err(e) => self.error(t!("err.copy_failed", error = format!("{e:#}"))),
            }
        }
        if action.saves() || after_completion {
            let fmt = format.unwrap_or(self.settings.screenshot.format);
            let path = naming::media_path(
                &self.settings.media_dir,
                "Screenshot",
                chrono::Local::now(),
                fmt.extension(),
            );
            match screenshot::save(&frame, fmt, &path) {
                Ok(()) => {
                    let name = path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned();
                    self.lib.files_changed(std::slice::from_ref(&path));
                    saved = Some((path, name));
                }
                Err(e) => self.error(t!("err.save_failed", error = format!("{e:#}"))),
            }
        }
        let size = format!("{}×{}", frame.width, frame.height);
        if after_completion {
            match self.settings.after_completion {
                AfterCompletionAction::OpenLibrary => {
                    self.show_window(ctx);
                    if !self.quit_when_idle {
                        self.page = Page::Library;
                    }
                }
                AfterCompletionAction::CopyFile => match saved.as_ref() {
                    Some((path, _)) => match clipboard::copy_files(&[path.as_path()]) {
                        Ok(()) => {
                            if !self.hidden_by_us {
                                self.success(t!("toast.shot_file_copied"));
                            }
                        }
                        Err(e) => {
                            self.show_window(ctx);
                            self.error(t!("err.copy_failed", error = format!("{e:#}")));
                            if !self.quit_when_idle {
                                self.page = Page::Library;
                            }
                        }
                    },
                    None => {
                        self.show_window(ctx);
                        if !self.quit_when_idle {
                            self.page = Page::Library;
                        }
                    }
                },
            }
        }
        let msg = match (copied, saved.as_ref().map(|(_, name)| name)) {
            (true, Some(name)) => t!("toast.shot_copied_saved", size = size, name = name),
            (false, Some(name)) => t!("toast.shot_saved", size = size, name = name),
            (true, None) => t!("toast.shot_copied", size = size),
            (false, None) => return,
        };
        if !copies_file {
            self.success(msg);
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
                match screen::capture_monitor(monitor.id) {
                    Ok(first) => {
                        let after = self.settings.screenshot.after_capture;
                        let purpose = match purpose {
                            overlay::Purpose::Area if !self.recorder.is_idle() => {
                                overlay::Purpose::Screenshot
                            }
                            p => p,
                        };
                        if self.hidden_by_us {
                            self.ghosted = self.native_window.is_some_and(|w| w.set_ghost(true));
                            if !self.ghosted {
                                self.show_window(ctx);
                            }
                        }
                        self.overlay = Some(Overlay::new(
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
                    Err(e) => {
                        self.restore_after_capture(ctx);
                        self.error(t!("err.capture_failed_detail", error = format!("{e:#}")));
                    }
                }
            }
            Pending::FullscreenScreenshot {
                monitor,
                after_completion,
            } => {
                let shot = screen::capture_monitor(monitor.id);
                if !after_completion {
                    self.restore_after_capture(ctx);
                }
                match shot {
                    Ok(f) => self.deliver_image(
                        ctx,
                        f,
                        self.settings.screenshot.after_capture,
                        None,
                        after_completion,
                    ),
                    Err(e) => {
                        self.show_window(ctx);
                        self.error(t!("err.capture_failed_detail", error = format!("{e:#}")));
                    }
                }
            }
            Pending::WindowScreenshot(id) => {
                let shot = screen::capture_window(id);
                self.restore_after_capture(ctx);
                match shot {
                    Ok(f) => self.deliver_image(
                        ctx,
                        f,
                        self.settings.screenshot.after_capture,
                        None,
                        false,
                    ),
                    Err(e) => self.error(t!("err.window_capture_failed", error = format!("{e:#}"))),
                }
            }
            Pending::BeginRecording(target, profile) => self.begin_recording(ctx, target, profile),
        }
    }

    fn restore_after_capture(&mut self, ctx: &egui::Context) {
        if self.hidden_by_us && self.recorder.is_idle() {
            self.show_window(ctx);
        }
    }
}
