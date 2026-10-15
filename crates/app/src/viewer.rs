//! Full-window media viewer over a dimmed page: images can be zoomed and panned; videos and
//! animations play with sound, adjustable speed and volume, and a draggable timeline.
//! Closes with Esc, the close button or a click outside the media.

use crate::theme;
use crate::player::Player;
use crossbeam_channel::Receiver;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::{Frame, MediaKind};
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Largest image side uploaded as one texture.
const MAX_TEXTURE: u32 = 8192;
const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

pub enum ViewerAction {
    Close,
    OpenFile(PathBuf),
}

pub struct Viewer {
    path: PathBuf,
    name: String,
    kind: MediaKind,
    texture: Option<egui::TextureHandle>,
    image_rx: Option<Receiver<Frame>>,
    /// Image zoom relative to "fit", and pan offset in points.
    zoom: f32,
    pan: Vec2,
    player: Option<Player>,
}

impl Viewer {
    /// `duration` / `audio`: what the library knows about the file (videos and animations).
    pub fn open(path: &Path, kind: MediaKind, duration: Option<f64>, audio: bool) -> Viewer {
        let name = path.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let mut v = Viewer { path: path.to_path_buf(), name, kind, texture: None, image_rx: None, zoom: 1.0, pan: Vec2::ZERO, player: None };
        if kind == MediaKind::Image {
            let (tx, rx) = crossbeam_channel::bounded(1);
            let p = path.to_path_buf();
            let _ = std::thread::Builder::new().name("viewer-image".into()).spawn(move || {
                match image::open(&p) {
                    Ok(img) => {
                        let img = if img.width().max(img.height()) > MAX_TEXTURE { img.thumbnail(MAX_TEXTURE, MAX_TEXTURE) } else { img };
                        let _ = tx.send(Frame::from_image(img.to_rgba8()));
                    }
                    Err(e) => log::warn!("viewer {}: {e}", p.display()),
                }
            });
            v.image_rx = Some(rx);
        } else {
            v.player = Some(Player::new(path, duration.unwrap_or(0.0), audio, kind == MediaKind::Animation, kind != MediaKind::Audio));
        }
        v
    }

    /// Draws the viewer over the whole window; returns what the user asked for.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<ViewerAction> {
        if let Some(rx) = &self.image_rx {
            if let Ok(f) = rx.try_recv() {
                self.texture = Some(upload(ctx, None, &f));
                self.image_rx = None;
            }
        }
        if let Some(p) = self.player.as_mut() {
            if let Some(f) = p.update() {
                self.texture = Some(upload(ctx, self.texture.take(), &f));
            }
            ctx.request_repaint_after(Duration::from_millis(8));
        }

