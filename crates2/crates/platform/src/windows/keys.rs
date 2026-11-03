pub(crate) fn print_screen_down() -> bool {
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SNAPSHOT};
    // SAFETY: plain query of the global key state.
    let state = unsafe { GetAsyncKeyState(i32::from(VK_SNAPSHOT)) };
    state as u16 & 0x8001 != 0
}
