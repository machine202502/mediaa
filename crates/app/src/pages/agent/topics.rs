use super::mentions::{self, Roster};
use crate::features::library::player::Player;
use crate::theme::Palette;
use eframe::egui::{
    self, Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, StrokeKind,
};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::outline::{Chapter, Part, Section, SectionKind};
use rust_i18n::t;

pub const SEGMENT_GAP: f32 = 2.0;
const TRACK: f32 = 6.0;
pub const TRACK_INSET: f32 = 6.0;
pub const KNOB: f32 = 7.0;
const UNPLAYED: f32 = 0.45;

fn open_id() -> egui::Id {
    egui::Id::new("studio-topics-open")
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub chapter: usize,
    pub x0: f32,
    pub x1: f32,
}

pub fn current_topic(chapters: &[Chapter], time: f64) -> Option<usize> {
    if chapters.is_empty() {
        return None;
    }
    Some(
        chapters
            .partition_point(|c| c.start <= time)
            .saturating_sub(1),
    )
}

pub fn segments(left: f32, width: f32, duration: f64, chapters: &[Chapter]) -> Vec<Segment> {
    if duration <= 0.0 || width <= 0.0 || chapters.is_empty() {
        return Vec::new();
    }
    let x = |time: f64| left + width * (time / duration).clamp(0.0, 1.0) as f32;
    let last = chapters.len() - 1;
    chapters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut x0 = if i == 0 {
                left
            } else {
                x(c.start) + SEGMENT_GAP / 2.0
            };
            let mut x1 = if i == last {
                left + width
            } else {
                x(c.end) - SEGMENT_GAP / 2.0
            };
            x0 = x0.max(left);
            x1 = x1.min(left + width);
            Segment {
                chapter: i,
                x0,
                x1: x1.max(x0),
            }
        })
        .collect()
}

pub fn part_at(c: &Chapter, time: f64) -> Option<usize> {
    let parts = &c.digest.parts;
    let last = parts.last()?;
    if time < parts[0].start || time > last.end {
        return None;
    }
    Some(parts.partition_point(|p| p.start <= time).saturating_sub(1))
}

