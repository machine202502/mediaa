use std::path::PathBuf;

pub fn socket_name(scope: &str) -> PathBuf {
    PathBuf::from(format!(r"\\.\pipe\mediaa-{scope}"))
}

pub fn prepare(_scope: &str) {}
