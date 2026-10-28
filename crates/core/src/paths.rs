use directories::{ProjectDirs, UserDirs};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub const APP_NAME: &str = "MediaA";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AppIdentity {
    pub name: &'static str,
    pub window_title: &'static str,
    pub application_id: &'static str,
    pub instance_id: &'static str,
}

pub const APP_IDENTITY: AppIdentity = AppIdentity {
    name: APP_NAME,
    window_title: "MediaA",
    application_id: "mediaa",
    instance_id: "MediaA.SingleInstance.v1",
};

macro_rules! path_type {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash)]
        pub struct $name(PathBuf);

        impl $name {
            pub fn new(path: impl Into<PathBuf>) -> Self {
                Self(path.into())
            }

            pub fn into_path(self) -> PathBuf {
                self.0
            }
        }

        impl AsRef<Path> for $name {
            fn as_ref(&self) -> &Path {
                &self.0
            }
        }

        impl Deref for $name {
            type Target = Path;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }
    };
}

path_type!(SourcePath);
path_type!(OutputPath);
path_type!(MediaDir);
path_type!(WorkDir);
path_type!(CachePath);

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

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
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

pub fn remove_stale_temporary_files(dir: &Path, max_age: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let now = SystemTime::now();
    let mut removed = 0;
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(crate::atomic_file::TEMP_PREFIX)
        {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if !metadata.is_file()
            || !metadata
                .modified()
                .ok()
                .and_then(|modified| now.duration_since(modified).ok())
                .is_some_and(|age| age > max_age)
        {
            continue;
        }
        if std::fs::remove_file(entry.path()).is_ok() {
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
        assert!(default_media_dir().ends_with(APP_IDENTITY.name));
        assert!(work_dir().starts_with(data_dir()));
        assert_eq!(models_dir(), data_dir().join("models"));
        assert_eq!(settings_file().file_name().unwrap(), "settings.toml");
    }

    #[test]
    fn app_identity_has_one_window_and_instance_owner() {
        assert_eq!(APP_IDENTITY.name, "MediaA");
        assert_eq!(APP_IDENTITY.window_title, APP_IDENTITY.name);
        assert_eq!(APP_IDENTITY.application_id, "mediaa");
        assert!(APP_IDENTITY.instance_id.starts_with(APP_IDENTITY.name));
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

    #[test]
    fn temporary_output_cleanup_never_removes_media() {
        let dir = tempfile::tempdir().unwrap();
        let temp = dir.path().join(".mediaa-interrupted.mp4");
        let media = dir.path().join("Screenshot_2026-10-25_06-00-00.png");
        std::fs::write(&temp, b"partial").unwrap();
        std::fs::write(&media, b"media").unwrap();
        let old = SystemTime::now() - Duration::from_secs(7200);
        for path in [&temp, &media] {
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(old)
                .unwrap();
        }
        assert_eq!(
            remove_stale_temporary_files(dir.path(), Duration::from_secs(3600)),
            1
        );
        assert!(!temp.exists());
        assert!(media.exists());
    }
}
