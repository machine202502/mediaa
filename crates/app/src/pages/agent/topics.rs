//! Topics of a recording in the studio: the seek bar cut into topic segments (with a tooltip
//! per segment and a tick at every part boundary) and the pinned block above the dialog —
//! folded to the current topic, unfolded to the whole list, each topic with its parts, digest
//! and quotes. The place of a topic or a part in time and on the bar are pure functions,
//! tested without a window.

use crate::player::Player;
use crate::theme::Palette;
use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, StrokeKind};
use egui_phosphor::regular as icon;
use mediaa_core::library::human_duration;
use mediaa_core::outline::{Chapter, Part};
use rust_i18n::t;

/// Gap between neighbouring topic segments of the seek bar.
pub const SEGMENT_GAP: f32 = 2.0;
/// Track height of the seek bar (the hit area around it stays 24 px).
const TRACK: f32 = 6.0;
/// How far the unplayed part of a segment is mixed from `accent_soft` towards `accent`: the
/// soft colour alone is almost the panel background in both themes.
const UNPLAYED: f32 = 0.45;

/// The current topic's block remembered across frames and restarts (like the studio layout).
fn open_id() -> egui::Id {
    egui::Id::new("studio-topics-open")
}

/// A topic's place on the seek bar, in x pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    pub chapter: usize,
    pub x0: f32,
    pub x1: f32,
}

/// The topic that is current at `time`: the last one that began by then (the first one before
/// the first topic starts, the last one after the last).
pub fn current_topic(chapters: &[Chapter], time: f64) -> Option<usize> {
    if chapters.is_empty() {
        return None;
    }
    Some(chapters.partition_point(|c| c.start <= time).saturating_sub(1))
}

/// The x ranges of the topics on a track `width` wide starting at `left`, `duration` seconds
/// long: every border between neighbours is inset by half of [`SEGMENT_GAP`], so the gap
/// between two segments is exactly that (the clip keeps zero-length topics from inverting).
pub fn segments(left: f32, width: f32, duration: f64, chapters: &[Chapter]) -> Vec<Segment> {
    if duration <= 0.0 || width <= 0.0 {
        return Vec::new();
    }
    let x = |time: f64| left + width * (time / duration).clamp(0.0, 1.0) as f32;
    let last = chapters.len().saturating_sub(1);
    chapters
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let mut x0 = x(c.start);
            let mut x1 = x(c.end);
            if i > 0 {
                x0 += SEGMENT_GAP / 2.0;
            }
            if i < last {
                x1 -= SEGMENT_GAP / 2.0;
            }
            Segment { chapter: i, x0, x1: x1.max(x0) }
        })
        .collect()
}

/// The part of a topic that is at `time`: the last one that began by then, when `time` is
/// inside the topic's parts at all (a normal topic has no parts).
pub fn part_at(c: &Chapter, time: f64) -> Option<usize> {
    let parts = &c.digest.parts;
    let last = parts.last()?;
    if time < parts[0].start || time > last.end {
        return None;
    }
    Some(parts.partition_point(|p| p.start <= time).saturating_sub(1))
}

/// The x of every boundary between neighbouring parts of a topic, on a track `width` wide
/// starting at `left`, `duration` seconds long (an empty list for a topic without parts).
/// The start of the first part is the topic's own border, so it gets no tick.
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

/// `a` moved `t` of the way towards `b` (both opaque palette colours).
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// The tooltip of a topic: «title / m:ss–m:ss / the first line of the summary»; with `part`
/// (the part under the pointer) the first line becomes «title — part title».
pub fn tooltip(c: &Chapter, part: Option<&Part>) -> String {
    let part = part.map(|p| p.title.trim()).filter(|title| !title.is_empty());
    let title = match part {
        Some(title) => format!("{} — {title}", c.digest.title),
        None => c.digest.title.clone(),
    };
    let mut out = format!("{title}\n{}–{}", human_duration(c.start), human_duration(c.end));
    let first = c.digest.summary.lines().next().unwrap_or_default().trim();
    if !first.is_empty() {
        out.push('\n');
        out.push_str(first);
    }
    out
}

