//! A catalog file opened in a tab, laid out like the media-analysis studio: a title bar, three
//! panes — dialog (the recording's text), the video, chat with the agent — separated by
//! draggable sashes, and a transport bar (play, time, position, volume, speed, mute, hide the
//! video, video only). Audio files have no video pane. Narrow tabs stack the panes.

use super::chat::Chat;
use super::dialog::Dialog;
use super::Services;
use crate::player::Player;
use crate::theme;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::MediaKind;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Playback speeds of the transport (as in media-analysis).
pub const SPEEDS: [f64; 6] = [0.8, 1.0, 1.25, 1.5, 2.0, 3.0];
/// Default pane proportions: dialog, video, chat.
const PANES: [f32; 3] = [1.15, 1.0, 0.95];
/// Narrowest / shortest pane.
const MIN_PANE: f32 = 150.0;
/// Below this width the panes stack (video, dialog, chat).
const NARROW: f32 = 980.0;
const SASH: f32 = 8.0;

/// Layout choices shared by all studios (kept across restarts).
#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Layout {
    sizes: [f32; 3],
    show_video: bool,
    cinema: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self { sizes: PANES, show_video: true, cinema: false }
    }
}

fn layout_id() -> egui::Id {
    egui::Id::new("studio-layout")
}

pub struct Studio {
    path: PathBuf,
    video: bool,
    player: Player,
    texture: Option<egui::TextureHandle>,
    dialog: Dialog,
    chat: Chat,
    /// Action from the dialog pane this frame.
    pending: Option<StudioAction>,
    /// Drawn this frame (tabs that are not shown are paused).
    pub drawn: bool,
}

impl Studio {
    /// Opens `path` paused at the start (its transcript in `ai_dir`); fails when it cannot be
    /// read.
    pub fn open(path: &Path, ai_dir: &Path) -> anyhow::Result<Studio> {
        let kind = MediaKind::from_path(path).ok_or_else(|| anyhow::anyhow!("{}: not audio or video", path.display()))?;
        let info = mediaa_media::info::probe(path)?;
        let video = kind == MediaKind::Video && info.has_video();
        let mut player = Player::new(path, info.duration_secs().unwrap_or(0.0), info.has_audio(), false, video);
        player.toggle_pause();
        Ok(Studio { path: path.to_path_buf(), video, player, texture: None, dialog: Dialog::new(path, ai_dir), chat: Chat::new(path, ai_dir), pending: None, drawn: false })
    }

    /// Moves playback to `t` seconds (a search hit opened from the catalog).
    pub fn seek(&mut self, t: f64) {
        self.player.seek(t);
    }

    pub fn pause(&mut self) {
        if !self.player.is_paused() {
            self.player.toggle_pause();
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, title: &str, services: Option<&Services>) -> Option<StudioAction> {
        self.drawn = true;
        if let Some(f) = self.player.update() {
            let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.data);
            match &mut self.texture {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => self.texture = Some(ui.ctx().load_texture(self.path.to_string_lossy(), img, egui::TextureOptions::LINEAR)),
            }
        }
        if !self.player.is_paused() {
            ui.ctx().request_repaint_after(Duration::from_millis(8));
        }
        let mut layout: Layout = ui.ctx().data_mut(|d| *d.get_persisted_mut_or_default(layout_id()));
        let before = layout;
        let p = theme::palette(ui);
        let mut action = None;

        let full = ui.available_rect_before_wrap();
        let top = Rect::from_min_size(full.min, egui::vec2(full.width(), 44.0));
        let bar = Rect::from_min_max(egui::pos2(full.min.x, full.max.y - 52.0), full.max);
        let stage = Rect::from_min_max(egui::pos2(full.min.x + 6.0, top.max.y), egui::pos2(full.max.x - 6.0, bar.min.y - 2.0));

        // Title bar: tools · title · (room for more tools).
        ui.scope_builder(egui::UiBuilder::new().max_rect(top.shrink2(egui::vec2(10.0, 6.0))), |ui| {
            ui.horizontal_centered(|ui| {
                if ui.add(theme::secondary(icon::ARROW_SQUARE_OUT, &t!("studio.open"))).clicked() {
                    action = Some(StudioAction::Open(self.path.clone()));
                }
                if ui.add(theme::secondary(icon::FOLDER_OPEN, &t!("lib.reveal"))).clicked() {
                    action = Some(StudioAction::Reveal(self.path.clone()));
                }
                ui.add_space(12.0);
                ui.add(egui::Label::new(RichText::new(title).size(18.0).strong()).truncate()).on_hover_text(self.path.to_string_lossy());
            });
        });

        self.stage(ui, stage, &mut layout, services, p);
        self.transport(ui, bar, &mut layout, p);

        if layout != before {
            ui.ctx().data_mut(|d| d.insert_persisted(layout_id(), layout));
        }
        ui.allocate_rect(full, Sense::hover());
        action.or(self.pending.take())
    }

