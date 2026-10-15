//! Media file names: `<title>_<YYYY-MM-DD>_<HH-MM-SS>[-N].<ext>`.
//!
//! The title is what the user sees and may change (several files can share it); the date stamp
//! (plus a counter when two files were made in the same second) keeps the file name unique.
//! The library only lists files following this pattern.

use chrono::{DateTime, Local, NaiveDateTime};
use std::path::{Path, PathBuf};

/// Date part of a media file name.
const STAMP_FORMAT: &str = "%Y-%m-%d_%H-%M-%S";
/// Length of a stamp without the counter (`2026-10-14_03-28-08`).
const STAMP_LEN: usize = 19;
/// Longest title kept in a file name (characters).
pub const MAX_TITLE: usize = 120;

/// A media file name split into its parts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaName {
    pub title: String,
    pub taken: NaiveDateTime,
    /// Counter that keeps names made in the same second apart (`None` = first one).
    pub counter: Option<u32>,
}

impl MediaName {
    /// The stamp as written in the file name (`2026-10-14_03-28-08` or `…-2`).
    pub fn stamp(&self) -> String {
        let date = self.taken.format(STAMP_FORMAT);
        match self.counter {
            Some(n) => format!("{date}-{n}"),
            None => date.to_string(),
        }
    }

    pub fn stem(&self) -> String {
        format!("{}_{}", self.title, self.stamp())
    }
}

/// Splits a file stem (no extension) into title and stamp; `None` if it does not follow the
/// `<title>_<YYYY-MM-DD>_<HH-MM-SS>[-N]` pattern.
pub fn parse_stem(stem: &str) -> Option<MediaName> {
    // Optional counter after the time.
    let (base, counter) = match stem.rsplit_once('-') {
        Some((b, n)) if !n.is_empty() && n.len() <= 6 && n.bytes().all(|c| c.is_ascii_digit()) && b.len() >= STAMP_LEN + 2 => {
            match parse_tail(b) {
                Some(_) => (b, Some(n.parse().ok()?)),
                None => (stem, None),
            }
        }
        _ => (stem, None),
    };
    let (title, taken) = parse_tail(base)?;
    Some(MediaName { title: title.to_string(), taken, counter })
}

/// `"<title>_<stamp>"` → (title, date) when `base` ends with a valid stamp after a non-empty title.
fn parse_tail(base: &str) -> Option<(&str, NaiveDateTime)> {
    if base.len() < STAMP_LEN + 2 || !base.is_char_boundary(base.len() - STAMP_LEN) {
        return None;
    }
    let (head, stamp) = base.split_at(base.len() - STAMP_LEN);
    let title = head.strip_suffix('_')?;
    if title.trim().is_empty() {
        return None;
    }
    let taken = NaiveDateTime::parse_from_str(stamp, STAMP_FORMAT).ok()?;
    Some((title, taken))
}

/// Parses a media file path (`None` for files that do not follow the naming pattern).
pub fn parse_path(path: &Path) -> Option<MediaName> {
    parse_stem(path.file_stem()?.to_str()?)
}

/// Makes arbitrary text (a window title, a name typed by the user) safe for a file name.
pub fn sanitize(text: &str, max_chars: usize) -> String {
    let cleaned: String = text
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => ' ',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated: String = collapsed.chars().take(max_chars).collect();
    // Windows forbids trailing dots/spaces.
    let trimmed = truncated.trim_end_matches(['.', ' ']).to_string();
    if trimmed.is_empty() {
        "untitled".to_string()
    } else {
        trimmed
    }
}

/// First free `dir/<title>_<stamp>[-N].ext` for a file made at `taken` (`skip` = a path that
/// may be reused, the file being renamed).
fn free_path(dir: &Path, title: &str, taken: NaiveDateTime, ext: &str, skip: Option<&Path>) -> PathBuf {
    let ext = ext.trim_start_matches('.');
    let mut name = MediaName { title: sanitize(title, MAX_TITLE), taken, counter: None };
    loop {
        let p = dir.join(format!("{}.{ext}", name.stem()));
        if !p.exists() || skip.is_some_and(|s| s == p) {
            return p;
        }
        name.counter = Some(name.counter.map_or(2, |n| n + 1));
    }
}

/// Path for a new media file titled `title`, made `at` (unique in `dir`).
pub fn media_path(dir: &Path, title: &str, at: DateTime<Local>, ext: &str) -> PathBuf {
    free_path(dir, title, at.naive_local(), ext, None)
}

