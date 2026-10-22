//! Chat answers as Markdown: one line of text is one row, so a list marker and a time link
//! sit on the text's line at the body size. The rows share one gap ([`LINE_GAP`]).
//!
//! [`layout`] decides the lines; [`show`] only draws them.

use super::topics;
use crate::theme;
use eframe::egui::{self, Color32, FontId, Stroke, TextFormat};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};

/// Gap between the lines of one answer. The only vertical spacing inside an answer's text.
pub const LINE_GAP: f32 = 2.0;
/// Body text, the same size as the rest of the chat.
const BODY: f32 = 13.0;
/// A heading is a little larger than the body; every heading uses this one size.
const HEADING: f32 = 15.0;

/// One run of text with one style.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub link: Option<String>,
    /// A speaker mention: the accent chip, same size as the text around it.
    pub mention: bool,
}

/// One row. A list item's marker is the start of this row, not a row of its own.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub spans: Vec<Span>,
    pub heading: Option<u8>,
}

struct List {
    /// Next number of an ordered list; `None` is a bullet list.
    next: Option<u64>,
}

struct Build {
    lines: Vec<Line>,
    cur: Line,
    lists: Vec<List>,
    /// The current row is a list item that has not taken its text yet.
    fresh_item: bool,
    strong: u32,
    emphasis: u32,
    code_block: bool,
    image: u32,
    link: Vec<String>,
    heading: Option<u8>,
}

impl Build {
    fn push(&mut self, text: &str, code: bool) {
        if text.is_empty() {
            return;
        }
        let strong = self.strong > 0 && !code;
        let emphasis = self.emphasis > 0 && !code;
        let link = (self.image == 0).then(|| self.link.last().cloned()).flatten();
        if let Some(last) = self.cur.spans.last_mut() {
            if last.strong == strong && last.emphasis == emphasis && last.code == code && last.link == link {
                last.text.push_str(text);
                return;
            }
        }
        self.cur.spans.push(Span { text: text.to_string(), strong, emphasis, code, link, mention: false });
        self.fresh_item = false;
    }

    fn push_text(&mut self, text: &str) {
        if !self.code_block {
            self.push(text, false);
            return;
        }
        for (i, line) in text.split('\n').enumerate() {
            if i > 0 {
                self.flush();
            }
            self.push(line, true);
        }
    }

    fn flush(&mut self) {
        let text: String = self.cur.spans.iter().map(|s| s.text.as_str()).collect();
        // A marker a list item never filled («1.», «•») is not a row.
        if text.trim().is_empty() || is_marker(text.trim()) {
            self.cur.spans.clear();
            return;
        }
        self.cur.heading = self.heading;
        self.lines.push(std::mem::take(&mut self.cur));
    }
}

/// `1.` or `•` with nothing after it.
fn is_marker(text: &str) -> bool {
    text == "•" || (text.ends_with('.') && text.len() > 1 && text[..text.len() - 1].chars().all(|c| c.is_ascii_digit()))
}

/// The lines of `markdown` (CommonMark). List markers share the item's line.
pub fn layout(markdown: &str) -> Vec<Line> {
    let mut b = Build {
        lines: Vec::new(),
        cur: Line::default(),
        lists: Vec::new(),
        fresh_item: false,
        strong: 0,
        emphasis: 0,
        code_block: false,
        image: 0,
        link: Vec::new(),
        heading: None,
    };
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Paragraph) => {
                if b.fresh_item {
                    b.fresh_item = false;
                } else {
                    b.flush();
                }
            }
            Event::End(TagEnd::Paragraph) | Event::End(TagEnd::Item) => b.flush(),
            Event::Start(Tag::Heading { level, .. }) => {
                b.flush();
                b.heading = Some(level as u8);
            }
            Event::End(TagEnd::Heading(_)) => {
                b.flush();
                b.heading = None;
            }
            Event::Start(Tag::List(start)) => b.lists.push(List { next: start }),
            Event::End(TagEnd::List(_)) => {
                b.lists.pop();
            }
            Event::Start(Tag::Item) => {
                b.flush();
                let marker = match b.lists.last_mut() {
                    Some(List { next: Some(n) }) => {
                        let marker = format!("{n}. ");
                        *n += 1;
                        marker
                    }
                    _ => "• ".to_string(),
                };
                b.push(&marker, false);
                b.fresh_item = true;
            }
            Event::Start(Tag::CodeBlock(_)) => {
                b.flush();
                b.code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                b.flush();
                b.code_block = false;
            }
            Event::Start(Tag::Strong) => b.strong += 1,
            Event::End(TagEnd::Strong) => b.strong = b.strong.saturating_sub(1),
            Event::Start(Tag::Emphasis) => b.emphasis += 1,
            Event::End(TagEnd::Emphasis) => b.emphasis = b.emphasis.saturating_sub(1),
            Event::Start(Tag::Link { dest_url, .. }) => b.link.push(dest_url.to_string()),
            Event::End(TagEnd::Link) => {
                b.link.pop();
            }
            Event::Start(Tag::Image { .. }) => b.image += 1,
            Event::End(TagEnd::Image) => b.image = b.image.saturating_sub(1),
            Event::Text(text) => b.push_text(&text),
            Event::Code(text) => b.push(&text, true),
            Event::SoftBreak => b.push(" ", false),
            Event::HardBreak => b.flush(),
            Event::Rule => {
                b.flush();
                b.push("—", false);
                b.flush();
            }
            _ => {}
        }
    }
    b.flush();
    b.lines
}

