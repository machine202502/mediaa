use super::chat::Chat;
use super::dialog::Dialog;
use super::topics;
use super::Services;
use crate::player::Player;
use crate::theme;
use eframe::egui::{self, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke};
use egui_phosphor::regular as icon;
use mediaa_agent::pipeline::Stage;
use mediaa_core::agent::AnalysisOptions;
use mediaa_core::library::human_duration;
use mediaa_core::MediaKind;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const SPEEDS: [f64; 6] = [0.8, 1.0, 1.25, 1.5, 2.0, 3.0];
const PANES: [f32; 3] = [1.15, 1.0, 0.95];
const MIN_PANE: f32 = 150.0;
const NARROW: f32 = 980.0;
const SASH: f32 = 8.0;
const ROW_GAP: f32 = 8.0;
const ROW_PAD: f32 = topics::TRACK_INSET;
const ROW_H: f32 = theme::ICON_BUTTON;
const ROW_ICON: f32 = 14.0;
const VOLUME_TRACK: f32 = 96.0;
const SPEED_TRACK: f32 = 84.0;
const KNOB: f32 = topics::KNOB;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
enum Maximized {
    #[default]
    None,
    Dialog,
    Chat,
}

#[derive(Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Layout {
    sizes: [f32; 3],
    show_video: bool,
    cinema: bool,
    maximized: Maximized,
    sync: bool,
}

impl Default for Layout {
    fn default() -> Self {
        Self { sizes: PANES, show_video: true, cinema: false, maximized: Maximized::None, sync: true }
    }
}

impl Layout {
    fn toggle_max(&mut self, which: Maximized) {
        if self.maximized == which {
            self.maximized = Maximized::None;
        } else {
            self.maximized = which;
            self.cinema = false;
        }
    }

    fn toggle_cinema(&mut self) {
        self.cinema = !self.cinema;
        if self.cinema {
            self.show_video = true;
            self.maximized = Maximized::None;
        }
    }

    fn toggle_video(&mut self) {
        self.show_video = !self.show_video;
        if !self.show_video {
            self.cinema = false;
        }
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
    pending: Option<StudioAction>,
    pub drawn: bool,
}

impl Studio {
    pub fn open(path: &Path, ai_dir: &Path) -> anyhow::Result<Studio> {
        let kind = MediaKind::from_path(path).ok_or_else(|| anyhow::anyhow!("{}: not audio or video", path.display()))?;
        let info = mediaa_media::info::probe(path)?;
        let video = kind == MediaKind::Video && info.has_video();
        let mut player = Player::new(path, info.duration_secs().unwrap_or(0.0), info.has_audio(), false, video);
        player.toggle_pause();
        Ok(Studio { path: path.to_path_buf(), video, player, texture: None, dialog: Dialog::new(path, ai_dir), chat: Chat::new(path, ai_dir), pending: None, drawn: false })
    }

    pub fn seek(&mut self, t: f64) {
        self.player.jump(t);
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
        self.player.set_links(layout.sync);

        let full = ui.available_rect_before_wrap();
        let top = Rect::from_min_size(full.min, egui::vec2(full.width(), 44.0));
        let bar = Rect::from_min_max(egui::pos2(full.min.x, full.max.y - 60.0), full.max);
        let stage = Rect::from_min_max(egui::pos2(full.min.x + 6.0, top.max.y), egui::pos2(full.max.x - 6.0, bar.min.y - 2.0));

        ui.scope_builder(egui::UiBuilder::new().max_rect(top.shrink2(egui::vec2(10.0, 6.0))), |ui| {
            ui.horizontal_centered(|ui| {
                let open = theme::icon_face(ui, icon::ARROW_SQUARE_OUT, &t!("studio.open"), theme::ICON_BUTTON);
                tip(&open, &t!("studio.open"));
                if open.clicked() {
                    action = Some(StudioAction::Open(self.path.clone()));
                }
                let reveal = theme::icon_face(ui, icon::FOLDER_OPEN, &t!("lib.reveal"), theme::ICON_BUTTON);
                tip(&reveal, &t!("lib.reveal"));
                if reveal.clicked() {
                    action = Some(StudioAction::Reveal(self.path.clone()));
                }
                let known = self.dialog.has_speakers();
                let names_tip = if known { t!("studio.names") } else { t!("studio.names_none") };
                let names = ui.add_enabled_ui(known, |ui| theme::icon_face(ui, icon::USERS_THREE, &names_tip, theme::ICON_BUTTON)).inner;
                tip(&names, &names_tip);
                if known && names.clicked() {
                    self.dialog.open_names();
                }
                let title_label = ui.add(egui::Label::new(RichText::new(title).size(18.0).strong()).truncate());
                tip(&title_label, &self.path.to_string_lossy());
            });
        });

        self.stage(ui, stage, &mut layout, services, p);
        self.transport(ui, bar, &mut layout, p);
        if layout.maximized != Maximized::None && !self.dialog.blocks_escape() && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            layout.maximized = Maximized::None;
        }

