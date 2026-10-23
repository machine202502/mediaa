use raw_window_handle::RawWindowHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NativeWindow(isize);

impl NativeWindow {
    pub fn from_raw(raw: RawWindowHandle) -> Option<Self> {
        match raw {
            RawWindowHandle::Win32(h) => Some(NativeWindow(h.hwnd.get())),
            _ => None,
        }
    }

    pub fn set_visible(self, visible: bool) -> bool {
        #[cfg(windows)]
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
            };
            let hwnd = self.0 as windows_sys::Win32::Foundation::HWND;
            // SAFETY: the handle belongs to this process' main window, which outlives the app
            // state that stores it; ShowWindow/SetForegroundWindow tolerate stale handles.
            unsafe {
                ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
                if visible {
                    SetForegroundWindow(hwnd);
                }
            }
            true
        }
        #[cfg(not(windows))]
        {
            let _ = visible;
            false
        }
    }

    pub fn set_ghost(self, ghost: bool) -> bool {
        #[cfg(windows)]
        {
            use std::sync::atomic::{AtomicIsize, Ordering};
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowLongPtrW, ShowWindow,
                GWL_EXSTYLE, LWA_ALPHA, SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WS_EX_LAYERED,
                WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
            };
            static SAVED_STYLE: AtomicIsize = AtomicIsize::new(-1);
            let hwnd = self.0 as windows_sys::Win32::Foundation::HWND;
            // SAFETY: plain style/attribute calls on this process' own top-level window.
            unsafe {
                if ghost {
                    let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    SAVED_STYLE.store(style, Ordering::Relaxed);
                    let extra = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW) as isize;
                    SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | extra);
                    SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
                    SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
                    ShowWindow(hwnd, SW_SHOWNOACTIVATE);
                } else {
                    ShowWindow(hwnd, SW_HIDE);
                    SetWindowDisplayAffinity(hwnd, WDA_NONE);
                    let saved = SAVED_STYLE.swap(-1, Ordering::Relaxed);
                    if saved != -1 {
                        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, saved);
                    }
                }
            }
            true
        }
        #[cfg(not(windows))]
        {
            let _ = ghost;
            false
        }
    }
}

pub fn exclude_from_capture(title: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE};
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: valid NUL-terminated UTF-16 title.
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
            !hwnd.is_null() && SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) != 0
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}

pub fn disable_open_animation(title: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED};
        use windows_sys::Win32::UI::WindowsAndMessaging::FindWindowW;
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: valid NUL-terminated UTF-16 title; the attribute value is a 4-byte BOOL.
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            let disable: i32 = 1;
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_TRANSITIONS_FORCEDISABLED as u32,
                (&disable as *const i32).cast(),
                std::mem::size_of::<i32>() as u32,
            ) == 0
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}