/// The link a character of `line` belongs to. `index` is a character offset; the character
/// just before it counts too, so a click on the end of a link still hits it.
fn link_at(line: &Line, index: usize) -> Option<&str> {
    let at = |index: usize| {
        let mut n = 0usize;
        for span in &line.spans {
            let len = span.text.chars().count();
            if index < n + len {
                return span.link.as_deref();
            }
            n += len;
        }
        None
    };
    at(index).or_else(|| index.checked_sub(1).and_then(at))
}

/// The spans as shown: `@sN` is the speaker's current name. A link stays a link.
fn shown_spans(spans: &[Span], roster: &super::mentions::Roster) -> Vec<Span> {
    let mut out = Vec::new();
    for span in spans {
        if span.mention {
            out.push(span.clone());
            continue;
        }
        for (text, mention) in roster.runs(&span.text) {
            if text.is_empty() {
                continue;
            }
            out.push(Span { text, mention, ..span.clone() });
        }
    }
    out
}

/// Draws `markdown`. A `seek:` link returns its target; an http link opens in the browser.
/// Speaker labels are the current names (`roster`).
pub fn show(ui: &mut egui::Ui, markdown: &str, roster: &super::mentions::Roster, p: theme::Palette) -> Option<String> {
    let lines = layout(markdown);
    if lines.is_empty() {
        return None;
    }
    let mut seek = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = LINE_GAP;
        let width = ui.available_width();
        for line in &lines {
            let size = if line.heading.is_some() { HEADING } else { BODY };
            let spans = shown_spans(&line.spans, roster);
            let shown = Line { spans, heading: line.heading };
            let mut job = egui::text::LayoutJob::default();
            job.wrap.max_width = width;
            for span in &shown.spans {
                let color = if span.mention || span.link.is_some() {
                    p.accent
                } else if span.code {
                    p.text_weak
                } else if span.strong {
                    ui.visuals().strong_text_color()
                } else {
                    p.text
                };
                job.append(
                    &span.text,
                    0.0,
                    TextFormat {
                        font_id: FontId::proportional(size),
                        color,
                        italics: span.emphasis,
                        underline: if span.link.is_some() { Stroke::new(1.0, p.accent) } else { Stroke::NONE },
                        background: if span.mention { p.accent_soft } else { Color32::TRANSPARENT },
                        ..TextFormat::default()
                    },
                );
            }
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
            let resp = ui.add(egui::Label::new(galley.clone()).sense(egui::Sense::click()));
            let hit = resp.interact_pointer_pos().or(resp.hover_pos()).map(|pos| galley.cursor_from_pos(pos - resp.rect.min));
            if let Some(cursor) = hit {
                if let Some(url) = link_at(&shown, cursor.index.0) {
                    if resp.hovered() {
                        resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
                        if url.starts_with("http://") || url.starts_with("https://") {
                            topics::text_tip(&resp, url, roster);
                        }
                    }
                    if resp.clicked() {
                        if let Some(target) = url.strip_prefix("seek:") {
                            seek = Some(target.to_string());
                        } else if url.starts_with("http://") || url.starts_with("https://") {
                            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                        }
                    }
                }
            }
        }
    });
    seek
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(line: &Line) -> String {
        line.spans.iter().map(|s| s.text.as_str()).collect()
    }

    #[test]
    fn a_list_marker_shares_its_line_with_the_text_and_a_time() {
        let lines = layout("1. В пятницу [1:05](seek:65) отчёт.\n2. Бюджет.");
        assert_eq!(lines.len(), 2, "a marker is not a line of its own: {lines:?}");
        assert_eq!(text(&lines[0]), "1. В пятницу 1:05 отчёт.");
        assert_eq!(text(&lines[1]), "2. Бюджет.");
        let time = lines[0].spans.iter().find(|s| s.text == "1:05").expect("the time");
        assert_eq!(time.link.as_deref(), Some("seek:65"));
        assert!(!time.code && !time.strong, "a time is an ordinary link, not a chip");
        assert!(lines.iter().all(|l| !is_marker(text(l).trim())), "no marker-only line");

        let bullets = layout("- один\n- два");
        assert_eq!(bullets.iter().map(text).collect::<Vec<_>>(), ["• один", "• два"]);
    }

    #[test]
    fn headings_bold_and_links_are_spans_of_the_line() {
        let lines = layout("# Заголовок\n\nэто **жирное** и *курсив*, см. [сайт](https://example.org/a).");
        assert_eq!(lines[0].heading, Some(1));
        assert_eq!(text(&lines[0]), "Заголовок");
        assert_eq!(lines[1].heading, None);
        assert!(lines[1].spans.iter().any(|s| s.strong && s.text == "жирное"));
        assert!(lines[1].spans.iter().any(|s| s.emphasis && s.text == "курсив"));
        let link = lines[1].spans.iter().find(|s| s.text == "сайт").unwrap();
        assert_eq!(link.link.as_deref(), Some("https://example.org/a"));
        assert!(!lines[1].spans.iter().any(|s| s.code));
    }

    #[test]
    fn a_time_inside_a_phrase_is_a_link_on_that_line() {
        let lines = layout("В пятницу [1:05](seek:65) отчёт.");
        assert_eq!(lines.len(), 1);
        assert_eq!(text(&lines[0]), "В пятницу 1:05 отчёт.");
        let time = lines[0].spans.iter().find(|s| s.link.is_some()).unwrap();
        assert_eq!((time.text.as_str(), time.link.as_deref(), time.code), ("1:05", Some("seek:65"), false));
    }
}