        if layout != before {
            ui.ctx().data_mut(|d| d.insert_persisted(layout_id(), layout));
        }
        ui.allocate_rect(full, Sense::hover());
        action.or(self.pending.take())
    }

    fn stage(&mut self, ui: &mut egui::Ui, rect: Rect, layout: &mut Layout, services: Option<&Services>, p: theme::Palette) {
        if layout.maximized == Maximized::Dialog {
            self.dialog_pane(ui, rect, layout, services, p);
            return;
        }
        if layout.maximized == Maximized::Chat {
            self.chat_pane(ui, rect, layout, services, p);
            return;
        }
        let show_video = self.video && layout.show_video;
        if self.video && layout.cinema {
            self.video_pane(ui, rect, p);
            return;
        }
        let narrow = rect.width() < NARROW;
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
                0 => self.dialog_pane(ui, pane, layout, services, p),
                1 => self.video_pane(ui, pane, p),
                _ => self.chat_pane(ui, pane, layout, services, p),
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

    fn dialog_pane(&mut self, ui: &mut egui::Ui, pane: Rect, layout: &mut Layout, services: Option<&Services>, p: theme::Palette) {
        let mut chrome = super::dialog::Chrome { sync: layout.sync, maximized: layout.maximized == Maximized::Dialog };
        if let Some(a) = self.dialog.ui(ui, pane, &mut self.player, services, p, &mut chrome) {
            self.pending = Some(a);
        }
        layout.sync = chrome.sync;
        if chrome.maximized {
            layout.maximized = Maximized::Dialog;
            layout.cinema = false;
        } else if layout.maximized == Maximized::Dialog {
            layout.maximized = Maximized::None;
        }
    }

    fn chat_pane(&mut self, ui: &mut egui::Ui, pane: Rect, layout: &mut Layout, services: Option<&Services>, p: theme::Palette) {
        if !self.dialog.has_replies() {
            chat_later(ui, pane, p);
        } else if let Some(a) = self.chat.ui(ui, pane, Some(&mut self.player), services, p) {
            self.pending = Some(a);
        }
        let on = layout.maximized == Maximized::Chat;
        let (glyph, text) = if on { (icon::ARROWS_IN, t!("studio.restore")) } else { (icon::ARROWS_OUT, t!("studio.maximize")) };
        if edge_button(ui, pane, true, glyph, &text, "chat").clicked() {
            layout.toggle_max(Maximized::Chat);
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
        if ui.interact(body, ui.id().with("studio-screen"), Sense::click()).clicked() {
            self.player.toggle_pause();
        }
    }

    fn transport(&mut self, ui: &mut egui::Ui, rect: Rect, layout: &mut Layout, p: theme::Palette) {
        let chapters = self.dialog.chapters();
        let roster = self.dialog.roster();
        ui.painter().hline(rect.x_range(), rect.min.y, Stroke::new(1.0, p.border));
        ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(12.0, 8.0))), |ui| {
            let width = ui.available_width();
            let row = ui.allocate_ui_with_layout(egui::vec2(width, ROW_H), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                let player = &mut self.player;
                let (ic, tip) = if player.is_paused() { (icon::PLAY, t!("viewer.play")) } else { (icon::PAUSE, t!("viewer.pause")) };
                if theme::icon_button(ui, ic, &tip).clicked() {
                    player.toggle_pause();
                }
                let (pos, duration) = (player.position(), player.duration());
                let shown = if duration > 0.0 && pos >= duration - 0.05 { duration } else { pos.floor() };
                ui.label(RichText::new(format!("{} / {}", human_duration(shown), human_duration(duration))).font(FontId::monospace(12.0)));

                let label = speed_label(player.speed());
                let boxes = row_boxes(ui.available_rect_before_wrap(), self.video, player.has_sound(), speed_label_width(ui));

                if let Some(b) = boxes.cinema {
                    let (ic, tip) = if layout.cinema { (icon::CORNERS_IN, t!("studio.cinema_off")) } else { (icon::CORNERS_OUT, t!("studio.cinema")) };
                    if at(ui, b, ROW_H, |ui| toggle_button(ui, ic, &tip, layout.cinema, p)).clicked() {
                        layout.toggle_cinema();
                    }
                }
                if let Some(b) = boxes.camera {
                    let (ic, tip) = if layout.show_video { (icon::VIDEO_CAMERA, t!("studio.hide_video")) } else { (icon::VIDEO_CAMERA_SLASH, t!("studio.show_video")) };
                    if at(ui, b, ROW_H, |ui| toggle_button(ui, ic, &tip, !layout.show_video, p)).clicked() {
                        layout.toggle_video();
                    }
                }
                ui.painter().text(boxes.label.center(), egui::Align2::CENTER_CENTER, label, FontId::monospace(12.0), p.text);
                let idx = SPEEDS.iter().position(|&s| s == player.speed()).unwrap_or(1);
                let (resp, v) = at(ui, boxes.speed, 24.0, |ui| slider(ui, SPEED_TRACK, idx as f32 / (SPEEDS.len() - 1) as f32, "studio-speed", p));
                resp.on_hover_text(t!("studio.speed"));
                if let Some(v) = v {
                    player.set_speed(SPEEDS[(v * (SPEEDS.len() - 1) as f32).round() as usize]);
                }
                ui.painter().text(boxes.gauge.center(), egui::Align2::CENTER_CENTER, icon::GAUGE, FontId::proportional(14.0), p.text_weak);
                ui.interact(boxes.gauge, ui.id().with("studio-speed-gauge"), Sense::hover()).on_hover_text(t!("studio.speed"));
                if let Some(b) = boxes.volume {
                    let (resp, v) = at(ui, b, 24.0, |ui| slider(ui, VOLUME_TRACK, player.volume(), "studio-volume", p));
                    resp.on_hover_text(t!("studio.volume"));
                    if let Some(v) = v {
                        player.set_volume(v);
                    }
                }
                if let Some(b) = boxes.speaker {
                    let muted = player.volume() == 0.0;
                    let (ic, tip) = if muted { (icon::SPEAKER_SLASH, t!("studio.unmute")) } else { (icon::SPEAKER_HIGH, t!("viewer.mute")) };
                    if at(ui, b, ROW_H, |ui| toggle_button(ui, ic, &tip, muted, p)).clicked() {
                        player.set_volume(if muted { 1.0 } else { 0.0 });
                    }
                }
                let frac = if duration > 0.0 { (pos / duration) as f32 } else { 0.0 };
                let seek = boxes.seek;
                let (resp, v) = at(ui, seek, 24.0, |ui| topics::seek_bar(ui, seek.width(), frac, duration, chapters, "studio-position", &roster, p));
                if let Some(f) = v {
                    let t = f64::from(f) * duration;
                    if resp.dragged() {
                        player.scrub(t);
                    } else {
                        player.jump(t);
                    }
                }
                if resp.drag_stopped() {
                    player.end_scrub();
                }
                resp.rect
            });
            if !chapters.is_empty() {
                let seek = row.inner;
                let top = row.response.rect.bottom() + 2.0;
                let left = seek.left() + topics::TRACK_INSET;
                let line = Rect::from_min_max(egui::pos2(left, top), egui::pos2(seek.right(), top + 16.0));
                let text = topics::current_label(chapters, self.player.position());
                ui.scope_builder(egui::UiBuilder::new().max_rect(line), |ui| {
                    let galley = super::mentions::galley(ui, &text, &roster, FontId::proportional(11.0), p.text_weak, ui.available_width(), true, false, p);
                    ui.add(egui::Label::new(galley).truncate());
                });
            }
        });
    }
}

