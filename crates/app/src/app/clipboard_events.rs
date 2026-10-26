use super::*;
use crate::services::clipboard::Purpose;

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
                        if !self.hidden_by_us {
                            self.success(success);
                        }
                        if sound {
                            mediaa_platform::audio::confirmation_tone();
                        }
                    }
                    Err(error) => {
                        self.show_window(ctx);
                        self.error(t!("err.copy_failed", error = error));
                        if !self.quit_when_idle {
                            self.page = Page::Library;
                        }
                    }
                },
            }
        }
    }
}
