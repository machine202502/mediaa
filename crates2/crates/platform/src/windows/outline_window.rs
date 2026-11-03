use crate::outline::OutlineAppearance;
use crate::window::WindowResult;

#[derive(Default)]
pub(crate) struct RegionOutline {
    hwnd: Option<windows_sys::Win32::Foundation::HWND>,
}

impl RegionOutline {
    pub(crate) fn show(
        &mut self,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        stroke_width: i32,
        appearance: OutlineAppearance,
    ) -> WindowResult {
        use windows_sys::Win32::Foundation::{GetLastError, ERROR_CLASS_ALREADY_EXISTS};
        use windows_sys::Win32::Graphics::Gdi::{
            CreateSolidBrush, DeleteObject, FillRect, GetDC, ReleaseDC,
        };
        use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, RegisterClassW, SetLayeredWindowAttributes,
            SetWindowDisplayAffinity, SetWindowPos, ShowWindow, CS_HREDRAW, CS_VREDRAW,
            LWA_COLORKEY, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SW_SHOWNOACTIVATE,
            WDA_EXCLUDEFROMCAPTURE, WNDCLASSW, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            WS_EX_TRANSPARENT, WS_POPUP,
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

        let class_name: Vec<u16> = "MediaA recording region outline"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let title: Vec<u16> = "MediaA recording area"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: all pointers passed to Win32 below refer to live, NUL-terminated data or owned handles.
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let mut class: WNDCLASSW = std::mem::zeroed();
            class.style = CS_HREDRAW | CS_VREDRAW;
            class.lpfnWndProc = Some(window_proc);
            class.hInstance = instance;
            class.lpszClassName = class_name.as_ptr();
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return WindowResult::Failed;
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
                    return WindowResult::Failed;
                }
                self.hwnd = Some(hwnd);
                SetLayeredWindowAttributes(hwnd, 0, 255, LWA_COLORKEY);
                SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
            }

            let Some(hwnd) = self.hwnd else {
                return WindowResult::Failed;
            };
            SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                x,
                y,
                width,
                height,
                SWP_NOACTIVATE | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
            ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let dc = GetDC(hwnd);
            if dc.is_null() {
                return WindowResult::Failed;
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
                OutlineAppearance::Active => 0x005246ff,
                OutlineAppearance::ActiveDim => 0x002e239a,
                OutlineAppearance::Paused => 0x00ff9446,
            };
            let border = CreateSolidBrush(color);
            let stroke = stroke_width.max(1).min(width.min(height));
            for edge in [
                windows_sys::Win32::Foundation::RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: stroke,
                },
                windows_sys::Win32::Foundation::RECT {
                    left: 0,
                    top: height - stroke,
                    right: width,
                    bottom: height,
                },
                windows_sys::Win32::Foundation::RECT {
                    left: 0,
                    top: 0,
                    right: stroke,
                    bottom: height,
                },
                windows_sys::Win32::Foundation::RECT {
                    left: width - stroke,
                    top: 0,
                    right: width,
                    bottom: height,
                },
            ] {
                FillRect(dc, &edge, border);
            }
            DeleteObject(border as *mut _);
            ReleaseDC(hwnd, dc);
            WindowResult::Ok
        }
    }

    pub(crate) fn hide(&mut self) {
        use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
        if let Some(hwnd) = self.hwnd {
            // SAFETY: hides our own live outline window.
            unsafe {
                ShowWindow(hwnd, SW_HIDE);
            }
        }
    }
}

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
