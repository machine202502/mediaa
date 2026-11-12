use crate::window::WindowResult;
use raw_window_handle::RawWindowHandle;

struct TitleSearch {
    target: Vec<u16>,
    found: isize,
}

fn find_own_window(title: &str) -> Option<isize> {
    use windows_sys::core::BOOL;
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId,
    };
    let mut search = TitleSearch {
        target: title.encode_utf16().collect(),
        found: 0,
    };
    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        // SAFETY: lparam points to the caller-owned TitleSearch that lives for the whole
        // EnumWindows call; the callback stores the first own window with the target title.
        let search = unsafe { &mut *(lparam as *mut TitleSearch) };
        let mut pid: u32 = 0;
        let thread = unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid != unsafe { GetCurrentProcessId() } || thread != unsafe { GetCurrentThreadId() } {
            return 1;
        }
        let mut buffer = [0_u16; 256];
        let len = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), 256) } as usize;
        if buffer[..len.min(buffer.len())] == search.target[..] {
            search.found = hwnd as isize;
            return 0;
        }
        1
    }
    // SAFETY: the callback and the context are valid for the duration of the call. Only
    // windows of the calling thread are inspected, so GetWindowTextW never sends a
    // cross-thread WM_GETTEXT that could block on a non-pumping helper window.
    unsafe {
        EnumWindows(Some(enum_proc), &mut search as *mut TitleSearch as LPARAM);
    }
    if search.found == 0 {
        None
    } else {
        Some(search.found)
    }
}

pub(crate) fn from_raw(raw: RawWindowHandle) -> Option<isize> {
    match raw {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

pub(crate) fn set_visible(handle: isize, visible: bool) -> WindowResult {
    if handle == 0 {
        return WindowResult::Failed;
    }
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetForegroundWindow, ShowWindow, SW_HIDE, SW_SHOW,
    };
    let hwnd = handle as windows_sys::Win32::Foundation::HWND;
    // SAFETY: the handle belongs to this process' main window, which outlives the app
    // state that stores it; ShowWindow/SetForegroundWindow tolerate stale handles.
    unsafe {
        ShowWindow(hwnd, if visible { SW_SHOW } else { SW_HIDE });
        if visible {
            SetForegroundWindow(hwnd);
        }
    }
    WindowResult::Ok
}

pub(crate) fn set_ghost(handle: isize, ghost: bool) -> WindowResult {
    if handle == 0 {
        return WindowResult::Failed;
    }
    use std::sync::atomic::{AtomicIsize, Ordering};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowLongPtrW,
        ShowWindow, GWL_EXSTYLE, LWA_ALPHA, SW_HIDE, SW_SHOWNOACTIVATE, WDA_EXCLUDEFROMCAPTURE,
        WDA_NONE, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };
    static SAVED_STYLE: AtomicIsize = AtomicIsize::new(-1);
    let hwnd = handle as windows_sys::Win32::Foundation::HWND;
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
    WindowResult::Ok
}

pub(crate) fn prepare_handle(window: isize) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_TOP,
        SWP_NOACTIVATE, SWP_NOSIZE, SW_HIDE, WS_EX_NOACTIVATE,
    };
    if window == 0 {
        return WindowResult::Failed;
    }
    // SAFETY: the handle belongs to this process' main window; moving and hiding it before the
    // first paint keeps it off-screen even when eframe shows the window after painting.
    unsafe {
        let hwnd = window as windows_sys::Win32::Foundation::HWND;
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE as isize);
        SetWindowPos(
            hwnd,
            HWND_TOP,
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, SW_HIDE);
        WindowResult::Ok
    }
}
pub(crate) fn hide_handle(window: isize) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, SW_HIDE, WS_EX_NOACTIVATE,
    };
    if window == 0 {
        return WindowResult::Failed;
    }
    // SAFETY: the handle belongs to this process' main window and is only hidden, not destroyed.
    unsafe {
        let hwnd = window as windows_sys::Win32::Foundation::HWND;
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE as isize);
        ShowWindow(hwnd, SW_HIDE);
        WindowResult::Ok
    }
}
pub(crate) fn hide_window(title: &str) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{ShowWindow, SW_HIDE};
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: the handle belongs to this process and is only hidden, not destroyed.
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
    }
    WindowResult::Ok
}

pub(crate) fn show_window(title: &str) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SetForegroundWindow, ShowWindow, SW_SHOW};
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: ShowWindow/SetForegroundWindow on this process' own top-level window.
    unsafe {
        ShowWindow(hwnd, SW_SHOW);
        SetForegroundWindow(hwnd);
    }
    WindowResult::Ok
}
pub(crate) fn prepare_background(title: &str) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_NOACTIVATE,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: plain style call on this process' own top-level window.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE as isize);
    }
    WindowResult::Ok
}

pub(crate) fn background_show(title: &str) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, ShowWindow, GWL_EXSTYLE, HWND_BOTTOM,
        SWP_NOACTIVATE, SWP_NOSIZE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: style, position and visibility calls on this process' own window.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | WS_EX_NOACTIVATE as isize);
        SetWindowPos(
            hwnd,
            HWND_BOTTOM,
            -32000,
            -32000,
            0,
            0,
            SWP_NOSIZE | SWP_NOACTIVATE,
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    WindowResult::Ok
}

