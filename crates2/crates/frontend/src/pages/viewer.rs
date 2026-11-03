use super::Ctx;
use crate::ui::theme;
use crate::ui::widgets::track_slider::track_slider;
use crate::ui_state::ViewerUi;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, StrokeKind,
    TextStyle, WidgetInfo, WidgetType,
};
use egui_phosphor::regular as icon;
use mediaa2_core::formats::MediaKind;
use mediaa2_core::library::human_duration;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_i18n::t;
use mediaa2_protocol::{ClientCommand, PreviewKind, PreviewState};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

const MIN_ZOOM: f64 = 0.1;
const MAX_ZOOM: f64 = 10.0;
const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];
const NOTE_COLOR: Color32 = Color32::from_rgb(163, 174, 193);
const PLAYBACK_NOTE: &str = "Playback in the system player";

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let Some(mut viewer) = ctx.ui.viewer.take() else {
        return;
    };
    let close = show_viewer(ctx, ui, &mut viewer);
    if close {
        ctx.ui.close_viewer();
    } else {
        ctx.ui.viewer = Some(viewer);
    }
}

fn show_viewer(ctx: &mut Ctx<'_>, ui: &mut egui::Ui, viewer: &mut ViewerUi) -> bool {
    let entry = ctx
        .view
        .library
        .entries
        .iter()
        .find(|entry| entry.path == viewer.path)
        .cloned();
    let kind = entry
        .as_ref()
        .map(|entry| entry.kind)
        .or_else(|| MediaKind::from_path(&viewer.path))
        .unwrap_or(MediaKind::Image);
    let name = entry.as_ref().map_or_else(
        || {
            viewer
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        },
        |entry| entry.name.clone(),
    );
    let duration = entry
        .as_ref()
        .and_then(|entry| entry.details)
        .and_then(|details| details.duration_secs)
        .unwrap_or(0.0);
    let texture = media_texture(ctx, ui, viewer, kind);
    let screen = ui.ctx().content_rect();
    let pan_id = egui::Id::new(("mediaa2-viewer-pan", viewer.path.clone()));
    let speed_id = egui::Id::new(("mediaa2-viewer-speed-open", viewer.path.clone()));
    let mut pan: egui::Vec2 = ui
        .ctx()
        .data(|data| data.get_temp(pan_id))
        .unwrap_or_default();
    let mut speed_open: bool = ui
        .ctx()
        .data(|data| data.get_temp(speed_id))
        .unwrap_or(false);
    let mut close = ui
        .ctx()
        .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let mut open_file = false;

    egui::Area::new(egui::Id::new("mediaa2-viewer"))
        .order(egui::Order::Foreground)
        .movable(false)
        .fade_in(false)
        .fixed_pos(screen.min)
        .show(ui.ctx(), |ui| {
            ui.set_min_size(screen.size());
            ui.painter()
                .rect_filled(screen, CornerRadius::ZERO, Color32::BLACK);
            let backdrop = ui.interact(screen, ui.id().with("backdrop"), Sense::click());
            let top = Rect::from_min_max(screen.min, egui::pos2(screen.max.x, screen.min.y + 52.0));
            let controls_height = 64.0;
            let bottom = Rect::from_min_max(
                egui::pos2(screen.min.x, screen.max.y - controls_height - 12.0),
                screen.max,
            );
            let area = Rect::from_min_max(
                egui::pos2(screen.min.x + 24.0, top.max.y),
                egui::pos2(screen.max.x - 24.0, bottom.min.y - 8.0),
            );

            let (media_rect, media_open) =
                media(ui, texture, kind, area, &mut viewer.zoom, &mut pan);
            open_file |= media_open;
            if top_bar(ui, top, &name) {
                close = true;
            }
            open_file |= controls(
                ui,
                bottom,
                viewer,
                kind,
                duration,
                &mut speed_open,
                &mut pan,
            );

            if backdrop.clicked()
                && backdrop
                    .interact_pointer_pos()
                    .is_some_and(|pos| media_rect.is_none_or(|rect| !rect.contains(pos)))
            {
                close = true;
            }
        });

    ui.ctx().data_mut(|data| {
        data.insert_temp(pan_id, pan);
        data.insert_temp(speed_id, speed_open);
    });
    if open_file {
        open_path(ctx, &viewer.path);
    }
    close
}

