//! Lightshot-style area selection in its own borderless fullscreen window (an egui deferred
//! viewport, so it works the same on every platform and the main window stays where it is).
//!
//! The window shows a frozen screenshot; the user drags a rectangle, may move/resize it, draw
//! on it (pen, line, arrow, rectangle, highlighter, text) and then copies / saves it.
//! Geometry lives in `mediaa_core::geometry`, shapes and their rasterization in
//! `mediaa_core::annotate` — this module is the interaction and painting layer.

use crate::theme;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Pos2, Rect as ERect, Sense, Stroke,
};
use egui_phosphor::regular as icon;
use mediaa_core::annotate::{self, Shape, ShapeKind, Style, Tool};
use mediaa_core::geometry::{Handle, Point, Rect, SelectionEditor};
use mediaa_core::layout::{place_panels, RectF};
use mediaa_core::settings::AfterCapture;
use mediaa_core::{Frame, ImageFormat};
use mediaa_platform::screen::MonitorInfo;
use rust_i18n::t;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// Screenshot from the app window: copy / save only.
    Screenshot,
    /// Screen area from the tray / global hotkey: copy, save or record it right away.
    Area,
    /// Pick the area to record (no drawing tools).
    RecordRegion,
}

#[derive(Debug)]
pub enum Outcome {
    Cancel,
    Image { frame: Frame, action: AfterCapture, format: Option<ImageFormat> },
    Region { monitor: MonitorInfo, rect: Rect },
    /// Area purpose only: record the selected area right away.
    RecordArea { monitor: MonitorInfo, rect: Rect },
}

/// Lightshot's palette.
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

struct TextEdit {
    shape: Shape,
    focus: bool,
}

/// Title of the overlay window (unique, so the native window can be found by it).
const WINDOW_TITLE: &str = "MediaA area picker";

struct State {
    purpose: Purpose,
    /// Every monitor with its frozen screenshot; only the starting one is captured up front,
    /// the others on demand (Tab), so the picker opens as fast as possible.
    shots: Vec<(MonitorInfo, Option<Frame>)>,
    current: usize,
    texture: Option<(usize, egui::TextureHandle)>,
    /// Frames drawn in the (still hidden) window; it is revealed once its content is ready.
    frames_drawn: u32,
    shown: bool,
    editor: SelectionEditor,
    tool: Option<Tool>,
    style: Style,
    shapes: Vec<Shape>,
    drawing: Option<Shape>,
    text: Option<TextEdit>,
    palette_open: bool,
    /// Measured panel sizes (previous frame), for collision-free placement.
    tools_size: Option<egui::Vec2>,
    actions_size: Option<egui::Vec2>,
    default_action: AfterCapture,
    opened: Instant,
    outcome: Option<Outcome>,
}

/// Owned by the app; the overlay window itself renders from a shared copy of the state.
pub struct Overlay {
    state: Arc<Mutex<State>>,
}