/// The seek bar: the playable track with a segment per topic (`2 px` gaps, the segment under
/// the pointer highlighted and its tooltip shown, a tick at every boundary between the parts
/// of a topic), the played part and the knob as before. A recording without topics gets the
/// plain bar. Returns the clicked or dragged fraction.
pub fn seek_bar(ui: &mut egui::Ui, width: f32, frac: f32, duration: f64, chapters: &[Chapter], id: &str, p: Palette) -> (egui::Response, Option<f32>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width.max(40.0), 24.0), Sense::hover());
    let resp = ui.interact(rect, ui.id().with(id), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 14.0, TRACK));
    let x = track.left() + track.width() * frac.clamp(0.0, 1.0);
    let segments = segments(track.left(), track.width(), duration, chapters);
    let hovered = resp.hover_pos().and_then(|pt| segments.iter().position(|s| (s.x0..=s.x1).contains(&pt.x)));
    // The part under the pointer, to name it in the tooltip.
    let hovered_part = resp.hover_pos().zip(hovered).and_then(|(pt, i)| {
        let time = f64::from((pt.x - track.left()) / track.width()) * duration;
        part_at(&chapters[i], time).map(|p| &chapters[i].digest.parts[p])
    });
    {
        let painter = ui.painter();
        // The base shows through the gaps, so they read as slots between the segments.
        painter.rect_filled(track, CornerRadius::same(3), p.border);
        for (i, seg) in segments.iter().enumerate() {
            let r = Rect::from_min_max(egui::pos2(seg.x0, track.min.y), egui::pos2(seg.x1, track.max.y));
            if r.width() < 0.5 {
                continue;
            }
            painter.rect_filled(r, CornerRadius::same(2), if hovered == Some(i) { p.accent } else { mix(p.accent_soft, p.accent, UNPLAYED) });
            // The played part of every segment up to the knob.
            if hovered != Some(i) && x > seg.x0 {
                let played = Rect::from_min_max(r.min, egui::pos2(x.min(seg.x1), r.max.y));
                if played.width() > 0.5 {
                    painter.rect_filled(played, CornerRadius::same(2), p.accent);
                }
            }
            // A thin tick at every boundary between the parts of this topic.
            for at in part_ticks(track.left(), track.width(), duration, &chapters[i].digest.parts) {
                if at <= seg.x0 || at >= seg.x1 {
                    continue;
                }
                let tick = Rect::from_min_size(egui::pos2(at.floor(), track.min.y), egui::vec2(1.0, track.height()));
                painter.rect_filled(tick, CornerRadius::ZERO, p.text_weak);
            }
        }
        if segments.is_empty() {
            painter.rect_filled(Rect::from_min_max(track.min, egui::pos2(x, track.max.y)), CornerRadius::same(2), p.accent);
        }
        let knob = if resp.hovered() || resp.dragged() { 7.0 } else { 6.0 };
        painter.circle(egui::pos2(x, track.center().y), knob, Color32::WHITE, Stroke::new(1.5, p.accent));
    }
    if resp.hovered() || hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if let Some(i) = hovered {
        egui::Tooltip::always_open(ui.ctx().clone(), ui.layer_id(), ui.id().with((id, "topic-tip")), egui::PopupAnchor::Pointer)
            .show(|ui| {
                ui.label(RichText::new(tooltip(&chapters[i], hovered_part)).size(12.0));
            });
    }
    let value = resp.interact_pointer_pos().filter(|_| resp.clicked() || resp.dragged()).map(|pt| ((pt.x - track.left()) / track.width()).clamp(0.0, 1.0));
    (resp, value)
}

/// The pinned block above the dialog. Folded: one line «m:ss Title (3/7)» of the current topic
/// (a click unfolds it). Unfolded: the topics in their own scroll at most `max_height` tall,
/// the current one highlighted, each with an expandable digest whose parts come first; a click
/// on a topic or on one of its parts returns the time to seek to (the dialog scrolls to the
/// first reply from there). A click on a quote seeks to the quote's time.
pub fn block(ui: &mut egui::Ui, chapters: &[Chapter], position: f64, player: &mut Player, max_height: f32, p: Palette) -> Option<f64> {
    if chapters.is_empty() {
        return None;
    }
    let mut open = ui.ctx().data_mut(|d| d.get_persisted::<bool>(open_id())).unwrap_or(false);
    let current = current_topic(chapters, position).unwrap_or(0);
    let clicked = if open {
        expanded(ui, chapters, current, player, max_height.max(96.0), p, &mut open)
    } else {
        folded(ui, chapters, current, p, &mut open)
    };
    if ui.ctx().data_mut(|d| d.get_persisted::<bool>(open_id())) != Some(open) {
        ui.ctx().data_mut(|d| d.insert_persisted(open_id(), open));
    }
    clicked
}