fn media_texture(
    ctx: &mut Ctx<'_>,
    ui: &egui::Ui,
    viewer: &ViewerUi,
    kind: MediaKind,
) -> Option<(egui::TextureId, egui::Vec2)> {
    if kind == MediaKind::Audio {
        return None;
    }
    let path = if kind == MediaKind::Image {
        viewer.path.clone()
    } else {
        let ready = match ctx.view.previews.get(&viewer.path) {
            Some(PreviewState::Ready(file)) => Some(file.clone()),
            _ => None,
        };
        match ready {
            Some(file) => file,
            None => {
                request_thumbnail(ctx, ui, &viewer.path);
                return None;
            }
        }
    };
    ctx.textures
        .get_or_load(ui.ctx(), &path)
        .map(|texture| (texture.id(), texture.size_vec2()))
}

fn request_thumbnail(ctx: &mut Ctx<'_>, ui: &egui::Ui, path: &Path) {
    let key = egui::Id::new("mediaa2-viewer-preview-requests");
    let mut requested: HashSet<PathBuf> =
        ui.data_mut(|data| data.get_temp(key).unwrap_or_default());
    if requested.insert(path.to_path_buf()) {
        ctx.actions.send(ClientCommand::Preview {
            path: path.to_path_buf(),
            kind: PreviewKind::Thumbnail,
        });
    }
    ui.data_mut(|data| data.insert_temp(key, requested));
}

fn media(
    ui: &mut egui::Ui,
    texture: Option<(egui::TextureId, egui::Vec2)>,
    kind: MediaKind,
    area: Rect,
    zoom: &mut f64,
    pan: &mut egui::Vec2,
) -> (Option<Rect>, bool) {
    let p = theme::palette(ui);
    if kind == MediaKind::Audio {
        let rect = Rect::from_center_size(
            area.center() + egui::vec2(0.0, -30.0),
            egui::vec2(220.0, 220.0),
        );
        ui.painter()
            .circle_filled(rect.center(), 110.0, p.accent_soft);
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            icon::MUSIC_NOTES,
            FontId::proportional(96.0),
            p.accent,
        );
        let clicked = ui
            .interact(rect, ui.id().with("audio-art"), Sense::click())
            .clicked();
        note(ui, area.center() + egui::vec2(0.0, 150.0));
        return (Some(rect), clicked);
    }
    let Some((texture_id, texture_size)) = texture else {
        let center = area.center();
        let glyph = if kind == MediaKind::Image {
            icon::IMAGE
        } else {
            media_icon(kind)
        };
        ui.painter().text(
            center + egui::vec2(0.0, -24.0),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(56.0),
            NOTE_COLOR,
        );
        let text = if kind == MediaKind::Image {
            "Image preview unavailable"
        } else {
            PLAYBACK_NOTE
        };
        ui.painter().text(
            center + egui::vec2(0.0, 28.0),
            Align2::CENTER_CENTER,
            text,
            FontId::proportional(14.0),
            NOTE_COLOR,
        );
        return (None, false);
    };
    let fit = (area.width() / texture_size.x.max(1.0))
        .min(area.height() / texture_size.y.max(1.0))
        .min(2.0);
    let shown = texture_size * (fit * (*zoom as f32).clamp(MIN_ZOOM as f32, MAX_ZOOM as f32));
    let rect = Rect::from_center_size(area.center() + *pan, shown);
    let response = ui.interact(
        rect.intersect(area),
        ui.id().with("media"),
        Sense::click_and_drag(),
    );
    ui.painter().with_clip_rect(area).image(
        texture_id,
        rect,
        Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    if kind == MediaKind::Image {
        if response.dragged() {
            let delta = response.drag_delta();
            pan.x += delta.x;
            pan.y += delta.y;
        }
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);
        if response.hovered() && scroll != 0.0 {
            let factor = (scroll / 200.0).exp();
            let new_zoom = (*zoom * f64::from(factor)).clamp(MIN_ZOOM, MAX_ZOOM);
            if let Some(cursor) = response.hover_pos() {
                let k = (new_zoom / *zoom) as f32;
                let offset = *pan - (cursor - area.center());
                *pan = offset * k + (cursor - area.center());
            }
            *zoom = new_zoom;
        }
        if response.double_clicked() {
            *zoom = if *zoom == 1.0 { 2.0 } else { 1.0 };
            *pan = egui::Vec2::ZERO;
        }
    } else {
        note(ui, egui::pos2(area.center().x, area.bottom() - 26.0));
    }
    (Some(rect.intersect(area)), false)
}

fn note(ui: &egui::Ui, center: egui::Pos2) {
    ui.painter().text(
        center,
        Align2::CENTER_CENTER,
        format!("{}  {PLAYBACK_NOTE}", icon::PLAY),
        FontId::proportional(13.0),
        NOTE_COLOR,
    );
}

fn media_icon(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => icon::IMAGE,
        MediaKind::Video => icon::FILE_VIDEO,
        MediaKind::Animation => icon::GIF,
        MediaKind::Audio => icon::MUSIC_NOTES,
    }
}

