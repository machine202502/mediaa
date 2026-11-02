use std::path::PathBuf;

pub fn socket_name(scope: &str) -> PathBuf {
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("mediaa-{user}-{scope}.sock"))
}

pub fn prepare(scope: &str) {
    let _ = std::fs::remove_file(socket_name(scope));
}
