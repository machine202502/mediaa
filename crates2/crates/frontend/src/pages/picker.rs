use super::Ctx;
use crate::ui::theme;
use eframe::egui;
use egui_phosphor::regular as icon;
use mediaa2_core::annotate::{
    arrow_head, arrow_shaft_end, Shape, ShapeKind, Style, Tool, FONT_MONO, FONT_SANS,
};
use mediaa2_core::geometry::{Handle, Point, Rect, SelectionEditor};
use mediaa2_core::settings::AfterCapture;
use mediaa2_core::ImageFormat;
use mediaa2_i18n::t;
use mediaa2_protocol::{ClientCommand, PickerCommand};

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

const TEXT_SIZES: [f32; 9] = [12.0, 16.0, 20.0, 24.0, 32.0, 40.0, 56.0, 72.0, 96.0];

const GAP: f32 = 10.0;

#[derive(Clone)]
struct EditState {
    select_mode: bool,
    undo: Vec<Vec<Shape>>,
    redo: Vec<Vec<Shape>>,
    active: Option<Shape>,
    text_pos: Option<(f32, f32)>,
    text_focus: bool,
    text_frame: Option<egui::Rect>,
    palette_open: bool,
    tools_size: Option<egui::Vec2>,
    actions_size: Option<egui::Vec2>,
    width_drag: Option<f32>,
}

impl Default for EditState {
    fn default() -> Self {
        Self {
            select_mode: true,
            undo: Vec::new(),
            redo: Vec::new(),
            active: None,
            text_pos: None,
            text_focus: false,
            text_frame: None,
            palette_open: false,
            tools_size: None,
            actions_size: None,
            width_drag: None,
        }
    }
}

#[derive(Clone, Copy)]
enum EditAction {
    Undo,
    Redo,
    Delete,
    Copy,
    Save,
    UseArea,
    Cancel,
    Monitor(u32),
}

struct Shortcuts {
    escape: bool,
    enter: bool,
    undo: bool,
    redo: bool,
    delete: bool,
    tab: bool,
    copy: bool,
}

