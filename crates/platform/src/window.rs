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

pub fn make_black_transparent(title: &str) -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            FindWindowW, GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
            GWL_EXSTYLE, LWA_COLORKEY, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
            WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
        };
        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: valid NUL-terminated title and Win32 window-style calls on our own viewport.
        unsafe {
            let hwnd = FindWindowW(std::ptr::null(), wide.as_ptr());
            if hwnd.is_null() {
                return false;
            }
            let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
            let extra = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW) as isize;
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | extra);
            SetLayeredWindowAttributes(hwnd, 0, 0, LWA_COLORKEY) != 0
                && SetWindowPos(
                    hwnd,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
                ) != 0
        }
    }
    #[cfg(not(windows))]
    {
        let _ = title;
        false
    }
}

#[derive(Default)]
pub struct RegionOutline {
    #[cfg(windows)]
    hwnd: Option<windows_sys::Win32::Foundation::HWND>,
}

impl RegionOutline {
    #[cfg(windows)]
    pub fn show(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        stroke_width: i32,
        appearance: crate::outline::OutlineAppearance,
    ) -> bool {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_CLASS_ALREADY_EXISTS};
        use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, DeleteObject, FillRect, GetDC, ReleaseDC};
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, RegisterClassW, SetLayeredWindowAttributes, SetWindowDisplayAffinity,
            SetWindowPos, ShowWindow, CS_HREDRAW, CS_VREDRAW, LWA_COLORKEY, SWP_NOACTIVATE, SWP_NOZORDER,
            SWP_SHOWWINDOW, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW,
            WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
        };

        unsafe extern "system" fn window_proc(
            hwnd: windows_sys::Win32::Foundation::HWND,
            message: u32,
            wparam: windows_sys::Win32::Foundation::WPARAM,
            lparam: windows_sys::Win32::Foundation::LPARAM,
        ) -> windows_sys::Win32::Foundation::LRESULT {
            // SAFETY: forwards messages for our own top-level outline window.
            unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
        }

        let class_name: Vec<u16> = "MediaA recording region outline".encode_utf16().chain(std::iter::once(0)).collect();
        let title: Vec<u16> = "MediaA recording area".encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: all pointers passed to Win32 below refer to live, NUL-terminated data or owned handles.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut class: WNDCLASSW = std::mem::zeroed();
            class.style = CS_HREDRAW | CS_VREDRAW;
            class.lpfnWndProc = Some(window_proc);
            class.hInstance = instance;
            class.lpszClassName = class_name.as_ptr();
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return false;
            }

            if self.hwnd.is_none() {
                let style = WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW;
                let hwnd = CreateWindowExW(
                    style,
                    class_name.as_ptr(),
                    title.as_ptr(),
                    WS_POPUP,
                    x,
                    y,
                    width,
                    height,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null_mut(),
                );
                if hwnd.is_null() {
                    return false;
                }
                self.hwnd = Some(hwnd);
                SetLayeredWindowAttributes(hwnd, 0, 255, LWA_COLORKEY);
                SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            }

            let Some(hwnd) = self.hwnd else {
                return false;
            };
            SetWindowPos(hwnd, std::ptr::null_mut(), x, y, width, height, SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW);
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let dc = GetDC(hwnd);
            if dc.is_null() {
                return false;
            }
            let rect = windows_sys::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: width,
                bottom: height,
            };
            let black = CreateSolidBrush(0);
            FillRect(dc, &rect, black);
            DeleteObject(black as *mut _);
            let color = match appearance {
                crate::outline::OutlineAppearance::Active => 0x005246ff,
                crate::outline::OutlineAppearance::ActiveDim => 0x002e239a,
                crate::outline::OutlineAppearance::Paused => 0x00ff9446,
            };
            let border = CreateSolidBrush(color);
            let stroke = stroke_width.max(1).min(width.min(height));
            for edge in [
                windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: width, bottom: stroke },
                windows_sys::Win32::Foundation::RECT { left: 0, top: height - stroke, right: width, bottom: height },
                windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: stroke, bottom: height },
                windows_sys::Win32::Foundation::RECT { left: width - stroke, top: 0, right: width, bottom: height },
            ] {
                FillRect(dc, &edge, border);
            }
            DeleteObject(border as *mut _);
            ReleaseDC(hwnd, dc);
            true
        }
    }

    #[cfg(windows)]
    pub fn hide(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        if let Some(hwnd) = self.hwnd {
            // SAFETY: hides our own live outline window.
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }

    #[cfg(not(windows))]
    pub fn show(
        &mut self,
        _x: i32,
        _y: i32,
        _width: i32,
        _height: i32,
        _stroke_width: i32,
        _appearance: crate::outline::OutlineAppearance,
    ) -> bool {
        false
    }

    #[cfg(not(windows))]
    pub fn hide(&mut self) {}
}

#[cfg(windows)]
impl Drop for RegionOutline {
    fn drop(&mut self) {
        if let Some(hwnd) = self.hwnd.take() {
            // SAFETY: destroys our own outline window exactly once.
            unsafe {
                windows_sys::Win32::UI::WindowsAndMessaging::DestroyWindow(hwnd);
            }
        }
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
