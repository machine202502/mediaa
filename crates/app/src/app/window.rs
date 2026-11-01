use super::*;

impl App {
    pub fn logo(&mut self, ctx: &egui::Context) -> egui::TextureHandle {
        let theme = self.settings.theme;
        if let Some((cached, texture)) = &self.logo {
            if *cached == theme {
                return texture.clone();
            }
        }
        let icon = crate::theme::icon_palette(theme);
        let img = egui::ColorImage::from_rgba_unmultiplied(
            [64, 64],
            &crate::icon::rgba_with(64, icon.deep, icon.mid, icon.light),
        );
        let texture =
            ctx.load_texture(format!("logo-{theme:?}"), img, egui::TextureOptions::LINEAR);
        self.logo = Some((theme, texture.clone()));
        texture
    }

    fn set_window_visible(&mut self, ctx: &egui::Context, visible: bool) {
        let native = self
            .native_window
            .is_some_and(|w| w.set_visible(visible).is_ok());
        if !native {
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(visible));
        }
        if visible {
            if ctx.input(|i| i.viewport().minimized == Some(true)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            ctx.request_repaint();
        }
    }

    pub(super) fn hide_window(&mut self, ctx: &egui::Context) {
        if !self.hidden_by_us {
            self.hidden_by_us = true;
            self.set_window_visible(ctx, false);
        }
    }

    pub fn show_window(&mut self, ctx: &egui::Context) {
        if std::mem::take(&mut self.ghosted) {
            if let Some(w) = self.native_window {
                w.set_ghost(false);
            }
        }
        self.hidden_by_us = false;
        self.set_window_visible(ctx, true);
    }

    pub(super) fn schedule(&mut self, ctx: &egui::Context, pending: Pending, hide: bool) {
        let delay = if hide {
            self.hide_window(ctx);
            HIDE_DELAY
        } else {
            Duration::ZERO
        };
        log::info!("scheduled {pending:?} in {delay:?} (hide: {hide})");
        self.pending = Some((Instant::now() + delay, pending));
        ctx.request_repaint_after(delay);
    }

    pub(super) fn current_monitor_index(&self, ctx: &egui::Context) -> usize {
        let pos = ctx.input(|i| {
            let ppp = i.viewport().native_pixels_per_point.unwrap_or(1.0);
            i.viewport().outer_rect.map(|r| r.center() * ppp)
        });
        pos.and_then(|p| {
            self.monitors.iter().position(|m| {
                p.x >= m.x as f32
                    && p.x < (m.x + m.width as i32) as f32
                    && p.y >= m.y as f32
                    && p.y < (m.y + m.height as i32) as f32
            })
        })
        .unwrap_or(0)
    }
}
