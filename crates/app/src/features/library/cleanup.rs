use mediaa_core::library::LibraryEntry;
use rust_i18n::t;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CleanupOlder {
    Week,
    Month,
}

impl CleanupOlder {
    fn days(self) -> u64 {
        match self {
            CleanupOlder::Week => 7,
            CleanupOlder::Month => 30,
        }
    }

    pub(crate) fn cutoff(self, now: SystemTime) -> SystemTime {
        now.checked_sub(Duration::from_secs(self.days() * 24 * 60 * 60))
            .unwrap_or(UNIX_EPOCH)
    }

    pub(crate) fn label(self) -> String {
        match self {
            CleanupOlder::Week => t!("lib.cleanup_week"),
            CleanupOlder::Month => t!("lib.cleanup_month"),
        }
        .into()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CleanupItem {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CleanupPlan {
    pub(crate) older: CleanupOlder,
    pub(crate) items: Vec<CleanupItem>,
}

impl CleanupPlan {
    pub(crate) fn bytes(&self) -> u64 {
        self.items.iter().map(|item| item.size).sum()
    }
}

#[derive(Debug)]
pub(crate) struct RemovedItem {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) proxy: mediaa_core::paths::CachePath,
}

#[derive(Debug)]
pub(crate) struct CleanupFailure {
    pub(crate) path: PathBuf,
    pub(crate) error: String,
}

#[derive(Debug, Default)]
pub(crate) struct CleanupReport {
    pub(crate) removed: Vec<RemovedItem>,
    pub(crate) failures: Vec<CleanupFailure>,
    pub(crate) skipped: usize,
}

impl CleanupReport {
    pub(crate) fn bytes(&self) -> u64 {
        self.removed.iter().map(|item| item.size).sum()
    }
}

pub(crate) fn cleanup_items(
    entries: &[LibraryEntry],
    older: CleanupOlder,
    now: SystemTime,
) -> Vec<CleanupItem> {
    let cutoff = older.cutoff(now);
    entries
        .iter()
        .filter(|entry| entry.modified < cutoff)
        .map(|entry| CleanupItem {
            path: entry.path.clone(),
            size: entry.size,
        })
        .collect()
}

pub(crate) fn plan_cleanup(
    entries: &[LibraryEntry],
    older: CleanupOlder,
    now: SystemTime,
) -> CleanupPlan {
    CleanupPlan {
        older,
        items: cleanup_items(entries, older, now),
    }
}

pub(crate) fn cleanup_selected(
    items: &[CleanupItem],
    mut move_to_trash: impl FnMut(&Path) -> anyhow::Result<()>,
) -> CleanupReport {
    let mut report = CleanupReport::default();
    for item in items {
        cleanup_one(item, item.size, &mut move_to_trash, &mut report);
    }
    report
}

fn cleanup_one(
    item: &CleanupItem,
    size: u64,
    move_to_trash: &mut impl FnMut(&Path) -> anyhow::Result<()>,
    report: &mut CleanupReport,
) {
    let proxy = mediaa_media::preview_cache::proxy_path(&item.path);
    match move_to_trash(&item.path) {
        Ok(()) => {
            report.removed.push(RemovedItem {
                path: item.path.clone(),
                size,
                proxy: proxy.clone(),
            });
            if let Err(error) = std::fs::remove_file(&proxy) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    report.failures.push(CleanupFailure {
                        path: item.path.clone(),
                        error: format!("preview: {error}"),
                    });
                }
            }
        }
        Err(error) => report.failures.push(CleanupFailure {
            path: item.path.clone(),
            error: format!("{error:#}"),
        }),
    }
}

pub(crate) fn cleanup_old_selected(
    items: &[CleanupItem],
    older: CleanupOlder,
    now: SystemTime,
    mut move_to_trash: impl FnMut(&Path) -> anyhow::Result<()>,
) -> CleanupReport {
    let cutoff = older.cutoff(now);
    let mut report = CleanupReport::default();
    for item in items {
        let metadata = match std::fs::metadata(&item.path) {
            Ok(metadata) => metadata,
            Err(error) => {
                report.failures.push(CleanupFailure {
                    path: item.path.clone(),
                    error: format!("{error:#}"),
                });
                continue;
            }
        };
        if !metadata.is_file() {
            report.failures.push(CleanupFailure {
                path: item.path.clone(),
                error: "path is no longer a media file".into(),
            });
            continue;
        }
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(error) => {
                report.failures.push(CleanupFailure {
                    path: item.path.clone(),
                    error: format!("modified time: {error}"),
                });
                continue;
            }
        };
        if modified >= cutoff {
            report.skipped += 1;
            continue;
        }
        cleanup_one(item, metadata.len(), &mut move_to_trash, &mut report);
    }
    report
}