pub fn show(ctx: &mut Ctx<'_>, ui: &mut egui::Ui) {
    let Some(mut picker) = ctx.ui.picker.clone() else {
        return;
    };
    let Some(view) = ctx.view.picker().cloned() else {
        return;
    };
    let texture = ctx.textures.get_or_load(ui.ctx(), &view.image).cloned();
    let available = ui.available_size();
    let image_size = texture
        .as_ref()
        .map(|texture| texture.size_vec2())
        .unwrap_or(available);
    let scale = (available.x / image_size.x)
        .min(available.y / image_size.y)
        .max(0.01);
    let size = image_size * scale;
    let origin = ui.min_rect().min
        + egui::vec2(
            (available.x - size.x).max(0.0) / 2.0,
            (available.y - size.y).max(0.0) / 2.0,
        );
    let image_rect = egui::Rect::from_min_size(origin, size);
    let bounds = Rect::new(0, 0, image_size.x as i32, image_size.y as i32);
    let monitor_id = view.monitor.id.get();
    let monitor_key = egui::Id::new(("picker-monitor", picker.session));
    let previous_monitor: Option<u32> = ui.data(|data| data.get_temp(monitor_key));
    if previous_monitor.is_some_and(|previous| previous != monitor_id) {
        picker.selection = None;
        picker.shapes.clear();
        picker.text = None;
    }
    ui.data_mut(|data| data.insert_temp(monitor_key, monitor_id));
    let state_id = egui::Id::new((
        "picker-editor",
        picker.session,
        monitor_id,
        image_size.x as i32,
        image_size.y as i32,
    ));
    let mut state: EditState = ui.data_mut(|data| data.get_temp(state_id).unwrap_or_default());
    let mut editor: SelectionEditor = ui.data_mut(|data| {
        data.get_temp(state_id.with("selection"))
            .unwrap_or_else(|| {
                let mut editor = SelectionEditor::new(bounds);
                editor.set_selection(picker.selection);
                editor
            })
    });
    if editor.bounds() != bounds {
        let selection = picker.selection;
        editor = SelectionEditor::new(bounds);
        editor.set_selection(selection);
    }
    if picker.selection.is_none() {
        editor.set_selection(None);
    }
    if picker.text.is_some() && state.text_pos.is_none() {
        let pos = picker
            .selection
            .map(|selection| (selection.x as f32 + 8.0, selection.y as f32 + 8.0))
            .unwrap_or((8.0, 8.0));
        state.text_pos = Some(pos);
    }

    let response = ui.allocate_rect(image_rect, egui::Sense::click_and_drag());
    let to_point = |pos: egui::Pos2| -> Point {
        Point::new(
            ((pos.x - image_rect.min.x) / scale).round() as i32,
            ((pos.y - image_rect.min.y) / scale).round() as i32,
        )
    };
    let to_pos = |p: (f32, f32)| -> egui::Pos2 {
        egui::pos2(
            image_rect.min.x + p.0 * scale,
            image_rect.min.y + p.1 * scale,
        )
    };
    let to_rect = |r: Rect| -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(
                image_rect.min.x + r.x as f32 * scale,
                image_rect.min.y + r.y as f32 * scale,
            ),
            egui::vec2(r.w as f32 * scale, r.h as f32 * scale),
        )
    };

    interaction(&response, &mut picker, &mut state, &mut editor, &to_point);

    picker.selection = editor.selection();
    let selection = editor.usable_selection();
    let selection_rect = selection.map(to_rect);

    if let Some(texture) = &texture {
        ui.painter().image(
            texture.id(),
            image_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            egui::Color32::WHITE,
        );
    } else {
        ui.painter()
            .rect_filled(image_rect, 0.0, egui::Color32::BLACK);
    }

    if let Some(rect) = selection_rect {
        dim_outside(ui, image_rect, rect);
        let painter = ui.painter_at(rect);
        draw_shapes(&painter, &picker.shapes, &to_pos, scale);
        if let Some(shape) = &state.active {
            draw_shape(&painter, shape, &to_pos, scale);
        }
        if state.select_mode && state.active.is_none() {
            draw_handles(ui, rect);
        }
        let color = theme::current_palette(ui.ctx()).accent;
        ui.painter().rect_stroke(
            rect,
            0.0,
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Outside,
        );
        if let Some(selection) = selection {
            draw_size_badge(ui, rect, image_rect, selection, state.text_frame);
        }
    } else {
        ui.painter()
            .rect_filled(image_rect, 0.0, egui::Color32::from_black_alpha(130));
        draw_shapes(ui.painter(), &picker.shapes, &to_pos, scale);
        draw_hint(ui, image_rect, ctx.monitors.len() > 1);
    }

    let text_frame = text_editor(ui, &mut picker, &mut state, &to_pos, scale);
    state.text_frame = text_frame;

    let mut action = None;
    if let Some(rect) = selection_rect
        .filter(|_| !editor.is_dragging() && state.active.is_none() && !response.dragged())
    {
        toolbars(ctx, ui, &mut picker, &mut state, rect, &mut action);
    } else if selection.is_none() {
        cancel_button(ui, image_rect, &mut action);
    }

    if picker.text.is_some() {
        if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            picker.text = None;
            state.text_pos = None;
            state.text_focus = false;
        }
    } else {
        let shortcuts = read_shortcuts(ui);
        if shortcuts.escape {
            action = Some(EditAction::Cancel);
        }
        if shortcuts.undo {
            action = Some(EditAction::Undo);
        }
        if shortcuts.redo {
            action = Some(EditAction::Redo);
        }
        if shortcuts.delete {
            action = Some(EditAction::Delete);
        }
        if shortcuts.tab {
            if let Some(index) = next_monitor(ctx, monitor_id) {
                action = Some(EditAction::Monitor(index));
            }
        }
        if shortcuts.copy && selection.is_some() {
            action = Some(if ctx.ui.record.picking_region {
                EditAction::UseArea
            } else {
                EditAction::Copy
            });
        }
        if shortcuts.enter && selection.is_some() {
            action = Some(if ctx.ui.record.picking_region {
                EditAction::UseArea
            } else {
                EditAction::Copy
            });
        }
    }

    if matches!(
        action,
        Some(EditAction::Copy | EditAction::Save | EditAction::UseArea)
    ) {
        commit_text(&mut picker, &mut state);
    }

    let closes = matches!(
        action,
        Some(EditAction::Copy | EditAction::Save | EditAction::UseArea | EditAction::Cancel)
    );
    match action {
        Some(EditAction::Undo) => {
            if let Some(previous) = state.undo.pop() {
                state.redo.push(picker.shapes.clone());
                picker.shapes = previous;
            }
        }
        Some(EditAction::Redo) => {
            if let Some(next) = state.redo.pop() {
                state.undo.push(picker.shapes.clone());
                picker.shapes = next;
            }
        }
        Some(EditAction::Delete) => {
            let previous = picker.shapes.clone();
            if picker.shapes.pop().is_some() {
                state.undo.push(previous);
                state.redo.clear();
            }
        }
        Some(EditAction::Copy) => commit(ctx, &picker, AfterCapture::CopyAndSave),
        Some(EditAction::Save) => commit(ctx, &picker, AfterCapture::Save),
        Some(EditAction::UseArea) => use_area(ctx, &picker),
        Some(EditAction::Cancel) => cancel(ctx, &picker),
        Some(EditAction::Monitor(index)) => {
            ctx.actions
                .send(ClientCommand::Picker(PickerCommand::Monitor {
                    session: picker.session,
                    index,
                }));
        }
        None => {}
    }
    if !closes {
        ctx.ui.picker = Some(picker);
    }
    ui.data_mut(|data| {
        data.insert_temp(state_id, state);
        data.insert_temp(state_id.with("selection"), editor);
    });
}

