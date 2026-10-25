use directories::{ProjectDirs, UserDirs};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const APP_NAME: &str = "MediaA";

fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("dev", APP_NAME, APP_NAME)
}

pub fn settings_file() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().join("settings.toml"))
        .unwrap_or_else(|| PathBuf::from("mediaa-settings.toml"))
}

pub fn data_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.data_local_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join(APP_NAME))
}

pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

pub fn work_dir() -> PathBuf {
    data_dir().join("work")
}

fn home() -> PathBuf {
    UserDirs::new()
        .map(|u| u.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn default_media_dir() -> PathBuf {
    UserDirs::new()
        .and_then(|u| u.document_dir().map(Path::to_path_buf))
        .unwrap_or_else(|| home().join("Documents"))
        .join(APP_NAME)
}

pub fn remove_stale_entries(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .is_some_and(|age| age > max_age);
        if !old {
            continue;
        }
        let path = entry.path();
        let res = if meta.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if res.is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dirs_end_with_app_name() {
        assert!(default_media_dir().ends_with(APP_NAME));
        assert!(work_dir().starts_with(data_dir()));
        assert_eq!(settings_file().file_name().unwrap(), "settings.toml");
    }

    #[test]
    fn stale_cleanup_keeps_fresh_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("fresh.tmp"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("session")).unwrap();
        assert_eq!(
            remove_stale_entries(dir.path(), Duration::from_secs(3600)),
            0
        );
        std::thread::sleep(Duration::from_millis(20));
        assert_eq!(remove_stale_entries(dir.path(), Duration::ZERO), 2);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn stale_cleanup_of_missing_dir_is_noop() {
        assert_eq!(
            remove_stale_entries(Path::new("/definitely/missing/dir"), Duration::ZERO),
            0
        );
    }
}
