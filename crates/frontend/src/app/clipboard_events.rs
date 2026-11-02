use super::*;
use crate::infra::clipboard_worker::Purpose;

impl App {
    pub(super) fn poll_clipboard(&mut self, ctx: &egui::Context) {
        while let Some(event) = self.clipboard.try_recv() {
            match event.purpose {
                Purpose::Toast(success) => match event.result {
                    Ok(()) => self.success(success),
                    Err(error) => self.error(t!("err.copy_failed", error = error)),
                },
                Purpose::Recording { success, sound } => match event.result {
                    Ok(()) => {
                        if !self.window.is_hidden() {
                            self.success(success);
                        }
                        if sound {
                            mediaa_platform::audio::confirmation_tone();
                        }
                    }
                    Err(error) => {
                        self.show_window(ctx);
                        self.error(t!("err.copy_failed", error = error));
                    }
                },
                Purpose::Silent => {
                    if let Err(error) = event.result {
                        log::warn!("clipboard action from notice failed: {error}");
                    }
                }
            }
        }
    }
}
