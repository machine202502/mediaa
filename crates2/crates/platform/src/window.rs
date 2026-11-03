use raw_window_handle::RawWindowHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindow(isize);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowResult {
    Ok,
    Unsupported,
    Failed,
}

impl WindowResult {
    pub fn is_ok(self) -> bool {
        matches!(self, Self::Ok)
    }
}

pub enum WindowOpen {
    Supported(NativeWindow),
    Unsupported(&'static str),
}

impl NativeWindow {
    pub fn from_raw(raw: RawWindowHandle) -> WindowOpen {
        match crate::windows::window::from_raw(raw) {
            Some(handle) => WindowOpen::Supported(NativeWindow(handle)),
            None => WindowOpen::Unsupported("native window handle is not supported"),
        }
    }

    pub fn set_visible(self, visible: bool) -> WindowResult {
        crate::windows::window::set_visible(self.0, visible)
    }

    pub fn set_ghost(self, ghost: bool) -> WindowResult {
        crate::windows::window::set_ghost(self.0, ghost)
    }
}

pub fn prepare_handle(window: isize) -> WindowResult {
    crate::windows::window::prepare_handle(window)
}

pub fn hide_handle(window: isize) -> WindowResult {
    crate::windows::window::hide_handle(window)
}

pub fn hide_window(title: &str) -> WindowResult {
    crate::windows::window::hide_window(title)
}

pub fn show_window(title: &str) -> WindowResult {
    crate::windows::window::show_window(title)
}

pub fn prepare_background(title: &str) -> WindowResult {
    crate::windows::window::prepare_background(title)
}

pub fn background_show(title: &str) -> WindowResult {
    crate::windows::window::background_show(title)
}

pub fn foreground_window() -> isize {
    crate::windows::window::foreground_window()
}

pub fn restore_foreground(window: isize) -> WindowResult {
    crate::windows::window::restore_foreground(window)
}

pub fn exclude_from_capture(title: &str) -> WindowResult {
    crate::windows::window::exclude_from_capture(title)
}

pub fn make_black_transparent(title: &str) -> WindowResult {
    crate::windows::window::make_black_transparent(title)
}

pub fn disable_open_animation(title: &str) -> WindowResult {
    crate::windows::window::disable_open_animation(title)
}

#[derive(Default)]
pub(crate) struct RegionOutline {
    inner: crate::windows::outline_window::RegionOutline,
}

impl RegionOutline {
    pub(crate) fn show(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        stroke_width: i32,
        appearance: crate::outline::OutlineAppearance,
    ) -> WindowResult {
        self.inner
            .show(x, y, width, height, stroke_width, appearance)
    }

    pub(crate) fn hide(&mut self) {
        self.inner.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_result_is_explicit() {
        assert!(WindowResult::Ok.is_ok());
        assert!(!WindowResult::Unsupported.is_ok());
        assert!(!WindowResult::Failed.is_ok());
    }
}