pub fn part_ticks(left: f32, width: f32, duration: f64, parts: &[Part]) -> Vec<f32> {
    if duration <= 0.0 || width <= 0.0 || parts.len() < 2 {
        return Vec::new();
    }
    let right = left + width;
    parts
        .iter()
        .skip(1)
        .map(|p| left + width * (p.start / duration).clamp(0.0, 1.0) as f32)
        .filter(|&x| x > left && x < right)
        .collect()
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

pub fn tooltip(c: &Chapter, part: Option<&Part>) -> String {
    let part = part
        .map(|p| p.title.trim())
        .filter(|title| !title.is_empty());
    match part {
        Some(title) => format!("{} — {title}", c.digest.title.trim()),
        None => c.digest.title.trim().to_string(),
    }
}

pub fn tooltip_x(pointer_x: f32, width: f32, content: Rect) -> f32 {
    let max_left = (content.right() - width).max(content.left());
    (pointer_x - width / 2.0).clamp(content.left(), max_left)
}

pub fn current_label(chapters: &[Chapter], time: f64) -> String {
    let Some(i) = current_topic(chapters, time) else {
        return String::new();
    };
    let c = &chapters[i];
    let title = c.digest.title.trim();
    match part_at(c, time)
        .map(|p| c.digest.parts[p].title.trim())
        .filter(|t| !t.is_empty())
    {
        Some(part) => format!("{title} — {part}"),
        None => title.to_string(),
    }
}

pub fn hover_text(c: &Chapter) -> String {
    let mut out = c.digest.summary.trim().to_string();
    let titles: Vec<&str> = c
        .digest
        .parts
        .iter()
        .map(|p| p.title.trim())
        .filter(|t| !t.is_empty())
        .collect();
    if !titles.is_empty() {
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(
            &titles
                .iter()
                .map(|t| format!("•  {t}"))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if out.is_empty() {
        out = c.digest.title.trim().to_string();
    }
    out
}

pub fn tooltip_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::popup(style).shadow(egui::epaint::Shadow::NONE)
}

pub const HEADER_H: f32 = 28.0;
pub const HEADER_ICON_X: f32 = 12.0;
pub const HEADER_TEXT_X: f32 = 34.0;
pub const HEADER_COUNT_X: f32 = 30.0;
pub const HEADER_CARET_X: f32 = 14.0;
const HEADER_TEXT_RIGHT: f32 = 74.0;
const CARET_HIT: f32 = 18.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Header {
    pub icon: egui::Pos2,
    pub text: Rect,
    pub count: egui::Pos2,
    pub caret: Rect,
}

pub fn header(rect: Rect) -> Header {
    let y = rect.center().y;
    Header {
        icon: egui::pos2(rect.left() + HEADER_ICON_X, y),
        text: Rect::from_min_max(
            egui::pos2(rect.left() + HEADER_TEXT_X, rect.top()),
            egui::pos2(
                (rect.right() - HEADER_TEXT_RIGHT).max(rect.left() + HEADER_TEXT_X),
                rect.bottom(),
            ),
        ),
        count: egui::pos2(rect.right() - HEADER_COUNT_X, y),
        caret: Rect::from_center_size(
            egui::pos2(rect.right() - HEADER_CARET_X, y),
            egui::vec2(CARET_HIT, CARET_HIT),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn seek_bar(
    ui: &mut egui::Ui,
    width: f32,
    frac: f32,
    duration: f64,
    chapters: &[Chapter],
    id: &str,
    roster: &Roster,
    p: Palette,
) -> (egui::Response, Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(40.0), 24.0), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    let track = Rect::from_center_size(
        rect.center(),
        egui::vec2(rect.width() - 2.0 * TRACK_INSET, TRACK),
    );
    let x = track.left() + KNOB + (track.width() - 2.0 * KNOB) * frac.clamp(0.0, 1.0);
    let segments = segments(track.left(), track.width(), duration, chapters);
    let hovered = resp
        .hover_pos()
        .and_then(|pt| segments.iter().position(|s| (s.x0..=s.x1).contains(&pt.x)));
    let hovered_part = resp.hover_pos().zip(hovered).and_then(|(pt, i)| {
        let time = f64::from((pt.x - track.left()) / track.width()) * duration;
        part_at(&chapters[i], time).map(|p| &chapters[i].digest.parts[p])
    });
    {
        let painter = ui.painter();
        painter.rect_filled(track, CornerRadius::same(3), p.border);
        for (i, seg) in segments.iter().enumerate() {
            let r = Rect::from_min_max(
                egui::pos2(seg.x0, track.min.y),
                egui::pos2(seg.x1, track.max.y),
            );
            if r.width() < 0.5 {
                continue;
            }
            painter.rect_filled(
                r,
                CornerRadius::same(2),
                if hovered == Some(i) {
                    p.accent
                } else {
                    mix(p.accent_soft, p.accent, UNPLAYED)
                },
            );
            if hovered != Some(i) && x > seg.x0 {
                let played = Rect::from_min_max(r.min, egui::pos2(x.min(seg.x1), r.max.y));
                if played.width() > 0.5 {
                    painter.rect_filled(played, CornerRadius::same(2), p.accent);
                }
            }
            for at in part_ticks(
                track.left(),
                track.width(),
                duration,
                &chapters[i].digest.parts,
            ) {
                if at <= seg.x0 || at >= seg.x1 {
                    continue;
                }
                let tick = Rect::from_min_size(
                    egui::pos2(at.floor(), track.min.y),
                    egui::vec2(1.0, track.height()),
                );
                painter.rect_filled(tick, CornerRadius::ZERO, p.text_weak);
            }
        }
        if segments.is_empty() {
            painter.rect_filled(
                Rect::from_min_max(track.min, egui::pos2(x, track.max.y)),
                CornerRadius::same(2),
                p.accent,
            );
        }
        let knob = if resp.hovered() || resp.dragged() {
            7.0
        } else {
            6.0
        };
        painter.circle(
            egui::pos2(x, track.center().y),
            knob,
            Color32::WHITE,
            Stroke::new(1.5, p.accent),
        );
    }
    if resp.hovered() || hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if let (Some(i), Some(pt)) = (hovered, resp.hover_pos()) {
        let text = tooltip(&chapters[i], hovered_part);
        let font = FontId::proportional(12.0);
        let shown = roster.render(&text);
        let text_width = ui
            .fonts_mut(|f| f.layout_no_wrap(shown, font.clone(), p.text))
            .size()
            .x;
        let margin = ui.style().spacing.menu_margin;
        let width = text_width + f32::from(margin.left) + f32::from(margin.right);
        let left = tooltip_x(pt.x, width, ui.ctx().content_rect());
        let mut tip = egui::Tooltip::always_open(
            ui.ctx().clone(),
            ui.layer_id(),
            ui.id().with((id, "topic-tip")),
            egui::PopupAnchor::Position(egui::pos2(left, track.min.y)),
        );
        tip.popup = tip
            .popup
            .align(egui::RectAlign::TOP_START)
            .width(width)
            .frame(tooltip_frame(ui.style()));
        tip.show(|ui| {
            let galley = mentions::galley(ui, &text, roster, font, p.text, width, false, false, p);
            ui.add(egui::Label::new(galley).wrap_mode(egui::TextWrapMode::Extend));
        });
    }
    let value = resp
        .interact_pointer_pos()
        .filter(|_| resp.clicked() || resp.dragged())
        .map(|pt| ((pt.x - track.left() - KNOB) / (track.width() - 2.0 * KNOB)).clamp(0.0, 1.0));
    (resp, value)
}

#[allow(clippy::too_many_arguments)]
pub fn block(
    ui: &mut egui::Ui,
    chapters: &[Chapter],
    position: f64,
    max_height: f32,
    reading: &mut Option<usize>,
    spin: bool,
    roster: &Roster,
    p: Palette,
) -> Option<f64> {
    if chapters.is_empty() {
        return None;
    }
    let mut open = ui
        .ctx()
        .data_mut(|d| d.get_persisted::<bool>(open_id()))
        .unwrap_or(false);
    let current = current_topic(chapters, position).unwrap_or(0);
    let clicked = if open {
        expanded(
            ui,
            chapters,
            current,
            max_height.max(96.0),
            p,
            &mut open,
            reading,
            spin,
            roster,
        )
    } else {
        folded(ui, chapters, current, p, &mut open, spin, roster)
    };
    if ui.ctx().data_mut(|d| d.get_persisted::<bool>(open_id())) != Some(open) {
        ui.ctx().data_mut(|d| d.insert_persisted(open_id(), open));
    }
    clicked
}

fn folded(
    ui: &mut egui::Ui,
    chapters: &[Chapter],
    current: usize,
    p: Palette,
    open: &mut bool,
    spin: bool,
    roster: &Roster,
) -> Option<f64> {
    let (rect, resp) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), HEADER_H), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    hover_tip(&resp, &chapters[current], roster);
    if resp.clicked() {
        *open = true;
    }
    let head = header(rect);
    let painter = ui.painter_at(rect);
    painter.rect(
        rect,
        CornerRadius::same(8),
        if resp.hovered() { p.card_hover } else { p.card },
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    header_icon(ui, head.icon, spin, p);
    let c = &chapters[current];
    let text = format!("{}  {}", human_duration(c.start), c.digest.title);
    let galley = elided(
        ui,
        &text,
        head.text.width(),
        FontId::proportional(13.0),
        p.text,
        roster,
        p,
    );
    painter.galley(
        egui::pos2(head.text.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        p.text,
    );
    painter.text(
        head.count,
        Align2::RIGHT_CENTER,
        format!("({}/{})", current + 1, chapters.len()),
        FontId::monospace(11.0),
        p.text_weak,
    );
    painter.text(
        head.caret.center(),
        Align2::CENTER_CENTER,
        icon::CARET_DOWN,
        FontId::proportional(12.0),
        p.text_weak,
    );
    None
}

#[allow(
    clippy::too_many_arguments,
    reason = "the header's pieces, plus whether the list icon is a spinner"
)]
fn expanded(
    ui: &mut egui::Ui,
    chapters: &[Chapter],
    current: usize,
    max_height: f32,
    p: Palette,
    open: &mut bool,
    reading: &mut Option<usize>,
    spin: bool,
    roster: &Roster,
) -> Option<f64> {
    let mut clicked = None;
    let card = egui::Frame::new()
        .fill(p.card)
        .corner_radius(CornerRadius::same(8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (head, resp) =
                ui.allocate_exact_size(egui::vec2(ui.available_width(), HEADER_H), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            if resp.clicked() {
                *open = false;
            }
            let h = header(head);
            let painter = painter_at(ui, head);
            header_icon(ui, h.icon, spin, p);
            painter_at(ui, h.text).text(
                egui::pos2(h.text.left(), head.center().y),
                Align2::LEFT_CENTER,
                t!("studio.topics"),
                FontId::proportional(13.0),
                p.text,
            );
            painter.text(
                h.caret.center(),
                Align2::CENTER_CENTER,
                icon::CARET_UP,
                FontId::proportional(12.0),
                p.text_weak,
            );
            ui.add_space(2.0);
            egui::Frame::new()
                .inner_margin(Margin {
                    left: 8,
                    right: 8,
                    top: 0,
                    bottom: 6,
                })
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height((max_height - 38.0).max(48.0))
                        .auto_shrink([false, true])
                        .id_salt("studio-topics-list")
                        .show(ui, |ui| {
                            for (i, c) in chapters.iter().enumerate() {
                                if let Some(topic) =
                                    topic_row(ui, i, c, i == current, reading, roster, p)
                                {
                                    clicked = Some(topic);
                                }
                                ui.add_space(2.0);
                            }
                        });
                });
        })
        .response
        .rect;
    ui.painter().rect(
        card,
        CornerRadius::same(8),
        Color32::TRANSPARENT,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    clicked
}

fn topic_row(
    ui: &mut egui::Ui,
    i: usize,
    c: &Chapter,
    current: bool,
    reading: &mut Option<usize>,
    roster: &Roster,
    p: Palette,
) -> Option<f64> {
    let (rect, row) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::click());
    let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
    let caret = Rect::from_center_size(
        egui::pos2(rect.right() - 10.0, rect.center().y),
        egui::vec2(18.0, 18.0),
    );
    let toggle = ui
        .interact(caret, ui.id().with(("topic-toggle", i)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    hover_tip(&row, c, roster);
    let on_caret = row
        .interact_pointer_pos()
        .is_some_and(|pt| caret.contains(pt));
    let mut clicked = None;
    if row.clicked() && !on_caret {
        clicked = Some(c.start);
    }
    if toggle.clicked() {
        *reading = Some(i);
    }
    let painter = painter_at(ui, rect);
    if current {
        painter.rect_filled(rect, CornerRadius::same(6), p.accent_soft);
    } else if row.hovered() || toggle.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.card_hover);
    }
    let font = FontId::monospace(11.5);
    let time = ui.fonts_mut(|f| f.layout_no_wrap(human_duration(c.start), font, p.accent));
    painter.galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - time.size().y / 2.0),
        time.clone(),
        p.accent,
    );
    let left = rect.left() + 8.0 + time.size().x + 10.0;
    let title = Rect::from_min_max(
        egui::pos2(left, rect.top()),
        egui::pos2(caret.left() - 2.0, rect.bottom()),
    );
    let color = if current { p.accent } else { p.text };
    let galley = elided(
        ui,
        &c.digest.title,
        title.width(),
        FontId::proportional(13.0),
        color,
        roster,
        p,
    );
    painter.galley(
        egui::pos2(title.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    painter.text(
        caret.center(),
        Align2::CENTER_CENTER,
        icon::CARET_RIGHT,
        FontId::proportional(12.0),
        p.text_weak,
    );
    clicked
}

const READING_STEP: f32 = 22.0;
const MARKER_W: f32 = 22.0;
const TIME_COL: f32 = 46.0;
const ITEM_LINE: f32 = 18.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReadingHeader {
    pub icon: egui::Pos2,
    pub text: Rect,
    pub prev: Rect,
    pub next: Rect,
    pub collapse: Rect,
}

pub fn reading_header(rect: Rect) -> ReadingHeader {
    let h = header(rect);
    ReadingHeader {
        icon: h.icon,
        text: h.text,
        prev: h.caret.translate(egui::vec2(-2.0 * READING_STEP, 0.0)),
        next: h.caret.translate(egui::vec2(-READING_STEP, 0.0)),
        collapse: h.caret,
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub marker: String,
    pub time: Option<f64>,
    pub text: String,
}

pub fn section_lines(section: &Section) -> Vec<Line> {
    let mut n = 0;
    section
        .items
        .iter()
        .filter(|item| !item.text.trim().is_empty())
        .map(|item| {
            n += 1;
            let marker = match section.kind {
                SectionKind::Steps => format!("{n}."),
                SectionKind::List => "•".to_string(),
                SectionKind::Text => String::new(),
            };
            Line {
                marker,
                time: item.time,
                text: item.text.trim().to_string(),
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
pub fn reading(
    ui: &mut egui::Ui,
    chapters: &[Chapter],
    current: &mut Option<usize>,
    player: &mut Player,
    building: bool,
    spin: bool,
    roster: &Roster,
    p: Palette,
) -> Option<usize> {
    let i = (*current)?.min(chapters.len().checked_sub(1)?);
    let c = &chapters[i];
    let esc = !ui.ctx().egui_wants_keyboard_input()
        && ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape));
    let (card, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    ui.painter()
        .rect_filled(card, CornerRadius::same(8), p.card);
    let head = Rect::from_min_size(card.min, egui::vec2(card.width(), HEADER_H));
    let h = reading_header(head);
    let painter = painter_at(ui, head);
    header_icon(ui, h.icon, spin, p);
    let text = format!("{}  {}", human_duration(c.start), c.digest.title.trim());
    let galley = elided(
        ui,
        &text,
        h.text.width(),
        FontId::proportional(13.0),
        p.text,
        roster,
        p,
    );
    painter.galley(
        egui::pos2(h.text.left(), head.center().y - galley.size().y / 2.0),
        galley,
        p.text,
    );
    let mut leave = esc;
    let buttons = [
        (
            h.prev,
            icon::CARET_LEFT,
            t!("analysis.prev_topic"),
            i.checked_sub(1),
            "prev",
        ),
        (
            h.next,
            icon::CARET_RIGHT,
            t!("analysis.next_topic"),
            (i + 1 < chapters.len()).then_some(i + 1),
            "next",
        ),
        (
            h.collapse,
            icon::CARET_UP,
            t!("analysis.collapse_topic"),
            None,
            "collapse",
        ),
    ];
    for (rect, glyph, tip, target, id) in buttons {
        let enabled = target.is_some() || id == "collapse";
        let resp = ui.interact(rect, ui.id().with(("reading", id)), Sense::click());
        if enabled && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            painter.rect_filled(rect, CornerRadius::same(5), p.card_hover);
        }
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            glyph,
            FontId::proportional(12.0),
            if enabled {
                p.text
            } else {
                p.text_weak.gamma_multiply(0.5)
            },
        );
        text_tip(&resp, &tip, roster);
        if enabled && resp.clicked() {
            match target {
                Some(t) => *current = Some(t),
                None => leave = true,
            }
        }
    }
    ui.painter()
        .hline(card.x_range(), head.bottom(), Stroke::new(1.0, p.border));
    let body = Rect::from_min_max(
        egui::pos2(card.left() + 12.0, head.bottom() + 6.0),
        egui::pos2(card.right() - 12.0, card.bottom() - 8.0),
    );
    let mut pending = None;
    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
        egui::ScrollArea::vertical()
            .id_salt(("studio-topic-reading", i))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let d = &c.digest;
                if d.sections.is_empty() {
                    pending = building.then_some(i);
                    ui.horizontal(|ui| {
                        if building {
                            ui.spinner();
                            ui.label(
                                RichText::new(t!("analysis.reading_pending")).color(p.text_weak),
                            );
                        } else {
                            ui.label(
                                RichText::new(t!("analysis.reading_missing")).color(p.text_weak),
                            );
                        }
                    });
                    if !d.summary.trim().is_empty() {
                        ui.add_space(6.0);
                        mentions::show(
                            ui,
                            d.summary.trim(),
                            roster,
                            FontId::proportional(13.0),
                            p.text,
                            false,
                            p,
                        );
                    }
                }
                for section in &d.sections {
                    let lines = section_lines(section);
                    if lines.is_empty() {
                        continue;
                    }
                    if !section.title.trim().is_empty() {
                        mentions::show(
                            ui,
                            section.title.trim(),
                            roster,
                            FontId::proportional(13.5),
                            ui.visuals().strong_text_color(),
                            false,
                            p,
                        );
                        ui.add_space(2.0);
                    }
                    for line in lines {
                        item_row(ui, &line, false, player, roster, p);
                    }
                    ui.add_space(10.0);
                }
                let parts: Vec<&Part> = d
                    .parts
                    .iter()
                    .filter(|part| part.end > part.start && !part.title.trim().is_empty())
                    .collect();
                if !parts.is_empty() {
                    ui.label(RichText::new(t!("analysis.parts")).size(13.5).strong());
                    ui.add_space(2.0);
                    for part in parts {
                        item_row(
                            ui,
                            &Line {
                                marker: "•".into(),
                                time: Some(part.start),
                                text: part.title.trim().to_string(),
                            },
                            false,
                            player,
                            roster,
                            p,
                        );
                    }
                    ui.add_space(10.0);
                }
                let quotes: Vec<_> = d
                    .quotes
                    .iter()
                    .filter(|q| !q.text.trim().is_empty())
                    .collect();
                if !quotes.is_empty() {
                    ui.label(RichText::new(t!("studio.quotes")).size(13.5).strong());
                    ui.add_space(2.0);
                    for q in quotes {
                        item_row(
                            ui,
                            &Line {
                                marker: String::new(),
                                time: Some(q.time),
                                text: q.text.trim().to_string(),
                            },
                            true,
                            player,
                            roster,
                            p,
                        );
                    }
                }
            });
    });
    ui.painter().rect(
        card,
        CornerRadius::same(8),
        Color32::TRANSPARENT,
        Stroke::new(1.0, p.border),
        StrokeKind::Inside,
    );
    if leave {
        *current = None;
    }
    pending
}

