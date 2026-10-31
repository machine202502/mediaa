use super::player::Player;
use crate::jobs::Job;
use crate::theme;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, Vec2};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::{Frame, MediaKind};
use mediaa_platform::audio::AudioOut;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

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
    image_job: Option<Job<Option<Frame>>>,
    audio_job: Option<Job<Result<AudioOut, String>>>,
    zoom: f32,
    pan: Vec2,
    player: Option<Player>,
}

impl Viewer {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cancel_pending(&mut self) {
        if let Some(job) = self.image_job.take() {
            job.cancel();
        }
        if let Some(job) = self.audio_job.take() {
            job.cancel();
        }
    }

    pub fn open(path: &Path, kind: MediaKind, duration: Option<f64>, audio: bool) -> Viewer {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut v = Viewer {
            path: path.to_path_buf(),
            name,
            kind,
            texture: None,
            image_job: None,
            audio_job: None,
            zoom: 1.0,
            pan: Vec2::ZERO,
            player: None,
        };
        if kind == MediaKind::Image {
            let p = path.to_path_buf();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            match Job::spawn(
                "viewer-image",
                cancel,
                move || match image::image_dimensions(&p)
                    .map_err(anyhow::Error::from)
                    .and_then(|(width, height)| {
                        mediaa_core::formats::ensure_safe_dimensions(width, height)?;
                        Ok(())
                    })
                    .and_then(|()| image::open(&p).map_err(anyhow::Error::from))
                {
                    Ok(img) => {
                        let img = if img.width().max(img.height()) > MAX_TEXTURE {
                            img.thumbnail(MAX_TEXTURE, MAX_TEXTURE)
                        } else {
                            img
                        };
                        (!worker_cancel.load(Ordering::Relaxed))
                            .then(|| Frame::from_image(img.to_rgba8()))
                    }
                    Err(e) => {
                        log::warn!("viewer {}: {e}", p.display());
                        None
                    }
                },
            ) {
                Ok(job) => v.image_job = Some(job),
                Err(error) => log::warn!("viewer image worker: {error}"),
            }
        } else {
            let mut player = Player::new(
                path,
                duration.unwrap_or(0.0),
                kind == MediaKind::Animation,
                kind != MediaKind::Audio,
            );
            if audio {
                player.expect_audio();
            }
            v.player = Some(player);
            if audio {
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = cancel.clone();
                match Job::spawn("viewer-audio-open", cancel, move || {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    AudioOut::open().map_err(|error| format!("{error:#}"))
                }) {
                    Ok(job) => v.audio_job = Some(job),
                    Err(error) => log::warn!("viewer audio worker: {error}"),
                }
            }
        }
        v
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<ViewerAction> {
        if let Some(job) = self.image_job.as_mut() {
            match job.poll() {
                Some(Ok(Some(frame))) => {
                    self.texture = Some(upload(ctx, None, &frame));
                    self.image_job = None;
                }
                Some(Ok(None)) => {
                    self.image_job = None;
                }
                Some(Err(error)) => {
                    log::warn!("viewer image worker stopped: {error:#}");
                    self.image_job = None;
                }
                None => {
                    ctx.request_repaint_after(Duration::from_millis(50));
                }
            }
        }
        if let Some(job) = self.audio_job.as_mut() {
            match job.poll() {
                Some(Ok(Ok(audio))) => {
                    if let Some(player) = self.player.as_mut() {
                        player.attach_audio(audio);
                    }
                    self.audio_job = None;
                }
                Some(Ok(Err(error))) => {
                    log::warn!("audio output: {error}");
                    self.audio_job = None;
                    if let Some(player) = self.player.as_mut() {
                        player.audio_unavailable();
                    }
                }
                Some(Err(error)) => {
                    log::warn!("audio output worker stopped: {error:#}");
                    self.audio_job = None;
                    if let Some(player) = self.player.as_mut() {
                        player.audio_unavailable();
                    }
                }
                None => {}
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
                ui.painter().rect_filled(
                    screen,
                    CornerRadius::ZERO,
                    Color32::from_black_alpha(232),
                );
                let bg = ui.interact(screen, ui.id().with("backdrop"), Sense::click());

                let top =
                    Rect::from_min_max(screen.min, egui::pos2(screen.max.x, screen.min.y + 52.0));
                let controls_h = 64.0;
                let bottom = Rect::from_min_max(
                    egui::pos2(screen.min.x, screen.max.y - controls_h - 12.0),
                    screen.max,
                );
                let area = Rect::from_min_max(
                    egui::pos2(screen.min.x + 24.0, top.max.y),
                    egui::pos2(screen.max.x - 24.0, bottom.min.y - 8.0),
                );

                let media = self.media(ui, area);
                self.top_bar(ui, top, &mut action);
                self.controls(ui, bottom, &mut action);

                let (esc, space, left, right) = ui.input(|i| {
                    (
                        i.key_pressed(egui::Key::Escape),
                        i.key_pressed(egui::Key::Space),
                        i.key_pressed(egui::Key::ArrowLeft),
                        i.key_pressed(egui::Key::ArrowRight),
                    )
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
                let outside = bg.clicked()
                    && bg
                        .interact_pointer_pos()
                        .is_some_and(|p| media.is_none_or(|m| !m.contains(p)));
                if esc || outside {
                    action.get_or_insert(ViewerAction::Close);
                }
            });
        action
    }

    fn media(&mut self, ui: &mut egui::Ui, area: Rect) -> Option<Rect> {
        if self.kind == MediaKind::Audio {
            let rect = Rect::from_center_size(area.center(), egui::vec2(220.0, 220.0));
            ui.painter()
                .circle_filled(rect.center(), 110.0, Color32::from_rgb(30, 40, 60));
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::MUSIC_NOTES,
                FontId::proportional(96.0),
                Color32::WHITE,
            );
            if ui
                .interact(rect, ui.id().with("audio-art"), Sense::click())
                .clicked()
            {
                if let Some(p) = self.player.as_mut() {
                    p.toggle_pause();
                }
            }
            return Some(rect);
        }
        let Some(tex) = &self.texture else {
            theme::preview_loader(ui, area.center(), 32.0);
            return None;
        };
        let size = tex.size_vec2();
        let fit = (area.width() / size.x).min(area.height() / size.y).min(2.0);
        let shown = size * fit * self.zoom;
        let rect = Rect::from_center_size(area.center() + self.pan, shown);
        let resp = ui.interact(
            rect.intersect(area),
            ui.id().with("media"),
            Sense::click_and_drag(),
        );
        ui.painter().with_clip_rect(area).image(
            tex.id(),
            rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
        if self
            .player
            .as_ref()
            .is_some_and(|player| player.preview_generating())
        {
            theme::preview_loader(ui, egui::pos2(area.right() - 22.0, area.top() + 22.0), 24.0);
        }

        if self.kind == MediaKind::Image {
            if resp.dragged() {
                self.pan += resp.drag_delta();
            }
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if resp.hovered() && scroll != 0.0 {
                let factor = (scroll / 200.0).exp();
                let new_zoom = (self.zoom * factor).clamp(MIN_ZOOM, MAX_ZOOM);
                if let Some(cursor) = resp.hover_pos() {
                    let k = new_zoom / self.zoom;
                    self.pan = (self.pan - (cursor - area.center())) * k + (cursor - area.center());
                }
                self.zoom = new_zoom;
            }
            if resp.double_clicked() {
                (self.zoom, self.pan) = if self.zoom == 1.0 {
                    (2.0, Vec2::ZERO)
                } else {
                    (1.0, Vec2::ZERO)
                };
            }
        } else if let Some(p) = self.player.as_mut() {
            if resp.clicked() {
                p.toggle_pause();
            }
        }
        Some(rect.intersect(area))
    }

    fn top_bar(&self, ui: &mut egui::Ui, rect: Rect, action: &mut Option<ViewerAction>) {
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(20.0, 10.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new(&self.name).size(15.0).color(Color32::WHITE));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, icon::X, &t!("viewer.close")).clicked() {
                            *action = Some(ViewerAction::Close);
                        }
                    });
                });
            },
        );
    }

    fn controls(&mut self, ui: &mut egui::Ui, rect: Rect, action: &mut Option<ViewerAction>) {
        let width = rect.width().min(980.0) - 32.0;
        let bar = Rect::from_center_size(rect.center(), egui::vec2(width, rect.height() - 12.0));
        ui.painter()
            .rect_filled(bar, CornerRadius::same(12), Color32::from_rgb(22, 28, 40));
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(14.0, 6.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    let mut open = false;
                    match self.player.as_mut() {
                        Some(p) => player_controls(p, ui, &mut open),
                        None => {
                            if theme::icon_button(
                                ui,
                                icon::MAGNIFYING_GLASS_MINUS,
                                &t!("viewer.zoom_out"),
                            )
                            .clicked()
                            {
                                self.zoom = (self.zoom / 1.25).max(MIN_ZOOM);
                            }
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    open = open_button(ui);
                                    ui.add_space(8.0);
                                    if theme::icon_button(ui, icon::ARROWS_IN, &t!("viewer.fit"))
                                        .clicked()
                                    {
                                        (self.zoom, self.pan) = (1.0, Vec2::ZERO);
                                    }
                                    let pct = RichText::new(format!("{:>4.0}%", self.zoom * 100.0))
                                        .color(Color32::WHITE)
                                        .font(FontId::monospace(12.0));
                                    ui.add_sized([48.0, 24.0], egui::Label::new(pct));
                                    if theme::icon_button(
                                        ui,
                                        icon::MAGNIFYING_GLASS_PLUS,
                                        &t!("viewer.zoom_in"),
                                    )
                                    .clicked()
                                    {
                                        self.zoom = (self.zoom * 1.25).min(MAX_ZOOM);
                                    }
                                    let w = ui.available_width();
                                    let (resp, frac) =
                                        track_slider(ui, w, zoom_to_frac(self.zoom), "viewer-zoom");
                                    if let Some(f) = frac {
                                        self.zoom = frac_to_zoom(f);
                                        if resp.double_clicked() {
                                            (self.zoom, self.pan) = (1.0, Vec2::ZERO);
                                        }
                                    }
                                },
                            );
                        }
                    }
                    if open {
                        *action = Some(ViewerAction::OpenFile(self.path.clone()));
                    }
                });
            },
        );
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.cancel_pending();
    }
}

