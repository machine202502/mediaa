use super::player::Player;
use crate::jobs::Job;
use crate::theme;
use crate::widgets::track_slider;
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
    Volume(f32),
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
    speed_open: bool,
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

    pub fn open(
        path: &Path,
        kind: MediaKind,
        duration: Option<f64>,
        audio: bool,
        volume: f32,
    ) -> Viewer {
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
            speed_open: false,
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
            player.set_volume(volume);
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
            let p = theme::palette(ui);
            let rect = Rect::from_center_size(area.center(), egui::vec2(220.0, 220.0));
            ui.painter()
                .circle_filled(rect.center(), 110.0, p.accent_soft);
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                icon::MUSIC_NOTES,
                FontId::proportional(96.0),
                p.accent,
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
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new(icon::X)
                                        .size(15.0)
                                        .color(egui::Color32::WHITE),
                                )
                                .frame(false),
                            )
                            .on_hover_text(t!("viewer.close"))
                            .clicked()
                        {
                            *action = Some(ViewerAction::Close);
                        }
                    });
                });
            },
        );
    }

    fn controls(&mut self, ui: &mut egui::Ui, rect: Rect, action: &mut Option<ViewerAction>) {
        let p = theme::palette(ui);
        let width = rect.width().min(980.0) - 32.0;
        let bar = Rect::from_center_size(rect.center(), egui::vec2(width, rect.height() - 12.0));
        ui.painter().rect(
            bar,
            CornerRadius::same(12),
            p.card,
            Stroke::new(1.0, p.border),
            egui::StrokeKind::Inside,
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(14.0, 6.0))),
            |ui| {
                ui.horizontal_centered(|ui| {
                    let mut open = false;
                    let mut volume = None;
                    match self.player.as_mut() {
                        Some(p) => volume = player_controls(p, ui, &mut open, &mut self.speed_open),
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
                                        .color(p.text)
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
                    if let Some(v) = volume {
                        *action = Some(ViewerAction::Volume(v));
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

fn speed_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, ui.spacing().interact_size.y),
        egui::Sense::click(),
    );
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect,
        egui::CornerRadius::same(6),
        visuals.bg_fill,
        visuals.bg_stroke,
        egui::StrokeKind::Inside,
    );
    let text = format!("{label}  {}", icon::CARET_DOWN);
    ui.painter().text(
        rect.right_center() - egui::vec2(8.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        text,
        egui::TextStyle::Button.resolve(ui.style()),
        visuals.text_color(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, ui.is_enabled(), label)
    });
    response
}
fn speed_text_width(ui: &egui::Ui) -> f32 {
    let font = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Button)
        .cloned()
        .unwrap_or_else(|| egui::FontId::proportional(14.0));
    SPEEDS
        .iter()
        .map(|speed| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(format!("{speed}×"), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max)
}

fn speed_button_width(ui: &egui::Ui) -> f32 {
    speed_text_width(ui) + 34.0
}

fn player_controls(
    p: &mut Player,
    ui: &mut egui::Ui,
    open: &mut bool,
    speed_open: &mut bool,
) -> Option<f32> {
    let mut volume_changed = None;
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
    let palette = theme::palette(ui);
    ui.label(
        RichText::new(time)
            .color(palette.text)
            .font(FontId::monospace(12.0)),
    );

    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        *open = open_button(ui);
        ui.add_space(8.0);
        let (_, v) = track_slider(ui, 96.0, p.volume(), "viewer-volume");
        if let Some(v) = v {
            p.set_volume(v);
            volume_changed = Some(v);
        }
        let ic = if p.volume() == 0.0 {
            icon::SPEAKER_SLASH
        } else {
            icon::SPEAKER_HIGH
        };
        if theme::icon_button(ui, ic, &t!("viewer.mute")).clicked() {
            let volume = if p.volume() == 0.0 { 1.0 } else { 0.0 };
            p.set_volume(volume);
            volume_changed = Some(volume);
        }
        ui.add_space(4.0);
        let speed_width = speed_button_width(ui);
        let menu_width = speed_text_width(ui) + 20.0;
        let button = speed_button(ui, &format!("{}×", p.speed()), speed_width);
        if button.clicked() {
            *speed_open = !*speed_open;
        }
        if *speed_open {
            let mut chosen = None;
            let area = egui::Area::new(egui::Id::new("viewer-speed-menu"))
                .order(egui::Order::Foreground)
                .pivot(egui::Align2::RIGHT_BOTTOM)
                .fixed_pos(button.rect.right_top() - egui::vec2(0.0, 4.0))
                .show(ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_width(menu_width);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for s in SPEEDS {
                            let selected = p.speed() == s;
                            let (rect, response) = ui.allocate_exact_size(
                                egui::vec2(menu_width, 22.0),
                                egui::Sense::click(),
                            );
                            response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    ui.is_enabled(),
                                    format!("{s}×"),
                                )
                            });
                            let palette = theme::palette(ui);
                            let (fill, color) = if selected {
                                (palette.accent, palette.on_accent)
                            } else if response.hovered() {
                                (palette.accent_soft, palette.accent)
                            } else {
                                (egui::Color32::TRANSPARENT, palette.text)
                            };
                            if ui.is_rect_visible(rect) {
                                ui.painter()
                                    .rect_filled(rect, egui::CornerRadius::same(5), fill);
                                ui.painter().text(
                                    rect.right_center() - egui::vec2(8.0, 0.0),
                                    egui::Align2::RIGHT_CENTER,
                                    format!("{s}×"),
                                    egui::TextStyle::Button.resolve(ui.style()),
                                    color,
                                );
                            }
                            if response.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if response.clicked() {
                                chosen = Some(s);
                            }
                        }
                    });
                });
            if let Some(s) = chosen {
                p.set_speed(s);
                *speed_open = false;
            }
            if let Some(pointer) = ui.ctx().pointer_interact_pos() {
                if ui.input(|i| i.pointer.any_click())
                    && !area.response.rect.contains(pointer)
                    && !button.rect.contains(pointer)
                {
                    *speed_open = false;
                }
            }
        }
        button.on_hover_text(t!("viewer.speed"));

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
    volume_changed
}