/// Renames a media file to `title`, keeping its date stamp; returns the new path.
pub fn rename(path: &Path, title: &str) -> anyhow::Result<PathBuf> {
    let name = parse_path(path).ok_or_else(|| anyhow::anyhow!("{} is not a MediaA file name", path.display()))?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or_default();
    // Same title: keep the exact name (and its counter).
    if sanitize(title, MAX_TITLE) == name.title {
        return Ok(path.to_path_buf());
    }
    let target = free_path(dir, title, name.taken, ext, Some(path));
    std::fs::rename(path, &target)?;
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at() -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, 13, 9, 5, 7).unwrap()
    }

    #[test]
    fn names_carry_the_title_and_a_date_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let p = media_path(dir.path(), "Screenshot", at(), ".png");
        assert_eq!(p.file_name().unwrap(), "Screenshot_2026-10-13_09-05-07.png");
        std::fs::write(&p, b"x").unwrap();
        let p2 = media_path(dir.path(), "Screenshot", at(), "png");
        assert_eq!(p2.file_name().unwrap(), "Screenshot_2026-10-13_09-05-07-2.png", "same second: counter");
        std::fs::write(&p2, b"x").unwrap();
        assert_eq!(media_path(dir.path(), "Screenshot", at(), "png").file_name().unwrap(), "Screenshot_2026-10-13_09-05-07-3.png");
        assert_eq!(media_path(dir.path(), "a/b: c?", at(), "png").file_name().unwrap(), "a b c_2026-10-13_09-05-07.png");
    }

    #[test]
    fn parsing_accepts_only_the_pattern() {
        let n = parse_stem("My_clip_2026-10-13_09-05-07").unwrap();
        assert_eq!((n.title.as_str(), n.counter), ("My_clip", None));
        assert_eq!(n.taken, at().naive_local());
        let n = parse_stem("Отпуск 2026_2026-10-13_09-05-07-12").unwrap();
        assert_eq!((n.title.as_str(), n.counter), ("Отпуск 2026", Some(12)));
        assert_eq!(n.stem(), "Отпуск 2026_2026-10-13_09-05-07-12");
        for bad in [
            "Screenshot 2026-10-13 09-05-07",
            "_2026-10-13_09-05-07",
            "x_2026-13-13_09-05-07",
            "x_2026-10-13_09-05",
            "x_2026-10-13_09-05-07-",
            "x2026-10-13_09-05-07",
            "photo",
            "",
        ] {
            assert_eq!(parse_stem(bad), None, "{bad}");
        }
        // A title that itself ends in a number is not mistaken for a counter.
        assert_eq!(parse_stem("Take-2_2026-10-13_09-05-07").unwrap().title, "Take-2");
        assert_eq!(parse_path(Path::new("/m/Rec [1 of 2]_2026-10-13_09-05-07.mp4")).unwrap().title, "Rec [1 of 2]");
    }

    #[test]
    fn rename_keeps_the_stamp_and_allows_equal_titles() {
        let dir = tempfile::tempdir().unwrap();
        let a = media_path(dir.path(), "Screenshot", at(), "png");
        std::fs::write(&a, b"a").unwrap();
        let later = Local.with_ymd_and_hms(2026, 10, 14, 1, 0, 0).unwrap();
        let b = media_path(dir.path(), "Recording", later, "mp4");
        std::fs::write(&b, b"b").unwrap();

        let a2 = rename(&a, "My").unwrap();
        assert_eq!(a2.file_name().unwrap(), "My_2026-10-13_09-05-07.png");
        assert!(!a.exists() && a2.exists());
        let b2 = rename(&b, "My").unwrap();
        assert_eq!(b2.file_name().unwrap(), "My_2026-10-14_01-00-00.mp4", "same title, different files");
        assert_eq!(rename(&a2, "My").unwrap(), a2, "unchanged title: unchanged name");

        // Same title and second: the counter keeps them apart.
        let c = media_path(dir.path(), "Other", at(), "png");
        std::fs::write(&c, b"c").unwrap();
        assert_eq!(rename(&c, "My").unwrap().file_name().unwrap(), "My_2026-10-13_09-05-07-2.png");
        assert!(rename(&dir.path().join("photo.png"), "x").is_err(), "foreign names are refused");
    }

    #[test]
    fn sanitize_strips_invalid_characters() {
        assert_eq!(sanitize("a/b\\c:d*e?f\"g<h>i|j", 100), "a b c d e f g h i j");
        assert_eq!(sanitize("  Visual   Studio Code.  ", 100), "Visual Studio Code");
        assert_eq!(sanitize("\u{7}", 10), "untitled");
        assert_eq!(sanitize("Привет мир", 6), "Привет");
    }
}