fn top_bar(ui: &mut egui::Ui, rect: Rect, name: &str) -> bool {
    let mut close = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(rect.shrink2(egui::vec2(20.0, 10.0))),
        |ui| {
            ui.horizontal_centered(|ui| {
                ui.label(RichText::new(name).size(15.0).color(Color32::WHITE));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let button = ui.add(
                        egui::Button::new(RichText::new(icon::X).size(15.0).color(Color32::WHITE))
                            .frame(false),
                    );
                    if button.on_hover_text(t!("viewer.close")).clicked() {
                        close = true;
                    }
                });
            });
        },
    );
    close
}

fn controls(
    ui: &mut egui::Ui,
    rect: Rect,
    viewer: &mut ViewerUi,
    kind: MediaKind,
    duration: f64,
    speed_open: &mut bool,
    pan: &mut egui::Vec2,
) -> bool {
    let p = theme::palette(ui);
    let width = rect.width().min(980.0) - 32.0;
    let bar = Rect::from_center_size(rect.center(), egui::vec2(width, rect.height() - 12.0));
    ui.painter().rect(
        bar,
        CornerRadius::same(12),
        p.card,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    let mut open = false;
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(bar.shrink2(egui::vec2(14.0, 6.0))),
        |ui| {
            ui.horizontal_centered(|ui| {
                if kind == MediaKind::Image {
                    open |= image_controls(ui, viewer, pan);
                } else {
                    open |= media_controls(ui, viewer, duration, speed_open);
                }
            });
        },
    );
    open
}

fn image_controls(ui: &mut egui::Ui, viewer: &mut ViewerUi, pan: &mut egui::Vec2) -> bool {
    if theme::icon_button(ui, icon::MAGNIFYING_GLASS_MINUS, &t!("viewer.zoom_out")).clicked() {
        viewer.zoom = (viewer.zoom / 1.25).max(MIN_ZOOM);
    }
    let mut open = false;
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        open = open_button(ui);
        ui.add_space(8.0);
        if theme::icon_button(ui, icon::ARROWS_IN, &t!("viewer.fit")).clicked() {
            viewer.zoom = 1.0;
            *pan = egui::Vec2::ZERO;
        }
        let p = theme::palette(ui);
        let percent = RichText::new(format!("{:>4.0}%", viewer.zoom * 100.0))
            .color(p.text)
            .font(FontId::monospace(12.0));
        ui.add_sized([48.0, 24.0], egui::Label::new(percent));
        if theme::icon_button(ui, icon::MAGNIFYING_GLASS_PLUS, &t!("viewer.zoom_in")).clicked() {
            viewer.zoom = (viewer.zoom * 1.25).min(MAX_ZOOM);
        }
        let width = ui.available_width();
        let (response, frac) = track_slider(ui, width, zoom_to_frac(viewer.zoom), "viewer-zoom");
        if let Some(frac) = frac {
            viewer.zoom = frac_to_zoom(frac);
            if response.double_clicked() {
                viewer.zoom = 1.0;
                *pan = egui::Vec2::ZERO;
            }
        }
    });
    open
}

fn media_controls(
    ui: &mut egui::Ui,
    viewer: &mut ViewerUi,
    duration: f64,
    speed_open: &mut bool,
) -> bool {
    let mut open = false;
    if theme::icon_button(ui, icon::PLAY, &t!("viewer.open_file")).clicked() {
        open = true;
    }
    let shown = if duration > 0.0 && viewer.position >= duration - 0.05 {
        duration
    } else {
        viewer.position.floor()
    };
    let time = format!("{} / {}", human_duration(shown), human_duration(duration));
    let p = theme::palette(ui);
    ui.label(
        RichText::new(time)
            .color(p.text)
            .font(FontId::monospace(12.0)),
    );
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if open_button(ui) {
            open = true;
        }
        ui.add_space(8.0);
        let (_, volume) = track_slider(ui, 96.0, viewer.volume, "viewer-volume");
        if let Some(volume) = volume {
            viewer.volume = volume;
        }
        let ic = if viewer.volume == 0.0 {
            icon::SPEAKER_SLASH
        } else {
            icon::SPEAKER_HIGH
        };
        if theme::icon_button(ui, ic, &t!("viewer.mute")).clicked() {
            viewer.volume = if viewer.volume == 0.0 { 1.0 } else { 0.0 };
        }
        ui.add_space(4.0);
        let speed_width = speed_button_width(ui);
        let menu_width = speed_text_width(ui) + 20.0;
        let button = speed_button(ui, &format!("{}×", viewer.speed), speed_width);
        if button.clicked() {
            *speed_open = !*speed_open;
        }
        if *speed_open {
            speed_menu(ui, &button, viewer, menu_width, speed_open);
        }
        button.on_hover_text(t!("viewer.speed"));
        let frac = if duration > 0.0 {
            (viewer.position / duration) as f32
        } else {
            0.0
        };
        let width = ui.available_width();
        ui.add_enabled_ui(false, |ui| {
            let _ = track_slider(ui, width, frac, "viewer-timeline");
        });
    });
    open
}