#[cfg(test)]
mod tests {
    use super::{frac_to_zoom, zoom_to_frac, Viewer, MAX_ZOOM, MIN_ZOOM};
    use mediaa_core::MediaKind;
    use std::path::Path;

    #[test]
    fn a_viewer_starts_with_the_saved_volume() {
        let viewer = Viewer::open(
            Path::new("missing.mp4"),
            MediaKind::Video,
            Some(4.0),
            false,
            0.25,
        );
        assert_eq!(viewer.player.as_ref().unwrap().volume(), 0.25);
    }
    #[test]
    fn closing_a_viewer_cancels_its_owned_lookup() {
        let mut viewer = Viewer::open(Path::new("missing.png"), MediaKind::Image, None, false, 1.0);
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

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_viewer_player() {
        use eframe::egui::{ColorImage, TextureOptions};

        let viewer = Viewer::open(
            Path::new("Player_2026-01-01_10-00-00.mp4"),
            MediaKind::Video,
            Some(60.0),
            false,
            1.0,
        );
        let mut harness = egui_kittest::Harness::builder()
            .with_size(eframe::egui::vec2(1180.0, 900.0))
            .build_ui_state(
                |ui, viewer: &mut Viewer| {
                    viewer.show(ui.ctx());
                },
                viewer,
            );
        crate::theme::install(&harness.ctx);
        let image = ColorImage::from_rgba_unmultiplied(
            [2, 2],
            &[
                20, 40, 80, 255, 20, 40, 80, 255, 20, 40, 80, 255, 20, 40, 80, 255,
            ],
        );
        harness.state_mut().texture = Some(harness.ctx.load_texture(
            "viewer-test",
            image,
            TextureOptions::LINEAR,
        ));
        harness.run_steps(2);
        let options = egui_kittest::SnapshotOptions::default().max_failed_pixels(200);
        harness.snapshot_options("viewer-player-100", &options);
        harness
            .state_mut()
            .player
            .as_mut()
            .expect("video player")
            .set_speed(1.25);
        harness.run_steps(2);
        harness.snapshot_options("viewer-player-125", &options);
        use egui_kittest::kittest::Queryable;
        harness.get_by_label("1.25×").click();
        harness.run_steps(2);
        harness.snapshot_options("viewer-player-menu", &options);
        let item = harness.get_by_label("0.75×").rect().center();
        harness.hover_at(item);
        harness.run_steps(2);
        harness.snapshot_options("viewer-player-menu-hover", &options);
    }

    #[cfg(windows)]
    #[test]
    fn ui_regression_snapshot_viewer_player_light() {
        use eframe::egui::{ColorImage, TextureOptions};

        let viewer = Viewer::open(
            Path::new("Player_2026-01-01_10-00-00.mp4"),
            MediaKind::Video,
            Some(60.0),
            false,
            1.0,
        );
        let mut harness = egui_kittest::Harness::builder()
            .with_size(eframe::egui::vec2(1180.0, 900.0))
            .build_ui_state(
                |ui, viewer: &mut Viewer| {
                    viewer.show(ui.ctx());
                },
                viewer,
            );
        crate::theme::install(&harness.ctx);
        crate::theme::apply_preference(&harness.ctx, mediaa_core::settings::ThemePreference::Light);
        let image = ColorImage::from_rgba_unmultiplied(
            [2, 2],
            &[
                20, 40, 80, 255, 20, 40, 80, 255, 20, 40, 80, 255, 20, 40, 80, 255,
            ],
        );
        harness.state_mut().texture = Some(harness.ctx.load_texture(
            "viewer-test-light",
            image,
            TextureOptions::LINEAR,
        ));
        harness
            .state_mut()
            .player
            .as_mut()
            .expect("video player")
            .set_speed(1.25);
        harness.run_steps(2);
        let options = egui_kittest::SnapshotOptions::default().max_failed_pixels(200);
        harness.snapshot_options("viewer-player-light", &options);
    }
}
