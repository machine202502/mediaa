pub mod audio;
pub mod clipboard;
pub mod cursor;
pub mod files;
pub mod keys;
pub mod outline;
pub mod screen;
pub mod window;
#[cfg(windows)]
pub mod wgc;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod stream;
#[cfg(target_os = "linux")]
pub mod x11;
