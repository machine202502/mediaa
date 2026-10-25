use super::*;

pub(super) fn cover_uv(tex: egui::Vec2, target: egui::Vec2) -> Rect {
    let (ta, a) = (tex.x / tex.y.max(1.0), target.x / target.y.max(1.0));
    if ta > a {
        let w = a / ta;
        Rect::from_min_max(
            egui::pos2((1.0 - w) / 2.0, 0.0),
            egui::pos2((1.0 + w) / 2.0, 1.0),
        )
    } else {
        let h = ta / a;
        Rect::from_min_max(
            egui::pos2(0.0, (1.0 - h) / 2.0),
            egui::pos2(1.0, (1.0 + h) / 2.0),
        )
    }
}

pub(super) const TOOLBAR_ONE_LINE: f32 = 1380.0;

pub(super) fn kind_filter(app: &mut App, ui: &mut egui::Ui) {
    theme::segmented(
        ui,
        &mut app.lib.query.kind,
        &[
            (KindFilter::All, t!("lib.all").into()),
            (KindFilter::Only(MediaKind::Image), t!("lib.images").into()),
            (KindFilter::Only(MediaKind::Video), t!("lib.videos").into()),
            (
                KindFilter::Only(MediaKind::Animation),
                t!("lib.animations").into(),
            ),
            (KindFilter::Only(MediaKind::Audio), t!("lib.audio").into()),
        ],
    );
}

pub(super) fn toolbar_tools(app: &mut App, ui: &mut egui::Ui, total: usize, view_w: f32) {
    ui.add(
        egui::TextEdit::singleline(&mut app.lib.query.search)
            .hint_text(format!("{}  {}", icon::MAGNIFYING_GLASS, t!("lib.search")))
            .desired_width((view_w - 260.0).clamp(120.0, 220.0)),
    )
    .on_hover_text(t!("lib.search_help"));
    let sort = &mut app.lib.query.sort;
    egui::ComboBox::from_id_salt("lib-sort")
        .width(150.0)
        .selected_text(sort_label(*sort))
        .show_ui(ui, |ui| {
            for s in Sort::ALL {
                ui.selectable_value(sort, s, sort_label(s));
            }
        });
    if theme::icon_button(ui, icon::ARROWS_CLOCKWISE, &t!("common.refresh")).clicked() {
        app.lib.refresh(&app.settings, &mut app.tasks);
    }
    if theme::icon_button(ui, icon::FOLDER_OPEN, &t!("lib.open_folder")).clicked() {
        let dir = library_dir(&app.settings);
        let _ = std::fs::create_dir_all(&dir);
        app.open_path(ui.ctx(), &dir);
    }
    cleanup_button(app, ui, CleanupOlder::Week);
    cleanup_button(app, ui, CleanupOlder::Month);
    theme::hint(ui, t!("lib.count", n = total));
}

pub(super) fn cleanup_button(app: &mut App, ui: &mut egui::Ui, older: CleanupOlder) {
    if app.lib.cleanup_active() {
        ui.add_enabled(false, theme::secondary(icon::CALENDAR_X, &older.label()))
            .on_hover_text(t!("lib.cleanup_running"));
        return;
    }
    let plan = app.lib.cleanup_plan(older);
    let count = plan.items.len();
    let tooltip = if count == 0 {
        t!("lib.cleanup_empty")
    } else {
        t!(
            "lib.cleanup_available",
            count = count,
            size = human_size(plan.bytes())
        )
    };
    let button = ui.add_enabled(
        count > 0,
        theme::secondary(icon::CALENDAR_X, &older.label()),
    );
    if button.on_hover_text(tooltip).clicked() {
        app.lib.ask_cleanup(older);
    }
}

pub(super) fn grid_columns(avail: f32) -> (usize, f32) {
    let cols = (((avail + GAP) / (MIN_TILE + GAP)).floor() as usize).max(1);
    let tile_w = ((avail - GAP * (cols - 1) as f32) / cols as f32)
        .floor()
        .max(1.0);
    (cols, tile_w)
}

