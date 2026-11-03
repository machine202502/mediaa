use serde::{Deserialize, Serialize};

pub const FONT_SANS: u16 = 0;
pub const FONT_MONO: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
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

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShapeKind {
    Pen(Vec<(f32, f32)>),
    Marker(Vec<(f32, f32)>),
    Line { from: (f32, f32), to: (f32, f32) },
    Arrow { from: (f32, f32), to: (f32, f32) },
    Rect { a: (f32, f32), b: (f32, f32) },
    Text { pos: (f32, f32), text: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    const RED: Style = Style::new([255, 0, 0, 255], 4.0);

    #[test]
    fn arrow_head_geometry() {
        let [tip, l, r] = arrow_head((0.0, 0.0), (100.0, 0.0), 4.0);
        assert_eq!(tip, (100.0, 0.0));
        assert!((l.0 - 84.0).abs() < 0.01 && (r.0 - 84.0).abs() < 0.01);
        assert!((l.1 + r.1).abs() < 0.01 && l.1.abs() > 5.0);
    }

    #[test]
    fn arrow_shaft_ends_inside_the_head() {
        let end = arrow_shaft_end((0.0, 0.0), (100.0, 0.0), 4.0);
        assert!(end.0 > 84.0 && end.0 < 100.0);
        assert!(end.1.abs() < 0.01);
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
    fn shapes_survive_serialization() {
        let shape = Shape {
            kind: ShapeKind::Arrow {
                from: (1.0, 2.0),
                to: (3.0, 4.0),
            },
            style: RED,
        };
        let text = toml::to_string(&shape).unwrap();
        let back: Shape = toml::from_str(&text).unwrap();
        assert_eq!(back, shape);

        let mut pen = Shape::begin(Tool::Pen, (1.0, 1.0), RED);
        pen.drag_to((8.0, 9.0));
        let back: Shape = toml::from_str(&toml::to_string(&pen).unwrap()).unwrap();
        assert_eq!(back, pen);

        let text = Shape {
            kind: ShapeKind::Text {
                pos: (0.0, 0.0),
                text: "Привет".into(),
            },
            style: RED,
        };
        let back: Shape = toml::from_str(&toml::to_string(&text).unwrap()).unwrap();
        assert_eq!(back, text);
    }
}
