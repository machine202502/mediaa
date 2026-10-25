use crate::theme;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect as ERect, Sense, Stroke,
};
use egui_phosphor::regular as icon;
use mediaa_core::geometry::{Handle, Point, Rect, SelectionEditor};
use mediaa_core::settings::AfterCapture;
use mediaa_core::{Frame, ImageFormat};
use mediaa_platform::screen::MonitorInfo;
use mediaa_render::annotate::{self, Shape, ShapeKind, Style, Tool};
use mediaa_render::layout::{place_panels, RectF};
use parking_lot::Mutex;
use rust_i18n::t;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Screenshot,
    Area,
    RecordRegion,
}

#[derive(Debug)]
pub enum Outcome {
    Cancel,
    Image {
        frame: Frame,
        action: AfterCapture,
        format: Option<ImageFormat>,
        after_completion: bool,
    },
    SelectedRegion {
        monitor: MonitorInfo,
        rect: Rect,
    },
    RecordArea {
        monitor: MonitorInfo,
        rect: Rect,
    },
}

const COLORS: [[u8; 4]; 8] = [
    [255, 59, 48, 255],
    [255, 149, 0, 255],
    [255, 214, 10, 255],
    [52, 199, 89, 255],
    [10, 132, 255, 255],
    [175, 82, 222, 255],
    [255, 255, 255, 255],
    [0, 0, 0, 255],
];

fn c32(c: [u8; 4]) -> Color32 {
    Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3])
}

fn consume_copy_shortcut(input: &mut egui::InputState) -> bool {
    let shortcut = input.consume_shortcut(&egui::KeyboardShortcut::new(
        egui::Modifiers::COMMAND,
        egui::Key::C,
    ));
    let mut semantic = false;
    input.events.retain(|event| {
        let copy = matches!(event, egui::Event::Copy);
        semantic |= copy;
        !copy
    });
    shortcut || semantic
}

struct TextEdit {
    shape: Shape,
    focus: bool,
}

const WINDOW_TITLE: &str = "MediaA area picker";

struct State {
    purpose: Purpose,
    after_completion: bool,
    shots: Vec<(MonitorInfo, Option<Frame>)>,
    current: usize,
    texture: Option<(usize, egui::TextureHandle)>,
    frames_drawn: u32,
    shown: bool,
    editor: SelectionEditor,
    tool: Option<Tool>,
    style: Style,
    shapes: Vec<Shape>,
    drawing: Option<Shape>,
    text: Option<TextEdit>,
    palette_open: bool,
    tools_size: Option<egui::Vec2>,
    actions_size: Option<egui::Vec2>,
    default_action: AfterCapture,
    opened: Instant,
    outcome: Option<Outcome>,
}

pub struct Overlay {
    state: Arc<Mutex<State>>,
}

impl Overlay {
    pub fn new(
        ctx: &egui::Context,
        purpose: Purpose,
        monitors: Vec<MonitorInfo>,
        start: usize,
        first: Frame,
        default_action: AfterCapture,
        after_completion: bool,
    ) -> Self {
        register_fonts(ctx);
        let current = start.min(monitors.len().saturating_sub(1));
        let mut editor =
            SelectionEditor::new(Rect::new(0, 0, first.width as i32, first.height as i32));
        if std::env::var_os("MEDIAA_DEV_PICKER").is_some() {
            editor.select_all();
        }
        let texture = Some((current, upload(ctx, &first)));
        let mut shots: Vec<(MonitorInfo, Option<Frame>)> =
            monitors.into_iter().map(|m| (m, None)).collect();
        shots[current].1 = Some(first);
        Self {
            state: Arc::new(Mutex::new(State {
                purpose,
                after_completion,
                shots,
                current,
                texture,
                frames_drawn: 0,
                shown: false,
                editor,
                tool: None,
                style: Style::new(COLORS[0], 4.0),
                shapes: Vec::new(),
                drawing: None,
                text: None,
                palette_open: false,
                tools_size: None,
                actions_size: None,
                default_action,
                opened: Instant::now(),
                outcome: None,
            })),
        }
    }