struct Row {
    right: f32,
    top: f32,
    bottom: f32,
}

impl Row {
    fn new(area: Rect) -> Self {
        Self { right: area.right(), top: area.top(), bottom: area.bottom() }
    }

    fn take(&mut self, body: f32) -> Rect {
        let rect = Rect::from_min_max(egui::pos2(self.right - body - 2.0 * ROW_PAD, self.top), egui::pos2(self.right, self.bottom));
        self.right = rect.left() - ROW_GAP;
        rect
    }

    fn seek(&self, area: Rect) -> Rect {
        Rect::from_min_max(area.min, egui::pos2(self.right, area.bottom()))
    }
}

struct Boxes {
    cinema: Option<Rect>,
    camera: Option<Rect>,
    label: Rect,
    speed: Rect,
    gauge: Rect,
    volume: Option<Rect>,
    speaker: Option<Rect>,
    seek: Rect,
}

fn row_boxes(area: Rect, video: bool, sound: bool, label_width: f32) -> Boxes {
    let mut boxes = Row::new(area);
    let cinema = video.then(|| boxes.take(ROW_ICON));
    let camera = video.then(|| boxes.take(ROW_ICON));
    let label = boxes.take(label_width);
    let speed = boxes.take(SPEED_TRACK);
    let gauge = boxes.take(ROW_ICON);
    let volume = sound.then(|| boxes.take(VOLUME_TRACK));
    let speaker = sound.then(|| boxes.take(ROW_ICON));
    let seek = boxes.seek(area);
    Boxes { cinema, camera, label, speed, gauge, volume, speaker, seek }
}