fn header_icon(ui: &egui::Ui, at: egui::Pos2, spin: bool, p: Palette) {
    if spin {
        egui::Spinner::new().size(14.0).color(p.accent).paint_at(
            ui,
            Rect::from_center_size(at + egui::vec2(7.0, 0.0), egui::vec2(14.0, 14.0)),
        );
    } else {
        ui.painter().text(
            at,
            Align2::LEFT_CENTER,
            icon::LIST_BULLETS,
            FontId::proportional(14.0),
            p.accent,
        );
    }
}

fn item_row(
    ui: &mut egui::Ui,
    line: &Line,
    italic: bool,
    player: &mut Player,
    roster: &Roster,
    p: Palette,
) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (marker, _) = ui.allocate_exact_size(egui::vec2(MARKER_W, ITEM_LINE), Sense::hover());
        if !line.marker.is_empty() {
            ui.painter().text(
                marker.left_center(),
                Align2::LEFT_CENTER,
                &line.marker,
                FontId::proportional(13.0),
                p.text_weak,
            );
        }
        if let Some(time) = line.time {
            let (slot, resp) =
                ui.allocate_exact_size(egui::vec2(TIME_COL, ITEM_LINE), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let color = if resp.hovered() { p.text } else { p.accent };
            let galley = ui.fonts_mut(|f| {
                f.layout_no_wrap(human_duration(time), FontId::monospace(11.0), color)
            });
            ui.painter().galley(
                egui::pos2(slot.left(), slot.center().y - galley.size().y / 2.0),
                galley.clone(),
                color,
            );
            if resp.hovered() {
                ui.painter().hline(
                    slot.left()..=slot.left() + galley.size().x,
                    slot.center().y + galley.size().y / 2.0,
                    Stroke::new(1.0, color),
                );
            }
            text_tip(&resp, &t!("studio.go_there"), roster);
            if resp.clicked() {
                player.seek(time);
            }
        }
        mentions::show(
            ui,
            &line.text,
            roster,
            FontId::proportional(13.0),
            p.text,
            italic,
            p,
        );
    });
    ui.add_space(3.0);
}