pub(crate) fn foreground_window() -> isize {
    use windows_sys::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    // SAFETY: returns a borrowed window handle; no ownership is taken.
    unsafe { GetForegroundWindow() as isize }
}

pub(crate) fn restore_foreground(window: isize) -> WindowResult {
    use windows_sys::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowThreadProcessId, SetForegroundWindow,
    };
    if window == 0 {
        return WindowResult::Failed;
    }
    // SAFETY: the handle is a top-level window of this or another process; attaching and
    // detaching the input queue only borrows the thread, and both calls are paired.
    unsafe {
        let hwnd = window as windows_sys::Win32::Foundation::HWND;
        let foreground_thread = GetWindowThreadProcessId(hwnd, std::ptr::null_mut());
        let current_thread = GetCurrentThreadId();
        let attached = foreground_thread != 0
            && foreground_thread != current_thread
            && AttachThreadInput(current_thread, foreground_thread, 1) != 0;
        let result = SetForegroundWindow(hwnd);
        if attached {
            AttachThreadInput(current_thread, foreground_thread, 0);
        }
        if result != 0 {
            WindowResult::Ok
        } else {
            WindowResult::Failed
        }
    }
}
pub(crate) fn exclude_from_capture(title: &str) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetWindowDisplayAffinity, WDA_EXCLUDEFROMCAPTURE,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: display affinity call on this process' own window.
    unsafe {
        if SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE) != 0 {
            WindowResult::Ok
        } else {
            WindowResult::Failed
        }
    }
}

pub(crate) fn make_black_transparent(title: &str) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, SetWindowPos,
        GWL_EXSTYLE, LWA_COLORKEY, SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
        SWP_NOZORDER, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: style and layered-attribute calls on this process' own viewport.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let extra = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW) as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | extra);
        if SetLayeredWindowAttributes(hwnd, 0, 0, LWA_COLORKEY) != 0
            && SetWindowPos(
                hwnd,
                std::ptr::null_mut(),
                0,
                0,
                0,
                0,
                SWP_FRAMECHANGED | SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER,
            ) != 0
        {
            WindowResult::Ok
        } else {
            WindowResult::Failed
        }
    }
}

static GHOST_STYLE: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(-1);

pub(crate) fn ghost_window(title: &str) -> WindowResult {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetLayeredWindowAttributes, SetWindowLongPtrW, GWL_EXSTYLE, LWA_ALPHA,
        WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: style and layered-attribute calls on this process' own viewport.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if GHOST_STYLE.load(Ordering::Relaxed) == -1 {
            GHOST_STYLE.store(style, Ordering::Relaxed);
        }
        let extra = (WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOOLWINDOW) as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style | extra);
        SetLayeredWindowAttributes(hwnd, 0, 0, LWA_ALPHA);
    }
    WindowResult::Ok
}

pub(crate) fn unghost_window(title: &str, was_visible: bool) -> WindowResult {
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SetLayeredWindowAttributes, SetWindowLongPtrW, ShowWindow, GWL_EXSTYLE, LWA_ALPHA, SW_HIDE,
        SW_SHOWNOACTIVATE,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: style, attribute and visibility calls on this process' own viewport.
    unsafe {
        let saved = GHOST_STYLE.swap(-1, Ordering::Relaxed);
        if saved != -1 {
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, saved);
        }
        SetLayeredWindowAttributes(hwnd, 0, 255, LWA_ALPHA);
        ShowWindow(
            hwnd,
            if was_visible {
                SW_SHOWNOACTIVATE
            } else {
                SW_HIDE
            },
        );
    }
    WindowResult::Ok
}

pub(crate) fn window_visible(title: &str) -> Option<bool> {
    use windows_sys::Win32::UI::WindowsAndMessaging::IsWindowVisible;
    let window = find_own_window(title)?;
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: visibility read on this process' own window.
    unsafe { Some(IsWindowVisible(hwnd) != 0) }
}

pub(crate) fn hide_taskbar_button(title: &str) -> WindowResult {
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        GetWindowLongPtrW, SetWindowLongPtrW, GWL_EXSTYLE, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: style call on this process' own window; egui re-applies the builder without
    // the taskbar flag, so the button is stripped natively after the window exists.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let fixed = (style & !WS_EX_APPWINDOW as isize) | WS_EX_TOOLWINDOW as isize;
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, fixed);
    }
    WindowResult::Ok
}

pub(crate) fn disable_open_animation(title: &str) -> WindowResult {
    use windows_sys::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMWA_TRANSITIONS_FORCEDISABLED,
    };
    let Some(window) = find_own_window(title) else {
        return WindowResult::Failed;
    };
    let hwnd = window as windows_sys::Win32::Foundation::HWND;
    // SAFETY: the attribute value is a 4-byte BOOL on this process' own window.
    unsafe {
        let disable: i32 = 1;
        if DwmSetWindowAttribute(
            hwnd,
            DWMWA_TRANSITIONS_FORCEDISABLED as u32,
            (&disable as *const i32).cast(),
            std::mem::size_of::<i32>() as u32,
        ) == 0
        {
            WindowResult::Ok
        } else {
            WindowResult::Failed
        }
    }
}