fn at<R>(ui: &mut egui::Ui, rect: Rect, height: f32, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let inner = Rect::from_center_size(rect.center(), egui::vec2(rect.width(), height));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)), add).inner
}

fn speed_label(speed: f64) -> String {
    format!("{speed:.2}×")
}

fn speed_label_width(ui: &egui::Ui) -> f32 {
    let font = FontId::monospace(12.0);
    SPEEDS.iter().map(|&s| ui.fonts_mut(|f| f.layout_no_wrap(speed_label(s), font.clone(), Color32::PLACEHOLDER)).size().x).fold(0.0, f32::max)
}

pub enum StudioAction {
    Open(PathBuf),
    Reveal(PathBuf),
    Analyse { path: PathBuf, from: Option<Stage>, options: AnalysisOptions, remember: bool },
    Stop(mediaa_queue::TaskId),
    FindNames(PathBuf),
    ResumeTask(mediaa_queue::TaskId),
    OpenAt(PathBuf, f64),
}

fn chat_later(ui: &mut egui::Ui, rect: Rect, p: theme::Palette) {
    let body = panel(ui, rect, &t!("studio.chat"), p);
    ui.scope_builder(egui::UiBuilder::new().max_rect(body.shrink2(egui::vec2(10.0, 8.0))), |ui| {
        ui.centered_and_justified(|ui| theme::hint(ui, t!("analysis.chat_later")));
    });
}

pub(super) const PANE_HEAD: f32 = 30.0;

pub(super) fn panel(ui: &egui::Ui, rect: Rect, title: &str, p: theme::Palette) -> Rect {
    panel_insets(ui, rect, title, 12.0, 12.0, p)
}

pub(super) fn panel_insets(ui: &egui::Ui, rect: Rect, title: &str, left: f32, right: f32, p: theme::Palette) -> Rect {
    let painter = ui.painter_at(rect);
    painter.rect(rect, CornerRadius::same(12), p.card, Stroke::new(1.0, p.border), egui::StrokeKind::Inside);
    let head = Rect::from_min_size(rect.min, egui::vec2(rect.width(), PANE_HEAD));
    painter.hline(head.x_range(), head.max.y, Stroke::new(1.0, p.border));
    let text = Rect::from_min_max(egui::pos2(head.left() + left, head.top()), egui::pos2(head.right() - right, head.bottom()));
    let mut clip = ui.painter_at(text);
    clip.set_clip_rect(text.intersect(ui.clip_rect()));
    clip.text(text.left_center(), egui::Align2::LEFT_CENTER, title, FontId::proportional(12.0), p.text_weak);
    Rect::from_min_max(egui::pos2(rect.min.x + 1.0, head.max.y + 1.0), rect.max - egui::vec2(1.0, 1.0))
}

pub(crate) fn tip(resp: &egui::Response, text: &str) {
    let mut tip = if resp.enabled() { egui::Tooltip::for_enabled(resp) } else { egui::Tooltip::for_disabled(resp) };
    tip.popup = tip.popup.frame(super::topics::tooltip_frame(resp.ctx.global_style().as_ref()));
    tip.show(|ui| {
        ui.label(text);
    });
}

