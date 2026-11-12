use crate::mentions::Roster;
use crate::ui::theme;
use eframe::egui::{self, Color32, FontId, Stroke, TextFormat};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};

pub const LINE_GAP: f32 = 2.0;
const BODY: f32 = 13.0;
const HEADING: f32 = 15.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub strong: bool,
    pub emphasis: bool,
    pub code: bool,
    pub link: Option<String>,
    pub mention: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Line {
    pub spans: Vec<Span>,
    pub heading: Option<u8>,
}

struct List {
    next: Option<u64>,
}

struct Build {
    lines: Vec<Line>,
    cur: Line,
    lists: Vec<List>,
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
        let link = (self.image == 0)
            .then(|| self.link.last().cloned())
            .flatten();
        if let Some(last) = self.cur.spans.last_mut() {
            if last.strong == strong
                && last.emphasis == emphasis
                && last.code == code
                && last.link == link
            {
                last.text.push_str(text);
                return;
            }
        }
        self.cur.spans.push(Span {
            text: text.to_string(),
            strong,
            emphasis,
            code,
            link,
            mention: false,
        });
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
        if text.trim().is_empty() || is_marker(text.trim()) {
            self.cur.spans.clear();
            return;
        }
        self.cur.heading = self.heading;
        self.lines.push(std::mem::take(&mut self.cur));
    }
}

fn is_marker(text: &str) -> bool {
    text == "•"
        || (text.ends_with('.')
            && text.len() > 1
            && text[..text.len() - 1].chars().all(|c| c.is_ascii_digit()))
}

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

fn shown_spans(spans: &[Span], roster: &Roster) -> Vec<Span> {
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
            out.push(Span {
                text,
                mention,
                ..span.clone()
            });
        }
    }
    out
}