fn interaction(
    response: &egui::Response,
    picker: &mut crate::ui_state::PickerUi,
    state: &mut EditState,
    editor: &mut SelectionEditor,
    to_point: &impl Fn(egui::Pos2) -> Point,
) {
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let p = to_point(pos);
            if picker.text.is_some() {
                commit_text(picker, state);
            }
            if state.select_mode || picker.selection.is_none() {
                editor.press(p);
            } else if let Some(selection) = editor.usable_selection() {
                if selection.contains(p) && picker.tool != Tool::Text {
                    state.active = Some(Shape::begin(
                        picker.tool,
                        (p.x as f32, p.y as f32),
                        picker.style,
                    ));
                }
            }
        }
    }
    if response.dragged() {
        if let Some(pos) = response.interact_pointer_pos() {
            let p = to_point(pos);
            if let Some(shape) = state.active.as_mut() {
                shape.drag_to((p.x as f32, p.y as f32));
            } else {
                editor.drag_to(p);
            }
        }
    }
    if response.drag_stopped() {
        editor.release();
        if let Some(shape) = state.active.take() {
            if shape.is_meaningful() {
                state.undo.push(picker.shapes.clone());
                state.redo.clear();
                picker.shapes.push(shape);
            }
        }
    }
    if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let p = to_point(pos);
            if picker.text.is_some() {
                commit_text(picker, state);
            }
            if !state.select_mode {
                if picker.tool == Tool::Text {
                    start_text(picker, state, (p.x as f32, p.y as f32));
                } else if matches!(picker.tool, Tool::Pen | Tool::Marker) {
                    let inside = editor
                        .usable_selection()
                        .is_some_and(|selection| selection.contains(p));
                    if inside {
                        let shape =
                            Shape::begin(picker.tool, (p.x as f32, p.y as f32), picker.style);
                        state.undo.push(picker.shapes.clone());
                        state.redo.clear();
                        picker.shapes.push(shape);
                    }
                }
            }
        }
    }
    if response.double_clicked() && state.select_mode && editor.usable_selection().is_none() {
        editor.select_all();
    }
    if let Some(hover) = response.hover_pos() {
        let cursor = if state.select_mode {
            match editor.hit_test(to_point(hover)) {
                Some(Handle::Move) => egui::CursorIcon::Move,
                Some(Handle::N | Handle::S) => egui::CursorIcon::ResizeVertical,
                Some(Handle::E | Handle::W) => egui::CursorIcon::ResizeHorizontal,
                Some(Handle::NW | Handle::SE) => egui::CursorIcon::ResizeNwSe,
                Some(Handle::NE | Handle::SW) => egui::CursorIcon::ResizeNeSw,
                None => egui::CursorIcon::Crosshair,
            }
        } else if picker.tool == Tool::Text {
            egui::CursorIcon::Text
        } else {
            egui::CursorIcon::Crosshair
        };
        response.ctx.set_cursor_icon(cursor);
    }
    if picker.tool == Tool::Text && !state.select_mode {
        let style = &mut picker.style;
        response.context_menu(|ui| text_menu(ui, style));
    }
}

fn start_text(picker: &mut crate::ui_state::PickerUi, state: &mut EditState, pos: (f32, f32)) {
    commit_text(picker, state);
    picker.text = Some(String::new());
    state.text_pos = Some(pos);
    state.text_focus = true;
    state.select_mode = false;
}

fn commit_text(picker: &mut crate::ui_state::PickerUi, state: &mut EditState) {
    let Some(text) = picker.text.take() else {
        return;
    };
    let pos = state.text_pos.take().unwrap_or((0.0, 0.0));
    state.text_focus = false;
    let shape = Shape {
        kind: ShapeKind::Text { pos, text },
        style: picker.style,
    };
    if shape.is_meaningful() {
        state.undo.push(picker.shapes.clone());
        state.redo.clear();
        picker.shapes.push(shape);
    }
}

fn read_shortcuts(ui: &egui::Ui) -> Shortcuts {
    ui.input_mut(|input| {
        let redo = input.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
            egui::Key::Z,
        ));
        let undo = input.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND,
            egui::Key::Z,
        ));
        let copy = input.consume_shortcut(&egui::KeyboardShortcut::new(
            egui::Modifiers::COMMAND,
            egui::Key::C,
        ));
        Shortcuts {
            escape: input.key_pressed(egui::Key::Escape),
            enter: input.key_pressed(egui::Key::Enter),
            undo,
            redo,
            delete: input.key_pressed(egui::Key::Delete),
            tab: input.consume_key(egui::Modifiers::NONE, egui::Key::Tab),
            copy,
        }
    })
}

fn next_monitor(ctx: &Ctx<'_>, current: u32) -> Option<u32> {
    if ctx.monitors.len() < 2 {
        return None;
    }
    let position = ctx
        .monitors
        .iter()
        .position(|monitor| monitor.id.get() == current)
        .unwrap_or(0);
    let next = (position + 1) % ctx.monitors.len();
    Some(ctx.monitors[next].index as u32)
}

fn toolbars(
    ctx: &mut Ctx<'_>,
    ui: &mut egui::Ui,
    picker: &mut crate::ui_state::PickerUi,
    state: &mut EditState,
    selection: egui::Rect,
    action: &mut Option<EditAction>,
) {
    let picking = ctx.ui.record.picking_region;
    let screen = ui.max_rect();
    let tools = (!picking).then(|| state.tools_size.unwrap_or(egui::vec2(46.0, 430.0)));
    let actions = state
        .actions_size
        .unwrap_or(egui::vec2(if picking { 260.0 } else { 360.0 }, 44.0));
    let layout = place_panels(screen, selection, tools, actions);
    let mut tool = picker.tool;
    let mut color = picker.style.color;
    let mut width = picker.style.width;
    if let Some(rect) = layout.tools {
        let size = dark_bar(
            ui.ctx(),
            &format!("picker-tools-{}", picker.session),
            rect.min,
            |ui| {
                tools_panel(ui, picker, state, &mut tool, &mut color, &mut width, action);
            },
        );
        remember_size(ui, &mut state.tools_size, size);
    }
    let size = dark_bar(
        ui.ctx(),
        &format!("picker-actions-{}", picker.session),
        layout.actions.min,
        |ui| {
            actions_panel(ui, picking, action);
        },
    );
    remember_size(ui, &mut state.actions_size, size);
    picker.tool = tool;
    picker.style.color = color;
    picker.style.width = width;
}