const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 10.0;

fn zoom_to_frac(zoom: f32) -> f32 {
    (0.5 + zoom.clamp(MIN_ZOOM, MAX_ZOOM).ln() / (2.0 * MAX_ZOOM.ln())).clamp(0.0, 1.0)
}

fn frac_to_zoom(frac: f32) -> f32 {
    ((frac - 0.5) * 2.0 * MAX_ZOOM.ln())
        .exp()
        .clamp(MIN_ZOOM, MAX_ZOOM)
}

fn open_button(ui: &mut egui::Ui) -> bool {
    ui.add(theme::secondary(
        icon::ARROW_SQUARE_OUT,
        &t!("viewer.open_file"),
    ))
    .clicked()
}

fn track_slider(
    ui: &mut egui::Ui,
    width: f32,
    frac: f32,
    id: &str,
) -> (egui::Response, Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(40.0), 24.0), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 14.0, 4.0));
    let accent = theme::DARK.accent;
    let x = track.left() + track.width() * frac.clamp(0.0, 1.0);
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), Color32::from_white_alpha(60));
    painter.rect_filled(
        Rect::from_min_max(track.min, egui::pos2(x, track.max.y)),
        CornerRadius::same(2),
        accent,
    );
    let knob = if resp.hovered() || resp.dragged() {
        8.0
    } else {
        7.0
    };
    painter.circle(
        egui::pos2(x, track.center().y),
        knob,
        Color32::WHITE,
        Stroke::new(1.5, accent),
    );
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let value = resp
        .interact_pointer_pos()
        .filter(|_| resp.clicked() || resp.dragged())
        .map(|p| ((p.x - track.left()) / track.width()).clamp(0.0, 1.0));
    (resp, value)
}

