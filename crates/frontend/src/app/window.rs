use super::*;

impl App {
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

    pub(super) fn window_event(&mut self, ctx: &egui::Context, event: WindowEvent) {
        let now = Instant::now();
        match self.window.on(event, now) {
            Ok(transition) => {
                for effect in transition.effects {
                    self.apply_window_effect(ctx, effect);
                }
            }
            Err(rejected) => log::warn!("window transition rejected: {rejected:?}"),
        }
    }

    fn apply_window_effect(&mut self, ctx: &egui::Context, effect: WindowEffect) {
        match effect {
            WindowEffect::ShowWindow => {
                self.set_window_visible(ctx, true);
            }
            WindowEffect::HideWindow => {
                self.set_window_visible(ctx, false);
            }
            WindowEffect::SendShowAck => {
                self.send_sync_command(mediaa_protocol::TRAY_SHOW_ACK, Vec::new());
            }
            WindowEffect::SendHideAck => {
                self.send_sync_command(mediaa_protocol::TRAY_HIDE_ACK, Vec::new());
            }
            WindowEffect::CancelClose => {
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            }
            WindowEffect::CloseWindow => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }

    pub(super) fn hide_window(&mut self, ctx: &egui::Context) {
        self.window_event(ctx, WindowEvent::Hide);
    }

    pub fn show_window(&mut self, ctx: &egui::Context) {
        self.window_event(ctx, WindowEvent::Show);
    }

    pub(super) fn schedule(&mut self, ctx: &egui::Context, pending: Pending, hide: bool) {
        let delay = if hide {
            let was_hidden = self.window.is_hidden();
            self.hide_window(ctx);
            if was_hidden {
                Duration::ZERO
            } else {
                HIDE_DELAY
            }
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
