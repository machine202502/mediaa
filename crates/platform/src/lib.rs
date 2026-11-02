pub mod audio;
pub mod clipboard;
pub mod cursor;
pub(crate) mod cursor_overlay;
pub mod files;
pub mod ipc;
pub mod keys;
#[cfg(target_os = "linux")]
pub(crate) mod linux;
pub mod mask;
pub mod outline;
pub mod screen;
pub mod single_instance;
pub mod startup;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) mod unix;
pub mod window;
#[cfg(windows)]
pub(crate) mod windows;

pub fn init() {
    #[cfg(windows)]
    {
        crate::windows::init::init();
    }
}