fn remember_size(ui: &egui::Ui, slot: &mut Option<egui::Vec2>, size: egui::Vec2) {
    if slot.is_none_or(|old| (old - size).length() > 0.5) {
        *slot = Some(size);
        ui.ctx().request_repaint();
    }
}

fn tools_panel(
    ui: &mut egui::Ui,
    picker: &mut crate::ui_state::PickerUi,
    state: &mut EditState,
    tool: &mut Tool,
    color: &mut [u8; 4],
    width: &mut f32,
    action: &mut Option<EditAction>,
) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        let tool_button = |ui: &mut egui::Ui, selected: bool, ic: &str, tip: String| {
            ui.add(
                egui::Button::new(egui::RichText::new(ic).size(17.0))
                    .selected(selected)
                    .min_size(egui::vec2(32.0, 32.0)),
            )
            .on_hover_text(tip)
        };
        if tool_button(
            ui,
            state.select_mode,
            icon::CURSOR,
            t!("overlay.tool_select").to_string(),
        )
        .clicked()
        {
            commit_text(picker, state);
            state.select_mode = true;
        }
        for (value, ic) in [
            (Tool::Pen, icon::PENCIL_SIMPLE),
            (Tool::Line, icon::LINE_SEGMENT),
            (Tool::Arrow, icon::ARROW_UP_RIGHT),
            (Tool::Rect, icon::RECTANGLE),
            (Tool::Marker, icon::HIGHLIGHTER),
            (Tool::Text, icon::TEXT_T),
        ] {
            let selected = !state.select_mode && *tool == value;
            let response = tool_button(ui, selected, ic, tool_label(value));
            if value == Tool::Text {
                response.context_menu(|ui| text_menu(ui, &mut picker.style));
            }
            if response.clicked() {
                commit_text(picker, state);
                *tool = value;
                state.select_mode = false;
                if value == Tool::Text && picker.text.is_none() {
                    let pos = picker
                        .selection
                        .map(|selection| (selection.x as f32 + 8.0, selection.y as f32 + 8.0))
                        .unwrap_or((8.0, 8.0));
                    picker.text = Some(String::new());
                    state.text_pos = Some(pos);
                    state.text_focus = true;
                }
            }
        }
        ui.separator();
        let (rect, response) = ui.allocate_exact_size(egui::vec2(32.0, 30.0), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, t!("overlay.color"))
        });
        ui.painter()
            .circle_filled(rect.center(), 10.0, color32(*color));
        ui.painter().circle_stroke(
            rect.center(),
            10.0,
            egui::Stroke::new(2.0, egui::Color32::WHITE),
        );
        if response.on_hover_text(t!("overlay.color")).clicked() {
            state.palette_open = !state.palette_open;
        }
        if state.palette_open {
            for value in COLORS {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(32.0, 16.0), egui::Sense::click());
                ui.painter()
                    .circle_filled(rect.center(), 7.0, color32(value));
                if value == *color {
                    ui.painter().circle_stroke(
                        rect.center(),
                        8.5,
                        egui::Stroke::new(1.5, egui::Color32::WHITE),
                    );
                }
                if response.clicked() {
                    *color = value;
                    picker.style.color = value;
                    state.palette_open = false;
                }
            }
        }
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(32.0, 26.0), egui::Sense::click_and_drag());
        ui.painter().line_segment(
            [
                rect.left_center() + egui::vec2(7.0, 0.0),
                rect.right_center() - egui::vec2(7.0, 0.0),
            ],
            egui::Stroke::new((width.min(12.0) / 1.5).max(1.0), egui::Color32::WHITE),
        );
        let response = response.on_hover_text(t!("overlay.thickness", n = *width));
        if response.drag_started() {
            state.width_drag = Some(*width);
        }
        if response.dragged() {
            if let Some(start) = state.width_drag {
                *width = (start + response.drag_delta().x * 0.1).clamp(1.0, 16.0);
            }
        }
        if response.drag_stopped() {
            state.width_drag = None;
        }
        if response.clicked() {
            *width = match *width as u32 {
                0..=2 => 4.0,
                3..=4 => 8.0,
                5..=8 => 12.0,
                _ => 2.0,
            };
        }
        ui.separator();
        let undo = !state.undo.is_empty();
        if ui
            .add_enabled(
                undo,
                egui::Button::new(egui::RichText::new(icon::ARROW_COUNTER_CLOCKWISE).size(17.0))
                    .min_size(egui::vec2(32.0, 32.0)),
            )
            .on_hover_text(t!("overlay.undo"))
            .clicked()
        {
            *action = Some(EditAction::Undo);
        }
        let redo = !state.redo.is_empty();
        if ui
            .add_enabled(
                redo,
                egui::Button::new(egui::RichText::new(icon::ARROW_CLOCKWISE).size(17.0))
                    .min_size(egui::vec2(32.0, 32.0)),
            )
            .on_hover_text("Redo (Ctrl+Shift+Z)")
            .clicked()
        {
            *action = Some(EditAction::Redo);
        }
        let delete = !picker.shapes.is_empty();
        if ui
            .add_enabled(
                delete,
                egui::Button::new(egui::RichText::new(icon::TRASH).size(17.0))
                    .min_size(egui::vec2(32.0, 32.0)),
            )
            .on_hover_text("Delete (Del)")
            .clicked()
        {
            *action = Some(EditAction::Delete);
        }
    });
}