/// The folded line: the current topic; a click unfolds the block.
fn folded(ui: &mut egui::Ui, chapters: &[Chapter], current: usize, p: Palette, open: &mut bool) -> Option<f64> {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("studio.topics_expand"));
    if resp.clicked() {
        *open = true;
    }
    let painter = ui.painter_at(rect);
    painter.rect(rect, CornerRadius::same(8), if resp.hovered() { p.card_hover } else { p.card }, Stroke::new(1.0, p.border), StrokeKind::Inside);
    painter.text(egui::pos2(rect.left() + 12.0, rect.center().y), Align2::LEFT_CENTER, icon::LIST_BULLETS, FontId::proportional(14.0), p.accent);
    let c = &chapters[current];
    let text = format!("{}  {}", human_duration(c.start), c.digest.title);
    let title = Rect::from_min_max(egui::pos2(rect.left() + 34.0, rect.top()), egui::pos2(rect.right() - 74.0, rect.bottom()));
    painter_at(ui, title).text(egui::pos2(title.left(), rect.center().y), Align2::LEFT_CENTER, text, FontId::proportional(13.0), p.text);
    painter.text(egui::pos2(rect.right() - 30.0, rect.center().y), Align2::RIGHT_CENTER, format!("({}/{})", current + 1, chapters.len()), FontId::monospace(11.0), p.text_weak);
    painter.text(egui::pos2(rect.right() - 14.0, rect.center().y), Align2::CENTER_CENTER, icon::CARET_DOWN, FontId::proportional(12.0), p.text_weak);
    None
}

/// The unfolded block: a header that folds it and the list of topics in their own scroll.
fn expanded(ui: &mut egui::Ui, chapters: &[Chapter], current: usize, player: &mut Player, max_height: f32, p: Palette, open: &mut bool) -> Option<f64> {
    let mut clicked = None;
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, p.border))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(8, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let (head, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text(t!("studio.topics_collapse"));
            if resp.clicked() {
                *open = false;
            }
            let painter = painter_at(ui, head);
            painter.text(egui::pos2(head.left() + 2.0, head.center().y), Align2::LEFT_CENTER, icon::LIST_BULLETS, FontId::proportional(14.0), p.accent);
            painter.text(egui::pos2(head.left() + 22.0, head.center().y), Align2::LEFT_CENTER, t!("studio.topics"), FontId::proportional(12.0), p.text_weak);
            painter.text(egui::pos2(head.right() - 8.0, head.center().y), Align2::CENTER_CENTER, icon::CARET_UP, FontId::proportional(12.0), p.text_weak);
            ui.add_space(2.0);
            // The list keeps its own scroll; the whole block stays under ~40 % of the pane.
            egui::ScrollArea::vertical()
                .max_height((max_height - 38.0).max(48.0))
                .auto_shrink([false, true])
                .id_salt("studio-topics-list")
                .show(ui, |ui| {
                    for (i, c) in chapters.iter().enumerate() {
                        if let Some(topic) = topic_row(ui, i, c, i == current, player, p) {
                            clicked = Some(topic);
                        }
                        ui.add_space(2.0);
                    }
                });
        });
    clicked
}