    /// The panes and the sashes between them.
    fn stage(&mut self, ui: &mut egui::Ui, rect: Rect, layout: &mut Layout, services: Option<&Services>, p: theme::Palette) {
        let show_video = self.video && layout.show_video;
        if self.video && layout.cinema {
            self.video_pane(ui, rect, p);
            return;
        }
        let narrow = rect.width() < NARROW;
        // Visible panes in display order (index into `sizes`: 0 dialog, 1 video, 2 chat).
        let order: Vec<usize> = match (show_video, narrow) {
            (false, _) => vec![0, 2],
            (true, true) => vec![1, 0, 2],
            (true, false) => vec![0, 1, 2],
        };
        let span = if narrow { rect.height() } else { rect.width() } - SASH * (order.len() - 1) as f32;
        let total: f32 = order.iter().map(|&i| layout.sizes[i]).sum();
        let mut at = if narrow { rect.min.y } else { rect.min.x };
        for (n, &i) in order.iter().enumerate() {
            let len = span * layout.sizes[i] / total;
            let pane = if narrow {
                Rect::from_min_max(egui::pos2(rect.min.x, at), egui::pos2(rect.max.x, at + len))
            } else {
                Rect::from_min_max(egui::pos2(at, rect.min.y), egui::pos2(at + len, rect.max.y))
            };
            match i {
                0 => {
                    if let Some(a) = self.dialog.ui(ui, pane, &mut self.player, services, p) {
                        self.pending = Some(a);
                    }
                }
                1 => self.video_pane(ui, pane, p),
                _ => {
                    if let Some(a) = self.chat.ui(ui, pane, Some(&mut self.player), services, p) {
                        self.pending = Some(a);
                    }
                }
            }
            at += len;
            if let Some(&next) = order.get(n + 1) {
                let sash = if narrow {
                    Rect::from_min_max(egui::pos2(rect.min.x, at), egui::pos2(rect.max.x, at + SASH))
                } else {
                    Rect::from_min_max(egui::pos2(at, rect.min.y), egui::pos2(at + SASH, rect.max.y))
                };
                let resp = ui.interact(sash, ui.id().with(("studio-sash", n)), Sense::drag());
                let active = resp.hovered() || resp.dragged();
                if active {
                    ui.ctx().set_cursor_icon(if narrow { egui::CursorIcon::ResizeVertical } else { egui::CursorIcon::ResizeHorizontal });
                    let line = if narrow {
                        Rect::from_center_size(sash.center(), egui::vec2(sash.width(), 2.0))
                    } else {
                        Rect::from_center_size(sash.center(), egui::vec2(2.0, sash.height()))
                    };
                    ui.painter().rect_filled(line, CornerRadius::same(1), p.accent);
                }
                if resp.dragged() {
                    let d = if narrow { resp.drag_delta().y } else { resp.drag_delta().x } / span * total;
                    let min = MIN_PANE / span * total;
                    let (a, b) = (layout.sizes[i], layout.sizes[next]);
                    let d = d.clamp(min - a, b - min);
                    layout.sizes[i] = a + d;
                    layout.sizes[next] = b - d;
                }
                at += SASH;
            }
        }
    }