pub fn text_tip(resp: &egui::Response, text: &str, roster: &Roster) {
    let mut tip = egui::Tooltip::for_enabled(resp);
    tip.popup = tip
        .popup
        .width(360.0)
        .frame(tooltip_frame(resp.ctx.global_style().as_ref()));
    tip.show(|ui| {
        ui.set_max_width(360.0);
        let p = crate::theme::palette(ui);
        mentions::show(
            ui,
            text,
            roster,
            FontId::proportional(12.0),
            p.text,
            false,
            p,
        );
    });
}

fn hover_tip(resp: &egui::Response, c: &Chapter, roster: &Roster) {
    let mut tip = egui::Tooltip::for_enabled(resp);
    tip.popup = tip
        .popup
        .width(360.0)
        .frame(tooltip_frame(resp.ctx.global_style().as_ref()));
    tip.show(|ui| {
        ui.set_max_width(360.0);
        let p = crate::theme::palette(ui);
        mentions::show(
            ui,
            &hover_text(c),
            roster,
            FontId::proportional(12.0),
            p.text,
            false,
            p,
        );
    });
}

fn painter_at(ui: &egui::Ui, rect: Rect) -> egui::Painter {
    let mut painter = ui.painter_at(rect);
    painter.set_clip_rect(rect.intersect(ui.clip_rect()));
    painter
}