        let mut action = None;
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("media-viewer"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                *ui.visuals_mut() = theme::dark_visuals();
                ui.set_min_size(screen.size());
                ui.painter().rect_filled(screen, CornerRadius::ZERO, Color32::from_black_alpha(232));
                // Background: clicks that reach it are outside the media and controls.
                let bg = ui.interact(screen, ui.id().with("backdrop"), Sense::click());

                let top = Rect::from_min_max(screen.min, egui::pos2(screen.max.x, screen.min.y + 52.0));
                let controls_h = 64.0;
                let bottom = Rect::from_min_max(egui::pos2(screen.min.x, screen.max.y - controls_h - 12.0), screen.max);
                let area = Rect::from_min_max(egui::pos2(screen.min.x + 24.0, top.max.y), egui::pos2(screen.max.x - 24.0, bottom.min.y - 8.0));

                let media = self.media(ui, area);
                self.top_bar(ui, top, &mut action);
                self.controls(ui, bottom, &mut action);

                let (esc, space, left, right) = ui.input(|i| {
                    (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Space), i.key_pressed(egui::Key::ArrowLeft), i.key_pressed(egui::Key::ArrowRight))
                });
                if let Some(p) = self.player.as_mut() {
                    if space {
                        p.toggle_pause();
                    }
                    if left || right {
                        let step = if right { 5.0 } else { -5.0 };
                        p.seek(p.position() + step);
                    }
                }
                let outside = bg.clicked() && bg.interact_pointer_pos().is_some_and(|p| media.is_none_or(|m| !m.contains(p)));
                if esc || outside {
                    action.get_or_insert(ViewerAction::Close);
                }
            });
        action
    }

    /// The picture or video frame, fitted into `area`; returns where it was drawn.
    fn media(&mut self, ui: &mut egui::Ui, area: Rect) -> Option<Rect> {
        if self.kind == MediaKind::Audio {
            // Sound only: a large note; clicking it pauses / resumes.
            let rect = Rect::from_center_size(area.center(), egui::vec2(220.0, 220.0));
            ui.painter().circle_filled(rect.center(), 110.0, Color32::from_rgb(30, 40, 60));
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, icon::MUSIC_NOTES, FontId::proportional(96.0), Color32::WHITE);
            if ui.interact(rect, ui.id().with("audio-art"), Sense::click()).clicked() {
                if let Some(p) = self.player.as_mut() {
                    p.toggle_pause();
                }
            }
            return Some(rect);
        }
        let Some(tex) = &self.texture else {
            egui::Spinner::new().size(32.0).color(Color32::WHITE).paint_at(ui, Rect::from_center_size(area.center(), egui::vec2(32.0, 32.0)));
            return None;
        };
        let size = tex.size_vec2();
        // Fit (small pictures up to 2×), then the user's zoom for images.
        let fit = (area.width() / size.x).min(area.height() / size.y).min(2.0);
        let shown = size * fit * self.zoom;
        let rect = Rect::from_center_size(area.center() + self.pan, shown);
        let resp = ui.interact(rect.intersect(area), ui.id().with("media"), Sense::click_and_drag());
        ui.painter().with_clip_rect(area).image(tex.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);

        if self.kind == MediaKind::Image {
            if resp.dragged() {
                self.pan += resp.drag_delta();
            }
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if resp.hovered() && scroll != 0.0 {
                let factor = (scroll / 200.0).exp();
                let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                // Keep the point under the cursor in place.
                if let Some(cursor) = resp.hover_pos() {
                    let k = new_zoom / self.zoom;
                    self.pan = (self.pan - (cursor - area.center())) * k + (cursor - area.center());
                }
                self.zoom = new_zoom;
            }
            if resp.double_clicked() {
                (self.zoom, self.pan) = if self.zoom == 1.0 { (2.0, Vec2::ZERO) } else { (1.0, Vec2::ZERO) };
            }
        } else if let Some(p) = self.player.as_mut() {
            if resp.clicked() {
                p.toggle_pause();
            }
        }
        Some(rect.intersect(area))
    }

    fn top_bar(&self, ui: &mut egui::Ui, rect: Rect, action: &mut Option<ViewerAction>) {
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(20.0, 10.0))), |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(&self.name).size(15.0).color(Color32::WHITE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if theme::icon_button(ui, icon::X, &t!("viewer.close")).clicked() {
                        *action = Some(ViewerAction::Close);
                    }
                });
            });
        });
    }

    /// Bottom bar: the controls fill its whole width, the slider taking what is left.
    fn controls(&mut self, ui: &mut egui::Ui, rect: Rect, action: &mut Option<ViewerAction>) {
        let width = rect.width().min(980.0) - 32.0;
        let bar = Rect::from_center_size(rect.center(), egui::vec2(width, rect.height() - 12.0));
        ui.painter().rect_filled(bar, CornerRadius::same(12), Color32::from_rgb(22, 28, 40));
        ui.scope_builder(egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(14.0, 6.0))), |ui| {
            ui.horizontal_centered(|ui| {
                let mut open = false;
                match self.player.as_mut() {
                    Some(p) => player_controls(p, ui, &mut open),
                    None => {
                        if theme::icon_button(ui, icon::MAGNIFYING_GLASS_MINUS, &t!("viewer.zoom_out")).clicked() {
                            self.zoom = (self.zoom / 1.25).max(MIN_ZOOM);
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            open = open_button(ui);
                            ui.add_space(8.0);
                            if theme::icon_button(ui, icon::ARROWS_IN, &t!("viewer.fit")).clicked() {
                                (self.zoom, self.pan) = (1.0, Vec2::ZERO);
                            }
                            let pct = RichText::new(format!("{:>4.0}%", self.zoom * 100.0)).color(Color32::WHITE).font(FontId::monospace(12.0));
                            ui.add_sized([48.0, 24.0], egui::Label::new(pct));
                            if theme::icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, &t!("viewer.zoom_in")).clicked() {
                                self.zoom = (self.zoom * 1.25).min(MAX_ZOOM);
                            }
                            // Zoom slider: 100 % in the middle, logarithmic both ways.
                            let w = ui.available_width();
                            let (resp, frac) = track_slider(ui, w, zoom_to_frac(self.zoom), "viewer-zoom");
                            if let Some(f) = frac {
                                self.zoom = frac_to_zoom(f);
                                if resp.double_clicked() {
                                    (self.zoom, self.pan) = (1.0, Vec2::ZERO);
                                }
                            }
                        });
                    }
                }
                if open {
                    *action = Some(ViewerAction::OpenFile(self.path.clone()));
                }
            });
        });
    }
}

const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 10.0;

/// Zoom (1.0 = fit) to a slider position: 0.5 is 100 %, the ends are `MIN_ZOOM` / `MAX_ZOOM`.
fn zoom_to_frac(zoom: f32) -> f32 {
    (0.5 + zoom.clamp(MIN_ZOOM, MAX_ZOOM).ln() / (2.0 * MAX_ZOOM.ln())).clamp(0.0, 1.0)
}

fn frac_to_zoom(frac: f32) -> f32 {
    ((frac - 0.5) * 2.0 * MAX_ZOOM.ln()).exp().clamp(MIN_ZOOM, MAX_ZOOM)
}

