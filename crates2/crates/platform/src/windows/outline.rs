use crate::outline::{OutlineAppearance, OutlineGeometry, RegionOutlineBackend};
use crate::window::{RegionOutline, WindowResult};

#[derive(Default)]
pub(crate) struct WindowsBackend {
    inner: RegionOutline,
}

impl RegionOutlineBackend for WindowsBackend {
    fn show(&mut self, geometry: OutlineGeometry, appearance: OutlineAppearance) -> WindowResult {
        self.inner.show(
            geometry.x,
            geometry.y,
            geometry.width,
            geometry.height,
            geometry.stroke_width,
            appearance,
        )
    }

    fn hide(&mut self) {
        self.inner.hide();
    }
}

pub(crate) fn backend() -> Box<dyn RegionOutlineBackend> {
    Box::new(WindowsBackend::default())
}
