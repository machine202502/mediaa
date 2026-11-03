use std::ffi::OsString;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use windows_sys::Win32::System::Threading::{CREATE_NO_WINDOW, DETACHED_PROCESS};

pub fn spawn_detached(args: &[OsString]) -> std::io::Result<Child> {
    Command::new(std::env::current_exe()?)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
}
