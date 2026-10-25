pub mod audio;
pub mod clipboard;
pub mod cursor;
pub mod files;
pub mod keys;
pub mod outline;
pub mod screen;
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub mod stream;
#[cfg(windows)]
pub mod wgc;
pub mod window;
#[cfg(target_os = "linux")]
pub mod x11;
