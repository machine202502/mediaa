//! Placement of the area-picker panels (tool column + action bar) around a selection:
//! both stay fully on screen and never overlap each other.

/// Axis-aligned rectangle in screen points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn overlaps(&self, o: &RectF) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }
    pub fn inside(&self, o: &RectF) -> bool {
        self.x >= o.x - 0.01 && self.y >= o.y - 0.01 && self.right() <= o.right() + 0.01 && self.bottom() <= o.bottom() + 0.01
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelLayout {
    pub tools: Option<RectF>,
    pub actions: RectF,
}

/// Distance kept from the selection and from the screen edges.
pub const GAP: f32 = 10.0;

fn clamp_into(r: RectF, screen: &RectF) -> RectF {
    let max_x = (screen.right() - GAP - r.w).max(screen.x + GAP);
    let max_y = (screen.bottom() - GAP - r.h).max(screen.y + GAP);
    RectF::new(r.x.clamp(screen.x + GAP, max_x), r.y.clamp(screen.y + GAP, max_y), r.w, r.h)
}

/// Positions for the tool column (`tools` = its size, `None` when there are no tools) and the
/// action bar of size `actions`, next to `sel`, inside `screen`.
pub fn place_panels(screen: RectF, sel: RectF, tools: Option<(f32, f32)>, actions: (f32, f32)) -> PanelLayout {
    let (aw, ah) = actions;
    let fits_right = |w: f32| sel.right() + GAP + w <= screen.right() - GAP;
    let fits_left = |w: f32| sel.x - GAP - w >= screen.x + GAP;
    let fits_below = sel.bottom() + GAP + ah <= screen.bottom() - GAP;
    let fits_above = sel.y - GAP - ah >= screen.y + GAP;

    // Tool column candidates: outside right, outside left, inside the right edge.
    let tool_candidates: Vec<Option<RectF>> = match tools {
        None => vec![None],
        Some((tw, th)) => {
            let mut v = Vec::new();
            if fits_right(tw) {
                v.push(RectF::new(sel.right() + GAP, sel.y, tw, th));
            }
            if fits_left(tw) {
                v.push(RectF::new(sel.x - GAP - tw, sel.y, tw, th));
            }
            v.push(RectF::new(sel.right() - GAP - tw, sel.y + GAP, tw, th));
            v.push(RectF::new(sel.x + GAP, sel.y + GAP, tw, th));
            v.into_iter().map(|r| Some(clamp_into(r, &screen))).collect()
        }
    };

    // Action bar rows: below, above, inside the bottom edge, inside the top edge.
    let mut rows = Vec::new();
    if fits_below {
        rows.push(sel.bottom() + GAP);
    }
    if fits_above {
        rows.push(sel.y - GAP - ah);
    }
    rows.push(sel.bottom() - GAP - ah);
    rows.push(sel.y + GAP);

    for tools_rect in &tool_candidates {
        // Align the bar to the selection edge on the tools' side, then to the other edge,
        // then next to the tool column itself (corners of the screen).
        let tools_on_right = tools_rect.is_some_and(|t| t.x >= sel.x + sel.w / 2.0);
        let mut xs = if tools_on_right { vec![sel.right() - aw, sel.x] } else { vec![sel.x, sel.right() - aw] };
        let mut ys = rows.clone();
        if let Some(t) = tools_rect {
            xs.extend([t.right() + GAP, t.x - GAP - aw]);
            ys.extend([t.bottom() + GAP, t.y - GAP - ah]);
        }
        for &y in &ys {
            for &x in &xs {
                let bar = clamp_into(RectF::new(x, y, aw, ah), &screen);
                if tools_rect.is_none_or(|t| !t.overlaps(&bar)) {
                    return PanelLayout { tools: *tools_rect, actions: bar };
                }
            }
        }
    }

    // Nothing fits side by side (tiny screen): stack the bar under the tool column.
    let tools_rect = tool_candidates[0];
    let y = tools_rect.map_or(sel.bottom() + GAP, |t| t.bottom() + GAP);
    PanelLayout {
        tools: tools_rect,
        actions: clamp_into(RectF::new(sel.x, y, aw, ah), &screen),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: RectF = RectF::new(0.0, 0.0, 1600.0, 1000.0);
    const TOOLS: (f32, f32) = (46.0, 380.0);
    const ACTIONS: (f32, f32) = (560.0, 44.0);

    fn check(l: &PanelLayout) {
        assert!(l.actions.inside(&SCREEN), "actions off screen: {l:?}");
        if let Some(t) = l.tools {
            assert!(t.inside(&SCREEN), "tools off screen: {l:?}");
            assert!(!t.overlaps(&l.actions), "panels overlap: {l:?}");
        }
    }

    #[test]
    fn typical_selection_tools_right_actions_below() {
        let sel = RectF::new(400.0, 200.0, 500.0, 300.0);
        let l = place_panels(SCREEN, sel, Some(TOOLS), ACTIONS);
        check(&l);
        let t = l.tools.unwrap();
        assert_eq!(t.x, sel.right() + GAP);
        assert_eq!(l.actions.y, sel.bottom() + GAP);
        assert_eq!(l.actions.right(), sel.right(), "bar ends at the selection edge, left of the tools");
    }

    #[test]
    fn selection_at_right_edge_moves_tools_left() {
        let sel = RectF::new(1100.0, 200.0, 495.0, 300.0);
        let l = place_panels(SCREEN, sel, Some(TOOLS), ACTIONS);
        check(&l);
        assert!(l.tools.unwrap().right() <= sel.x);
    }

    #[test]
    fn selection_at_bottom_puts_actions_above() {
        let sel = RectF::new(400.0, 600.0, 600.0, 390.0);
        let l = place_panels(SCREEN, sel, Some(TOOLS), ACTIONS);
        check(&l);
        assert_eq!(l.actions.bottom(), sel.y - GAP);
    }

    #[test]
    fn fullscreen_selection_keeps_everything_inside() {
        let l = place_panels(SCREEN, SCREEN, Some(TOOLS), ACTIONS);
        check(&l);
    }

    #[test]
    fn tall_narrow_selection_does_not_overlap() {
        // The tool column is taller than the selection; the bar must not slide under it.
        let sel = RectF::new(700.0, 300.0, 80.0, 60.0);
        let l = place_panels(SCREEN, sel, Some(TOOLS), ACTIONS);
        check(&l);
    }

    #[test]
    fn no_tools_for_record_region() {
        let sel = RectF::new(100.0, 100.0, 300.0, 200.0);
        let l = place_panels(SCREEN, sel, None, (260.0, 44.0));
        check(&l);
        assert!(l.tools.is_none());
    }

    #[test]
    fn exhaustive_positions_never_overlap_or_leave_screen() {
        for x in (0..1600).step_by(97) {
            for y in (0..1000).step_by(83) {
                for (w, h) in [(20.0, 20.0), (300.0, 150.0), (900.0, 700.0)] {
                    let sel = RectF::new(x as f32, y as f32, w, h);
                    let sel = RectF::new(sel.x.min(1600.0 - w), sel.y.min(1000.0 - h), w, h);
                    check(&place_panels(SCREEN, sel, Some(TOOLS), ACTIONS));
                }
            }
        }
    }
}
