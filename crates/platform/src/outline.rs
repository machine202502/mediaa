#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutlineGeometry {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub stroke_width: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutlineAppearance {
    Active,
    ActiveDim,
    Paused,
}

pub trait RegionOutlineBackend {
    fn show(&mut self, geometry: OutlineGeometry, appearance: OutlineAppearance) -> bool;
    fn hide(&mut self);
}

pub struct RegionOutline {
    backend: Box<dyn RegionOutlineBackend>,
}

impl RegionOutline {
    pub fn with_backend(backend: impl RegionOutlineBackend + 'static) -> Self {
        Self { backend: Box::new(backend) }
    }

    pub fn show(&mut self, geometry: OutlineGeometry, appearance: OutlineAppearance) -> bool {
        self.backend.show(geometry, appearance)
    }

    pub fn hide(&mut self) {
        self.backend.hide();
    }
}

impl Default for RegionOutline {
    fn default() -> Self {
        Self { backend: platform_backend() }
    }
}

fn platform_backend() -> Box<dyn RegionOutlineBackend> {
    #[cfg(windows)]
    {
        Box::new(WindowsBackend::default())
    }
    #[cfg(not(windows))]
    {
        Box::new(UnsupportedBackend)
    }
}

#[cfg(windows)]
#[derive(Default)]
struct WindowsBackend {
    inner: crate::window::RegionOutline,
}

#[cfg(windows)]
impl RegionOutlineBackend for WindowsBackend {
    fn show(&mut self, geometry: OutlineGeometry, appearance: OutlineAppearance) -> bool {
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

#[cfg(not(windows))]
struct UnsupportedBackend;

#[cfg(not(windows))]
impl RegionOutlineBackend for UnsupportedBackend {
    fn show(&mut self, _geometry: OutlineGeometry, _appearance: OutlineAppearance) -> bool {
        false
    }

    fn hide(&mut self) {}
}
