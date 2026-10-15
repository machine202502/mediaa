//! Keys that UI toolkits do not report. egui has no `PrintScreen` key (and Windows often
//! delivers only its key-up), so the hotkey field polls it directly while capturing.

/// Whether PrintScreen is currently held down (Windows); always `false` elsewhere.
pub fn print_screen_down() -> bool {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_SNAPSHOT};
        // SAFETY: plain query of the global key state.
        let state = unsafe { GetAsyncKeyState(i32::from(VK_SNAPSHOT)) };
        // High bit: down now; low bit: pressed since the last call.
        state as u16 & 0x8001 != 0
    }
    #[cfg(not(windows))]
    {
        false
    }
}
