use super::*;

impl State {
    pub(super) fn text_editor(&mut self, ui: &mut egui::Ui, map: Mapping) {
        let Some(edit) = self.text.as_mut() else {
            return;
        };
        let ShapeKind::Text { pos, text } = &mut edit.shape.kind else {
            return;
        };
        let font = text_font(
            ui.ctx(),
            &edit.shape.style,
            map.len(edit.shape.style.font_size),
        );
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

    pub(super) fn keyboard(&mut self, ctx: &egui::Context) {
        use egui::Key;
        if self.text.is_some() {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.text = None;
            }
            return;
        }
        let (esc, enter, copy, save, undo, tab, all, arrows, shift) = ctx.input_mut(|i| {
            let esc = i.key_pressed(Key::Escape);
            let enter = i.key_pressed(Key::Enter);
            let copy = consume_copy_shortcut(i);
            let save = i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                Key::S,
            ));
            let undo = i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                Key::Z,
            ));
            let tab = i.key_pressed(Key::Tab);
            let all = i.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                Key::A,
            ));
            let arrows = (
                i.key_pressed(Key::ArrowRight) as i32 - i.key_pressed(Key::ArrowLeft) as i32,
                i.key_pressed(Key::ArrowDown) as i32 - i.key_pressed(Key::ArrowUp) as i32,
            );
            (
                esc,
                enter,
                copy,
                save,
                undo,
                tab,
                all,
                arrows,
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
        let Some(sel) = self.editor.usable_selection() else {
            return;
        };
        if self.purpose == Purpose::RecordRegion {
            if copy {
                self.finish_image(AfterCapture::Copy, None);
            } else if enter {
                self.outcome = Some(Outcome::SelectedRegion {
                    monitor: self.monitor(),
                    rect: sel,
                });
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

    pub(super) fn toolbars(&mut self, ctx: &egui::Context, map: Mapping, screen: ERect) {
        let Some(sel) = self.editor.usable_selection() else {
            return;
        };
        if self.editor.is_dragging() || self.drawing.is_some() {
            return;
        }
        let s = map.rect(sel);
        let to_f = |r: ERect| RectF::new(r.min.x, r.min.y, r.width(), r.height());
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
        let size = dark_bar(ctx, "overlay-actions", at(layout.actions), |ui| {
            self.action_buttons(ui)
        });
        self.remember_size(ctx, false, size);
    }

    fn remember_size(&mut self, ctx: &egui::Context, tools: bool, size: egui::Vec2) {
        let slot = if tools {
            &mut self.tools_size
        } else {
            &mut self.actions_size
        };
        if slot.is_none_or(|old| (old - size).length() > 0.5) {
            *slot = Some(size);
            ctx.request_repaint();
        }
    }

    fn action_buttons(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let btn = |ui: &mut egui::Ui, ic: &str, label: &str| {
                ui.add(egui::Button::new(format!("{ic}  {label}")).min_size(egui::vec2(0.0, 30.0)))
            };
            match self.purpose {
                Purpose::RecordRegion => {
                    if ui
                        .add(theme::primary(ui, icon::SELECTION, &t!("overlay.choose")))
                        .on_hover_text(t!("overlay.choose_hint"))
                        .clicked()
                    {
                        if let Some(rect) = self.editor.usable_selection() {
                            self.outcome = Some(Outcome::SelectedRegion {
                                monitor: self.monitor(),
                                rect,
                            });
                        }
                    }
                }
                Purpose::Screenshot | Purpose::Area => {
                    if btn(ui, icon::COPY, &t!("overlay.copy"))
                        .on_hover_text("Ctrl+C")
                        .clicked()
                    {
                        self.finish_image(AfterCapture::Copy, None);
                    }
                    for fmt in ImageFormat::ALL {
                        if btn(
                            ui,
                            icon::FLOPPY_DISK,
                            &t!("overlay.save", format = fmt.label()),
                        )
                        .clicked()
                        {
                            self.finish_image(AfterCapture::Save, Some(fmt));
                        }
                    }
                    if self.purpose == Purpose::Area
                        && btn(ui, icon::RECORD, &t!("overlay.record"))
                            .on_hover_text(t!("overlay.record_hint"))
                            .clicked()
                    {
                        if let Some(rect) = self.editor.usable_selection() {
                            self.outcome = Some(Outcome::RecordArea {
                                monitor: self.monitor(),
                                rect,
                            });
                        }
                    }
                }
            }
            if btn(ui, icon::X, &t!("common.cancel"))
                .on_hover_text("Esc")
                .clicked()
            {
                self.outcome = Some(Outcome::Cancel);
            }
        });
    }

    pub(super) fn text_menu(&mut self, ui: &mut egui::Ui) {
        ui.set_min_width(200.0);
        ui.label(egui::RichText::new(t!("overlay.font")).strong());
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for (i, face) in annotate::fonts().iter().enumerate() {
                    let i = i as u16;
                    let font = text_font(
                        ui.ctx(),
                        &Style {
                            font: i,
                            ..self.style
                        },
                        16.0,
                    );
                    if ui
                        .selectable_label(
                            self.style.font == i,
                            egui::RichText::new(&face.name).font(font),
                        )
                        .clicked()
                    {
                        self.style.font = i;
                    }
                }
            });
        ui.separator();
        ui.label(egui::RichText::new(t!("overlay.font_size")).strong());
        ui.horizontal_wrapped(|ui| {
            for size in annotate::TEXT_SIZES {
                if ui
                    .selectable_label(self.style.font_size == size, format!("{size:.0}"))
                    .clicked()
                {
                    self.style.font_size = size;
                }
            }
        });
        ui.add(egui::Slider::new(&mut self.style.font_size, 8.0..=200.0).suffix(" px"));
        if let Some(t) = self.text.as_mut() {
            (t.shape.style.font, t.shape.style.font_size) = (self.style.font, self.style.font_size);
        }
    }

    fn tool_buttons(&mut self, ui: &mut egui::Ui) {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let tool_btn = |ui: &mut egui::Ui, selected: bool, ic: &str, tip: String| {
                ui.add(
                    egui::Button::new(egui::RichText::new(ic).size(17.0))
                        .selected(selected)
                        .min_size(egui::vec2(32.0, 32.0)),
                )
                .on_hover_text(tip)
            };
            if tool_btn(
                ui,
                self.tool.is_none(),
                icon::CURSOR,
                t!("overlay.tool_select").into(),
            )
            .clicked()
            {
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
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 32.0), Sense::click());
            ui.painter()
                .circle_filled(rect.center(), 10.0, c32(self.style.color));
            ui.painter()
                .circle_stroke(rect.center(), 10.0, Stroke::new(2.0, Color32::WHITE));
            if resp.on_hover_text(t!("overlay.color")).clicked() {
                self.palette_open = !self.palette_open;
            }
            if self.palette_open {
                for c in COLORS {
                    let (rect, resp) =
                        ui.allocate_exact_size(egui::vec2(32.0, 22.0), Sense::click());
                    ui.painter().circle_filled(rect.center(), 8.0, c32(c));
                    if c == self.style.color {
                        ui.painter().circle_stroke(
                            rect.center(),
                            9.5,
                            Stroke::new(1.5, Color32::WHITE),
                        );
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
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(32.0, 28.0), Sense::click());
            ui.painter().line_segment(
                [
                    rect.left_center() + egui::vec2(7.0, 0.0),
                    rect.right_center() - egui::vec2(7.0, 0.0),
                ],
                Stroke::new(self.style.width.min(12.0) / 1.5, Color32::WHITE),
            );
            if resp
                .on_hover_text(t!("overlay.thickness", n = self.style.width))
                .clicked()
            {
                self.style.width = match self.style.width as u32 {
                    0..=2 => 4.0,
                    3..=4 => 8.0,
                    5..=8 => 12.0,
                    _ => 2.0,
                };
            }
            ui.separator();
            let can_undo = !self.shapes.is_empty();
            if ui
                .add_enabled(
                    can_undo,
                    egui::Button::new(
                        egui::RichText::new(icon::ARROW_COUNTER_CLOCKWISE).size(17.0),
                    )
                    .min_size(egui::vec2(32.0, 32.0)),
                )
                .on_hover_text(t!("overlay.undo"))
                .clicked()
            {
                self.shapes.pop();
            }
        });
    }
}
