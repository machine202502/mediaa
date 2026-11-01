use std::path::PathBuf;

pub(crate) fn program_log_path() -> PathBuf {
    mediaa_core::paths::logs_dir().join("mediaa.log")
}

pub(crate) fn read_program_log() -> String {
    let path = program_log_path();
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let previous = std::fs::read_to_string(path.with_file_name("mediaa.log.1")).unwrap_or_default();
    if previous.is_empty() {
        current
    } else {
        format!("{previous}{current}")
    }
}
