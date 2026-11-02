use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{CreateSolidBrush, UpdateWindow};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetSystemMetrics,
    PeekMessageW, RegisterClassW, SetLayeredWindowAttributes, ShowWindow, TranslateMessage,
    LWA_ALPHA, MSG, PM_REMOVE, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN, SW_HIDE, SW_SHOWNOACTIVATE, WNDCLASSW, WS_EX_LAYERED, WS_EX_NOACTIVATE,
    WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

const MASK_ALPHA: u8 = 110;

pub struct ScreenMask {
    hwnd: HWND,
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: the window procedure only forwards to the default handler, which owns the message
    // contract; the window belongs to this process and is destroyed before the class is dropped.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

impl ScreenMask {
    pub fn create() -> Option<Self> {
        // SAFETY: all handles are created and released in this process; the class name lives
        // until RegisterClassW returns and the window owns a copy of the registered name.
        unsafe {
            let class_name = wide("MediaA screen mask");
            let instance = GetModuleHandleW(std::ptr::null());
            let brush = CreateSolidBrush(0);
            let class = WNDCLASSW {
                style: 0,
                lpfnWndProc: Some(wndproc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: brush,
                lpszMenuName: std::ptr::null(),
                lpszClassName: class_name.as_ptr(),
            };
            RegisterClassW(&class);
            let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
            let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
            let width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
            let height = GetSystemMetrics(SM_CYVIRTUALSCREEN);
            let hwnd = CreateWindowExW(
                WS_EX_LAYERED
                    | WS_EX_TOPMOST
                    | WS_EX_TOOLWINDOW
                    | WS_EX_NOACTIVATE
                    | WS_EX_TRANSPARENT,
                class_name.as_ptr(),
                wide("MediaA picker mask").as_ptr(),
                WS_POPUP,
                x,
                y,
                width,
                height,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            if hwnd.is_null() {
                return None;
            }
            SetLayeredWindowAttributes(hwnd, 0, MASK_ALPHA, LWA_ALPHA);
            Some(Self { hwnd })
        }
    }

    pub fn show(&self) {
        // SAFETY: the handle is owned by this struct until Drop.
        unsafe {
            ShowWindow(self.hwnd, SW_SHOWNOACTIVATE);
            UpdateWindow(self.hwnd);
        }
    }

    pub fn hide(&self) {
        // SAFETY: the handle is owned by this struct until Drop.
        unsafe {
            ShowWindow(self.hwnd, SW_HIDE);
        }
    }

    pub fn pump(&self) {
        // SAFETY: the message buffer is initialised and every returned message is dispatched to
        // its own window procedure before the buffer is reused.
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            while PeekMessageW(&mut message, self.hwnd, 0, 0, PM_REMOVE) != 0 {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

impl Drop for ScreenMask {
    fn drop(&mut self) {
        // SAFETY: the handle is destroyed exactly once, since Drop runs once.
        unsafe {
            DestroyWindow(self.hwnd);
        }
    }
}