fn upload(
    ctx: &egui::Context,
    reuse: Option<egui::TextureHandle>,
    f: &Frame,
) -> egui::TextureHandle {
    let img =
        egui::ColorImage::from_rgba_unmultiplied([f.width as usize, f.height as usize], &f.data);
    match reuse {
        Some(mut t) => {
            t.set(img, egui::TextureOptions::LINEAR);
            t
        }
        None => ctx.load_texture("media-viewer", img, egui::TextureOptions::LINEAR),
    }
}

fn player_controls(p: &mut Player, ui: &mut egui::Ui, open: &mut bool) {
    let (ic, tip) = if p.is_paused() {
        (icon::PLAY, t!("viewer.play"))
    } else {
        (icon::PAUSE, t!("viewer.pause"))
    };
    if theme::icon_button(ui, ic, &tip).clicked() {
        p.toggle_pause();
    }
    let pos = p.position();
    let duration = p.duration();
    let shown = if duration > 0.0 && pos >= duration - 0.05 {
        duration
    } else {
        pos.floor()
    };
    let time = format!("{} / {}", human_duration(shown), human_duration(duration));
    ui.label(
        RichText::new(time)
            .color(Color32::WHITE)
            .font(FontId::monospace(12.0)),
    );

    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        *open = open_button(ui);
        ui.add_space(8.0);
        if p.has_sound() {
            let (_, v) = track_slider(ui, 96.0, p.volume(), "viewer-volume");
            if let Some(v) = v {
                p.set_volume(v);
            }
            let ic = if p.volume() == 0.0 {
                icon::SPEAKER_SLASH
            } else {
                icon::SPEAKER_HIGH
            };
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
                    if ui
                        .selectable_label(p.speed() == s, format!("{s}×"))
                        .clicked()
                    {
                        p.set_speed(s);
                    }
                }
            })
            .response
            .on_hover_text(t!("viewer.speed"));

        let frac = if duration > 0.0 {
            (pos / duration) as f32
        } else {
            0.0
        };
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
    use super::{frac_to_zoom, zoom_to_frac, Viewer, MAX_ZOOM, MIN_ZOOM};
    use mediaa_core::MediaKind;
    use std::path::Path;

    #[test]
    fn closing_a_viewer_cancels_its_owned_lookup() {
        let mut viewer = Viewer::open(Path::new("missing.png"), MediaKind::Image, None, false);
        assert!(viewer.image_job.is_some());
        viewer.cancel_pending();
        assert!(viewer.image_job.is_none());
    }

    #[test]
    fn zoom_slider_is_centred_on_100_percent() {
        assert!((zoom_to_frac(1.0) - 0.5).abs() < 1e-6);
        assert_eq!(zoom_to_frac(MIN_ZOOM), 0.0);
        assert_eq!(zoom_to_frac(MAX_ZOOM), 1.0);
        for z in [0.25, 0.5, 2.0, 4.0] {
            assert!((frac_to_zoom(zoom_to_frac(z)) - z).abs() < 1e-4, "{z}");
        }
        assert!(
            zoom_to_frac(2.0) - 0.5 > 0.0
                && (zoom_to_frac(2.0) - 0.5 - (0.5 - zoom_to_frac(0.5))).abs() < 1e-6,
            "symmetric"
        );
    }
}