impl Overlay {
    /// `first` is the frozen screenshot of `monitors[start]`. The texture is uploaded right
    /// away, before the window exists, so its very first frame already shows the picture.
    pub fn new(
        ctx: &egui::Context,
        purpose: Purpose,
        monitors: Vec<MonitorInfo>,
        start: usize,
        first: Frame,
        default_action: AfterCapture,
    ) -> Self {
        register_fonts(ctx);
        let current = start.min(monitors.len().saturating_sub(1));
        let mut editor = SelectionEditor::new(Rect::new(0, 0, first.width as i32, first.height as i32));
        if std::env::var_os("MEDIAA_DEV_PICKER").is_some() {
            editor.select_all(); // dev: start with a selection, to check the toolbars
        }
        let texture = Some((current, upload(ctx, &first)));
        let mut shots: Vec<(MonitorInfo, Option<Frame>)> = monitors.into_iter().map(|m| (m, None)).collect();
        shots[current].1 = Some(first);
        Self {
            state: Arc::new(Mutex::new(State {
                purpose,
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

    /// Registers the overlay window for this frame (call every frame from the main window's
    /// `ui`) and returns the outcome once the user is done.
    pub fn show(&self, ctx: &egui::Context) -> Option<Outcome> {
        let (id, builder, focus) = {
            let mut s = self.state.lock().expect("overlay lock");
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
                // Created hidden and revealed by `State::ui` once the frozen screen is painted:
                // no empty or half-sized window is ever visible.
                .with_visible(false)
                // A plain window covering the monitor (logical units); switching to borderless
                // fullscreen at creation time breaks swapchain creation on some GPUs.
                .with_position([monitor.x as f32 / monitor.scale, monitor.y as f32 / monitor.scale])
                .with_inner_size([monitor.width as f32 / monitor.scale, monitor.height as f32 / monitor.scale])
                .with_always_on_top()
                .with_taskbar(false)
                .with_active(true);
            (id, builder, s.shown && s.opened.elapsed() < Duration::from_millis(600))
        };
        if focus {
            // Window managers may open it behind the active window; insist for a moment.
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::WindowLevel(egui::WindowLevel::AlwaysOnTop));
            ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus);
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let state = self.state.clone();
        ctx.show_viewport_deferred(id, builder, move |ui, _class| {
            let mut s = state.lock().expect("overlay lock");
            s.ui(ui);
            if s.outcome.is_some() {
                ui.ctx().request_repaint_of(egui::ViewportId::ROOT);
            }
        });
        None
    }
}

fn upload(ctx: &egui::Context, frame: &Frame) -> egui::TextureHandle {
    let img = egui::ColorImage::from_rgba_unmultiplied([frame.width as usize, frame.height as usize], &frame.data);
    ctx.load_texture("overlay-shot", img, egui::TextureOptions::LINEAR)
}

/// Maps between screen points of the overlay window and image pixels of the screenshot.
#[derive(Clone, Copy)]
struct Mapping {
    origin: Pos2,
    scale: f32,
}

impl Mapping {
    fn to_px(self, p: Pos2) -> (f32, f32) {
        ((p.x - self.origin.x) * self.scale, (p.y - self.origin.y) * self.scale)
    }
    fn to_point(self, p: Pos2) -> Point {
        let (x, y) = self.to_px(p);
        Point::new(x.round() as i32, y.round() as i32)
    }
    fn to_pt(self, p: (f32, f32)) -> Pos2 {
        Pos2::new(self.origin.x + p.0 / self.scale, self.origin.y + p.1 / self.scale)
    }
    fn rect(self, r: Rect) -> ERect {
        ERect::from_min_max(self.to_pt((r.x as f32, r.y as f32)), self.to_pt((r.right() as f32, r.bottom() as f32)))
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
        self.shots[self.current].1.as_ref().expect("current monitor is always captured")
    }

    /// Moves to the next display, capturing it now if it has not been yet.
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
            // A new window is created for the other display: reveal it the same way.
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

    /// The selected area with annotations burnt in.
    fn result(&mut self) -> Option<Frame> {
        self.commit_text();
        let sel = self.editor.usable_selection()?;
        Some(annotate::render_crop(self.frame(), sel, &self.shapes))
    }

    fn finish_image(&mut self, action: AfterCapture, format: Option<ImageFormat>) {
        if let Some(frame) = self.result() {
            self.outcome = Some(Outcome::Image { frame, action, format });
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
        let map = Mapping { origin: screen.min, scale: fw as f32 / screen.width().max(1.0) };
        let painter = ui.painter().clone();
        painter.image(tex.id(), screen, ERect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);

        self.interact(ui, map, screen);
        self.paint(&painter, map, screen);
        self.text_editor(ui, map);
        self.keyboard(&ctx);
        if self.outcome.is_none() {
            self.toolbars(&ctx, map, screen);
        }
        self.reveal(&ctx);
    }

    /// Shows the window once two frames with the frozen screen have been painted (the first
    /// one may still be sized before the window settles), with the OS open animation off.
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
        // Keep the picker itself out of recordings that may be running
        // (`MEDIAA_DEV_PICKER=1` keeps it capturable, for UI checks).
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
            // Selection mode: draw / move / resize the rectangle.
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
                    ui.ctx().set_cursor_icon(match self.editor.hit_test(map.to_point(hover)) {
                        Some(Handle::Move) => CursorIcon::Move,
                        Some(Handle::N | Handle::S) => CursorIcon::ResizeVertical,
                        Some(Handle::E | Handle::W) => CursorIcon::ResizeHorizontal,
                        Some(Handle::NW | Handle::SE) => CursorIcon::ResizeNwSe,
                        Some(Handle::NE | Handle::SW) => CursorIcon::ResizeNeSw,
                        None => CursorIcon::Crosshair,
                    });
                }
            }
            // Text: click inside the selection to start typing.
            Some(Tool::Text) => {
                // Right click: font and size.
                resp.context_menu(|ui| self.text_menu(ui));
                if resp.hover_pos().is_some_and(inside) {
                    ui.ctx().set_cursor_icon(CursorIcon::Text);
                }
                if resp.clicked() {
                    if let Some(p) = resp.interact_pointer_pos().filter(|p| inside(*p)) {
                        self.commit_text();
                        self.text = Some(TextEdit { shape: Shape::begin(Tool::Text, map.to_px(p), self.style), focus: true });
                    }
                }
            }
            // Drawing tools.
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
                    if let (Some(shape), Some(p)) = (self.drawing.as_mut(), resp.interact_pointer_pos()) {
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
                        self.shapes.push(Shape::begin(tool, map.to_px(p), self.style));
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
            ERect::from_min_max(Pos2::new(screen.min.x, s.min.y), Pos2::new(s.min.x, s.max.y)),
            ERect::from_min_max(Pos2::new(s.max.x, s.min.y), Pos2::new(screen.max.x, s.max.y)),
        ] {
            painter.rect_filled(r, CornerRadius::ZERO, dim);
        }

        // Annotations, clipped to the selection like in the final image.
        let clipped = painter.with_clip_rect(s);
        for shape in self.shapes.iter().chain(self.drawing.as_ref()) {
            paint_shape(&clipped, map, shape);
        }

        painter.rect_stroke(s, CornerRadius::ZERO, Stroke::new(1.5, accent), egui::StrokeKind::Outside);
        if self.tool.is_none() {
            for c in [s.left_top(), s.right_top(), s.left_bottom(), s.right_bottom(), s.center_top(), s.center_bottom(), s.left_center(), s.right_center()] {
                let h = ERect::from_center_size(c, egui::vec2(8.0, 8.0));
                painter.rect_filled(h, CornerRadius::same(2), Color32::WHITE);
                painter.rect_stroke(h, CornerRadius::same(2), Stroke::new(1.0, accent), egui::StrokeKind::Inside);
            }
        }
        let galley = painter.layout_no_wrap(format!("{} × {}", sel.w, sel.h), FontId::proportional(13.0), Color32::WHITE);
        let pos = Pos2::new(s.min.x, (s.min.y - galley.size().y - 10.0).max(screen.min.y + 4.0));
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
        let bg = ERect::from_min_size(pos - egui::vec2(18.0, 12.0), galley.size() + egui::vec2(36.0, 24.0));
        painter.rect_filled(bg, CornerRadius::same(12), Color32::from_black_alpha(200));
        painter.galley(pos, galley, Color32::WHITE);
    }

    /// In-place text editor for the text tool (committed on Enter or when clicking elsewhere).
    fn text_editor(&mut self, ui: &mut egui::Ui, map: Mapping) {
        let Some(edit) = self.text.as_mut() else { return };
        let ShapeKind::Text { pos, text } = &mut edit.shape.kind else { return };
        let font = text_font(ui.ctx(), &edit.shape.style, map.len(edit.shape.style.font_size));
        let color = c32(edit.shape.style.color);
        let mut commit = false;
        egui::Area::new(egui::Id::new("overlay-text"))
            .order(egui::Order::Foreground)
            .fixed_pos(map.to_pt(*pos))
            .show(ui.ctx(), |ui| {
                let te = egui::TextEdit::singleline(text)
                    .font(font)
                    .text_color(color)
                    .frame(egui::Frame::NONE)
                    .background_color(Color32::from_black_alpha(40))
                    .desired_width(f32::INFINITY.min(600.0))
                    .hint_text(t!("overlay.text_hint"));
                let resp = ui.add(te);
                if edit.focus {
                    resp.request_focus();
                    edit.focus = false;
                }
                if resp.lost_focus() {
                    commit = true;
                }
            });
        if commit {
            self.commit_text();
        }
    }

    fn keyboard(&mut self, ctx: &egui::Context) {
        use egui::Key;
        if self.text.is_some() {
            // The text field owns the keyboard; Esc drops the text being typed.
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.text = None;
            }
            return;
        }
        let (esc, enter, copy, save, undo, tab, all, arrows, shift) = ctx.input(|i| {
            let k = |key: Key| i.key_pressed(key);
            (
                k(Key::Escape),
                k(Key::Enter),
                i.modifiers.command && k(Key::C),
                i.modifiers.command && k(Key::S),
                i.modifiers.command && k(Key::Z),
                k(Key::Tab),
                i.modifiers.command && k(Key::A),
                (k(Key::ArrowRight) as i32 - k(Key::ArrowLeft) as i32, k(Key::ArrowDown) as i32 - k(Key::ArrowUp) as i32),
                i.modifiers.shift,
            )
        });
        if esc {
            if self.tool.is_some() {
                self.tool = None;
            } else {
                self.outcome = Some(Outcome::Cancel);
            }
            return;
        }
        if undo {
            self.shapes.pop();
        }
        if tab {
            self.switch_monitor();
            return;
        }
        if self.tool.is_none() {
            if all {
                self.editor.select_all();
            }
            if arrows != (0, 0) {
                self.editor.nudge(arrows.0, arrows.1, shift);
            }
        }
        let Some(sel) = self.editor.usable_selection() else { return };
        if self.purpose == Purpose::RecordRegion {
            if enter {
                self.outcome = Some(Outcome::Region { monitor: self.monitor(), rect: sel });
            }
            return;
        }
        if copy {
            self.finish_image(AfterCapture::Copy, None);
        } else if save {
            self.finish_image(AfterCapture::Save, None);
        } else if enter {
            self.finish_image(self.default_action, None);
        }
    }

    fn toolbars(&mut self, ctx: &egui::Context, map: Mapping, screen: ERect) {
        let Some(sel) = self.editor.usable_selection() else { return };
        if self.editor.is_dragging() || self.drawing.is_some() {
            return;
        }
        let s = map.rect(sel);
        let to_f = |r: ERect| RectF::new(r.min.x, r.min.y, r.width(), r.height());
        // Panel sizes from the previous frame (estimates before the first one).
        let tools = (self.purpose != Purpose::RecordRegion)
            .then(|| self.tools_size.unwrap_or(egui::vec2(46.0, 420.0)))
            .map(|v| (v.x, v.y));
        let actions = self.actions_size.unwrap_or(egui::vec2(560.0, 44.0));
        let layout = place_panels(to_f(screen), to_f(s), tools, (actions.x, actions.y));
        let at = |r: RectF| Pos2::new(r.x, r.y);
        if let Some(t) = layout.tools {
            let size = dark_bar(ctx, "overlay-tools", at(t), |ui| self.tool_buttons(ui));
            self.remember_size(ctx, true, size);
        }
        let size = dark_bar(ctx, "overlay-actions", at(layout.actions), |ui| self.action_buttons(ui));
        self.remember_size(ctx, false, size);
    }

    /// Stores a panel size; if it changed, repaint so the layout uses the real size at once.
    fn remember_size(&mut self, ctx: &egui::Context, tools: bool, size: egui::Vec2) {
        let slot = if tools { &mut self.tools_size } else { &mut self.actions_size };
        if slot.is_none_or(|old| (old - size).length() > 0.5) {
            *slot = Some(size);
            ctx.request_repaint();
        }
    }

    fn action_buttons(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let btn = |ui: &mut egui::Ui, ic: &str, label: &str| ui.add(egui::Button::new(format!("{ic}  {label}")).min_size(egui::vec2(0.0, 30.0)));
            match self.purpose {
                Purpose::RecordRegion => {
                    if ui.add(theme::primary(ui, icon::RECORD, &t!("overlay.record"))).clicked() {
                        if let Some(rect) = self.editor.usable_selection() {
                            self.outcome = Some(Outcome::Region { monitor: self.monitor(), rect });
                        }
                    }
                }
                Purpose::Screenshot | Purpose::Area => {
                    if btn(ui, icon::COPY, &t!("overlay.copy")).on_hover_text("Ctrl+C").clicked() {
                        self.finish_image(AfterCapture::Copy, None);
                    }
                    for fmt in ImageFormat::ALL {
                        if btn(ui, icon::FLOPPY_DISK, &t!("overlay.save", format = fmt.label())).clicked() {
                            self.finish_image(AfterCapture::Save, Some(fmt));
                        }
                    }
                    if self.purpose == Purpose::Area && btn(ui, icon::RECORD, &t!("overlay.record")).on_hover_text(t!("overlay.record_hint")).clicked() {
                        if let Some(rect) = self.editor.usable_selection() {
                            self.outcome = Some(Outcome::RecordArea { monitor: self.monitor(), rect });
                        }
                    }
                }
            }
            if btn(ui, icon::X, &t!("common.cancel")).on_hover_text("Esc").clicked() {
                self.outcome = Some(Outcome::Cancel);
            }
        });
    }

    /// Font and size of text annotations (right-click menu of the text tool).
    fn text_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(200.0);
        ui.label(egui::RichText::new(t!("overlay.font")).strong());
        egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
            for (i, face) in annotate::fonts().iter().enumerate() {
                let i = i as u16;
                let font = text_font(ui.ctx(), &Style { font: i, ..self.style }, 16.0);
                if ui.selectable_label(self.style.font == i, egui::RichText::new(&face.name).font(font)).clicked() {
                    self.style.font = i;
                }
            }
        });
        ui.separator();
        ui.label(egui::RichText::new(t!("overlay.font_size")).strong());
        ui.horizontal_wrapped(|ui| {
            for size in annotate::TEXT_SIZES {
                if ui.selectable_label(self.style.font_size == size, format!("{size:.0}")).clicked() {
                    self.style.font_size = size;
                }
            }
        });
        ui.add(egui::Slider::new(&mut self.style.font_size, 8.0..=200.0).suffix(" px"));
        // Applies to the text being typed as well.
        if let Some(t) = self.text.as_mut() {
            (t.shape.style.font, t.shape.style.font_size) = (self.style.font, self.style.font_size);
        }
    }

    fn tool_buttons(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let tool_btn = |ui: &mut egui::Ui, selected: bool, ic: &str, tip: String| {
                ui.add(egui::Button::new(egui::RichText::new(ic).size(17.0)).selected(selected).min_size(egui::vec2(32.0, 32.0)))
                    .on_hover_text(tip)
            };
            if tool_btn(ui, self.tool.is_none(), icon::CURSOR, t!("overlay.tool_select").into()).clicked() {
                self.commit_text();
                self.tool = None;
            }
            for (tool, ic, tip) in [
                (Tool::Pen, icon::PENCIL_SIMPLE, t!("overlay.tool_pen")),
                (Tool::Line, icon::LINE_SEGMENT, t!("overlay.tool_line")),
                (Tool::Arrow, icon::ARROW_UP_RIGHT, t!("overlay.tool_arrow")),
                (Tool::Rect, icon::RECTANGLE, t!("overlay.tool_rect")),
                (Tool::Marker, icon::HIGHLIGHTER, t!("overlay.tool_marker")),
                (Tool::Text, icon::TEXT_T, t!("overlay.tool_text")),
            ] {
                let resp = tool_btn(ui, self.tool == Some(tool), ic, tip.into());
                if tool == Tool::Text {
                    resp.context_menu(|ui| self.text_menu(ui));
                }
                if resp.clicked() {
                    self.commit_text();
                    self.tool = Some(tool);
                }
            }
            ui.separator();
            // Current colour; click to open the palette.
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), Sense::click());
            ui.painter().circle_filled(rect.center(), 10.0, c32(self.style.color));
            ui.painter().circle_stroke(rect.center(), 10.0, Stroke::new(2.0, Color32::WHITE));
            if resp.on_hover_text(t!("overlay.color")).clicked() {
                self.palette_open = !self.palette_open;
            }
            if self.palette_open {
                for c in COLORS {
                    let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 22.0), Sense::click());
                    ui.painter().circle_filled(rect.center(), 8.0, c32(c));
                    if c == self.style.color {
                        ui.painter().circle_stroke(rect.center(), 9.5, Stroke::new(1.5, Color32::WHITE));
                    }
                    if resp.clicked() {
                        self.style.color = c;
                        self.palette_open = false;
                        if let Some(t) = self.text.as_mut() {
                            t.shape.style.color = c;
                        }
                    }
                }
            }
            // Thickness: click cycles 2 → 4 → 8 → 12.
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 28.0), Sense::click());
            ui.painter().line_segment(
                [rect.left_center() + egui::vec2(7.0, 0.0), rect.right_center() - egui::vec2(7.0, 0.0)],
                Stroke::new(self.style.width.min(12.0) / 1.5, Color32::WHITE),
            );
            if resp.on_hover_text(t!("overlay.thickness", n = self.style.width)).clicked() {
                self.style.width = match self.style.width as u32 {
                    0..=2 => 4.0,
                    3..=4 => 8.0,
                    5..=8 => 12.0,
                    _ => 2.0,
                };
            }
            ui.separator();
            let can_undo = !self.shapes.is_empty();
            if ui.add_enabled(can_undo, egui::Button::new(egui::RichText::new(icon::ARROW_COUNTER_CLOCKWISE).size(17.0)).min_size(egui::vec2(32.0, 32.0)))
                .on_hover_text(t!("overlay.undo"))
                .clicked()
            {
                self.shapes.pop();
            }
        });
    }
}

