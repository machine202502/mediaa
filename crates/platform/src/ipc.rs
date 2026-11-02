pub fn socket_name(scope: &str) -> std::path::PathBuf {
    imp::socket_name(scope)
}

pub fn prepare(scope: &str) {
    imp::prepare(scope)
}

#[cfg(windows)]
use crate::windows::ipc as imp;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::unix::ipc as imp;

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    use std::path::PathBuf;

    pub fn socket_name(_scope: &str) -> PathBuf {
        panic!("MediaA supports Windows only")
    }

    pub fn prepare(_scope: &str) {}
}
