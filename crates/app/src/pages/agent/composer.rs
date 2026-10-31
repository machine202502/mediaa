use eframe::egui;
use mediaa_agent::assistant::quote;
use mediaa_agent::tools::{format_time, parse_time};
use mediaa_core::speakers::{self, Query, Who};
use std::collections::HashMap;

use super::mentions::Roster;

const PREVIEW: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Mention {
        label: String,
    },
    Quote {
        label: String,
        start: String,
        end: String,
        text: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Clock {
    pub start: usize,
    pub end: usize,
    pub seek: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    pub label: String,
    pub shown: String,
    pub start: String,
    pub end: String,
    pub text: String,
}

impl Quote {
    pub fn from_reply(
        speaker: Option<u32>,
        shown: &str,
        start: f64,
        end: f64,
        text: &str,
    ) -> Quote {
        Quote {
            label: speaker.map(speakers::label).unwrap_or_default(),
            shown: shown.to_string(),
            start: format_time(start),
            end: format_time(end),
            text: text.to_string(),
        }
    }
}

pub fn quote_id(path: &str) -> egui::Id {
    egui::Id::new(("mediaa-quote", path))
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Field {
    pub text: String,
    pub chips: Vec<Chip>,
}

impl Field {
    pub fn set_plain(&mut self, text: &str) {
        self.text = text.to_string();
        self.chips.clear();
    }

    pub fn to_message(&self) -> String {
        let mut text = self.text.clone();
        for chip in self.chips.iter().rev() {
            if chip.start > chip.end || chip.end > text.len() {
                continue;
            }
            text.replace_range(chip.start..chip.end, &stored(chip));
        }
        text
    }

    pub fn relabel(&mut self, shown: impl Fn(&str) -> String) {
        let mut text = String::new();
        let mut at = 0;
        let mut chips = Vec::new();
        for chip in &self.chips {
            if chip.start < at || chip.end > self.text.len() || chip.start > chip.end {
                continue;
            }
            text.push_str(&self.text[at..chip.start]);
            let display = display_of(chip, &shown);
            let start = text.len();
            text.push_str(&display);
            chips.push(Chip {
                start,
                end: text.len(),
                kind: chip.kind.clone(),
            });
            at = chip.end;
        }
        if at <= self.text.len() {
            text.push_str(&self.text[at..]);
        }
        self.text = text;
        self.chips = chips;
    }

    pub fn reconcile(&mut self, new_text: String) -> Option<usize> {
        if new_text == self.text {
            return None;
        }
        let (old_lo, old_hi, new_lo, new_hi) = edit_bounds(&self.text, &new_text);
        let delta = new_text.len() as isize - self.text.len() as isize;
        let mut remove = Vec::new();
        let mut keep = Vec::new();
        for chip in self.chips.drain(..) {
            let touched = chip.start < old_hi && chip.end > old_lo;
            if touched {
                let from = if chip.start < old_lo {
                    chip.start
                } else {
                    new_lo
                };
                let to = if chip.end > old_hi {
                    new_hi + chip.end - old_hi
                } else {
                    new_hi
                };
                if from < to {
                    remove.push((from, to));
                }
                continue;
            }
            if chip.start >= old_hi {
                keep.push(Chip {
                    start: (chip.start as isize + delta) as usize,
                    end: (chip.end as isize + delta) as usize,
                    kind: chip.kind,
                });
            } else {
                keep.push(chip);
            }
        }
        let mut text = new_text;
        remove.sort_by_key(|r| r.0);
        let mut merged: Vec<(usize, usize)> = Vec::new();
        for (start, end) in remove {
            if let Some(last) = merged.last_mut() {
                if start <= last.1 {
                    last.1 = last.1.max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }
        let mut caret = None;
        for (start, end) in merged.iter().rev() {
            let start = (*start).min(text.len());
            let end = (*end).min(text.len());
            if start < end {
                text.replace_range(start..end, "");
                caret = Some(start);
            }
            let cut = end - start;
            for chip in &mut keep {
                if chip.start >= end {
                    chip.start -= cut;
                    chip.end -= cut;
                }
            }
        }
        keep.retain(|c| {
            c.start < c.end
                && c.end <= text.len()
                && text.is_char_boundary(c.start)
                && text.is_char_boundary(c.end)
        });
        keep.sort_by_key(|c| c.start);
        self.text = text;
        self.chips = keep;
        caret
    }

    pub fn backspace_at(&mut self, cursor: usize) -> bool {
        let Some(i) = self.chips.iter().position(|c| c.end == cursor) else {
            return false;
        };
        self.remove_chip(i);
        true
    }

    pub fn delete_at(&mut self, cursor: usize) -> bool {
        let Some(i) = self.chips.iter().position(|c| c.start == cursor) else {
            return false;
        };
        self.remove_chip(i);
        true
    }

    pub fn arrow(&self, cursor: usize, right: bool) -> Option<usize> {
        self.chips.iter().find_map(|c| {
            if right && cursor == c.start {
                Some(c.end)
            } else if !right && cursor == c.end {
                Some(c.start)
            } else {
                None
            }
        })
    }

    pub fn snap(&self, cursor: usize) -> usize {
        for c in &self.chips {
            if cursor > c.start && cursor < c.end {
                let mid = c.start + (c.end - c.start) / 2;
                return if cursor >= mid { c.end } else { c.start };
            }
        }
        cursor
    }

    pub fn covers(&self, byte: usize) -> bool {
        self.chips.iter().any(|c| byte > c.start && byte < c.end)
    }

    pub fn mention_query(&self, cursor: usize) -> Option<Query> {
        if self.covers(cursor) {
            return None;
        }
        let query = speakers::mention_query(&self.text, cursor)?;
        if self
            .chips
            .iter()
            .any(|c| query.at >= c.start && query.at < c.end)
        {
            return None;
        }
        Some(query)
    }

    pub fn insert_mention(&mut self, at: usize, query: &str, label: &str, shown: &str) {
        let end = (at + '@'.len_utf8() + query.len()).min(self.text.len());
        let body = format!("@{shown}");
        let insert = format!("{body} ");
        self.splice(
            at,
            end,
            &insert,
            at,
            at + body.len(),
            Kind::Mention {
                label: label.to_string(),
            },
        );
    }

    pub fn name_mentions(&mut self, roster: &Roster) {
        let mut names: HashMap<String, Option<&Who>> = HashMap::new();
        for who in &roster.speakers {
            let name = who.name.trim();
            if name.is_empty() {
                continue;
            }
            names
                .entry(key(name))
                .and_modify(|w| *w = None)
                .or_insert(Some(who));
        }
        let mut found = Vec::new();
        for (lo, hi) in self.plain_segments() {
            for (at, word) in words(&self.text[lo..hi]) {
                let start = lo + at;
                if self.text[..start].ends_with('@') {
                    continue;
                }
                let Some(Some(who)) = names.get(&key(word)) else {
                    continue;
                };
                found.push((start, start + word.len(), *who));
            }
        }
        for (from, to, who) in found.into_iter().rev() {
            let label = match who.recording {
                Some(recording) => speakers::folder_label(who.index, recording),
                None => speakers::label(who.index),
            };
            let shown = Roster::row_label(who);
            let insert = format!("@{shown}");
            self.splice(
                from,
                to,
                &insert,
                from,
                from + insert.len(),
                Kind::Mention { label },
            );
        }
    }

    fn plain_segments(&self) -> Vec<(usize, usize)> {
        let mut chips: Vec<(usize, usize)> = self
            .chips
            .iter()
            .filter(|c| c.start < c.end && c.end <= self.text.len())
            .map(|c| (c.start, c.end))
            .collect();
        chips.sort_unstable();
        let mut out = Vec::new();
        let mut at = 0;
        for (start, end) in chips {
            if start > at {
                out.push((at, start));
            }
            at = at.max(end);
        }
        if at < self.text.len() {
            out.push((at, self.text.len()));
        }
        out
    }

    pub fn insert_quote(&mut self, at: usize, quote: &Quote) {
        let at = floor(self.text.len().min(at), &self.text);
        let body = quote_text(&quote.shown, &quote.start, &quote.end, &quote.text);
        let gap = if at > 0 && !self.text[..at].ends_with(char::is_whitespace) {
            " "
        } else {
            ""
        };
        let insert = format!("{gap}{body} ");
        let start = at + gap.len();
        self.splice(
            at,
            at,
            &insert,
            start,
            start + body.len(),
            Kind::Quote {
                label: quote.label.clone(),
                start: quote.start.clone(),
                end: quote.end.clone(),
                text: quote.text.clone(),
            },
        );
    }

    pub fn clocks(&self) -> Vec<Clock> {
        clocks_outside(
            &self.text,
            &self
                .chips
                .iter()
                .map(|c| (c.start, c.end))
                .collect::<Vec<_>>(),
        )
    }

    pub fn seek_at(&self, byte: usize) -> Option<f64> {
        if self.covers(byte) {
            return None;
        }
        self.clocks()
            .into_iter()
            .find(|c| byte >= c.start && byte < c.end)
            .map(|c| c.seek)
    }

    fn remove_chip(&mut self, index: usize) {
        let chip = self.chips.remove(index);
        if chip.start < chip.end && chip.end <= self.text.len() {
            self.text.replace_range(chip.start..chip.end, "");
        }
        let cut = chip.end.saturating_sub(chip.start);
        for c in &mut self.chips {
            if c.start >= chip.end {
                c.start -= cut;
                c.end -= cut;
            }
        }
    }

    fn splice(
        &mut self,
        from: usize,
        to: usize,
        insert: &str,
        chip_start: usize,
        chip_end: usize,
        kind: Kind,
    ) {
        let from = from.min(self.text.len());
        let to = to.min(self.text.len()).max(from);
        self.text.replace_range(from..to, insert);
        let delta = insert.len() as isize - (to - from) as isize;
        let mut chips = Vec::new();
        for c in self.chips.drain(..) {
            if c.end <= from {
                chips.push(c);
            } else if c.start >= to {
                chips.push(Chip {
                    start: (c.start as isize + delta) as usize,
                    end: (c.end as isize + delta) as usize,
                    kind: c.kind,
                });
            }
        }
        if chip_start < chip_end && chip_end <= self.text.len() {
            chips.push(Chip {
                start: chip_start,
                end: chip_end,
                kind,
            });
        }
        chips.sort_by_key(|c| c.start);
        self.chips = chips;
    }
}

fn stored(chip: &Chip) -> String {
    match &chip.kind {
        Kind::Mention { label } => label.clone(),
        Kind::Quote {
            label,
            start,
            end,
            text,
        } => quote::token(label, start, end, text),
    }
}

fn display_of(chip: &Chip, shown: &impl Fn(&str) -> String) -> String {
    match &chip.kind {
        Kind::Mention { label } => format!("@{}", shown(label)),
        Kind::Quote {
            label,
            start,
            end,
            text,
        } => quote_text(&shown(label), start, end, text),
    }
}

fn quote_text(shown: &str, start: &str, end: &str, text: &str) -> String {
    let who = if shown.is_empty() {
        String::new()
    } else {
        format!("@{shown} ")
    };
    format!("{who}{start}–{end} {}", preview(text))
}

fn preview(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let count = flat.chars().count();
    let body: String = flat.chars().take(PREVIEW).collect();
    if count > PREVIEW {
        format!("«{body}…»")
    } else {
        format!("«{body}»")
    }
}

fn key(name: &str) -> String {
    name.to_lowercase().replace('ё', "е")
}

fn words(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut start = None;
    for (i, ch) in text.char_indices() {
        if ch.is_alphanumeric() || ch == '_' {
            start.get_or_insert(i);
        } else if let Some(s) = start.take() {
            out.push((s, &text[s..i]));
        }
    }
    if let Some(s) = start {
        out.push((s, &text[s..]));
    }
    out
}

fn floor(at: usize, text: &str) -> usize {
    (0..=at.min(text.len()))
        .rev()
        .find(|i| text.is_char_boundary(*i))
        .unwrap_or(0)
}

fn edit_bounds(old: &str, new: &str) -> (usize, usize, usize, usize) {
    let raw = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    let prefix = floor(raw, old).min(floor(raw, new));
    let suffix = old[prefix..]
        .bytes()
        .rev()
        .zip(new[prefix..].bytes().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old_hi = floor(old.len() - suffix, old).max(prefix);
    let new_hi = floor(new.len() - suffix, new).max(prefix);
    (prefix, old_hi, prefix, new_hi)
}

pub fn clocks_in(text: &str) -> Vec<Clock> {
    clocks_outside(text, &[])
}

fn clocks_outside(text: &str, skip: &[(usize, usize)]) -> Vec<Clock> {
    let mut out = Vec::new();
    let mut i = 0;
    let bytes = text.as_bytes();
    while i < bytes.len() {
        if skip.iter().any(|&(a, b)| i >= a && i < b) {
            i += 1;
            continue;
        }
        if bytes[i] == b'[' {
            if let Some(rel) = text[i + 1..].find(']') {
                let close = i + 1 + rel;
                let end = close + 1;
                let inside = !skip.iter().any(|&(a, b)| i < b && end > a);
                if inside {
                    if let Some(seek) = span_start(&text[i + 1..close]) {
                        out.push(Clock {
                            start: i,
                            end,
                            seek,
                        });
                    }
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}

fn span_start(inner: &str) -> Option<f64> {
    if inner.is_empty() || inner.chars().any(char::is_whitespace) {
        return None;
    }
    let (left, right) = split_dash(inner);
    let start = clock(left)?;
    if let Some(right) = right {
        clock(right)?;
    }
    Some(start)
}

fn split_dash(inner: &str) -> (&str, Option<&str>) {
    for dash in ['–', '—', '-'] {
        if let Some((a, b)) = inner.split_once(dash) {
            return (a, Some(b));
        }
    }
    (inner, None)
}

fn clock(text: &str) -> Option<f64> {
    if !text.contains(':') {
        return None;
    }
    parse_time(text)
}

#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Text(String),
    Mention(String),
    Quote(String),
    Clock { text: String, seek: f64 },
}

pub fn pieces(stored: &str, name: impl Fn(&str) -> Option<String>) -> Vec<Piece> {
    let mut out = Vec::new();
    let mut rest = stored;
    let mut plain = String::new();
    let flush = |plain: &mut String, out: &mut Vec<Piece>| {
        if plain.is_empty() {
            return;
        }
        push_plain(plain, out, &name);
        plain.clear();
    };
    while !rest.is_empty() {
        if let Some(i) = rest.find('\u{E000}') {
            plain.push_str(&rest[..i]);
            let after = &rest[i + '\u{E000}'.len_utf8()..];
            let Some(j) = after.find('\u{E001}') else {
                plain.push_str(&rest[i..]);
                break;
            };
            flush(&mut plain, &mut out);
            if let Some(text) = quote_piece(&after[..j], &name) {
                out.push(Piece::Quote(text));
            } else {
                plain.push('\u{E000}');
                plain.push_str(&after[..j]);
                plain.push('\u{E001}');
            }
            rest = &after[j + '\u{E001}'.len_utf8()..];
        } else {
            plain.push_str(rest);
            break;
        }
    }
    flush(&mut plain, &mut out);
    out
}

fn quote_piece(body: &str, name: &impl Fn(&str) -> Option<String>) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let label = v.get("label")?.as_str()?;
    let start = v.get("start")?.as_str()?;
    let end = v.get("end")?.as_str()?;
    let text = v.get("text")?.as_str()?;
    let shown = name(label).unwrap_or_default();
    Some(quote_text(&shown, start, end, text))
}

fn push_plain(text: &str, out: &mut Vec<Piece>, name: &impl Fn(&str) -> Option<String>) {
    let clocks = clocks_in(text);
    let mut at = 0;
    for clock in clocks {
        if clock.start > at {
            push_mentions(&text[at..clock.start], out, name);
        }
        out.push(Piece::Clock {
            text: text[clock.start..clock.end].to_string(),
            seek: clock.seek,
        });
        at = clock.end;
    }
    if at < text.len() {
        push_mentions(&text[at..], out, name);
    }
}

fn push_mentions(text: &str, out: &mut Vec<Piece>, name: &impl Fn(&str) -> Option<String>) {
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'@' {
            if let Some(end) = label_end(text, i) {
                if let Some(shown) = name(&text[i..end]) {
                    if i > at {
                        out.push(Piece::Text(text[at..i].to_string()));
                    }
                    out.push(Piece::Mention(format!("@{shown}")));
                    i = end;
                    at = i;
                    continue;
                }
            }
        }
        i += 1;
    }
    if at < text.len() {
        out.push(Piece::Text(text[at..].to_string()));
    }
}

fn label_end(text: &str, at: usize) -> Option<usize> {
    let rest = text.get(at + 1..)?;
    let b = rest.as_bytes();
    if b.first() != Some(&b's') {
        return None;
    }
    let mut j = 1;
    if j >= b.len() || !b[j].is_ascii_digit() {
        return None;
    }
    while j < b.len() && b[j].is_ascii_digit() {
        j += 1;
    }
    if b.get(j) == Some(&b'#') {
        let mut k = j + 1;
        if k >= b.len() || !b[k].is_ascii_digit() {
            return None;
        }
        while k < b.len() && b[k].is_ascii_digit() {
            k += 1;
        }
        j = k;
    }
    let end = at + 1 + j;
    let boundary = text[end..]
        .chars()
        .next()
        .is_none_or(|c| !c.is_alphanumeric() && c != '_');
    if !boundary || speakers::parse_label(&text[at..end]).is_none() {
        return None;
    }
    Some(end)
}

#[allow(
    clippy::too_many_arguments,
    reason = "explicit parameters of one UI call"
)]
pub fn layout(
    ui: &egui::Ui,
    text: &str,
    chips: &[Chip],
    clocks: &[Clock],
    wrap: f32,
    text_color: egui::Color32,
    accent: egui::Color32,
    soft: egui::Color32,
) -> std::sync::Arc<egui::Galley> {
    let font = ui
        .style()
        .text_styles
        .get(&egui::TextStyle::Body)
        .cloned()
        .unwrap_or_else(|| egui::FontId::proportional(14.0));
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap.max(0.0);
    paint(
        &mut job,
        text,
        chips.iter().map(|c| (c.start, c.end, true)),
        clocks,
        font,
        text_color,
        accent,
        soft,
    );
    ui.fonts_mut(|f| f.layout_job(job))
}

pub fn show_user(
    ui: &mut egui::Ui,
    stored: &str,
    name: impl Fn(&str) -> Option<String>,
    font: egui::FontId,
    text_color: egui::Color32,
    accent: egui::Color32,
    soft: egui::Color32,
) -> Option<f64> {
    let parts = pieces(stored, name);
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width().max(0.0);
    let mut clocks = Vec::new();
    let base = egui::TextFormat {
        font_id: font.clone(),
        color: text_color,
        ..Default::default()
    };
    let chip = egui::TextFormat {
        font_id: font.clone(),
        color: accent,
        background: soft,
        ..Default::default()
    };
    let time = egui::TextFormat {
        font_id: font,
        color: accent,
        ..Default::default()
    };
    for part in &parts {
        match part {
            Piece::Text(s) => job.append(s, 0.0, base.clone()),
            Piece::Mention(s) | Piece::Quote(s) => job.append(s, 0.0, chip.clone()),
            Piece::Clock { text, seek } => {
                let start = job.text.len();
                job.append(text, 0.0, time.clone());
                clocks.push((start, job.text.len(), *seek));
            }
        }
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let resp = ui.add(egui::Label::new(galley.clone()).sense(egui::Sense::click()));
    let seek = |pos: egui::Pos2| -> Option<f64> {
        let cursor = galley.cursor_from_pos(pos - resp.rect.min);
        let text = galley.job.text.as_str();
        let at = text
            .char_indices()
            .nth(cursor.index.0)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        clocks
            .iter()
            .find(|(a, b, _)| at >= *a && at < *b)
            .map(|(_, _, seek)| *seek)
    };
    if let Some(pos) = resp.hover_pos() {
        if seek(pos).is_some() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
    }
    resp.clicked()
        .then(|| resp.interact_pointer_pos().and_then(seek))
        .flatten()
}

#[allow(
    clippy::too_many_arguments,
    reason = "explicit parameters of one UI call"
)]
fn paint(
    job: &mut egui::text::LayoutJob,
    text: &str,
    chips: impl Iterator<Item = (usize, usize, bool)>,
    clocks: &[Clock],
    font: egui::FontId,
    text_color: egui::Color32,
    accent: egui::Color32,
    soft: egui::Color32,
) {
    let mut marks: Vec<(usize, usize, bool)> = chips.map(|(a, b, _)| (a, b, true)).collect();
    for c in clocks {
        marks.push((c.start, c.end, false));
    }
    marks.sort_by_key(|m| m.0);
    let plain = egui::TextFormat {
        font_id: font.clone(),
        color: text_color,
        ..Default::default()
    };
    let chip = egui::TextFormat {
        font_id: font.clone(),
        color: accent,
        background: soft,
        ..Default::default()
    };
    let time = egui::TextFormat {
        font_id: font,
        color: accent,
        ..Default::default()
    };
    let mut at = 0;
    for (start, end, is_chip) in marks {
        if start < at || end > text.len() || start > end {
            continue;
        }
        if start > at {
            job.append(&text[at..start], 0.0, plain.clone());
        }
        job.append(
            &text[start..end],
            0.0,
            if is_chip { chip.clone() } else { time.clone() },
        );
        at = end;
    }
    if at < text.len() {
        job.append(&text[at..], 0.0, plain);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(label: &str) -> Option<String> {
        match label {
            "@s1" => Some("Анна".into()),
            "@s2" => Some("Говорящий 2".into()),
            _ => None,
        }
    }

    #[test]
    fn moments_spans_hours_and_invalid_brackets() {
        let clocks =
            clocks_in("в [1:40] и [0:00-30:00] и [0:00–30:00] и [1:05:30], не [99:99] и не [1:60]");
        assert_eq!(
            clocks.iter().map(|c| c.seek).collect::<Vec<_>>(),
            vec![100.0, 0.0, 0.0, 3930.0]
        );
        assert!(clocks_in("[99:99]").is_empty());
        assert!(clocks_in("не [5] и не [см.]").is_empty());
    }

    #[test]
    fn a_mention_and_a_quote_are_one_piece_each() {
        let mut field = Field::default();
        field.set_plain("см. @ан");
        let at = field.text.find('@').unwrap();
        field.insert_mention(at, "ан", "@s1", "Анна");
        assert_eq!(field.text, "см. @Анна ");
        assert_eq!(field.chips.len(), 1);
        assert_eq!(field.to_message(), "см. @s1 ");
        assert!(field.covers(field.chips[0].start + 1));
        assert_eq!(field.snap(field.chips[0].start + 1), field.chips[0].start);
        field.set_plain("@нет");
        assert!(field.chips.is_empty(), "only the list inserts a mention");
        assert!(field.mention_query(field.text.len()).is_some());
        assert!(clocks_in(&field.text).is_empty());

        field.set_plain("см. @ан");
        let at = field.text.find('@').unwrap();
        field.insert_mention(at, "ан", "@s1", "Анна");
        let end = field.chips[0].end;
        assert!(field.backspace_at(end));
        assert!(field.chips.is_empty());
        assert_eq!(field.text, "см.  ");

        field.set_plain("");
        field.insert_mention(0, "", "@s1", "Анна");
        let new = field.text.replacen('н', "", 1);
        field.reconcile(new);
        assert!(
            field.chips.is_empty(),
            "an edit inside the chip removes it whole"
        );
        assert!(
            !field.text.contains('А') && !field.text.contains('н'),
            "{}",
            field.text
        );

        let quote = Quote::from_reply(
            Some(0),
            "Анна",
            100.0,
            149.0,
            "Отчёт в пятницу утром обязательно",
        );
        field.set_plain("а ");
        field.insert_quote(field.text.len(), &quote);
        assert_eq!(field.chips.len(), 1);
        assert!(field.text.contains("@Анна 1:40–2:29 «"));
        let stored = field.to_message();
        assert!(stored.contains('\u{E000}') && !stored.contains("Анна"));
        let end = field.chips[0].end;
        assert_eq!(field.arrow(field.chips[0].start, true), Some(end));
        assert!(field.delete_at(field.chips[0].start));
        assert!(field.chips.is_empty());

        field.set_plain("");
        field.insert_quote(0, &quote);
        field.relabel(|label| {
            if label == "@s1" {
                "Михаил".into()
            } else {
                label.trim_start_matches('@').into()
            }
        });
        assert!(field.text.starts_with("@Михаил 1:40–2:29 "));
        assert!(
            field.to_message().contains("@s1"),
            "the stored label does not follow the rename"
        );
    }

    #[test]
    fn a_stored_message_shows_names_quotes_and_clocks() {
        let token = quote::token("@s1", "1:40", "2:29", "Отчёт");
        let text = format!("скажи {token} про [1:40] и @s2, не @нет");
        let parts = pieces(&text, names);
        assert!(parts
            .iter()
            .any(|p| matches!(p, Piece::Quote(s) if s.starts_with("@Анна 1:40–2:29 «Отчёт»"))));
        assert!(parts
            .iter()
            .any(|p| matches!(p, Piece::Clock { seek, .. } if *seek == 100.0)));
        assert!(parts
            .iter()
            .any(|p| matches!(p, Piece::Mention(s) if s == "@Говорящий 2")));
        assert!(parts
            .iter()
            .any(|p| matches!(p, Piece::Text(s) if s.contains("@нет"))));
    }

    fn roster(speakers: &[(u32, Option<u32>, &str)]) -> Roster {
        Roster {
            speakers: speakers
                .iter()
                .map(|&(index, recording, name)| Who {
                    index,
                    recording,
                    name: name.to_string(),
                    title: "Планёрка".to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn a_word_equal_to_a_name_becomes_a_mention() {
        let one = roster(&[(0, None, "Анна")]);
        let mut field = Field::default();
        field.set_plain("Анна, что решили?");
        field.name_mentions(&one);
        assert_eq!(field.text, "@Анна, что решили?");
        assert_eq!(field.to_message(), "@s1, что решили?");
        assert_eq!(field.chips.len(), 1);
        field.name_mentions(&one);
        assert_eq!(field.chips.len(), 1, "a mention is not converted twice");

        field.set_plain("анна и Анна");
        field.name_mentions(&one);
        assert_eq!(field.to_message(), "@s1 и @s1", "case does not matter");

        field.set_plain("Анне и Аннами");
        field.name_mentions(&one);
        assert!(field.chips.is_empty(), "an inflected form is not a name");
        assert_eq!(field.to_message(), "Анне и Аннами");

        field.set_plain("@Анна, привет");
        field.name_mentions(&one);
        assert!(field.chips.is_empty(), "an @word stays text");
    }

    #[test]
    fn a_name_two_speakers_share_stays_text() {
        let mut field = Field::default();
        field.set_plain("Анна, что решили?");
        field.name_mentions(&roster(&[(0, None, "Анна"), (1, None, "Анна")]));
        assert!(field.chips.is_empty());
        assert_eq!(field.to_message(), "Анна, что решили?");
    }

    #[test]
    fn a_folder_name_mentions_its_recording() {
        let mut field = Field::default();
        field.set_plain("Анна, что решили?");
        field.name_mentions(&roster(&[(0, Some(2), "Анна")]));
        assert_eq!(field.text, "@Анна (Планёрка), что решили?");
        assert_eq!(field.to_message(), "@s1#2, что решили?");
    }
}