fn actions_panel(ui: &mut egui::Ui, picking: bool, action: &mut Option<EditAction>) {
    ui.horizontal(|ui| {
        let button = |ui: &mut egui::Ui, ic: &str, label: &str| {
            ui.add(egui::Button::new(format!("{ic}  {label}")).min_size(egui::vec2(0.0, 30.0)))
        };
        if picking {
            if ui
                .add(theme::primary(ui, icon::SELECTION, &t!("overlay.choose")))
                .on_hover_text(t!("overlay.choose_hint"))
                .clicked()
            {
                *action = Some(EditAction::UseArea);
            }
        } else {
            if button(ui, icon::COPY, &t!("overlay.copy"))
                .on_hover_text("Ctrl+C")
                .clicked()
            {
                *action = Some(EditAction::Copy);
            }
            if button(ui, icon::FLOPPY_DISK, &t!("overlay.save"))
                .on_hover_text(t!("overlay.save_hint"))
                .clicked()
            {
                *action = Some(EditAction::Save);
            }
        }
        if button(ui, icon::X, &t!("common.cancel"))
            .on_hover_text("Esc")
            .clicked()
        {
            *action = Some(EditAction::Cancel);
        }
    });
}

fn text_menu(ui: &mut egui::Ui, style: &mut Style) {
    ui.set_min_width(200.0);
    ui.label(egui::RichText::new(t!("overlay.font")).strong());
    egui::ScrollArea::vertical()
        .max_height(260.0)
        .show(ui, |ui| {
            for (font, name) in [(FONT_SANS, "Sans"), (FONT_MONO, "Mono")] {
                let rich = egui::RichText::new(name).font(text_font(font, 16.0));
                if ui.selectable_label(style.font == font, rich).clicked() {
                    style.font = font;
                }
            }
        });
    ui.separator();
    ui.label(egui::RichText::new(t!("overlay.font_size")).strong());
    ui.horizontal_wrapped(|ui| {
        for size in TEXT_SIZES {
            if ui
                .selectable_label(style.font_size == size, format!("{size:.0}"))
                .clicked()
            {
                style.font_size = size;
            }
        }
    });
    ui.add(egui::Slider::new(&mut style.font_size, 8.0..=200.0).suffix(" px"));
}

fn text_editor(
    ui: &mut egui::Ui,
    picker: &mut crate::ui_state::PickerUi,
    state: &mut EditState,
    to_pos: &impl Fn((f32, f32)) -> egui::Pos2,
    scale: f32,
) -> Option<egui::Rect> {
    let mut text = picker.text.clone()?;
    let mut pos = state.text_pos.unwrap_or((0.0, 0.0));
    let font = text_font(picker.style.font, (picker.style.font_size * scale).max(8.0));
    let color = color32(picker.style.color);
    let accent = theme::current_palette(ui.ctx()).accent;
    let focus = state.text_focus;
    state.text_focus = false;
    let mut commit = false;
    let area = egui::Area::new(egui::Id::new("picker-text"))
        .order(egui::Order::Foreground)
        .fixed_pos(to_pos(pos) - egui::vec2(9.5, 9.5))
        .show(ui.ctx(), |ui| {
            let enter =
                ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
            let measured = if text.is_empty() {
                t!("overlay.text_hint").to_string()
            } else {
                text.clone()
            };
            let width = ui
                .painter()
                .layout_no_wrap(measured, font.clone(), color)
                .size()
                .x
                .clamp(180.0, 600.0);
            let frame = egui::Frame::new()
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(1.5, accent))
                .corner_radius(egui::CornerRadius::same(4))
                .inner_margin(egui::Margin::same(8))
                .show(ui, |ui| {
                    let response = ui.add(
                        egui::TextEdit::multiline(&mut text)
                            .font(font.clone())
                            .text_color(color)
                            .frame(egui::Frame::NONE)
                            .desired_width(width)
                            .desired_rows(1)
                            .return_key(egui::KeyboardShortcut::new(
                                egui::Modifiers::SHIFT,
                                egui::Key::Enter,
                            ))
                            .hint_text(t!("overlay.text_hint")),
                    );
                    if focus {
                        response.request_focus();
                    }
                    response
                });
            let frame_rect = frame.response.rect;
            let text_rect = frame.inner.rect;
            let drag_rects = [
                egui::Rect::from_min_max(
                    frame_rect.min,
                    egui::pos2(frame_rect.max.x, text_rect.min.y),
                ),
                egui::Rect::from_min_max(
                    egui::pos2(frame_rect.min.x, text_rect.max.y),
                    frame_rect.max,
                ),
                egui::Rect::from_min_max(
                    egui::pos2(frame_rect.min.x, text_rect.min.y),
                    egui::pos2(text_rect.min.x, text_rect.max.y),
                ),
                egui::Rect::from_min_max(
                    egui::pos2(text_rect.max.x, text_rect.min.y),
                    egui::pos2(frame_rect.max.x, text_rect.max.y),
                ),
            ];
            let mut dragging = false;
            for (index, rect) in drag_rects.into_iter().enumerate() {
                if rect.width() <= 0.0 || rect.height() <= 0.0 {
                    continue;
                }
                let response = ui
                    .interact(
                        rect,
                        ui.id().with(("text-frame-drag", index)),
                        egui::Sense::drag(),
                    )
                    .on_hover_cursor(egui::CursorIcon::Move);
                dragging |= response.dragged();
            }
            if dragging {
                let delta = ui.input(|input| input.pointer.delta()) * scale;
                pos.0 += delta.x;
                pos.1 += delta.y;
                ui.ctx().request_repaint();
            }
            let press = ui.input(|input| input.pointer.press_origin());
            if press.is_some_and(|p| frame_rect.contains(p)) {
                frame.inner.request_focus();
            }
            if frame.inner.lost_focus()
                && !dragging
                && press.is_none_or(|p| !frame_rect.contains(p))
            {
                commit = true;
            }
            if enter {
                commit = true;
            }
            (frame_rect, text_rect)
        })
        .inner;
    let (frame_rect, _) = area;
    state.text_pos = Some(pos);
    picker.text = Some(text);
    if commit {
        commit_text(picker, state);
        state.text_frame = None;
        return None;
    }
    Some(frame_rect)
}