fn elided(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font: FontId,
    color: Color32,
    roster: &Roster,
    p: Palette,
) -> std::sync::Arc<egui::Galley> {
    mentions::galley(ui, text, roster, font, color, width, true, false, p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::outline::Digest;

    fn chapter(start: f64, end: f64) -> Chapter {
        Chapter {
            start,
            end,
            digest: Digest {
                title: format!("тема {start}"),
                ..Digest::default()
            },
        }
    }

    #[test]
    fn the_block_header_keeps_its_geometry_in_both_states() {
        let rect = Rect::from_min_size(egui::pos2(100.0, 200.0), egui::vec2(400.0, HEADER_H));
        let h = header(rect);
        assert_eq!(h.icon, egui::pos2(112.0, 214.0), "the list icon");
        assert_eq!(h.text.left(), 134.0, "the text starts here in both states");
        assert_eq!(
            (h.text.top(), h.text.right()),
            (200.0, 426.0),
            "the same clipping rect"
        );
        assert_eq!(h.count, egui::pos2(470.0, 214.0), "the count, folded only");
        assert_eq!(
            h.caret.center(),
            egui::pos2(486.0, 214.0),
            "the caret, same place"
        );
        assert_eq!(
            (h.icon.y, h.text.center().y, h.caret.center().y),
            (214.0, 214.0, 214.0),
            "one centre line"
        );
        let h = header(Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(80.0, HEADER_H),
        ));
        assert!(h.text.right() >= h.text.left());
    }

    #[test]
    fn the_current_topic_by_time() {
        let chapters = [
            chapter(0.0, 60.0),
            chapter(60.0, 120.0),
            chapter(120.0, 180.0),
        ];
        assert_eq!(current_topic(&[], 10.0), None);
        assert_eq!(
            current_topic(&chapters, -1.0),
            Some(0),
            "before the first topic"
        );
        assert_eq!(current_topic(&chapters, 0.0), Some(0));
        assert_eq!(current_topic(&chapters, 59.9), Some(0));
        assert_eq!(
            current_topic(&chapters, 60.0),
            Some(1),
            "the border belongs to the next topic"
        );
        assert_eq!(current_topic(&chapters, 179.0), Some(2));
        assert_eq!(
            current_topic(&chapters, 500.0),
            Some(2),
            "after the last topic"
        );
    }

    #[test]
    fn segments_have_two_pixel_gaps_and_keep_the_times() {
        let chapters = [
            chapter(0.0, 20.0),
            chapter(20.0, 50.0),
            chapter(50.0, 100.0),
        ];
        let s = segments(10.0, 100.0, 100.0, &chapters);
        assert_eq!(s.len(), 3);
        assert_eq!(
            (s[0].x0, s[0].x1),
            (10.0, 29.0),
            "20 % of 100 px, minus half a gap"
        );
        assert_eq!((s[1].x0, s[1].x1), (31.0, 59.0));
        assert_eq!(
            (s[2].x0, s[2].x1),
            (61.0, 110.0),
            "no gap at the outer edges"
        );
        for w in s.windows(2) {
            assert!((w[1].x0 - w[0].x1 - SEGMENT_GAP).abs() < 1e-6, "{w:?}");
        }
    }

    #[test]
    fn segments_cover_the_whole_bar() {
        let chapters = [chapter(5.0, 50.0), chapter(50.0, 95.0)];
        let s = segments(10.0, 100.0, 100.0, &chapters);
        assert_eq!(
            (s[0].x0, s[0].x1),
            (10.0, 59.0),
            "from the bar's left edge to the border"
        );
        assert_eq!(
            (s[1].x0, s[1].x1),
            (61.0, 110.0),
            "from the border to the bar's right edge"
        );
        let s = segments(10.0, 100.0, 100.0, &[chapter(5.0, 95.0)]);
        assert_eq!(
            (s[0].x0, s[0].x1),
            (10.0, 110.0),
            "one topic fills the whole bar"
        );
    }

    #[test]
    fn segments_clip_to_the_bar_and_survive_odd_input() {
        assert!(
            segments(0.0, 100.0, 0.0, &[chapter(0.0, 10.0)]).is_empty(),
            "no duration"
        );
        assert!(
            segments(0.0, 0.0, 100.0, &[chapter(0.0, 10.0)]).is_empty(),
            "no width"
        );
        assert!(segments(0.0, 100.0, 100.0, &[]).is_empty());
        let s = segments(0.0, 100.0, 100.0, &[chapter(0.0, 200.0)]);
        assert_eq!(
            (s[0].x0, s[0].x1),
            (0.0, 100.0),
            "an end past the recording stays on the bar"
        );
        let s = segments(
            0.0,
            100.0,
            100.0,
            &[chapter(50.0, 50.0), chapter(50.0, 100.0)],
        );
        assert_eq!(
            (s[0].x0, s[0].x1),
            (0.0, 49.0),
            "the first topic covers the start up to its border"
        );
        assert_eq!(
            (s[1].x0, s[1].x1),
            (51.0, 100.0),
            "and a zero-length topic does not invert"
        );
    }

    #[test]
    fn the_tooltip_is_only_the_title() {
        let c = Chapter {
            start: 40.0,
            end: 100.0,
            digest: Digest {
                title: " План ".into(),
                summary: "Решили\nвторое".into(),
                ..Digest::default()
            },
        };
        assert_eq!(tooltip(&c, None), "План", "no time, no summary");
        let c = Chapter {
            start: 0.0,
            end: 10.0,
            digest: Digest {
                title: "Пусто".into(),
                summary: "Решили".into(),
                ..Digest::default()
            },
        };
        assert_eq!(tooltip(&c, None), "Пусто");
    }

    #[test]
    fn the_tooltip_is_centred_on_the_pointer_inside_the_window() {
        let content = Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1000.0, 800.0));
        assert_eq!(tooltip_x(500.0, 100.0, content), 450.0);
        assert_eq!(
            tooltip_x(10.0, 100.0, content),
            0.0,
            "slides in at the left edge"
        );
        assert_eq!(
            tooltip_x(995.0, 100.0, content),
            900.0,
            "and at the right one"
        );
        assert_eq!(
            tooltip_x(500.0, 1200.0, content),
            0.0,
            "a tooltip wider than the window starts at its edge"
        );
    }

    #[test]
    fn the_line_under_the_bar_names_the_topic_and_its_part() {
        let mut c = chapter(0.0, 100.0);
        c.digest.title = "Тема".into();
        c.digest.parts = parts();
        assert_eq!(current_label(&[], 10.0), "");
        assert_eq!(
            current_label(&[chapter(10.0, 100.0)], 0.0),
            "тема 10",
            "the first topic is current before it starts"
        );
        assert_eq!(current_label(&[c.clone()], 10.0), "Тема — Начало");
        assert_eq!(current_label(&[c.clone()], 50.0), "Тема — Середина");
        c.digest.parts[1].title = " ".into();
        assert_eq!(
            current_label(&[c], 50.0),
            "Тема",
            "a part without a title is not named"
        );
    }

    #[test]
    fn the_hover_text_is_the_summary_and_the_part_titles() {
        let mut c = Chapter {
            start: 0.0,
            end: 100.0,
            digest: Digest {
                title: "Тема".into(),
                summary: " Решили ".into(),
                parts: parts(),
                ..Digest::default()
            },
        };
        assert_eq!(hover_text(&c), "Решили\n\n•  Начало\n•  Середина\n•  Конец");
        c.digest.summary = "  ".into();
        assert_eq!(hover_text(&c), "•  Начало\n•  Середина\n•  Конец");
        c.digest.parts.clear();
        assert_eq!(hover_text(&c), "Тема", "nothing else to say");
    }

    fn parts() -> Vec<Part> {
        vec![
            Part {
                start: 0.0,
                end: 30.0,
                title: "Начало".into(),
            },
            Part {
                start: 30.0,
                end: 70.0,
                title: "Середина".into(),
            },
            Part {
                start: 70.0,
                end: 100.0,
                title: "Конец".into(),
            },
        ]
    }

    #[test]
    fn the_part_at_a_time() {
        let c = Chapter {
            start: 0.0,
            end: 100.0,
            digest: Digest {
                title: "Тема".into(),
                parts: parts(),
                ..Digest::default()
            },
        };
        assert_eq!(part_at(&c, -1.0), None, "before the parts");
        assert_eq!(part_at(&c, 0.0), Some(0));
        assert_eq!(part_at(&c, 29.9), Some(0));
        assert_eq!(
            part_at(&c, 30.0),
            Some(1),
            "the border belongs to the next part"
        );
        assert_eq!(part_at(&c, 70.0), Some(2));
        assert_eq!(part_at(&c, 100.0), Some(2));
        assert_eq!(part_at(&c, 100.1), None, "after the parts");
        assert_eq!(
            part_at(&chapter(0.0, 100.0), 10.0),
            None,
            "a topic without parts"
        );
        assert_eq!(
            part_at(
                &Chapter {
                    digest: Digest {
                        parts: parts(),
                        ..Digest::default()
                    },
                    ..chapter(0.0, 100.0)
                },
                50.0
            ),
            Some(1),
            "no parts, no time"
        );
    }

    #[test]
    fn part_ticks_stand_on_the_boundaries() {
        assert_eq!(part_ticks(10.0, 100.0, 100.0, &parts()), vec![40.0, 80.0]);
        assert!(
            part_ticks(10.0, 100.0, 100.0, &parts()[..1]).is_empty(),
            "one part has no boundary"
        );
        assert!(part_ticks(10.0, 100.0, 100.0, &[]).is_empty());
        assert!(
            part_ticks(10.0, 0.0, 100.0, &parts()).is_empty(),
            "no width"
        );
        assert!(
            part_ticks(10.0, 100.0, 0.0, &parts()).is_empty(),
            "no duration"
        );
        let over = vec![
            Part {
                start: 0.0,
                end: 50.0,
                title: String::new(),
            },
            Part {
                start: 150.0,
                end: 200.0,
                title: String::new(),
            },
        ];
        assert!(
            part_ticks(10.0, 100.0, 100.0, &over).is_empty(),
            "a boundary past the bar is dropped"
        );
    }

    #[test]
    fn the_tooltip_names_the_part_under_the_pointer() {
        let c = Chapter {
            start: 40.0,
            end: 100.0,
            digest: Digest {
                title: "План".into(),
                summary: "Решили\nвторое".into(),
                parts: parts(),
                ..Digest::default()
            },
        };
        assert_eq!(tooltip(&c, c.digest.parts.get(1)), "План — Середина");
        let blank = Part {
            start: 0.0,
            end: 1.0,
            title: "  ".into(),
        };
        assert_eq!(
            tooltip(&c, Some(&blank)),
            "План",
            "a part without a title is not named"
        );
    }

    #[test]
    fn the_unplayed_colour_is_stronger_than_the_soft_one() {
        for palette in [crate::theme::LIGHT, crate::theme::DARK] {
            let unplayed = mix(palette.accent_soft, palette.accent, UNPLAYED);
            let distance = |a: Color32, b: Color32| {
                (f32::from(a.r()) - f32::from(b.r())).abs()
                    + (f32::from(a.g()) - f32::from(b.g())).abs()
                    + (f32::from(a.b()) - f32::from(b.b())).abs()
            };
            assert!(
                distance(unplayed, palette.accent_soft) > 100.0,
                "unplayed vs accent_soft: {unplayed:?}"
            );
            assert!(
                distance(unplayed, palette.border) > 60.0,
                "unplayed vs the gap base: {unplayed:?}"
            );
            assert!(
                distance(unplayed, palette.accent) > 120.0,
                "unplayed vs played: {unplayed:?}"
            );
        }
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 0.0), Color32::BLACK);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 1.0), Color32::WHITE);
    }

    #[test]
    fn the_breakdown_header_keeps_the_blocks_header_geometry() {
        let rect = Rect::from_min_size(egui::pos2(100.0, 200.0), egui::vec2(400.0, HEADER_H));
        let (h, r) = (header(rect), reading_header(rect));
        assert_eq!(
            (r.icon, r.text),
            (h.icon, h.text),
            "the icon and the text do not move"
        );
        assert_eq!(
            r.collapse, h.caret,
            "the collapse button is where the caret is"
        );
        assert_eq!(r.next.center().y, h.caret.center().y);
        assert_eq!(r.prev.center().y, h.caret.center().y);
        assert!((r.collapse.center().x - r.next.center().x - READING_STEP).abs() < 1e-6);
        assert!((r.next.center().x - r.prev.center().x - READING_STEP).abs() < 1e-6);
        assert!(r.prev.left() >= r.text.right(), "{r:?}");
        assert!(r.prev.right() <= r.next.left() && r.next.right() <= r.collapse.left());
    }

    fn item(text: &str, time: Option<f64>) -> mediaa_core::outline::Item {
        mediaa_core::outline::Item {
            text: text.into(),
            time,
        }
    }

    #[test]
    fn steps_are_numbered_lists_bulleted_and_prose_bare() {
        let items = vec![
            item("раз", Some(10.0)),
            item("  ", Some(20.0)),
            item("два", None),
            item("три", Some(30.0)),
        ];
        let steps = Section {
            title: "Шаги".into(),
            kind: SectionKind::Steps,
            items: items.clone(),
        };
        let lines = section_lines(&steps);
        assert_eq!(
            lines.iter().map(|l| l.marker.as_str()).collect::<Vec<_>>(),
            ["1.", "2.", "3."],
            "an empty item is skipped and not counted"
        );
        assert_eq!(
            lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["раз", "два", "три"],
            "the order is kept"
        );
        assert_eq!(
            lines.iter().map(|l| l.time).collect::<Vec<_>>(),
            [Some(10.0), None, Some(30.0)],
            "a missing time stays missing"
        );
        let list = Section {
            kind: SectionKind::List,
            ..steps.clone()
        };
        assert!(section_lines(&list).iter().all(|l| l.marker == "•"));
        let text = Section {
            kind: SectionKind::Text,
            ..steps
        };
        assert!(section_lines(&text).iter().all(|l| l.marker.is_empty()));
        assert!(section_lines(&Section::default()).is_empty());
    }
}