/// Floating panel with fixed dark styling (readable over any screen content and theme).
fn dark_bar(ctx: &egui::Context, id: &str, pos: Pos2, add: impl FnOnce(&mut egui::Ui)) -> egui::Vec2 {
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
                .shadow(egui::epaint::Shadow { offset: [0, 4], blur: 14, spread: 0, color: Color32::from_black_alpha(120) })
                .show(ui, add);
        })
        .response
        .rect
        .size()
}

/// Paints one annotation in screen space (mirrors `annotate::render`).
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
            let marker = Stroke::new(map.len(shape.style.width * annotate::MARKER_WIDTH_FACTOR), Color32::from_rgba_unmultiplied(c[0], c[1], c[2], annotate::MARKER_ALPHA));
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
            painter.text(map.to_pt(*pos), Align2::LEFT_TOP, text, text_font(painter.ctx(), &shape.style, map.len(shape.style.font_size)), color);
        }
    }
}

/// Makes the system fonts of [`annotate::fonts`] available to egui (the two built-in ones are
/// egui's own proportional and monospace fonts). Takes effect from the next frame.
fn register_fonts(ctx: &egui::Context) {
    use egui::epaint::text::{FontInsert, FontPriority, InsertFontFamily};
    for face in annotate::fonts().iter().skip(2) {
        let mut data = egui::FontData::from_owned(face.data.as_ref().clone());
        data.index = face.index;
        ctx.add_font(FontInsert::new(
            &face.name,
            data,
            vec![InsertFontFamily { family: egui::FontFamily::Name(face.name.as_str().into()), priority: FontPriority::Highest }],
        ));
    }
}

/// egui font for a text annotation; falls back to the default font until a system font has
/// been loaded by egui.
fn text_font(ctx: &egui::Context, style: &Style, size: f32) -> FontId {
    let family = match style.font {
        annotate::FONT_SANS => egui::FontFamily::Proportional,
        annotate::FONT_MONO => egui::FontFamily::Monospace,
        i => match annotate::fonts().get(usize::from(i)) {
            Some(face) => {
                let family = egui::FontFamily::Name(face.name.as_str().into());
                if ctx.fonts(|f| f.families().contains(&family)) { family } else { egui::FontFamily::Proportional }
            }
            None => egui::FontFamily::Proportional,
        },
    };
    FontId::new(size, family)
}
