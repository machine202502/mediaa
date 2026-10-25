use crate::frame::Frame;
use crate::geometry::Rect;
use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use std::sync::{Arc, OnceLock};
use tiny_skia::{FillRule, LineCap, LineJoin, Paint, PathBuilder, PixmapMut, Stroke, Transform};

pub const FONT: &[u8] = epaint_default_fonts::UBUNTU_LIGHT;

#[derive(Clone)]
pub struct FontFace {
    pub name: String,
    pub data: Arc<Vec<u8>>,
    pub index: u32,
}

impl std::fmt::Debug for FontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontFace")
            .field("name", &self.name)
            .field("index", &self.index)
            .finish()
    }
}

pub const FONT_SANS: u16 = 0;
pub const FONT_MONO: u16 = 1;

const SYSTEM_FONTS: [&str; 18] = [
    "Arial",
    "Segoe UI",
    "Calibri",
    "Verdana",
    "Tahoma",
    "Times New Roman",
    "Georgia",
    "Consolas",
    "Courier New",
    "Comic Sans MS",
    "Impact",
    "Helvetica",
    "Times",
    "Menlo",
    "DejaVu Sans",
    "DejaVu Serif",
    "Liberation Sans",
    "Noto Serif",
];

pub fn fonts() -> &'static [FontFace] {
    static FONTS: OnceLock<Vec<FontFace>> = OnceLock::new();
    FONTS.get_or_init(|| {
        let builtin = |name: &str, data: &[u8]| FontFace {
            name: name.into(),
            data: Arc::new(data.to_vec()),
            index: 0,
        };
        let mut v = vec![
            builtin("Ubuntu", FONT),
            builtin("Hack", epaint_default_fonts::HACK_REGULAR),
        ];
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        for name in SYSTEM_FONTS {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                ..Default::default()
            };
            let Some(id) = db.query(&query) else { continue };
            let found = db
                .face(id)
                .is_some_and(|f| f.families.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)));
            if let Some(Some((data, index))) =
                found.then(|| db.with_face_data(id, |d, i| (d.to_vec(), i)))
            {
                if FontRef::try_from_slice_and_index(&data, index).is_ok() {
                    v.push(FontFace {
                        name: name.into(),
                        data: Arc::new(data),
                        index,
                    });
                }
            }
        }
        v
    })
}

pub const TEXT_SIZES: [f32; 9] = [12.0, 16.0, 20.0, 24.0, 32.0, 40.0, 56.0, 72.0, 96.0];

pub const MARKER_WIDTH_FACTOR: f32 = 4.0;
pub const MARKER_ALPHA: u8 = 90;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    Pen,
    Line,
    Arrow,
    Rect,
    Marker,
    Text,
}

