//! Integer pixel geometry plus the interactive selection editor used by the region overlay.
//!
//! The overlay converts pointer positions to *image pixels* before calling into
//! [`SelectionEditor`], so all logic here is DPI-independent and unit-testable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

impl Point {
    pub const fn new(x: i32, y: i32) -> Self {
        Self { x, y }
    }
}

/// Axis-aligned rectangle; `w`/`h` are never negative.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// Rectangle spanned by two arbitrary corners (inclusive of both pixels' top-left corners).
    pub fn from_points(a: Point, b: Point) -> Self {
        let x0 = a.x.min(b.x);
        let y0 = a.y.min(b.y);
        let x1 = a.x.max(b.x);
        let y1 = a.y.max(b.y);
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn area(&self) -> i64 {
        i64::from(self.w.max(0)) * i64::from(self.h.max(0))
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.right().min(other.right());
        let y1 = self.bottom().min(other.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    /// Clamps the rectangle into `bounds`, keeping as much of it as possible.
    pub fn clamp_to(&self, bounds: &Rect) -> Rect {
        self.intersect(bounds)
            .unwrap_or(Rect::new(bounds.x, bounds.y, 0, 0))
    }

    /// Shrinks width/height to even numbers (required by yuv420p video encoders).
    pub fn even(&self) -> Rect {
        Rect::new(self.x, self.y, self.w & !1, self.h & !1)
    }

    pub fn translate(&self, dx: i32, dy: i32) -> Rect {
        Rect::new(self.x + dx, self.y + dy, self.w, self.h)
    }

    /// Scales all coordinates by `factor` (e.g. logical points to physical pixels).
    pub fn scale(&self, factor: f64) -> Rect {
        let x0 = (f64::from(self.x) * factor).round() as i32;
        let y0 = (f64::from(self.y) * factor).round() as i32;
        let x1 = (f64::from(self.right()) * factor).round() as i32;
        let y1 = (f64::from(self.bottom()) * factor).round() as i32;
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

/// Parts of a selection that can be grabbed with the mouse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Handle {
    Move,
    N,
    S,
    E,
    W,
    NE,
    NW,
    SE,
    SW,
}

impl Handle {
    fn moves_left(self) -> bool {
        matches!(self, Handle::W | Handle::NW | Handle::SW)
    }
    fn moves_right(self) -> bool {
        matches!(self, Handle::E | Handle::NE | Handle::SE)
    }
    fn moves_top(self) -> bool {
        matches!(self, Handle::N | Handle::NE | Handle::NW)
    }
    fn moves_bottom(self) -> bool {
        matches!(self, Handle::S | Handle::SE | Handle::SW)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Drag {
    /// Drawing a brand new selection from an anchor point.
    Creating { anchor: Point },
    /// Moving or resizing the existing selection.
    Editing { handle: Handle, origin: Point, start: Rect },
}

/// Lightshot-style selection state: draw a rectangle, then move/resize it with handles.
#[derive(Debug, Clone)]
pub struct SelectionEditor {
    bounds: Rect,
    selection: Option<Rect>,
    drag: Option<Drag>,
    /// Pointer tolerance (in pixels) for grabbing edges and corners.
    pub grab_radius: i32,
}

impl SelectionEditor {
    pub fn new(bounds: Rect) -> Self {
        Self {
            bounds,
            selection: None,
            drag: None,
            grab_radius: 8,
        }
    }

    pub fn bounds(&self) -> Rect {
        self.bounds
    }

    /// Current selection (may be empty while the user is still dragging).
    pub fn selection(&self) -> Option<Rect> {
        self.selection
    }

    /// Selection that is large enough to be used.
    pub fn usable_selection(&self) -> Option<Rect> {
        self.selection.filter(|r| r.w >= 2 && r.h >= 2)
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    pub fn set_selection(&mut self, rect: Option<Rect>) {
        self.selection = rect.map(|r| r.clamp_to(&self.bounds));
        self.drag = None;
    }

    pub fn select_all(&mut self) {
        self.set_selection(Some(self.bounds));
    }

    /// Which handle would be grabbed at `p`, if any.
    pub fn hit_test(&self, p: Point) -> Option<Handle> {
        let r = self.usable_selection()?;
        let g = self.grab_radius;
        let near = |a: i32, b: i32| (a - b).abs() <= g;
        let in_x = p.x >= r.x - g && p.x <= r.right() + g;
        let in_y = p.y >= r.y - g && p.y <= r.bottom() + g;
        if !(in_x && in_y) {
            return None;
        }
        let l = near(p.x, r.x);
        let rr = near(p.x, r.right());
        let t = near(p.y, r.y);
        let b = near(p.y, r.bottom());
        Some(match (l, rr, t, b) {
            (true, _, true, _) => Handle::NW,
            (_, true, true, _) => Handle::NE,
            (true, _, _, true) => Handle::SW,
            (_, true, _, true) => Handle::SE,
            (true, _, _, _) => Handle::W,
            (_, true, _, _) => Handle::E,
            (_, _, true, _) => Handle::N,
            (_, _, _, true) => Handle::S,
            _ if r.contains(p) => Handle::Move,
            _ => return None,
        })
    }

    fn clamp_point(&self, p: Point) -> Point {
        Point::new(
            p.x.clamp(self.bounds.x, self.bounds.right()),
            p.y.clamp(self.bounds.y, self.bounds.bottom()),
        )
    }

    /// Mouse button pressed at `p`.
    pub fn press(&mut self, p: Point) {
        let p = self.clamp_point(p);
        self.drag = Some(match (self.hit_test(p), self.selection) {
            (Some(handle), Some(start)) => Drag::Editing {
                handle,
                origin: p,
                start,
            },
            _ => {
                self.selection = Some(Rect::new(p.x, p.y, 0, 0));
                Drag::Creating { anchor: p }
            }
        });
    }

    /// Pointer moved to `p` while the button is held.
    pub fn drag_to(&mut self, p: Point) {
        let p = self.clamp_point(p);
        match self.drag {
            None => {}
            Some(Drag::Creating { anchor }) => {
                self.selection = Some(Rect::from_points(anchor, p));
            }
            Some(Drag::Editing {
                handle: Handle::Move,
                origin,
                start,
            }) => {
                let b = self.bounds;
                let nx = (start.x + p.x - origin.x).clamp(b.x, b.right() - start.w);
                let ny = (start.y + p.y - origin.y).clamp(b.y, b.bottom() - start.h);
                self.selection = Some(Rect::new(nx, ny, start.w, start.h));
            }
            Some(Drag::Editing {
                handle,
                origin,
                start,
            }) => {
                let dx = p.x - origin.x;
                let dy = p.y - origin.y;
                let mut x0 = start.x;
                let mut y0 = start.y;
                let mut x1 = start.right();
                let mut y1 = start.bottom();
                if handle.moves_left() {
                    x0 += dx;
                }
                if handle.moves_right() {
                    x1 += dx;
                }
                if handle.moves_top() {
                    y0 += dy;
                }
                if handle.moves_bottom() {
                    y1 += dy;
                }
                // Dragging an edge past the opposite one flips the rectangle, like most editors.
                let r = Rect::from_points(Point::new(x0, y0), Point::new(x1, y1));
                self.selection = Some(r.clamp_to(&self.bounds));
            }
        }
    }

    /// Mouse button released.
    pub fn release(&mut self) {
        self.drag = None;
        if self.selection.is_some_and(|r| r.w < 2 || r.h < 2) {
            self.selection = None;
        }
    }

    /// Moves the selection by keyboard arrows; `resize` grows/shrinks instead.
    pub fn nudge(&mut self, dx: i32, dy: i32, resize: bool) {
        let Some(r) = self.selection else { return };
        let b = self.bounds;
        let next = if resize {
            Rect::new(r.x, r.y, (r.w + dx).max(2), (r.h + dy).max(2)).clamp_to(&b)
        } else {
            Rect::new(
                (r.x + dx).clamp(b.x, b.right() - r.w),
                (r.y + dy).clamp(b.y, b.bottom() - r.h),
                r.w,
                r.h,
            )
        };
        self.selection = Some(next);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const B: Rect = Rect::new(0, 0, 1920, 1080);

    #[test]
    fn from_points_normalizes() {
        let r = Rect::from_points(Point::new(50, 80), Point::new(10, 20));
        assert_eq!(r, Rect::new(10, 20, 40, 60));
    }

    #[test]
    fn intersect_and_clamp() {
        let r = Rect::new(-10, -10, 50, 50);
        assert_eq!(r.clamp_to(&B), Rect::new(0, 0, 40, 40));
        assert_eq!(Rect::new(5000, 0, 10, 10).intersect(&B), None);
        assert!(Rect::new(5000, 0, 10, 10).clamp_to(&B).is_empty());
    }

    #[test]
    fn even_dimensions() {
        assert_eq!(Rect::new(1, 1, 101, 57).even(), Rect::new(1, 1, 100, 56));
    }

    #[test]
    fn scale_keeps_edges_consistent() {
        let r = Rect::new(10, 10, 15, 15).scale(1.5);
        assert_eq!(r, Rect::new(15, 15, 23, 23));
        assert_eq!(Rect::new(0, 0, 100, 50).scale(2.0), Rect::new(0, 0, 200, 100));
    }

    #[test]
    fn create_selection_by_dragging() {
        let mut ed = SelectionEditor::new(B);
        ed.press(Point::new(100, 100));
        ed.drag_to(Point::new(40, 300));
        ed.release();
        assert_eq!(ed.selection(), Some(Rect::new(40, 100, 60, 200)));
    }

    #[test]
    fn click_without_drag_clears_selection() {
        let mut ed = SelectionEditor::new(B);
        ed.press(Point::new(100, 100));
        ed.release();
        assert_eq!(ed.selection(), None);
    }

    #[test]
    fn drag_is_clamped_to_bounds() {
        let mut ed = SelectionEditor::new(B);
        ed.press(Point::new(1900, 1000));
        ed.drag_to(Point::new(5000, 5000));
        ed.release();
        assert_eq!(ed.selection(), Some(Rect::new(1900, 1000, 20, 80)));
    }

    #[test]
    fn hit_test_finds_handles() {
        let mut ed = SelectionEditor::new(B);
        ed.set_selection(Some(Rect::new(100, 100, 200, 100)));
        assert_eq!(ed.hit_test(Point::new(100, 100)), Some(Handle::NW));
        assert_eq!(ed.hit_test(Point::new(303, 198)), Some(Handle::SE));
        assert_eq!(ed.hit_test(Point::new(200, 100)), Some(Handle::N));
        assert_eq!(ed.hit_test(Point::new(98, 150)), Some(Handle::W));
        assert_eq!(ed.hit_test(Point::new(200, 150)), Some(Handle::Move));
        assert_eq!(ed.hit_test(Point::new(10, 10)), None);
    }

    #[test]
    fn move_selection_stays_inside_bounds() {
        let mut ed = SelectionEditor::new(B);
        ed.set_selection(Some(Rect::new(100, 100, 200, 100)));
        ed.press(Point::new(150, 150));
        ed.drag_to(Point::new(50, 140));
        assert_eq!(ed.selection(), Some(Rect::new(0, 90, 200, 100)));
        ed.drag_to(Point::new(5000, 5000));
        assert_eq!(ed.selection(), Some(Rect::new(1720, 980, 200, 100)));
        ed.release();
    }

    #[test]
    fn resize_from_corner_and_flip() {
        let mut ed = SelectionEditor::new(B);
        ed.set_selection(Some(Rect::new(100, 100, 200, 100)));
        ed.press(Point::new(300, 200)); // SE corner
        ed.drag_to(Point::new(350, 260));
        assert_eq!(ed.selection(), Some(Rect::new(100, 100, 250, 160)));
        // Drag the right edge past the left one: the rectangle flips.
        ed.drag_to(Point::new(50, 260));
        assert_eq!(ed.selection(), Some(Rect::new(50, 100, 50, 160)));
        ed.release();
    }

    #[test]
    fn edge_resize_changes_one_axis() {
        let mut ed = SelectionEditor::new(B);
        ed.set_selection(Some(Rect::new(100, 100, 200, 100)));
        ed.press(Point::new(200, 100)); // N edge
        ed.drag_to(Point::new(260, 40));
        ed.release();
        assert_eq!(ed.selection(), Some(Rect::new(100, 40, 200, 160)));
    }

    #[test]
    fn pressing_outside_starts_new_selection() {
        let mut ed = SelectionEditor::new(B);
        ed.set_selection(Some(Rect::new(100, 100, 200, 100)));
        ed.press(Point::new(800, 800));
        ed.drag_to(Point::new(900, 850));
        ed.release();
        assert_eq!(ed.selection(), Some(Rect::new(800, 800, 100, 50)));
    }

    #[test]
    fn nudge_moves_and_resizes() {
        let mut ed = SelectionEditor::new(B);
        ed.set_selection(Some(Rect::new(0, 0, 10, 10)));
        ed.nudge(-5, 3, false);
        assert_eq!(ed.selection(), Some(Rect::new(0, 3, 10, 10)));
        ed.nudge(5, -20, true);
        assert_eq!(ed.selection(), Some(Rect::new(0, 3, 15, 2)));
    }

    #[test]
    fn select_all_uses_bounds() {
        let mut ed = SelectionEditor::new(B);
        ed.select_all();
        assert_eq!(ed.usable_selection(), Some(B));
    }
}
