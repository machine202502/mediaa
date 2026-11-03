pub mod audio;
pub mod clipboard;
pub mod cursor;
pub(crate) mod cursor_overlay;
pub mod dialogs;
pub mod files;
pub mod hotkeys;
pub mod ipc;
pub mod keys;
pub mod mask;
pub mod outline;
pub mod process;
pub mod screen;
pub mod single_instance;
pub mod startup;
pub mod tray;
pub mod tray_icon;
pub mod window;
#[cfg(windows)]
pub(crate) mod windows;

pub fn init() {
    #[cfg(windows)]
    {
        crate::windows::init::init();
    }
}