fn draw_handles(ui: &egui::Ui, rect: egui::Rect) {
    let accent = theme::current_palette(ui.ctx()).accent;
    for center in [
        rect.left_top(),
        rect.right_top(),
        rect.left_bottom(),
        rect.right_bottom(),
        rect.center_top(),
        rect.center_bottom(),
        rect.left_center(),
        rect.right_center(),
    ] {
        let handle = egui::Rect::from_center_size(center, egui::vec2(8.0, 8.0));
        ui.painter()
            .rect_filled(handle, egui::CornerRadius::same(2), egui::Color32::WHITE);
        ui.painter().rect_stroke(
            handle,
            egui::CornerRadius::same(2),
            egui::Stroke::new(1.0, accent),
            egui::StrokeKind::Inside,
        );
    }
}

fn draw_size_badge(
    ui: &egui::Ui,
    selection: egui::Rect,
    screen: egui::Rect,
    value: Rect,
    avoid: Option<egui::Rect>,
) {
    let galley = ui.painter().layout_no_wrap(
        format!("{} × {}", value.w, value.h),
        egui::FontId::proportional(13.0),
        egui::Color32::WHITE,
    );
    let size = galley.size() + egui::vec2(12.0, 6.0);
    let badge = place_size_badge(selection, screen, size, avoid);
    ui.painter().rect_filled(
        badge,
        egui::CornerRadius::same(6),
        egui::Color32::from_black_alpha(210),
    );
    ui.painter().galley(
        badge.min + egui::vec2(6.0, 3.0),
        galley,
        egui::Color32::WHITE,
    );
}

fn place_size_badge(
    selection: egui::Rect,
    screen: egui::Rect,
    size: egui::Vec2,
    avoid: Option<egui::Rect>,
) -> egui::Rect {
    let gap = 10.0;
    let inset = 4.0;
    let positions = [
        egui::pos2(selection.left(), selection.top() - size.y - gap),
        egui::pos2(selection.right() - size.x, selection.top() - size.y - gap),
        egui::pos2(selection.left(), selection.bottom() + gap),
        egui::pos2(selection.right() - size.x, selection.bottom() + gap),
        selection.left_top() + egui::vec2(inset, inset),
        egui::pos2(selection.right() - size.x - inset, selection.top() + inset),
        egui::pos2(
            selection.left() + inset,
            selection.bottom() - size.y - inset,
        ),
        egui::pos2(
            selection.right() - size.x - inset,
            selection.bottom() - size.y - inset,
        ),
    ];
    positions
        .into_iter()
        .enumerate()
        .map(|(priority, pos)| {
            let pos = egui::pos2(
                pos.x.clamp(screen.left(), screen.right() - size.x),
                pos.y.clamp(screen.top(), screen.bottom() - size.y),
            );
            (priority, egui::Rect::from_min_size(pos, size))
        })
        .min_by(|(priority_a, a), (priority_b, b)| {
            let overlap = |rect: egui::Rect| {
                avoid.map_or(0.0, |avoid| {
                    let w =
                        (rect.right().min(avoid.right()) - rect.left().max(avoid.left())).max(0.0);
                    let h =
                        (rect.bottom().min(avoid.bottom()) - rect.top().max(avoid.top())).max(0.0);
                    w * h
                })
            };
            overlap(*a)
                .total_cmp(&overlap(*b))
                .then_with(|| priority_a.cmp(priority_b))
        })
        .map(|(_, badge)| badge)
        .expect("badge positions are never empty")
}