    fn video_pane(&mut self, ui: &mut egui::Ui, rect: Rect, p: theme::Palette) {
        let body = panel(ui, rect, &t!("studio.video"), p);
        ui.painter().rect_filled(body, CornerRadius { nw: 0, ne: 0, sw: 11, se: 11 }, Color32::from_rgb(10, 10, 12));
        match &self.texture {
            Some(tex) => {
                let size = tex.size_vec2();
                let scale = (body.width() / size.x).min(body.height() / size.y);
                let r = Rect::from_center_size(body.center(), size * scale);
                egui::Image::new((tex.id(), r.size())).paint_at(ui, r);
            }
            None => {
                egui::Spinner::new().size(24.0).color(Color32::WHITE).paint_at(ui, Rect::from_center_size(body.center(), egui::vec2(24.0, 24.0)));
            }
        }
        // Click on the picture: play / pause.
        if ui.interact(body, ui.id().with("studio-screen"), Sense::click()).clicked() {
            self.player.toggle_pause();
        }
    }

    /// Play · time · position (fills the rest) · volume · speed · mute · hide video · video only.
    fn transport(&mut self, ui: &mut egui::Ui, rect: Rect, layout: &mut Layout, p: theme::Palette) {
        ui.painter().hline(rect.x_range(), rect.min.y, Stroke::new(1.0, p.border));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(12.0, 8.0))), |ui| {
            ui.horizontal_centered(|ui| {
                let player = &mut self.player;
                let (ic, tip) = if player.is_paused() { (icon::PLAY, t!("viewer.play")) } else { (icon::PAUSE, t!("viewer.pause")) };
                if theme::icon_button(ui, ic, &tip).clicked() {
                    player.toggle_pause();
                }
                let (pos, duration) = (player.position(), player.duration());
                let shown = if duration > 0.0 && pos >= duration - 0.05 { duration } else { pos.floor() };
                ui.label(RichText::new(format!("{} / {}", human_duration(shown), human_duration(duration))).font(FontId::monospace(12.0)));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.video {
                        let (ic, tip) = if layout.cinema { (icon::CORNERS_IN, t!("studio.cinema_off")) } else { (icon::CORNERS_OUT, t!("studio.cinema")) };
                        if toggle_button(ui, ic, &tip, layout.cinema, p).clicked() {
                            layout.cinema = !layout.cinema;
                            if layout.cinema {
                                layout.show_video = true;
                            }
                        }
                        let (ic, tip) = if layout.show_video { (icon::VIDEO_CAMERA, t!("studio.hide_video")) } else { (icon::VIDEO_CAMERA_SLASH, t!("studio.show_video")) };
                        if toggle_button(ui, ic, &tip, !layout.show_video, p).clicked() {
                            layout.show_video = !layout.show_video;
                            if !layout.show_video {
                                layout.cinema = false;
                            }
                        }
                    }
                    let player = &mut self.player;
                    if player.has_sound() {
                        let muted = player.volume() == 0.0;
                        let (ic, tip) = if muted { (icon::SPEAKER_SLASH, t!("studio.unmute")) } else { (icon::SPEAKER_HIGH, t!("viewer.mute")) };
                        if toggle_button(ui, ic, &tip, muted, p).clicked() {
                            player.set_volume(if muted { 1.0 } else { 0.0 });
                        }
                    }
                    // Speed: a stepped slider with its value.
                    let idx = SPEEDS.iter().position(|&s| s == player.speed()).unwrap_or(1);
                    ui.label(RichText::new(format!("{}×", SPEEDS[idx])).font(FontId::monospace(12.0)));
                    let (resp, v) = slider(ui, 84.0, idx as f32 / (SPEEDS.len() - 1) as f32, "studio-speed", p);
                    resp.on_hover_text(t!("studio.speed"));
                    if let Some(v) = v {
                        player.set_speed(SPEEDS[(v * (SPEEDS.len() - 1) as f32).round() as usize]);
                    }
                    if player.has_sound() {
                        let (resp, v) = slider(ui, 96.0, player.volume(), "studio-volume", p);
                        resp.on_hover_text(t!("studio.volume"));
                        if let Some(v) = v {
                            player.set_volume(v);
                        }
                    }
                    // Position: whatever width is left.
                    let frac = if duration > 0.0 { (pos / duration) as f32 } else { 0.0 };
                    let (resp, v) = slider(ui, ui.available_width(), frac, "studio-position", p);
                    if let Some(f) = v {
                        let t = f64::from(f) * duration;
                        if resp.dragged() {
                            player.scrub(t);
                        } else {
                            player.seek(t);
                        }
                    }
                    if resp.drag_stopped() {
                        player.end_scrub();
                    }
                });
            });
        });
    }
}