fn open_button(ui: &mut egui::Ui) -> bool {
    ui.add(theme::secondary(
        icon::ARROW_SQUARE_OUT,
        &t!("viewer.open_file"),
    ))
    .clicked()
}

fn speed_button(ui: &mut egui::Ui, label: &str, width: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(width, ui.spacing().interact_size.y),
        Sense::click(),
    );
    let visuals = ui.style().interact(&response);
    ui.painter().rect(
        rect,
        CornerRadius::same(6),
        visuals.bg_fill,
        visuals.bg_stroke,
        StrokeKind::Inside,
    );
    let text = format!("{label}  {}", icon::CARET_DOWN);
    ui.painter().text(
        rect.right_center() - egui::vec2(8.0, 0.0),
        Align2::RIGHT_CENTER,
        text,
        TextStyle::Button.resolve(ui.style()),
        visuals.text_color(),
    );
    response.widget_info(|| WidgetInfo::labeled(WidgetType::ComboBox, ui.is_enabled(), label));
    response
}

fn speed_text_width(ui: &egui::Ui) -> f32 {
    let font = ui
        .style()
        .text_styles
        .get(&TextStyle::Button)
        .cloned()
        .unwrap_or_else(|| FontId::proportional(14.0));
    SPEEDS
        .iter()
        .map(|speed| {
            ui.fonts_mut(|fonts| {
                fonts
                    .layout_no_wrap(format!("{speed}×"), font.clone(), Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max)
}

fn speed_button_width(ui: &egui::Ui) -> f32 {
    speed_text_width(ui) + 34.0
}

fn speed_menu(
    ui: &mut egui::Ui,
    button: &egui::Response,
    viewer: &mut ViewerUi,
    menu_width: f32,
    speed_open: &mut bool,
) {
    let mut chosen = None;
    let area = egui::Area::new(egui::Id::new("mediaa2-viewer-speed-menu"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_BOTTOM)
        .fixed_pos(button.rect.right_top() - egui::vec2(0.0, 4.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.set_width(menu_width);
                ui.spacing_mut().item_spacing.y = 2.0;
                for speed in SPEEDS {
                    let selected = viewer.speed == speed;
                    let (rect, response) =
                        ui.allocate_exact_size(egui::vec2(menu_width, 22.0), Sense::click());
                    response.widget_info(|| {
                        WidgetInfo::labeled(
                            WidgetType::Button,
                            ui.is_enabled(),
                            format!("{speed}×"),
                        )
                    });
                    let p = theme::palette(ui);
                    let (fill, color) = if selected {
                        (p.accent, p.on_accent)
                    } else if response.hovered() {
                        (p.accent_soft, p.accent)
                    } else {
                        (Color32::TRANSPARENT, p.text)
                    };
                    if ui.is_rect_visible(rect) {
                        ui.painter().rect_filled(rect, CornerRadius::same(5), fill);
                        ui.painter().text(
                            rect.right_center() - egui::vec2(8.0, 0.0),
                            Align2::RIGHT_CENTER,
                            format!("{speed}×"),
                            TextStyle::Button.resolve(ui.style()),
                            color,
                        );
                    }
                    if response.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if response.clicked() {
                        chosen = Some(speed);
                    }
                }
            });
        });
    if let Some(speed) = chosen {
        viewer.speed = speed;
        *speed_open = false;
    }
    if let Some(pointer) = ui.ctx().pointer_interact_pos() {
        if ui.input(|input| input.pointer.any_click())
            && !area.response.rect.contains(pointer)
            && !button.rect.contains(pointer)
        {
            *speed_open = false;
        }
    }
}

fn zoom_to_frac(zoom: f64) -> f32 {
    (0.5 + zoom.clamp(MIN_ZOOM, MAX_ZOOM).ln() / (2.0 * MAX_ZOOM.ln())).clamp(0.0, 1.0) as f32
}

fn frac_to_zoom(frac: f32) -> f64 {
    ((f64::from(frac) - 0.5) * 2.0 * MAX_ZOOM.ln())
        .exp()
        .clamp(MIN_ZOOM, MAX_ZOOM)
}

fn open_path(ctx: &mut Ctx<'_>, path: &Path) {
    if let Err(error) = mediaa2_platform::files::open(path) {
        ctx.actions.notice(
            NoticeLevel::Error,
            t!("notice.error").to_string(),
            format!("{error:#}"),
        );
    }
}