fn draw_hint(ui: &egui::Ui, image: egui::Rect, multiple_monitors: bool) {
    let mut text = t!("overlay.hint").to_string();
    if multiple_monitors {
        text.push_str(&t!("overlay.hint_tab"));
    }
    let galley =
        ui.painter()
            .layout_no_wrap(text, egui::FontId::proportional(16.0), egui::Color32::WHITE);
    let pos = image.center() - galley.size() / 2.0;
    let background = egui::Rect::from_min_size(
        pos - egui::vec2(18.0, 12.0),
        galley.size() + egui::vec2(36.0, 24.0),
    );
    ui.painter().rect_filled(
        background,
        egui::CornerRadius::same(12),
        egui::Color32::from_black_alpha(200),
    );
    ui.painter().galley(pos, galley, egui::Color32::WHITE);
}

fn cancel_button(ui: &egui::Ui, image: egui::Rect, action: &mut Option<EditAction>) {
    egui::Area::new(egui::Id::new("picker-cancel"))
        .fixed_pos(image.center() + egui::vec2(-52.0, 30.0))
        .order(egui::Order::Foreground)
        .show(ui.ctx(), |ui| {
            if ui
                .add(
                    egui::Button::new(format!("{}  {}", icon::X, t!("common.cancel")))
                        .min_size(egui::vec2(104.0, 30.0)),
                )
                .on_hover_text("Esc")
                .clicked()
            {
                *action = Some(EditAction::Cancel);
            }
        });
}

fn dim_outside(ui: &egui::Ui, image: egui::Rect, selection: egui::Rect) {
    let color = egui::Color32::from_black_alpha(130);
    for rect in [
        egui::Rect::from_min_max(image.min, egui::pos2(image.max.x, selection.min.y)),
        egui::Rect::from_min_max(egui::pos2(image.min.x, selection.max.y), image.max),
        egui::Rect::from_min_max(
            egui::pos2(image.min.x, selection.min.y),
            egui::pos2(selection.min.x, selection.max.y),
        ),
        egui::Rect::from_min_max(
            egui::pos2(selection.max.x, selection.min.y),
            egui::pos2(image.max.x, selection.max.y),
        ),
    ] {
        if rect.width() > 0.0 && rect.height() > 0.0 {
            ui.painter().rect_filled(rect, 0.0, color);
        }
    }
}

fn draw_shapes(
    painter: &egui::Painter,
    shapes: &[Shape],
    to_pos: &impl Fn((f32, f32)) -> egui::Pos2,
    scale: f32,
) {
    for shape in shapes {
        draw_shape(painter, shape, to_pos, scale);
    }
}

fn draw_shape(
    painter: &egui::Painter,
    shape: &Shape,
    to_pos: &impl Fn((f32, f32)) -> egui::Pos2,
    scale: f32,
) {
    let color = color32(shape.style.color);
    let width = (shape.style.width * scale).max(1.0);
    match &shape.kind {
        ShapeKind::Pen(points) | ShapeKind::Marker(points) => {
            let points: Vec<egui::Pos2> = points.iter().map(|p| to_pos(*p)).collect();
            let marker = matches!(shape.kind, ShapeKind::Marker(_));
            painter.add(egui::Shape::line(
                points,
                egui::Stroke::new(if marker { width * 2.5 } else { width }, color),
            ));
        }
        ShapeKind::Line { from, to } => {
            painter.line_segment(
                [to_pos(*from), to_pos(*to)],
                egui::Stroke::new(width, color),
            );
        }
        ShapeKind::Arrow { from, to } => {
            let end = arrow_shaft_end(*from, *to, shape.style.width);
            painter.line_segment(
                [to_pos(*from), to_pos(end)],
                egui::Stroke::new(width, color),
            );
            let [tip, left, right] = arrow_head(*from, *to, shape.style.width);
            painter.add(egui::Shape::convex_polygon(
                vec![to_pos(tip), to_pos(left), to_pos(right)],
                color,
                egui::Stroke::NONE,
            ));
        }
        ShapeKind::Rect { a, b } => {
            let rect = egui::Rect::from_two_pos(to_pos(*a), to_pos(*b));
            painter.rect_stroke(
                rect,
                0.0,
                egui::Stroke::new(width, color),
                egui::StrokeKind::Inside,
            );
        }
        ShapeKind::Text { pos, text } => {
            painter.text(
                to_pos(*pos),
                egui::Align2::LEFT_TOP,
                text,
                text_font(shape.style.font, (shape.style.font_size * scale).max(8.0)),
                color,
            );
        }
    }
}

fn text_font(font: u16, size: f32) -> egui::FontId {
    let family = if font == FONT_MONO {
        egui::FontFamily::Monospace
    } else {
        egui::FontFamily::Proportional
    };
    egui::FontId::new(size, family)
}

fn tool_label(tool: Tool) -> String {
    match tool {
        Tool::Pen => t!("overlay.tool_pen").to_string(),
        Tool::Line => t!("overlay.tool_line").to_string(),
        Tool::Arrow => t!("overlay.tool_arrow").to_string(),
        Tool::Rect => t!("overlay.tool_rect").to_string(),
        Tool::Marker => t!("overlay.tool_marker").to_string(),
        Tool::Text => t!("overlay.tool_text").to_string(),
    }
}

fn color32(color: [u8; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(color[0], color[1], color[2], color[3])
}

fn dark_bar(
    ctx: &egui::Context,
    id: &str,
    pos: egui::Pos2,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Vec2 {
    let palette = theme::current_palette(ctx);
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Foreground)
        .fixed_pos(pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(palette.card)
                .stroke(egui::Stroke::new(1.0, palette.border))
                .corner_radius(egui::CornerRadius::same(10))
                .inner_margin(egui::Margin::same(6))
                .shadow(egui::epaint::Shadow {
                    offset: [0, 4],
                    blur: 14,
                    spread: 0,
                    color: egui::Color32::from_black_alpha(120),
                })
                .show(ui, add);
        })
        .response
        .rect
        .size()
}