impl Tool {
    pub const ALL: [Tool; 6] = [
        Tool::Pen,
        Tool::Line,
        Tool::Arrow,
        Tool::Rect,
        Tool::Marker,
        Tool::Text,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    pub color: [u8; 4],
    pub width: f32,
    pub font: u16,
    pub font_size: f32,
}

impl Style {
    pub const fn new(color: [u8; 4], width: f32) -> Style {
        Style {
            color,
            width,
            font: FONT_SANS,
            font_size: 24.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShapeKind {
    Pen(Vec<(f32, f32)>),
    Marker(Vec<(f32, f32)>),
    Line { from: (f32, f32), to: (f32, f32) },
    Arrow { from: (f32, f32), to: (f32, f32) },
    Rect { a: (f32, f32), b: (f32, f32) },
    Text { pos: (f32, f32), text: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: ShapeKind,
    pub style: Style,
}

impl Shape {
    pub fn begin(tool: Tool, p: (f32, f32), style: Style) -> Shape {
        let kind = match tool {
            Tool::Pen => ShapeKind::Pen(vec![p]),
            Tool::Marker => ShapeKind::Marker(vec![p]),
            Tool::Line => ShapeKind::Line { from: p, to: p },
            Tool::Arrow => ShapeKind::Arrow { from: p, to: p },
            Tool::Rect => ShapeKind::Rect { a: p, b: p },
            Tool::Text => ShapeKind::Text {
                pos: p,
                text: String::new(),
            },
        };
        Shape { kind, style }
    }

    pub fn drag_to(&mut self, p: (f32, f32)) {
        match &mut self.kind {
            ShapeKind::Pen(pts) | ShapeKind::Marker(pts) => {
                if pts
                    .last()
                    .is_none_or(|l| (l.0 - p.0).hypot(l.1 - p.1) >= 1.0)
                {
                    pts.push(p);
                }
            }
            ShapeKind::Line { to, .. } | ShapeKind::Arrow { to, .. } => *to = p,
            ShapeKind::Rect { b, .. } => *b = p,
            ShapeKind::Text { .. } => {}
        }
    }

    pub fn is_meaningful(&self) -> bool {
        let far = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1) >= 2.0;
        match &self.kind {
            ShapeKind::Pen(pts) | ShapeKind::Marker(pts) => !pts.is_empty(),
            ShapeKind::Line { from, to } | ShapeKind::Arrow { from, to } => far(*from, *to),
            ShapeKind::Rect { a, b } => (a.0 - b.0).abs() >= 2.0 && (a.1 - b.1).abs() >= 2.0,
            ShapeKind::Text { text, .. } => !text.trim().is_empty(),
        }
    }
}

pub fn arrow_head(from: (f32, f32), to: (f32, f32), width: f32) -> [(f32, f32); 3] {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = dx.hypot(dy).max(1e-3);
    let (ux, uy) = (dx / len, dy / len);
    let head = (width * 4.0).max(12.0).min(len);
    let half = head * 0.55;
    let base = (to.0 - ux * head, to.1 - uy * head);
    [
        to,
        (base.0 - uy * half, base.1 + ux * half),
        (base.0 + uy * half, base.1 - ux * half),
    ]
}

pub fn arrow_shaft_end(from: (f32, f32), to: (f32, f32), width: f32) -> (f32, f32) {
    let [tip, l, r] = arrow_head(from, to, width);
    let base = ((l.0 + r.0) / 2.0, (l.1 + r.1) / 2.0);
    ((base.0 + tip.0) / 2.0, (base.1 + tip.1) / 2.0)
}

fn paint(color: [u8; 4]) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(color[0], color[1], color[2], color[3]);
    p.anti_alias = true;
    p
}

fn stroke(width: f32) -> Stroke {
    Stroke {
        width: width.max(0.5),
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

fn polyline(pts: &[(f32, f32)]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = pts.split_first()?;
    pb.move_to(first.0, first.1);
    if rest.is_empty() {
        pb.line_to(first.0 + 0.01, first.1);
    }
    for p in rest {
        pb.line_to(p.0, p.1);
    }
    pb.finish()
}

fn draw_text(pm: &mut PixmapMut, pos: (f32, f32), text: &str, style: &Style) {
    let (size, color) = (style.font_size, style.color);
    let face = fonts().get(usize::from(style.font));
    let font = match face.map(|f| FontRef::try_from_slice_and_index(&f.data, f.index)) {
        Some(Ok(font)) => font,
        _ => match FontRef::try_from_slice(FONT) {
            Ok(font) => font,
            Err(_) => return,
        },
    };
    let scaled = font.as_scaled(PxScale::from(size));
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let line_h = scaled.height() + scaled.line_gap();
    for (row, line) in text.lines().enumerate() {
        let mut x = pos.0;
        let baseline = pos.1 + scaled.ascent() + row as f32 * line_h;
        let mut prev = None;
        for ch in line.chars() {
            let id = scaled.glyph_id(ch);
            if let Some(p) = prev {
                x += scaled.kern(p, id);
            }
            let glyph =
                id.with_scale_and_position(PxScale::from(size), ab_glyph::point(x, baseline));
            x += scaled.h_advance(id);
            prev = Some(id);
            let Some(outline) = font.outline_glyph(glyph) else {
                continue;
            };
            let bounds = outline.px_bounds();
            let data = pm.data_mut();
            outline.draw(|gx, gy, coverage| {
                let px = bounds.min.x as i32 + gx as i32;
                let py = bounds.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= w || py >= h {
                    return;
                }
                let a = coverage.clamp(0.0, 1.0) * f32::from(color[3]) / 255.0;
                let i = (py as usize * w as usize + px as usize) * 4;
                for c in 0..3 {
                    let dst = f32::from(data[i + c]);
                    data[i + c] = (dst + (f32::from(color[c]) - dst) * a).round() as u8;
                }
                data[i + 3] =
                    (f32::from(data[i + 3]) + (255.0 - f32::from(data[i + 3])) * a).round() as u8;
            });
        }
    }
}

pub fn render(frame: &mut Frame, shapes: &[Shape]) {
    let (w, h) = (frame.width, frame.height);
    let Some(mut pm) = PixmapMut::from_bytes(&mut frame.data, w, h) else {
        return;
    };
    let t = Transform::identity();
    for s in shapes {
        let c = s.style.color;
        match &s.kind {
            ShapeKind::Pen(pts) => {
                if let Some(path) = polyline(pts) {
                    pm.stroke_path(&path, &paint(c), &stroke(s.style.width), t, None);
                }
            }
            ShapeKind::Marker(pts) => {
                if let Some(path) = polyline(pts) {
                    let color = [c[0], c[1], c[2], MARKER_ALPHA];
                    let mut st = stroke(s.style.width * MARKER_WIDTH_FACTOR);
                    st.line_cap = LineCap::Square;
                    pm.stroke_path(&path, &paint(color), &st, t, None);
                }
            }
            ShapeKind::Line { from, to } => {
                if let Some(path) = polyline(&[*from, *to]) {
                    pm.stroke_path(&path, &paint(c), &stroke(s.style.width), t, None);
                }
            }
            ShapeKind::Arrow { from, to } => {
                let end = arrow_shaft_end(*from, *to, s.style.width);
                if let Some(path) = polyline(&[*from, end]) {
                    pm.stroke_path(&path, &paint(c), &stroke(s.style.width), t, None);
                }
                let [a, b, d] = arrow_head(*from, *to, s.style.width);
                let mut pb = PathBuilder::new();
                pb.move_to(a.0, a.1);
                pb.line_to(b.0, b.1);
                pb.line_to(d.0, d.1);
                pb.close();
                if let Some(path) = pb.finish() {
                    pm.fill_path(&path, &paint(c), FillRule::Winding, t, None);
                }
            }
            ShapeKind::Rect { a, b } => {
                let (x0, y0) = (a.0.min(b.0), a.1.min(b.1));
                let (x1, y1) = (a.0.max(b.0), a.1.max(b.1));
                if let Some(r) = tiny_skia::Rect::from_ltrb(x0, y0, x1, y1) {
                    let path = PathBuilder::from_rect(r);
                    let mut st = stroke(s.style.width);
                    st.line_join = LineJoin::Miter;
                    pm.stroke_path(&path, &paint(c), &st, t, None);
                }
            }
            ShapeKind::Text { pos, text } => draw_text(&mut pm, *pos, text, &s.style),
        }
    }
}

pub fn render_crop(full: &Frame, rect: Rect, shapes: &[Shape]) -> Frame {
    let mut out = full.crop(rect);
    let r = rect.clamp_to(&Rect::new(0, 0, full.width as i32, full.height as i32));
    let shifted: Vec<Shape> = shapes
        .iter()
        .map(|s| translate(s, -(r.x as f32), -(r.y as f32)))
        .collect();
    render(&mut out, &shifted);
    out
}

fn translate(s: &Shape, dx: f32, dy: f32) -> Shape {
    let m = |p: &(f32, f32)| (p.0 + dx, p.1 + dy);
    let kind = match &s.kind {
        ShapeKind::Pen(pts) => ShapeKind::Pen(pts.iter().map(m).collect()),
        ShapeKind::Marker(pts) => ShapeKind::Marker(pts.iter().map(m).collect()),
        ShapeKind::Line { from, to } => ShapeKind::Line {
            from: m(from),
            to: m(to),
        },
        ShapeKind::Arrow { from, to } => ShapeKind::Arrow {
            from: m(from),
            to: m(to),
        },
        ShapeKind::Rect { a, b } => ShapeKind::Rect { a: m(a), b: m(b) },
        ShapeKind::Text { pos, text } => ShapeKind::Text {
            pos: m(pos),
            text: text.clone(),
        },
    };
    Shape {
        kind,
        style: s.style,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Style = Style::new([255, 0, 0, 255], 4.0);

    fn white(w: u32, h: u32) -> Frame {
        Frame::filled(w, h, [255, 255, 255, 255])
    }

    fn is_red(f: &Frame, x: u32, y: u32) -> bool {
        let p = f.pixel(x, y);
        p[0] > 200 && p[1] < 60 && p[2] < 60
    }

    fn is_white(f: &Frame, x: u32, y: u32) -> bool {
        f.pixel(x, y) == [255, 255, 255, 255]
    }

    #[test]
    fn rectangle_strokes_edges_only() {
        let mut f = white(100, 100);
        let s = Shape {
            kind: ShapeKind::Rect {
                a: (20.0, 20.0),
                b: (80.0, 70.0),
            },
            style: RED,
        };
        render(&mut f, &[s]);
        assert!(is_red(&f, 20, 45), "left edge");
        assert!(is_red(&f, 50, 70), "bottom edge");
        assert!(is_white(&f, 50, 45), "inside stays untouched");
        assert!(is_white(&f, 5, 5), "outside stays untouched");
    }

    #[test]
    fn pen_and_line_draw_along_path() {
        let mut f = white(100, 100);
        let mut pen = Shape::begin(Tool::Pen, (10.0, 10.0), RED);
        pen.drag_to((10.4, 10.2));
        pen.drag_to((90.0, 10.0));
        assert!(matches!(&pen.kind, ShapeKind::Pen(p) if p.len() == 2));
        let mut line = Shape::begin(Tool::Line, (50.0, 20.0), RED);
        line.drag_to((50.0, 90.0));
        render(&mut f, &[pen, line]);
        assert!(is_red(&f, 50, 10));
        assert!(is_red(&f, 50, 60));
        assert!(is_white(&f, 80, 60));
    }

    #[test]
    fn arrow_has_a_head() {
        let mut f = white(200, 100);
        let s = Shape {
            kind: ShapeKind::Arrow {
                from: (10.0, 50.0),
                to: (190.0, 50.0),
            },
            style: RED,
        };
        render(&mut f, &[s]);
        assert!(is_red(&f, 100, 50), "shaft");
        assert!(is_red(&f, 177, 56), "head");
        assert!(is_white(&f, 100, 58), "shaft is thin");
    }

    #[test]
    fn arrow_head_geometry() {
        let [tip, l, r] = arrow_head((0.0, 0.0), (100.0, 0.0), 4.0);
        assert_eq!(tip, (100.0, 0.0));
        assert!((l.0 - 84.0).abs() < 0.01 && (r.0 - 84.0).abs() < 0.01);
        assert!((l.1 + r.1).abs() < 0.01 && l.1.abs() > 5.0);
    }

    #[test]
    fn marker_is_translucent() {
        let mut f = white(100, 100);
        let mut s = Shape::begin(Tool::Marker, (10.0, 50.0), RED);
        s.drag_to((90.0, 50.0));
        render(&mut f, &[s]);
        let p = f.pixel(50, 50);
        assert!(
            p[0] == 255 && p[1] > 100 && p[1] < 255,
            "blended, not opaque: {p:?}"
        );
    }

    #[test]
    fn text_is_rasterized() {
        let mut f = white(200, 80);
        let s = Shape {
            kind: ShapeKind::Text {
                pos: (10.0, 10.0),
                text: "Привет".into(),
            },
            style: RED,
        };
        render(&mut f, &[s]);
        let red = (0..200)
            .flat_map(|x| (0..80).map(move |y| (x, y)))
            .filter(|&(x, y)| is_red(&f, x, y))
            .count();
        assert!(red > 50, "glyph pixels: {red}");
        assert!(is_white(&f, 195, 75));
    }

    #[test]
    fn every_offered_font_renders_text() {
        let fonts = fonts();
        assert!(fonts.len() >= 2, "built-in fonts are always there");
        assert_eq!(fonts[usize::from(FONT_MONO)].name, "Hack");
        for (i, face) in fonts.iter().enumerate() {
            let mut f = white(240, 80);
            let style = Style {
                font: i as u16,
                font_size: 32.0,
                ..RED
            };
            render(
                &mut f,
                &[Shape {
                    kind: ShapeKind::Text {
                        pos: (5.0, 5.0),
                        text: "Ab".into(),
                    },
                    style,
                }],
            );
            let red = (0..240)
                .flat_map(|x| (0..80).map(move |y| (x, y)))
                .filter(|&(x, y)| is_red(&f, x, y))
                .count();
            assert!(red > 30, "{}: {red} glyph pixels", face.name);
        }
    }

    #[test]
    fn bigger_font_size_draws_more() {
        let count = |size: f32| {
            let mut f = white(400, 120);
            let style = Style {
                font_size: size,
                ..RED
            };
            render(
                &mut f,
                &[Shape {
                    kind: ShapeKind::Text {
                        pos: (5.0, 5.0),
                        text: "Text".into(),
                    },
                    style,
                }],
            );
            (0..400)
                .flat_map(|x| (0..120).map(move |y| (x, y)))
                .filter(|&(x, y)| is_red(&f, x, y))
                .count()
        };
        assert!(count(72.0) > count(24.0) * 3);
    }

    #[test]
    fn meaningless_shapes_are_detected() {
        assert!(!Shape::begin(Tool::Rect, (5.0, 5.0), RED).is_meaningful());
        assert!(!Shape::begin(Tool::Text, (5.0, 5.0), RED).is_meaningful());
        assert!(
            Shape::begin(Tool::Pen, (5.0, 5.0), RED).is_meaningful(),
            "a dot is fine"
        );
        let mut a = Shape::begin(Tool::Arrow, (0.0, 0.0), RED);
        a.drag_to((30.0, 0.0));
        assert!(a.is_meaningful());
    }

    #[test]
    fn render_crop_translates_shapes() {
        let full = white(200, 200);
        let s = Shape {
            kind: ShapeKind::Line {
                from: (100.0, 110.0),
                to: (180.0, 110.0),
            },
            style: RED,
        };
        let out = render_crop(&full, Rect::new(100, 100, 50, 50), &[s]);
        assert_eq!((out.width, out.height), (50, 50));
        assert!(is_red(&out, 25, 10));
        assert!(is_white(&out, 25, 40));
    }
}
