use std::path::Path;
use std::time::{Duration, SystemTime};

pub(crate) const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SweepReport {
    pub previews: usize,
    pub atlas_pages: usize,
}

pub(crate) fn sweep(media_dir: &Path, now: SystemTime, ttl: Duration) -> SweepReport {
    let Some(cutoff) = now.checked_sub(ttl) else {
        return SweepReport::default();
    };
    let cache = media_dir.join(".cache");
    SweepReport {
        previews: mediaa_media::preview_cache::prune_previews(media_dir, cutoff),
        atlas_pages: mediaa_storage::thumb_cache::prune_pages(&cache, cutoff),
    }
}

#[cfg(test)]
mod tests {
    use super::sweep;
    use std::time::{Duration, SystemTime};

    fn age(path: &std::path::Path, time: SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(time)
            .unwrap();
    }

    #[test]
    fn stale_previews_and_atlas_pages_are_removed() {
        let dir = tempfile::tempdir().unwrap();
        let previews = dir.path().join(".cache").join("previews");
        let cache = dir.path().join(".cache");
        std::fs::create_dir_all(&previews).unwrap();
        let old_preview = previews.join("old.mp4");
        let fresh_preview = previews.join("hover-new.mp4");
        let old_page = cache.join("thumbs-0.jpg");
        let fresh_page = cache.join("thumbs-1.jpg");
        for path in [&old_preview, &old_page] {
            std::fs::write(path, b"old").unwrap();
            age(
                path,
                SystemTime::now() - Duration::from_secs(40 * 24 * 60 * 60),
            );
        }
        for path in [&fresh_preview, &fresh_page] {
            std::fs::write(path, b"new").unwrap();
        }

        let report = sweep(dir.path(), SystemTime::now(), super::CACHE_TTL);
        assert_eq!(report.previews, 1);
        assert_eq!(report.atlas_pages, 1);
        assert!(!old_preview.exists());
        assert!(fresh_preview.exists());
        assert!(!old_page.exists());
        assert!(fresh_page.exists());
    }
}