pub enum StudioAction {
    Open(PathBuf),
    Reveal(PathBuf),
    /// Queue a transcription of the file.
    Transcribe(PathBuf),
    /// Continue a failed transcription task.
    ResumeTask(mediaa_core::queue::TaskId),
    /// Open a recording at a time (a citation in a folder's chat).
    OpenAt(PathBuf, f64),
}

/// A pane frame with its title row; returns the body rect.
pub(super) fn panel(ui: &egui::Ui, rect: Rect, title: &str, p: theme::Palette) -> Rect {
    let painter = ui.painter_at(rect);
    painter.rect(rect, CornerRadius::same(12), p.card, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let head = Rect::from_min_size(rect.min, egui::vec2(rect.width(), 30.0));
    painter.hline(head.x_range(), head.max.y, Stroke::new(1.0, p.border));
    painter.text(head.left_center() + egui::vec2(12.0, 0.0), egui::Align2::LEFT_CENTER, title, FontId::proportional(12.0), p.text_weak);
    Rect::from_min_max(egui::pos2(rect.min.x + 1.0, head.max.y + 1.0), rect.max - egui::vec2(1.0, 1.0))
}

/// An icon button that stays highlighted while `on`.
fn toggle_button(ui: &mut egui::Ui, ic: &str, tip: &str, on: bool, p: theme::Palette) -> egui::Response {
    let resp = theme::icon_button(ui, ic, tip);
    if on {
        ui.painter().rect_filled(resp.rect, CornerRadius::same(6), p.accent_soft);
        ui.painter().text(resp.rect.center(), egui::Align2::CENTER_CENTER, ic, FontId::proportional(14.0), p.accent);
    }
    resp
}

/// Thin track with an accent fill and a round knob; the new value while clicked or dragged.
fn slider(ui: &mut egui::Ui, width: f32, frac: f32, id: &str, p: theme::Palette) -> (egui::Response, Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(40.0), 24.0), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 14.0, 4.0));
    let x = track.left() + track.width() * frac.clamp(0.0, 1.0);
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), p.border);
    painter.rect_filled(Rect::from_min_max(track.min, egui::pos2(x, track.max.y)), CornerRadius::same(2), p.accent);
    let knob = if resp.hovered() || resp.dragged() { 7.0 } else { 6.0 };
    painter.circle(egui::pos2(x, track.center().y), knob, Color32::WHITE, Stroke::new(1.5, p.accent));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let value = resp.interact_pointer_pos().filter(|_| resp.clicked() || resp.dragged()).map(|pt| ((pt.x - track.left()) / track.width()).clamp(0.0, 1.0));
    (resp, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_include_normal_and_layout_defaults() {
        assert!(SPEEDS.contains(&1.0) && SPEEDS.windows(2).all(|w| w[0] < w[1]));
        let l = Layout::default();
        assert!(l.show_video && !l.cinema && l.sizes == PANES);
    }
}