pub(super) fn tile(
    app: &mut App,
    ui: &mut egui::Ui,
    e: &LibraryEntry,
    width: f32,
    previewing: &mut bool,
) -> Option<Action> {
    let width = width - 2.0 * TILE_BORDER;
    let p = theme::palette(ui);
    let mut action = None;
    let mut media_resp = None;
    let mut name_resp: Option<egui::Response> = None;
    let tile_rect = Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(width, width * MEDIA_ASPECT + 70.0),
    );
    let data = if ui.is_rect_visible(tile_rect) {
        app.thumbs.get(e, e.details.is_none())
    } else {
        Default::default()
    };
    let info: Option<Details> = e.details.or_else(|| data.info.as_ref().map(details_of));
    if e.details.is_none() {
        if let Some(d) = info {
            app.lib.set_details(&e.path, d);
        }
    }
    let hovered = app.lib.hovered.as_ref() == Some(&e.path);
    let shown = egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(TILE_BORDER, p.border))
        .corner_radius(CornerRadius::same(10))
        .show(ui, |ui| {
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.set_max_width(width);
                ui.spacing_mut().item_spacing.y = 0.0;
                let size = egui::vec2(width, (width * MEDIA_ASPECT).round());
                let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
                media_resp = Some(resp.clone());
                let painter = ui.painter_at(rect);
                let top = CornerRadius {
                    nw: 10,
                    ne: 10,
                    sw: 0,
                    se: 0,
                };
                painter.rect_filled(rect, top, p.card_hover);

                let playable = matches!(e.kind, MediaKind::Video | MediaKind::Animation);
                let dev = mediaa_core::dev::preview_filter()
                    .is_some_and(|name| !name.is_empty() && e.name.contains(&name));
                let active = resp.hovered() || resp.dragged() || dev;
                let mut texture = data.texture.clone();
                let mut timeline = None;
                let mut playing_at = None;
                let mut loading = false;
                if playable && active && !app.lib.cleanup_active() {
                    *previewing = true;
                    let pv = app.lib.previews.play(
                        ui.ctx(),
                        &e.path,
                        info.and_then(|i| i.duration_secs),
                    );
                    let scrub = Rect::from_min_max(
                        egui::pos2(rect.left(), rect.bottom() - SCRUB_ZONE),
                        rect.max,
                    );
                    if resp.is_pointer_button_down_on() {
                        let origin = ui.input(|i| i.pointer.press_origin());
                        if let (Some(o), Some(now)) = (origin, resp.interact_pointer_pos()) {
                            if scrub.contains(o) {
                                let frac = ((now.x - rect.left() - 8.0) / (rect.width() - 16.0))
                                    .clamp(0.0, 1.0);
                                pv.seek(f64::from(frac) * pv.length());
                            }
                        }
                    }
                    if let Some(t) = pv.frame() {
                        texture = Some(t.clone());
                    }
                    loading = pv.is_loading();
                    playing_at = Some(pv.position());
                    timeline = Some((pv.position() / pv.length()) as f32);
                }

                match &texture {
                    Some(t) => {
                        let uv = cover_uv(t.size_vec2(), rect.size());
                        egui::Image::new((t.id(), rect.size()))
                            .uv(uv)
                            .corner_radius(top)
                            .paint_at(ui, rect);
                    }
                    None => {
                        let ic = match e.kind {
                            MediaKind::Image => icon::IMAGE,
                            MediaKind::Video => icon::FILE_VIDEO,
                            MediaKind::Animation => icon::GIF,
                            MediaKind::Audio => icon::MUSIC_NOTES,
                        };
                        painter.text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            ic,
                            FontId::proportional(32.0),
                            p.text_weak,
                        );
                    }
                }
                if loading {
                    let spinner = Rect::from_center_size(rect.center(), egui::vec2(28.0, 28.0));
                    egui::Spinner::new()
                        .color(Color32::WHITE)
                        .paint_at(ui, spinner);
                }

                let pad = 7.0;
                overlay_badge(
                    &painter,
                    rect.left_top() + egui::vec2(pad, pad),
                    egui::Align2::LEFT_TOP,
                    e.extension(),
                );
                overlay_badge(
                    &painter,
                    rect.right_top() + egui::vec2(-pad, pad),
                    egui::Align2::RIGHT_TOP,
                    human_size(e.size),
                );
                let bottom = rect.bottom() - pad - if timeline.is_some() { 14.0 } else { 0.0 };
                if let Some(i) = info.filter(|i| i.width > 0) {
                    overlay_badge(
                        &painter,
                        egui::pos2(rect.left() + pad, bottom),
                        egui::Align2::LEFT_BOTTOM,
                        format!("{}×{}", i.width, i.height),
                    );
                }
                if e.kind != MediaKind::Image {
                    let mut text = String::from(icon::PLAY);
                    if let Some(d) = playing_at
                        .map(f64::floor)
                        .or_else(|| info.and_then(|i| i.duration_secs))
                    {
                        text = format!("{text} {}", human_duration(d));
                    }
                    if info.is_some_and(|i| i.audio) {
                        text = format!("{text}  {}", icon::SPEAKER_HIGH);
                    }
                    overlay_badge(
                        &painter,
                        egui::pos2(rect.right() - pad, bottom),
                        egui::Align2::RIGHT_BOTTOM,
                        text,
                    );
                }
                if let Some(frac) = timeline {
                    let y = rect.bottom() - 9.0;
                    let (l, r) = (rect.left() + 8.0, rect.right() - 8.0);
                    let track = Rect::from_min_max(egui::pos2(l, y - 2.0), egui::pos2(r, y + 2.0));
                    painter.rect_filled(
                        track,
                        CornerRadius::same(2),
                        Color32::from_white_alpha(90),
                    );
                    let x = l + (r - l) * frac;
                    painter.rect_filled(
                        Rect::from_min_max(track.min, egui::pos2(x, track.max.y)),
                        CornerRadius::same(2),
                        p.accent,
                    );
                    painter.circle_filled(egui::pos2(x, y), 6.0, Color32::WHITE);
                    painter.circle_stroke(egui::pos2(x, y), 6.0, Stroke::new(1.5, p.accent));
                }
                if resp.double_clicked() && !app.lib.cleanup_active() {
                    action = Some(Action::View(e.path.clone(), e.kind, info));
                }

                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(STRIP_PAD, 6))
                    .show(ui, |ui| {
                        let inner = width - 2.0 * f32::from(STRIP_PAD);
                        ui.set_width(inner);
                        ui.set_max_width(inner);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        let label = if hovered {
                            RichText::new(&e.name).size(12.5).color(p.text_weak)
                        } else {
                            RichText::new(&e.title).size(12.5)
                        };
                        name_resp =
                            Some(ui.add(egui::Label::new(label).truncate().sense(Sense::click())));
                        if !app.lib.cleanup_active() {
                            if let Some(a) = tools_strip(ui, &tools(e)) {
                                action = Some(a);
                            }
                        }
                    });
            })
        });
    if let Some(target) = media_resp
        .into_iter()
        .chain(name_resp)
        .reduce(|a, b| a.union(b))
    {
        if !app.lib.cleanup_active() {
            if let Some(a) = tools_menu(egui::Popup::context_menu(&target), &tools(e)) {
                action = Some(a);
            }
        }
    }
    if shown.response.contains_pointer() {
        app.lib.hover_next = Some(e.path.clone());
    }
    action
}