pub fn show(
    ui: &mut egui::Ui,
    markdown: &str,
    roster: &Roster,
    p: theme::Palette,
) -> Option<String> {
    let lines = layout(markdown);
    if lines.is_empty() {
        return None;
    }
    let mut seek = None;
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = LINE_GAP;
        let width = ui.available_width();
        for line in &lines {
            let size = if line.heading.is_some() {
                HEADING
            } else {
                BODY
            };
            let spans = shown_spans(&line.spans, roster);
            let shown = Line {
                spans,
                heading: line.heading,
            };
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
                        underline: if span.link.is_some() {
                            Stroke::new(1.0, p.accent)
                        } else {
                            Stroke::NONE
                        },
                        background: if span.mention {
                            p.accent_soft
                        } else {
                            Color32::TRANSPARENT
                        },
                        ..TextFormat::default()
                    },
                );
            }
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
            let resp = ui.add(egui::Label::new(galley.clone()).sense(egui::Sense::click()));
            let hit = resp
                .interact_pointer_pos()
                .or(resp.hover_pos())
                .map(|pos| galley.cursor_from_pos(pos - resp.rect.min));
            if let Some(cursor) = hit {
                if let Some(url) = link_at(&shown, cursor.index.0) {
                    if resp.hovered() {
                        resp.clone().on_hover_cursor(egui::CursorIcon::PointingHand);
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

pub fn parse_time(text: &str) -> Option<f64> {
    let t = text
        .trim()
        .trim_matches(|c| matches!(c, '[' | ']' | '@' | ' '))
        .replace(',', ".");
    if t.is_empty() {
        return None;
    }
    let parts: Vec<&str> = t.split(':').collect();
    if parts.len() > 3 {
        return None;
    }
    let mut total = 0.0;
    for (i, part) in parts.iter().enumerate() {
        let v: f64 = part
            .trim()
            .parse()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        if i > 0 && v >= 60.0 {
            return None;
        }
        total = total * 60.0 + v;
    }
    Some(total)
}

fn link_bare(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let starts = chars[i].is_ascii_digit()
            && (i == 0
                || !(chars[i - 1].is_alphanumeric() || matches!(chars[i - 1], ':' | '.' | ',')));
        if starts {
            let mut j = i;
            while j < chars.len() && (chars[j].is_ascii_digit() || chars[j] == ':') {
                j += 1;
            }
            let token: String = chars[i..j]
                .iter()
                .collect::<String>()
                .trim_end_matches(':')
                .to_string();
            let shape = token.split(':').collect::<Vec<_>>();
            let ok = (2..=3).contains(&shape.len())
                && shape[0].len() <= 2
                && shape[1..].iter().all(|p| p.len() == 2);
            if let (true, Some(t)) = (ok, parse_time(&token).filter(|_| ok)) {
                out.push_str(&format!("[{token}](seek:{t})"));
                i += token.chars().count();
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

pub fn linkify(text: &str, bare_times: bool) -> String {
    let bare = |s: &str| {
        if bare_times {
            link_bare(s)
        } else {
            s.to_string()
        }
    };
    let mut out = String::with_capacity(text.len() + 32);
    let mut rest = text;
    while let Some(open) = rest.find('[') {
        out.push_str(&bare(&rest[..open]));
        let after = &rest[open + 1..];
        let close = after.find(']');
        if let Some(end) = close
            .filter(|&c| after[c + 1..].starts_with('('))
            .and_then(|c| after[c + 1..].find(')').map(|e| c + 1 + e))
        {
            out.push('[');
            out.push_str(&after[..=end]);
            rest = &after[end + 1..];
            continue;
        }
        let link = close.and_then(|c| {
            let inside = &after[..c];
            let (recording, time) = match inside.strip_prefix('#').and_then(|r| r.split_once(' ')) {
                Some((n, time)) => (Some(n.parse::<usize>().ok()?), time),
                None => (None, inside),
            };
            let start = time.split(['–', '-']).next().unwrap_or_default();
            let t = start.contains(':').then(|| parse_time(start)).flatten()?;
            Some((
                c,
                inside,
                recording.map_or_else(|| t.to_string(), |n| format!("{n}:{t}")),
            ))
        });
        match link {
            Some((c, inside, target)) => {
                out.push_str(&format!("[{inside}](seek:{target})"));
                rest = &after[c + 1..];
            }
            None => {
                out.push('[');
                rest = after;
            }
        }
    }
    out.push_str(&bare(rest));
    out
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
        assert_eq!(
            lines.len(),
            2,
            "a marker is not a line of its own: {lines:?}"
        );
        assert_eq!(text(&lines[0]), "1. В пятницу 1:05 отчёт.");
        assert_eq!(text(&lines[1]), "2. Бюджет.");
        let time = lines[0]
            .spans
            .iter()
            .find(|s| s.text == "1:05")
            .expect("the time");
        assert_eq!(time.link.as_deref(), Some("seek:65"));
        assert!(
            !time.code && !time.strong,
            "a time is an ordinary link, not a chip"
        );
        assert!(
            lines.iter().all(|l| !is_marker(text(l).trim())),
            "no marker-only line"
        );

        let bullets = layout("- один\n- два");
        assert_eq!(
            bullets.iter().map(text).collect::<Vec<_>>(),
            ["• один", "• два"]
        );
    }

    #[test]
    fn headings_bold_and_links_are_spans_of_the_line() {
        let lines =
            layout("# Заголовок\n\nэто **жирное** и *курсив*, см. [сайт](https://example.org/a).");
        assert_eq!(lines[0].heading, Some(1));
        assert_eq!(text(&lines[0]), "Заголовок");
        assert_eq!(lines[1].heading, None);
        assert!(lines[1]
            .spans
            .iter()
            .any(|s| s.strong && s.text == "жирное"));
        assert!(lines[1]
            .spans
            .iter()
            .any(|s| s.emphasis && s.text == "курсив"));
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
        assert_eq!(
            (time.text.as_str(), time.link.as_deref(), time.code),
            ("1:05", Some("seek:65"), false)
        );
    }

    #[test]
    fn bare_times_become_seek_links() {
        assert_eq!(
            linkify("на 1:05 было", true),
            "на [1:05](seek:65) было".to_string()
        );
        assert_eq!(linkify("на 1:05 было", false), "на 1:05 было".to_string());
        assert_eq!(
            linkify("версия 2.1 от 12:30:45", true),
            "версия 2.1 от [12:30:45](seek:45045)".to_string()
        );
        assert_eq!(linkify("год 2024", true), "год 2024".to_string());
    }

    #[test]
    fn a_bracketed_time_is_linkified_without_touching_real_links() {
        assert_eq!(
            linkify("[1:05] шаг", true),
            "[1:05](seek:65) шаг".to_string()
        );
        assert_eq!(
            linkify("[текст](https://example.org) в 2:00", true),
            "[текст](https://example.org) в [2:00](seek:120)".to_string()
        );
        assert_eq!(
            linkify("[#2 1:05–1:10] фрагмент", true),
            "[#2 1:05–1:10](seek:2:65) фрагмент".to_string()
        );
    }

    #[test]
    fn parse_time_accepts_clock_shapes_only() {
        assert_eq!(parse_time("1:05"), Some(65.0));
        assert_eq!(parse_time("12:30:45"), Some(45_045.0));
        assert_eq!(parse_time("[65]"), Some(65.0));
        assert_eq!(parse_time("1,5"), Some(1.5));
        assert_eq!(parse_time("2024"), Some(2024.0));
        assert_eq!(parse_time("1:75"), None);
        assert_eq!(parse_time(""), None);
    }
}
