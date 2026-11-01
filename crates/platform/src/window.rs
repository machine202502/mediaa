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
        #[cfg(windows)]
        {
            match crate::windows::window::from_raw(raw) {
                Some(handle) => WindowOpen::Supported(NativeWindow(handle)),
                None => WindowOpen::Unsupported("native window handle is not supported"),
            }
        }
        #[cfg(not(windows))]
        {
            let _ = raw;
            WindowOpen::Unsupported("native windows are not implemented on this platform")
        }
    }

    pub fn set_visible(self, visible: bool) -> WindowResult {
        #[cfg(windows)]
        {
            crate::windows::window::set_visible(self.0, visible)
        }
        #[cfg(not(windows))]
        {
            let _ = visible;
            WindowResult::Unsupported
        }
    }

    pub fn set_ghost(self, ghost: bool) -> WindowResult {
        #[cfg(windows)]
        {
            crate::windows::window::set_ghost(self.0, ghost)
        }
        #[cfg(not(windows))]
        {
            let _ = ghost;
            WindowResult::Unsupported
        }
    }
}

pub fn exclude_from_capture(title: &str) -> WindowResult {
    #[cfg(windows)]
    {
        crate::windows::window::exclude_from_capture(title)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        WindowResult::Unsupported
    }
}

pub fn make_black_transparent(title: &str) -> WindowResult {
    #[cfg(windows)]
    {
        crate::windows::window::make_black_transparent(title)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        WindowResult::Unsupported
    }
}

#[derive(Default)]
pub(crate) struct RegionOutline {
    #[cfg(windows)]
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
        #[cfg(windows)]
        {
            self.inner
                .show(x, y, width, height, stroke_width, appearance)
        }
        #[cfg(not(windows))]
        {
            let _ = (x, y, width, height, stroke_width, appearance);
            WindowResult::Unsupported
        }
    }

    pub(crate) fn hide(&mut self) {
        #[cfg(windows)]
        self.inner.hide();
    }
}

pub fn disable_open_animation(title: &str) -> WindowResult {
    #[cfg(windows)]
    {
        crate::windows::window::disable_open_animation(title)
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        WindowResult::Unsupported
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