/// One topic in the unfolded list: a click on it returns its start (the time to seek to); the
/// caret unfolds its digest. The current topic is highlighted.
fn topic_row(ui: &mut egui::Ui, i: usize, c: &Chapter, current: bool, player: &mut Player, p: Palette) -> Option<f64> {
    let digest_id = ui.id().with(("topic-digest", i));
    let digest_open = ui.data(|d| d.get_temp::<bool>(digest_id)).unwrap_or(false);
    let (rect, row) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 26.0), Sense::click());
    let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
    let caret = Rect::from_center_size(egui::pos2(rect.right() - 10.0, rect.center().y), egui::vec2(18.0, 18.0));
    let toggle = ui
        .interact(caret, ui.id().with(("topic-toggle", i)), Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(t!("studio.digest"));
    let on_caret = row.interact_pointer_pos().is_some_and(|pt| caret.contains(pt));
    let mut clicked = None;
    if row.clicked() && !on_caret {
        clicked = Some(c.start);
    }
    if toggle.clicked() {
        ui.data_mut(|d| d.insert_temp(digest_id, !digest_open));
    }
    let painter = painter_at(ui, rect);
    if current {
        painter.rect_filled(rect, CornerRadius::same(6), p.accent_soft);
    } else if row.hovered() || toggle.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.card_hover);
    }
    let font = FontId::monospace(11.5);
    let time = ui.fonts_mut(|f| f.layout_no_wrap(human_duration(c.start), font, p.accent));
    painter.galley(egui::pos2(rect.left() + 8.0, rect.center().y - time.size().y / 2.0), time.clone(), p.accent);
    let left = rect.left() + 8.0 + time.size().x + 10.0;
    let title = Rect::from_min_max(egui::pos2(left, rect.top()), egui::pos2(caret.left() - 2.0, rect.bottom()));
    painter_at(ui, title).text(egui::pos2(left, rect.center().y), Align2::LEFT_CENTER, &c.digest.title, FontId::proportional(13.0), if current { p.accent } else { p.text });
    painter.text(caret.center(), Align2::CENTER_CENTER, if digest_open { icon::CARET_DOWN } else { icon::CARET_RIGHT }, FontId::proportional(12.0), p.text_weak);
    if digest_open {
        if let Some(start) = digest_ui(ui, i, c, player, p) {
            clicked = Some(start);
        }
    }
    clicked
}

/// The digest of a topic under its row: its parts first (a click returns the part's start),
/// then the summary, decisions, actions, questions and quotes (a click on a quote's time seeks
/// there).
fn digest_ui(ui: &mut egui::Ui, i: usize, c: &Chapter, player: &mut Player, p: Palette) -> Option<f64> {
    let d = &c.digest;
    ui.indent(("topic-digest-body", i), |ui| {
        let mut clicked = None;
        // The pieces a long merged topic is made of; a normal topic has none.
        let parts: Vec<&Part> = d.parts.iter().filter(|part| part.end > part.start).collect();
        if !parts.is_empty() {
            ui.indent(("topic-parts", i), |ui| {
                for part in parts {
                    if part_row(ui, part, p) {
                        clicked = Some(part.start);
                    }
                }
            });
            ui.add_space(4.0);
        }
        if !d.summary.trim().is_empty() {
            ui.add(egui::Label::new(RichText::new(d.summary.trim()).size(12.5)).wrap());
        }
        for (label, items) in [(t!("studio.decisions"), &d.decisions), (t!("studio.actions"), &d.actions), (t!("studio.questions"), &d.questions)] {
            // Empty strings are written while the outline is still being filled in.
            let items: Vec<&String> = items.iter().filter(|s| !s.trim().is_empty()).collect();
            if items.is_empty() {
                continue;
            }
            ui.add_space(4.0);
            ui.label(RichText::new(label).size(11.5).strong().color(p.text_weak));
            for item in items {
                ui.add(egui::Label::new(RichText::new(format!("•  {item}")).size(12.5)).wrap());
            }
        }
        let quotes: Vec<_> = d.quotes.iter().filter(|q| !q.text.trim().is_empty()).collect();
        if !quotes.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(t!("studio.quotes")).size(11.5).strong().color(p.text_weak));
            for q in quotes {
                ui.horizontal(|ui| {
                    let time = ui.add(egui::Button::new(RichText::new(human_duration(q.time)).font(FontId::monospace(11.0)).color(p.accent)).frame(false));
                    if time.on_hover_text(t!("studio.go_there")).clicked() {
                        player.seek(q.time);
                    }
                    ui.add(egui::Label::new(RichText::new(&q.text).size(12.5).italics()).wrap());
                });
            }
        }
        ui.add_space(2.0);
        clicked
    })
    .inner
}