pub(crate) fn edge_button(ui: &mut egui::Ui, pane: Rect, right: bool, glyph: &str, text: &str, salt: &str) -> egui::Response {
    let x = if right { pane.right() - 4.0 - theme::ICON_BUTTON } else { pane.left() + 4.0 };
    let y = pane.top() + (PANE_HEAD - theme::ICON_BUTTON) / 2.0;
    let rect = Rect::from_min_size(egui::pos2(x, y), egui::vec2(theme::ICON_BUTTON, theme::ICON_BUTTON));
    let resp = ui.interact(rect, ui.id().with(("pane-tool", salt)), Sense::click());
    if ui.is_rect_visible(rect) {
        let p = theme::palette(ui);
        let (fill, stroke, color) = if resp.is_pointer_button_down_on() {
            (p.accent, p.accent, p.on_accent)
        } else if resp.hovered() {
            (p.accent_soft, p.accent, p.text)
        } else {
            (Color32::TRANSPARENT, Color32::TRANSPARENT, p.text)
        };
        ui.painter().rect(rect, CornerRadius::same(6), fill, Stroke::new(1.0, stroke), egui::StrokeKind::Inside);
        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, glyph, FontId::proportional(14.0), color);
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    tip(&resp, text);
    resp
}

fn toggle_button(ui: &mut egui::Ui, ic: &str, tip: &str, on: bool, p: theme::Palette) -> egui::Response {
    let resp = theme::icon_button(ui, ic, tip);
    if on {
        ui.painter().rect_filled(resp.rect, CornerRadius::same(6), p.accent_soft);
        ui.painter().text(resp.rect.center(), egui::Align2::CENTER_CENTER, ic, FontId::proportional(14.0), p.accent);
    }
    resp
}

fn slider(ui: &mut egui::Ui, track_width: f32, frac: f32, id: &str, p: theme::Palette) -> (egui::Response, Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(track_width + 2.0 * ROW_PAD, 24.0), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    let (track, centre) = slider_parts(rect, frac);
    let x = centre.x;
    let painter = ui.painter();
    painter.rect_filled(track, CornerRadius::same(2), p.border);
    painter.rect_filled(Rect::from_min_max(track.min, egui::pos2(x, track.max.y)), CornerRadius::same(2), p.accent);
    let knob = if resp.hovered() || resp.dragged() { KNOB } else { KNOB - 1.0 };
    painter.circle(centre, knob, Color32::WHITE, Stroke::new(1.5, p.accent));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let value = resp.interact_pointer_pos().filter(|_| resp.clicked() || resp.dragged()).map(|pt| ((pt.x - track.left() - KNOB) / (track.width() - 2.0 * KNOB)).clamp(0.0, 1.0));
    (resp, value)
}