fn open_button(ui: &mut egui::Ui) -> bool {
    ui.add(theme::secondary(icon::ARROW_SQUARE_OUT, &t!("viewer.open_file"))).clicked()
}

/// Slider drawn like the video timeline (thin track, accent fill, white knob). Returns the
/// response and the new position while it is clicked or dragged.
fn track_slider(ui: &mut egui::Ui, width: f32, frac: f32, id: &str) -> (egui::Response, Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(40.0), 24.0), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 14.0, 4.0));
    let accent = theme::DARK.accent;
    let x = track.left() + track.width() * frac.clamp(0.0, 1.0);
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), Color32::from_white_alpha(60));
    painter.rect_filled(Rect::from_min_max(track.min, egui::pos2(x, track.max.y)), CornerRadius::same(2), accent);
    let knob = if resp.hovered() || resp.dragged() { 8.0 } else { 7.0 };
    painter.circle(egui::pos2(x, track.center().y), knob, Color32::WHITE, Stroke::new(1.5, accent));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let value = resp
        .interact_pointer_pos()
        .filter(|_| resp.clicked() || resp.dragged())
        .map(|p| ((p.x - track.left()) / track.width()).clamp(0.0, 1.0));
    (resp, value)
}

fn upload(ctx: &egui::Context, reuse: Option<egui::TextureHandle>, f: &Frame) -> egui::TextureHandle {
    let img = egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.data);
    match reuse {
        Some(mut t) => {
            t.set(img, egui::TextureOptions::LINEAR);
            t
        }
        None => ctx.load_texture("media-viewer", img, egui::TextureOptions::LINEAR),
    }
}

/// Play · time · timeline (fills the rest) · speed · sound · open file.
fn player_controls(p: &mut Player, ui: &mut egui::Ui, open: &mut bool) {
    let (ic, tip) = if p.is_paused() { (icon::PLAY, t!("viewer.play")) } else { (icon::PAUSE, t!("viewer.pause")) };
    if theme::icon_button(ui, ic, &tip).clicked() {
        p.toggle_pause();
    }
    let pos = p.position();
    let duration = p.duration();
    // Whole seconds elapsed; at the very end exactly the duration.
    let shown = if duration > 0.0 && pos >= duration - 0.05 { duration } else { pos.floor() };
    let time = format!("{} / {}", human_duration(shown), human_duration(duration));
    ui.label(RichText::new(time).color(Color32::WHITE).font(FontId::monospace(12.0)));

    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        *open = open_button(ui);
        ui.add_space(8.0);
        if p.has_sound() {
            let (_, v) = track_slider(ui, 96.0, p.volume(), "viewer-volume");
            if let Some(v) = v {
                p.set_volume(v);
            }
            let ic = if p.volume() == 0.0 { icon::SPEAKER_SLASH } else { icon::SPEAKER_HIGH };
            if theme::icon_button(ui, ic, &t!("viewer.mute")).clicked() {
                p.set_volume(if p.volume() == 0.0 { 1.0 } else { 0.0 });
            }
            ui.add_space(4.0);
        }
        egui::ComboBox::from_id_salt("viewer-speed")
            .width(58.0)
            .selected_text(format!("{}×", p.speed()))
            .show_ui(ui, |ui| {
                for s in SPEEDS {
                    if ui.selectable_label(p.speed() == s, format!("{s}×")).clicked() {
                        p.set_speed(s);
                    }
                }
            })
            .response
            .on_hover_text(t!("viewer.speed"));

        // Timeline: whatever width is left. Click to seek; dragging moves the clock and the
        // decoders restart once at the release.
        let frac = if duration > 0.0 { (pos / duration) as f32 } else { 0.0 };
        let w = ui.available_width();
        let (resp, value) = track_slider(ui, w, frac, "viewer-timeline");
        if let Some(f) = value {
            let t = f64::from(f) * duration;
            if resp.dragged() {
                p.scrub(t);
            } else {
                p.seek(t);
            }
        }
        if resp.drag_stopped() {
            p.end_scrub();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{frac_to_zoom, zoom_to_frac, MAX_ZOOM, MIN_ZOOM};

    #[test]
    fn zoom_slider_is_centred_on_100_percent() {
        assert!((zoom_to_frac(1.0) - 0.5).abs() < 1e-6);
        assert_eq!(zoom_to_frac(MIN_ZOOM), 0.0);
        assert_eq!(zoom_to_frac(MAX_ZOOM), 1.0);
        for z in [0.25, 0.5, 2.0, 4.0] {
            assert!((frac_to_zoom(zoom_to_frac(z)) - z).abs() < 1e-4, "{z}");
        }
        assert!(zoom_to_frac(2.0) - 0.5 > 0.0 && (zoom_to_frac(2.0) - 0.5 - (0.5 - zoom_to_frac(0.5))).abs() < 1e-6, "symmetric");
    }
}