/// One part of a long topic in its digest: «m:ss  title»; a click returns its start (the same
/// path as a topic click: the dialog scrolls to the first reply of the part).
fn part_row(ui: &mut egui::Ui, part: &Part, p: Palette) -> bool {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(4), p.card_hover);
    }
    let painter = painter_at(ui, rect);
    let font = FontId::monospace(11.0);
    let time = ui.fonts_mut(|f| f.layout_no_wrap(human_duration(part.start), font, p.accent));
    painter.galley(egui::pos2(rect.left() + 4.0, rect.center().y - time.size().y / 2.0), time.clone(), p.accent);
    let left = rect.left() + 4.0 + time.size().x + 8.0;
    let title = Rect::from_min_max(egui::pos2(left, rect.top()), rect.max);
    painter_at(ui, title).text(egui::pos2(left, rect.center().y), Align2::LEFT_CENTER, part.title.trim(), FontId::proportional(12.5), p.text);
    resp.clicked()
}

/// A painter clipped to `rect` (long texts are cut instead of running over the neighbours).
fn painter_at(ui: &egui::Ui, rect: Rect) -> egui::Painter {
    let mut painter = ui.painter_at(rect);
    painter.set_clip_rect(rect.intersect(ui.clip_rect()));
    painter
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_core::outline::Digest;

    fn chapter(start: f64, end: f64) -> Chapter {
        Chapter { start, end, digest: Digest { title: format!("тема {start}"), ..Digest::default() } }
    }

    #[test]
    fn the_current_topic_by_time() {
        let chapters = [chapter(0.0, 60.0), chapter(60.0, 120.0), chapter(120.0, 180.0)];
        assert_eq!(current_topic(&[], 10.0), None);
        assert_eq!(current_topic(&chapters, -1.0), Some(0), "before the first topic");
        assert_eq!(current_topic(&chapters, 0.0), Some(0));
        assert_eq!(current_topic(&chapters, 59.9), Some(0));
        assert_eq!(current_topic(&chapters, 60.0), Some(1), "the border belongs to the next topic");
        assert_eq!(current_topic(&chapters, 179.0), Some(2));
        assert_eq!(current_topic(&chapters, 500.0), Some(2), "after the last topic");
    }

    #[test]
    fn segments_have_two_pixel_gaps_and_keep_the_times() {
        let chapters = [chapter(0.0, 20.0), chapter(20.0, 50.0), chapter(50.0, 100.0)];
        let s = segments(10.0, 100.0, 100.0, &chapters);
        assert_eq!(s.len(), 3);
        assert_eq!((s[0].x0, s[0].x1), (10.0, 29.0), "20 % of 100 px, minus half a gap");
        assert_eq!((s[1].x0, s[1].x1), (31.0, 59.0));
        assert_eq!((s[2].x0, s[2].x1), (61.0, 110.0), "no gap at the outer edges");
        for w in s.windows(2) {
            assert!((w[1].x0 - w[0].x1 - SEGMENT_GAP).abs() < 1e-6, "{w:?}");
        }
    }

    #[test]
    fn segments_clip_to_the_bar_and_survive_odd_input() {
        assert!(segments(0.0, 100.0, 0.0, &[chapter(0.0, 10.0)]).is_empty(), "no duration");
        assert!(segments(0.0, 0.0, 100.0, &[chapter(0.0, 10.0)]).is_empty(), "no width");
        assert!(segments(0.0, 100.0, 100.0, &[]).is_empty());
        let s = segments(0.0, 100.0, 100.0, &[chapter(0.0, 200.0)]);
        assert_eq!((s[0].x0, s[0].x1), (0.0, 100.0), "an end past the recording stays on the bar");
        let s = segments(0.0, 100.0, 100.0, &[chapter(50.0, 50.0), chapter(50.0, 100.0)]);
        assert_eq!((s[0].x0, s[0].x1), (50.0, 50.0), "a zero-length topic does not invert");
        assert!((s[1].x0 - 51.0).abs() < 1e-6);
    }

    #[test]
    fn the_tooltip_is_title_time_and_the_first_line() {
        let c = Chapter {
            start: 40.0,
            end: 100.0,
            digest: Digest { title: "План".into(), summary: "Решили\nвторое".into(), ..Digest::default() },
        };
        assert_eq!(tooltip(&c, None), "План\n00:40–01:40\nРешили");
        let c = Chapter { start: 0.0, end: 10.0, digest: Digest { title: "Пусто".into(), summary: " \n".into(), ..Digest::default() } };
        assert_eq!(tooltip(&c, None), "Пусто\n00:00–00:10", "an empty summary is left out");
        let c = Chapter { start: 3600.0, end: 3725.0, digest: Digest { title: "Час".into(), ..Digest::default() } };
        assert_eq!(tooltip(&c, None), "Час\n1:00:00–1:02:05");
    }

    fn parts() -> Vec<Part> {
        vec![
            Part { start: 0.0, end: 30.0, title: "Начало".into() },
            Part { start: 30.0, end: 70.0, title: "Середина".into() },
            Part { start: 70.0, end: 100.0, title: "Конец".into() },
        ]
    }

    #[test]
    fn the_part_at_a_time() {
        let c = Chapter { start: 0.0, end: 100.0, digest: Digest { title: "Тема".into(), parts: parts(), ..Digest::default() } };
        assert_eq!(part_at(&c, -1.0), None, "before the parts");
        assert_eq!(part_at(&c, 0.0), Some(0));
        assert_eq!(part_at(&c, 29.9), Some(0));
        assert_eq!(part_at(&c, 30.0), Some(1), "the border belongs to the next part");
        assert_eq!(part_at(&c, 70.0), Some(2));
        assert_eq!(part_at(&c, 100.0), Some(2));
        assert_eq!(part_at(&c, 100.1), None, "after the parts");
        assert_eq!(part_at(&chapter(0.0, 100.0), 10.0), None, "a topic without parts");
        assert_eq!(part_at(&Chapter { digest: Digest { parts: parts(), ..Digest::default() }, ..chapter(0.0, 100.0) }, 50.0), Some(1), "no parts, no time");
    }

    #[test]
    fn part_ticks_stand_on_the_boundaries() {
        // 30/70 of a 100 s recording on a 100 px track starting at 10.
        assert_eq!(part_ticks(10.0, 100.0, 100.0, &parts()), vec![40.0, 80.0]);
        assert!(part_ticks(10.0, 100.0, 100.0, &parts()[..1]).is_empty(), "one part has no boundary");
        assert!(part_ticks(10.0, 100.0, 100.0, &[]).is_empty());
        assert!(part_ticks(10.0, 0.0, 100.0, &parts()).is_empty(), "no width");
        assert!(part_ticks(10.0, 100.0, 0.0, &parts()).is_empty(), "no duration");
        let over = vec![Part { start: 0.0, end: 50.0, title: String::new() }, Part { start: 150.0, end: 200.0, title: String::new() }];
        assert!(part_ticks(10.0, 100.0, 100.0, &over).is_empty(), "a boundary past the bar is dropped");
    }

    #[test]
    fn the_tooltip_names_the_part_under_the_pointer() {
        let c = Chapter {
            start: 40.0,
            end: 100.0,
            digest: Digest { title: "План".into(), summary: "Решили\nвторое".into(), parts: parts(), ..Digest::default() },
        };
        assert_eq!(tooltip(&c, c.digest.parts.get(1)), "План — Середина\n00:40–01:40\nРешили");
        let blank = Part { start: 0.0, end: 1.0, title: "  ".into() };
        assert_eq!(tooltip(&c, Some(&blank)), "План\n00:40–01:40\nРешили", "a part without a title is not named");
    }

    #[test]
    fn the_unplayed_colour_is_stronger_than_the_soft_one() {
        // The soft colour is nearly the panel background; the mix must stay clearly apart from
        // it, from the played accent and from the gap base, in both themes.
        for palette in [crate::theme::LIGHT, crate::theme::DARK] {
            let unplayed = mix(palette.accent_soft, palette.accent, UNPLAYED);
            let distance = |a: Color32, b: Color32| {
                (f32::from(a.r()) - f32::from(b.r())).abs() + (f32::from(a.g()) - f32::from(b.g())).abs() + (f32::from(a.b()) - f32::from(b.b())).abs()
            };
            assert!(distance(unplayed, palette.accent_soft) > 100.0, "unplayed vs accent_soft: {unplayed:?}");
            assert!(distance(unplayed, palette.border) > 60.0, "unplayed vs the gap base: {unplayed:?}");
            assert!(distance(unplayed, palette.accent) > 120.0, "unplayed vs played: {unplayed:?}");
        }
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 0.0), Color32::BLACK);
        assert_eq!(mix(Color32::BLACK, Color32::WHITE, 1.0), Color32::WHITE);
    }
}