fn slider_parts(rect: Rect, frac: f32) -> (Rect, egui::Pos2) {
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 2.0 * ROW_PAD, 4.0));
    let x = track.left() + KNOB + (track.width() - 2.0 * KNOB) * frac.clamp(0.0, 1.0);
    (track, egui::pos2(x, track.center().y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speeds_include_normal_and_layout_defaults() {
        assert!(SPEEDS.contains(&1.0) && SPEEDS.windows(2).all(|w| w[0] < w[1]));
        let l = Layout::default();
        assert!(l.show_video && !l.cinema && l.sync && l.maximized == Maximized::None && l.sizes == PANES);
    }

    #[test]
    fn maximize_restores_the_same_sizes_and_yields_to_cinema() {
        let mut layout = Layout::default();
        let sizes = [2.0, 3.0, 4.0];
        layout.sizes = sizes;
        layout.cinema = true;
        layout.toggle_max(Maximized::Dialog);
        assert_eq!(layout.maximized, Maximized::Dialog);
        assert!(!layout.cinema, "a pane and the video-only view are not on together");
        assert_eq!(layout.sizes, sizes);
        layout.toggle_max(Maximized::Chat);
        assert_eq!(layout.maximized, Maximized::Chat);
        assert_eq!(layout.sizes, sizes, "switching panes keeps the split");
        layout.toggle_max(Maximized::Chat);
        assert_eq!(layout.maximized, Maximized::None);
        assert_eq!(layout.sizes, sizes, "restore brings the same split back");
        layout.toggle_max(Maximized::Dialog);
        layout.toggle_cinema();
        assert!(layout.cinema && layout.show_video && layout.maximized == Maximized::None);
        assert_eq!(layout.sizes, sizes);
        layout.toggle_video();
        assert!(!layout.show_video && !layout.cinema);
        assert_eq!(layout.sizes, sizes);
    }

    fn body(box_: Rect, width: f32) -> Rect {
        Rect::from_center_size(box_.center(), egui::vec2(width, box_.height()))
    }

    fn gap(left: Rect, right: Rect) -> f32 {
        right.left() - left.right()
    }

    #[test]
    fn the_transport_row_uses_one_gap() {
        let area = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(600.0, ROW_H));
        let b = row_boxes(area, true, true, 34.0);
        let boxes = [b.cinema.unwrap(), b.camera.unwrap(), b.label, b.speed, b.gauge, b.volume.unwrap(), b.speaker.unwrap()];
        let bodies = [ROW_ICON, ROW_ICON, 34.0, SPEED_TRACK, ROW_ICON, VOLUME_TRACK, ROW_ICON];
        for (box_, body) in boxes.iter().zip(bodies) {
            assert!((box_.width() - body - 2.0 * ROW_PAD).abs() < 1e-6, "every control keeps the same padding around its body");
            assert_eq!(box_.center().y, area.center().y, "icons and sliders share one centre line");
        }
        for pair in boxes.windows(2) {
            assert!((gap(pair[1], pair[0]) - ROW_GAP).abs() < 1e-6, "one gap between neighbouring boxes");
        }
        assert!((gap(b.seek, b.speaker.unwrap()) - ROW_GAP).abs() < 1e-6, "and the same gap before the seek bar");
        assert_eq!(b.seek.left(), area.left());
        assert!((theme::ICON_BUTTON - ROW_ICON - 2.0 * ROW_PAD).abs() < 1e-6);
        let b = row_boxes(area, false, false, 34.0);
        assert!(b.cinema.is_none() && b.camera.is_none() && b.volume.is_none() && b.speaker.is_none());
        assert_eq!(b.seek.right(), b.gauge.left() - ROW_GAP);
    }

    #[test]
    fn every_visible_gap_is_the_same_at_both_ends_of_the_volume() {
        let area = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(900.0, ROW_H));
        let label_w = 36.0;
        let mut previous: Option<Vec<f32>> = None;
        for volume in [0.0, 1.0] {
            let b = row_boxes(area, true, true, label_w);
            let seek = body(b.seek, b.seek.width() - 2.0 * topics::TRACK_INSET);
            let (volume_track, _) = slider_parts(b.volume.unwrap(), volume);
            let (speed_track, _) = slider_parts(b.speed, 1.0 / (SPEEDS.len() - 1) as f32);
            for frac in [0.0, 1.0] {
                let (t, centre) = slider_parts(b.volume.unwrap(), frac);
                assert!(centre.x - KNOB >= t.left() - 1e-6 && centre.x + KNOB <= t.right() + 1e-6, "the thumb never leaves its track");
            }
            let bodies = [
                seek,
                body(b.speaker.unwrap(), ROW_ICON),
                volume_track,
                body(b.gauge, ROW_ICON),
                speed_track,
                body(b.label, label_w),
                body(b.camera.unwrap(), ROW_ICON),
                body(b.cinema.unwrap(), ROW_ICON),
            ];
            let gaps: Vec<f32> = bodies.windows(2).map(|pair| gap(pair[0], pair[1])).collect();
            for g in &gaps {
                assert!((g - (ROW_GAP + 2.0 * ROW_PAD)).abs() < 1e-6, "every visible gap is one spacing: {gaps:?}");
            }
            if let Some(prev) = &previous {
                assert_eq!(prev, &gaps, "the volume value changes no gap");
            }
            previous = Some(gaps);
        }
    }

    #[test]
    fn the_speed_label_is_the_same_width_for_every_speed() {
        assert_eq!(speed_label(1.0), "1.00×");
        assert_eq!(speed_label(1.25), "1.25×");
        let counts: Vec<usize> = SPEEDS.iter().map(|&s| speed_label(s).chars().count()).collect();
        assert!(counts.windows(2).all(|w| w[0] == w[1]), "{counts:?}");
    }
}