fn clamp_into(rect: egui::Rect, screen: egui::Rect) -> egui::Rect {
    let max_x = (screen.right() - GAP - rect.width()).max(screen.left() + GAP);
    let max_y = (screen.bottom() - GAP - rect.height()).max(screen.top() + GAP);
    egui::Rect::from_min_size(
        egui::pos2(
            rect.min.x.clamp(screen.left() + GAP, max_x),
            rect.min.y.clamp(screen.top() + GAP, max_y),
        ),
        rect.size(),
    )
}

fn place_panels(
    screen: egui::Rect,
    selection: egui::Rect,
    tools: Option<egui::Vec2>,
    actions: egui::Vec2,
) -> PanelLayout {
    let fits_right = |width: f32| selection.right() + GAP + width <= screen.right() - GAP;
    let fits_left = |width: f32| selection.left() - GAP - width >= screen.left() + GAP;
    let fits_below = selection.bottom() + GAP + actions.y <= screen.bottom() - GAP;
    let fits_above = selection.top() - GAP - actions.y >= screen.top() + GAP;

    let tool_candidates: Vec<Option<egui::Rect>> = match tools {
        None => vec![None],
        Some(size) => {
            let mut candidates = Vec::new();
            if fits_right(size.x) {
                candidates.push(egui::Rect::from_min_size(
                    egui::pos2(selection.right() + GAP, selection.top()),
                    size,
                ));
            }
            if fits_left(size.x) {
                candidates.push(egui::Rect::from_min_size(
                    egui::pos2(selection.left() - GAP - size.x, selection.top()),
                    size,
                ));
            }
            candidates.push(egui::Rect::from_min_size(
                egui::pos2(selection.right() - GAP - size.x, selection.top() + GAP),
                size,
            ));
            candidates.push(egui::Rect::from_min_size(
                egui::pos2(selection.left() + GAP, selection.top() + GAP),
                size,
            ));
            candidates
                .into_iter()
                .map(|rect| Some(clamp_into(rect, screen)))
                .collect()
        }
    };

    let mut rows = Vec::new();
    if fits_below {
        rows.push(selection.bottom() + GAP);
    }
    if fits_above {
        rows.push(selection.top() - GAP - actions.y);
    }
    rows.push(selection.bottom() - GAP - actions.y);
    rows.push(selection.top() + GAP);

    for tools_rect in &tool_candidates {
        let tools_on_right = tools_rect
            .is_some_and(|tools| tools.left() >= selection.left() + selection.width() / 2.0);
        let mut xs = if tools_on_right {
            vec![selection.right() - actions.x, selection.left()]
        } else {
            vec![selection.left(), selection.right() - actions.x]
        };
        let mut ys = rows.clone();
        if let Some(tools) = tools_rect {
            xs.extend([tools.right() + GAP, tools.left() - GAP - actions.x]);
            ys.extend([tools.bottom() + GAP, tools.top() - GAP - actions.y]);
        }
        for &y in &ys {
            for &x in &xs {
                let bar = clamp_into(egui::Rect::from_min_size(egui::pos2(x, y), actions), screen);
                if tools_rect.is_none_or(|tools| !tools.intersects(bar)) {
                    return PanelLayout {
                        tools: *tools_rect,
                        actions: bar,
                    };
                }
            }
        }
    }

    let tools_rect = tool_candidates[0];
    let y = tools_rect.map_or(selection.bottom() + GAP, |tools| tools.bottom() + GAP);
    PanelLayout {
        tools: tools_rect,
        actions: clamp_into(
            egui::Rect::from_min_size(egui::pos2(selection.left(), y), actions),
            screen,
        ),
    }
}

struct PanelLayout {
    tools: Option<egui::Rect>,
    actions: egui::Rect,
}

fn commit(ctx: &mut Ctx<'_>, picker: &crate::ui_state::PickerUi, action: AfterCapture) {
    let Some(selection) = picker.selection else {
        return;
    };
    ctx.actions
        .send(ClientCommand::Picker(PickerCommand::Commit {
            session: picker.session,
            selection,
            shapes: picker.shapes.clone(),
            format: ImageFormat::Png,
            action,
            output: None,
        }));
    ctx.ui.close_picker();
}

fn use_area(ctx: &mut Ctx<'_>, picker: &crate::ui_state::PickerUi) {
    let Some(selection) = picker.selection else {
        return;
    };
    let Some(view) = ctx.view.picker() else {
        return;
    };
    ctx.ui.record.region = Some((view.monitor.id.get(), selection));
    ctx.ui.record.picking_region = false;
    ctx.actions
        .send(ClientCommand::Picker(PickerCommand::Cancel {
            session: picker.session,
        }));
    ctx.ui.close_picker();
}

fn cancel(ctx: &mut Ctx<'_>, picker: &crate::ui_state::PickerUi) {
    ctx.actions
        .send(ClientCommand::Picker(PickerCommand::Cancel {
            session: picker.session,
        }));
    ctx.ui.close_picker();
}
