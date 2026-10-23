use crate::formats::MediaKind;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Details {
    pub width: u32,
    pub height: u32,
    pub duration_secs: Option<f64>,
    pub audio: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LibraryEntry {
    pub path: PathBuf,
    pub name: String,
    pub title: String,
    pub kind: MediaKind,
    pub size: u64,
    pub modified: SystemTime,
    pub taken: chrono::NaiveDateTime,
    pub details: Option<Details>,
}

impl LibraryEntry {
    pub fn extension(&self) -> String {
        self.path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_uppercase()
    }

    pub fn file_url(&self) -> String {
        file_url(&self.path)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KindFilter {
    #[default]
    All,
    Only(MediaKind),
}

impl KindFilter {
    pub fn matches(self, kind: MediaKind) -> bool {
        match self {
            KindFilter::All => true,
            KindFilter::Only(k) => k == kind,
        }
    }
}

pub fn group_by_day<'a>(entries: &[&'a LibraryEntry]) -> Vec<(chrono::NaiveDate, Vec<&'a LibraryEntry>)> {
    let mut groups: Vec<(chrono::NaiveDate, Vec<&LibraryEntry>)> = Vec::new();
    for e in entries {
        let day = chrono::DateTime::<chrono::Local>::from(e.modified).date_naive();
        match groups.last_mut() {
            Some((d, list)) if *d == day => list.push(e),
            _ => groups.push((day, vec![e])),
        }
    }
    groups
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

pub fn human_duration(secs: f64) -> String {
    let total = secs.max(0.0).round() as u64;
    let (h, m, s) = (total / 3600, (total / 60) % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

pub fn file_url(path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let s = abs.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !s.starts_with('/') {
        out.push('/');
    }
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b'-' | b'_' | b'.' | b'~' | b':' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, modified: SystemTime) -> LibraryEntry {
        LibraryEntry { path: name.into(), name: name.into(), title: name.into(), kind: MediaKind::Image, size: 1, modified, taken: chrono::NaiveDateTime::default(), details: None }
    }

    #[test]
    fn groups_consecutive_entries_by_local_day() {
        use chrono::{Local, TimeZone};
        let at = |d: u32, h: u32| SystemTime::from(Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap());
        let list = [entry("a", at(14, 20)), entry("b", at(14, 9)), entry("c", at(12, 23)), entry("d", at(12, 1))];
        let refs: Vec<_> = list.iter().collect();
        let groups = group_by_day(&refs);
        let days: Vec<_> = groups.iter().map(|(d, l)| (d.to_string(), l.len())).collect();
        assert_eq!(days, [("2026-10-14".to_string(), 2), ("2026-10-12".to_string(), 2)]);
        assert!(group_by_day(&[]).is_empty());
    }

    #[test]
    fn sizes_and_durations() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
        assert_eq!(human_duration(65.4), "01:05");
        assert_eq!(human_duration(3725.0), "1:02:05");
    }

    #[test]
    fn file_url_escapes() {
        let url = file_url(Path::new("/tmp/my clip #1.mp4"));
        assert!(url.starts_with("file:///"), "{url}");
        assert!(url.ends_with("/my%20clip%20%231.mp4"), "{url}");
    }
}