    pub fn show(&self, ctx: &egui::Context) -> Option<Outcome> {
        let (id, builder, focus) = {
            let mut s = self.state.lock();
            if let Some(outcome) = s.outcome.take() {
                return Some(outcome);
            }
            let Some((monitor, _)) = s.shots.get(s.current) else {
                return Some(Outcome::Cancel);
            };
            let id = egui::ViewportId::from_hash_of(("mediaa-overlay", s.current, monitor.id));
            let builder = egui::ViewportBuilder::default()
                .with_title(WINDOW_TITLE)
                .with_decorations(false)
                .with_visible(false)
                .with_position([
                    monitor.x as f32 / monitor.scale,
                    monitor.y as f32 / monitor.scale,
                ])
                .with_inner_size([
                    monitor.width as f32 / monitor.scale,
                    monitor.height as f32 / monitor.scale,
                ])
                .with_always_on_top()
                .with_taskbar(false)
                .with_active(true);
            (
                id,
                builder,
                s.shown && s.opened.elapsed() < Duration::from_millis(600),
            )
        };
        if focus {
            ctx.send_viewport_cmd_to(
                id,
                egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop),
            );
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let state = self.state.clone();
        ctx.show_viewport_deferred(id, builder, move |ui, _class| {
            let mut s = state.lock();
            s.ui(ui);
            if s.outcome.is_some() {
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        None
    }
}

fn upload(ctx: &egui::Context, frame: &Frame) -> egui::TextureHandle {
    let img = egui::ColorImage::from_rgba_unmultiplied(
        [frame.width as usize, frame.height as usize],
        &frame.data,
    );
    ctx.load_texture("overlay-shot", img, egui::TextureOptions::LINEAR)
}

#[derive(Clone, Copy)]
struct Mapping {
    origin: Pos2,
    scale: f32,
}

impl Mapping {
    fn to_px(self, p: Pos2) -> (f32, f32) {
        (
            (p.x - self.origin.x) * self.scale,
            (p.y - self.origin.y) * self.scale,
        )
    }
    fn to_point(self, p: Pos2) -> Point {
        let (x, y) = self.to_px(p);
        Point::new(x.round() as i32, y.round() as i32)
    }
    fn to_pt(self, p: (f32, f32)) -> Pos2 {
        Pos2::new(
            self.origin.x + p.0 / self.scale,
            self.origin.y + p.1 / self.scale,
        )
    }
    fn rect(self, r: Rect) -> ERect {
        ERect::from_min_max(
            self.to_pt((r.x as f32, r.y as f32)),
            self.to_pt((r.right() as f32, r.bottom() as f32)),
        )
    }
    fn len(self, px: f32) -> f32 {
        px / self.scale
    }
}

impl State {
    fn monitor(&self) -> MonitorInfo {
        self.shots[self.current].0.clone()
    }

    fn frame(&self) -> &Frame {
        self.shots[self.current]
            .1
            .as_ref()
            .expect("current monitor is always captured")
    }

    fn switch_monitor(&mut self) {
        let n = self.shots.len();
        for step in 1..n {
            let next = (self.current + step) % n;
            if self.shots[next].1.is_none() {
                match mediaa_platform::screen::capture_monitor(self.shots[next].0.id) {
                    Ok(f) => self.shots[next].1 = Some(f),
                    Err(e) => {
                        log::warn!("capture {}: {e:#}", self.shots[next].0.name);
                        continue;
                    }
                }
            }
            self.current = next;
            let f = self.frame();
            self.editor = SelectionEditor::new(Rect::new(0, 0, f.width as i32, f.height as i32));
            self.shapes.clear();
            self.drawing = None;
            self.text = None;
            self.frames_drawn = 0;
            self.shown = false;
            self.opened = Instant::now();
            return;
        }
    }

    fn commit_text(&mut self) {
        if let Some(t) = self.text.take() {
            if t.shape.is_meaningful() {
                self.shapes.push(t.shape);
            }
        }
    }

    fn result(&mut self) -> Option<Frame> {
        self.commit_text();
        let sel = self.editor.usable_selection()?;
        Some(annotate::render_crop(self.frame(), sel, &self.shapes))
    }

    fn finish_image(&mut self, action: AfterCapture, format: Option<ImageFormat>) {
        if let Some(frame) = self.result() {
            self.outcome = Some(Outcome::Image {
                frame,
                action,
                format,
                after_completion: self.after_completion,
            });
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if ctx.input(|i| i.viewport().close_requested()) {
            self.outcome = Some(Outcome::Cancel);
            return;
        }
        let fw = self.frame().width;
        let tex = match &self.texture {
            Some((idx, t)) if *idx == self.current => t.clone(),
            _ => {
                let t = upload(&ctx, self.frame());
                self.texture = Some((self.current, t.clone()));
                t
            }
        };
        let screen = ui.max_rect();
        let map = Mapping {
            origin: screen.min,
            scale: fw as f32 / screen.width().max(1.0),
        };
        let painter = ui.painter().clone();
        painter.image(
            tex.id(),
            screen,
            ERect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
            Color32::WHITE,
        );

        self.interact(ui, map, screen);
        self.paint(&painter, map, screen);
        self.text_editor(ui, map);
        self.keyboard(&ctx);
        if self.outcome.is_none() {
            self.toolbars(&ctx, map, screen);
        }
        self.reveal(&ctx);
    }

    fn reveal(&mut self, ctx: &egui::Context) {
        if self.shown {
            return;
        }
        self.frames_drawn += 1;
        if self.frames_drawn < 2 {
            ctx.request_repaint();
            return;
        }
        mediaa_platform::window::disable_open_animation(WINDOW_TITLE);
        if std::env::var_os("MEDIAA_DEV_PICKER").is_none() {
            mediaa_platform::window::exclude_from_capture(WINDOW_TITLE);
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        self.shown = true;
        self.opened = Instant::now();
    }

    fn interact(&mut self, ui: &mut egui::Ui, map: Mapping, screen: ERect) {
        let resp = ui.interact(screen, ui.id().with("overlay-bg"), Sense::click_and_drag());
        let sel = self.editor.usable_selection();
        let inside = |p: Pos2| sel.is_some_and(|r| r.contains(map.to_point(p)));
        let press = ui.input(|i| i.pointer.press_origin());

        match self.tool {
            None => {
                if resp.drag_started() {
                    if let Some(origin) = press {
                        self.editor.press(map.to_point(origin));
                    }
                }
                if resp.dragged() {
                    if let Some(p) = resp.interact_pointer_pos() {
                        self.editor.drag_to(map.to_point(p));
                    }
                }
                if resp.drag_stopped() {
                    self.editor.release();
                }
                if resp.double_clicked() && sel.is_none() {
                    self.editor.select_all();
                }
                if let Some(hover) = resp.hover_pos() {
                    ui.ctx()
                        .set_cursor_icon(match self.editor.hit_test(map.to_point(hover)) {
                            Some(Handle::Move) => CursorIcon::Move,
                            Some(Handle::N | Handle::S) => CursorIcon::ResizeVertical,
                            Some(Handle::E | Handle::W) => CursorIcon::ResizeHorizontal,
                            Some(Handle::NW | Handle::SE) => CursorIcon::ResizeNwSe,
                            Some(Handle::NE | Handle::SW) => CursorIcon::ResizeNeSw,
                            None => CursorIcon::Crosshair,
                        });
                }
            }
            Some(Tool::Text) => {
                resp.context_menu(|ui| self.text_menu(ui));
                if resp.hover_pos().is_some_and(inside) {
                    ui.ctx().set_cursor_icon(CursorIcon::Text);
                }
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos().filter(|p| inside(*p)) {
                        self.commit_text();
                        self.text = Some(TextEdit {
                            shape: Shape::begin(Tool::Text, map.to_px(p), self.style),
                            focus: true,
                        });
                    }
                }
            }
            Some(tool) => {
                if resp.hover_pos().is_some_and(inside) {
                    ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
                }
                if resp.drag_started() {
                    if let Some(origin) = press.filter(|p| inside(*p)) {
                        self.commit_text();
                        self.drawing = Some(Shape::begin(tool, map.to_px(origin), self.style));
                    }
                }
                if resp.dragged() {
                    if let (Some(shape), Some(p)) =
                        (self.drawing.as_mut(), resp.interact_pointer_pos())
                    {
                        shape.drag_to(map.to_px(p));
                    }
                }
                if resp.drag_stopped() {
                    if let Some(shape) = self.drawing.take().filter(Shape::is_meaningful) {
                        self.shapes.push(shape);
                    }
                }
                if resp.clicked() && matches!(tool, Tool::Pen | Tool::Marker) {
                    if let Some(p) = resp.interact_pointer_pos().filter(|p| inside(*p)) {
                        self.shapes
                            .push(Shape::begin(tool, map.to_px(p), self.style));
                    }
                }
            }
        }
    }

    fn paint(&self, painter: &egui::Painter, map: Mapping, screen: ERect) {
        let dim = Color32::from_black_alpha(130);
        let accent = theme::DARK.accent;
        let Some(sel) = self.editor.selection().filter(|r| !r.is_empty()) else {
            painter.rect_filled(screen, CornerRadius::ZERO, dim);
            self.hint(painter, screen);
            return;
        };
        let s = map.rect(sel);
        for r in [
            ERect::from_min_max(screen.min, Pos2::new(screen.max.x, s.min.y)),
            ERect::from_min_max(Pos2::new(screen.min.x, s.max.y), screen.max),
            ERect::from_min_max(
                Pos2::new(screen.min.x, s.min.y),
                Pos2::new(s.min.x, s.max.y),
            ),
            ERect::from_min_max(
                Pos2::new(s.max.x, s.min.y),
                Pos2::new(screen.max.x, s.max.y),
            ),
        ] {
            painter.rect_filled(r, CornerRadius::ZERO, dim);
        }

        let clipped = painter.with_clip_rect(s);
        for shape in self.shapes.iter().chain(self.drawing.as_ref()) {
            paint_shape(&clipped, map, shape);
        }

        painter.rect_stroke(
            s,
            CornerRadius::ZERO,
            Stroke::new(1.5, accent),
            egui::StrokeKind::Outside,
        );
        if self.tool.is_none() {
            for c in [
                s.left_top(),
                s.right_top(),
                s.left_bottom(),
                s.right_bottom(),
                s.center_top(),
                s.center_bottom(),
                s.left_center(),
                s.right_center(),
            ] {
                let h = ERect::from_center_size(c, egui::vec2(8.0, 8.0));
                painter.rect_filled(h, CornerRadius::same(2), Color32::WHITE);
                painter.rect_stroke(
                    h,
                    CornerRadius::same(2),
                    Stroke::new(1.0, accent),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let galley = painter.layout_no_wrap(
            format!("{} × {}", sel.w, sel.h),
            FontId::proportional(13.0),
            Color32::WHITE,
        );
        let pos = Pos2::new(
            s.min.x,
            (s.min.y - galley.size().y - 10.0).max(screen.min.y + 4.0),
        );
        let bg = ERect::from_min_size(pos, galley.size() + egui::vec2(12.0, 6.0));
        painter.rect_filled(bg, CornerRadius::same(6), Color32::from_black_alpha(210));
        painter.galley(pos + egui::vec2(6.0, 3.0), galley, Color32::WHITE);
    }

    fn hint(&self, painter: &egui::Painter, screen: ERect) {
        let mut text = t!("overlay.hint").to_string();
        if self.shots.len() > 1 {
            text.push_str(&t!("overlay.hint_tab"));
        }
        let galley = painter.layout_no_wrap(text, FontId::proportional(16.0), Color32::WHITE);
        let pos = screen.center() - galley.size() / 2.0;
        let bg = ERect::from_min_size(
            pos - egui::vec2(18.0, 12.0),
            galley.size() + egui::vec2(36.0, 24.0),
        );
        painter.rect_filled(bg, CornerRadius::same(12), Color32::from_black_alpha(200));
        painter.galley(pos, galley, Color32::WHITE);
    }
}

mod editor;

fn dark_bar(
    ctx: &egui::Context,
    id: &str,
    pos: Pos2,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Vec2 {
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            *ui.visuals_mut() = theme::dark_visuals();
            egui::Frame::new()
                .fill(Color32::from_rgb(22, 28, 40))
                .stroke(Stroke::new(1.0, Color32::from_rgb(60, 72, 96)))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(egui::Margin::same(6))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 14,
                    spread: 0,
                    color: Color32::from_black_alpha(120),
                })
                .show(ui, add);
        })
        .response
        .rect
        .size()
}

fn paint_shape(painter: &egui::Painter, map: Mapping, shape: &Shape) {
    let color = c32(shape.style.color);
    let stroke = Stroke::new(map.len(shape.style.width), color);
    let pts = |v: &[(f32, f32)]| v.iter().map(|p| map.to_pt(*p)).collect::<Vec<_>>();
    match &shape.kind {
        ShapeKind::Pen(v) => {
            if v.len() == 1 {
                painter.circle_filled(map.to_pt(v[0]), stroke.width / 2.0, color);
            } else {
                painter.add(egui::Shape::line(pts(v), stroke));
            }
        }
        ShapeKind::Marker(v) => {
            let c = shape.style.color;
            let marker = Stroke::new(
                map.len(shape.style.width * annotate::MARKER_WIDTH_FACTOR),
                Color32::from_rgba_unmultiplied(c[0], c[1], c[2], annotate::MARKER_ALPHA),
            );
            painter.add(egui::Shape::line(pts(v), marker));
        }
        ShapeKind::Line { from, to } => {
            painter.line_segment([map.to_pt(*from), map.to_pt(*to)], stroke);
        }
        ShapeKind::Arrow { from, to } => {
            let end = annotate::arrow_shaft_end(*from, *to, shape.style.width);
            painter.line_segment([map.to_pt(*from), map.to_pt(end)], stroke);
            let head = annotate::arrow_head(*from, *to, shape.style.width);
            painter.add(egui::Shape::convex_polygon(pts(&head), color, Stroke::NONE));
        }
        ShapeKind::Rect { a, b } => {
            let r = ERect::from_two_pos(map.to_pt(*a), map.to_pt(*b));
            painter.rect_stroke(r, CornerRadius::ZERO, stroke, egui::StrokeKind::Middle);
        }
        ShapeKind::Text { pos, text } => {
            painter.text(
                map.to_pt(*pos),
                Align2::LEFT_TOP,
                text,
                text_font(painter.ctx(), &shape.style, map.len(shape.style.font_size)),
                color,
            );
        }
    }
}

fn register_fonts(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    for face in annotate::fonts().iter().skip(2) {
        let mut data = egui::FontData::from_owned(face.data.as_ref().clone());
        data.index = face.index;
        ctx.add_font(FontInsert::new(
            &face.name,
            data,
            vec![InsertFontFamily {
                family: egui::FontFamily::Name(face.name.as_str().into()),
                priority: FontPriority::Highest,
            }],
        ));
    }
}

fn text_font(ctx: &egui::Context, style: &Style, size: f32) -> FontId {
    let family = match style.font {
        annotate::FONT_SANS => egui::FontFamily::Proportional,
        annotate::FONT_MONO => egui::FontFamily::Monospace,
        i => match annotate::fonts().get(usize::from(i)) {
            Some(face) => {
                let family = egui::FontFamily::Name(face.name.as_str().into());
                if ctx.fonts(|f| f.families().contains(&family)) {
                    family
                } else {
                    egui::FontFamily::Proportional
                }
            }
            None => egui::FontFamily::Proportional,
        },
    };
    FontId::new(size, family)
}

#[cfg(test)]
mod tests {
    use super::consume_copy_shortcut;
    use eframe::egui;

    #[test]
    fn semantic_copy_event_is_consumed_once() {
        let mut input = egui::InputState::default();
        input.events.push(egui::Event::Copy);
        assert!(consume_copy_shortcut(&mut input));
        assert!(!consume_copy_shortcut(&mut input));
    }
}
