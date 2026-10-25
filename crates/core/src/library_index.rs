use crate::formats::MediaKind;
use crate::library::{Details, KindFilter, LibraryEntry};
use crate::naming;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const CACHE_DIR: &str = ".cache";
pub fn cache_dir(media_dir: &Path) -> PathBuf {
    media_dir.join(CACHE_DIR)
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    Newest,
    Oldest,
    Title,
    Largest,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Newest, Sort::Oldest, Sort::Title, Sort::Largest];

    pub fn by_date(self) -> bool {
        matches!(self, Sort::Newest | Sort::Oldest)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub kind: KindFilter,
    pub search: String,
    pub sort: Sort,
}

#[derive(Debug, Default)]
pub struct Search {
    title: Option<Regex>,
    date: Option<Regex>,
}

impl Search {
    pub fn parse(text: &str) -> Search {
        let (title, date) = text.split_once("&&").map_or((text, ""), |(t, d)| (t, d));
        Search {
            title: simple_pattern(title),
            date: simple_pattern(date),
        }
    }

    pub fn matches(&self, e: &LibraryEntry) -> bool {
        self.title.as_ref().is_none_or(|m| m.is_match(&e.title))
            && self
                .date
                .as_ref()
                .is_none_or(|m| m.is_match(&e.taken.format("%Y-%m-%d_%H-%M-%S").to_string()))
    }
}

pub fn simple_pattern(search: &str) -> Option<Regex> {
    let s = search.trim();
    if s.trim_matches('*').is_empty() {
        return None;
    }
    let (start, s) = s.strip_prefix('^').map_or((false, s), |r| (true, r));
    let (end, s) = s.strip_suffix('$').map_or((false, s), |r| (true, r));
    let body = s
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(".*");
    let pattern = format!(
        "(?i){}{body}{}",
        if start { "^" } else { "" },
        if end { "$" } else { "" }
    );
    Regex::new(&pattern).ok()
}

pub struct LibraryIndex {
    dir: PathBuf,
    entries: Vec<LibraryEntry>,
    version: u64,
    db: Option<rusqlite::Connection>,
}

fn to_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn from_ms(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

fn read_entry(path: &Path) -> Option<LibraryEntry> {
    entry_from(path, &std::fs::metadata(path).ok()?)
}

pub fn scan_dir(dir: &Path) -> Vec<LibraryEntry> {
    let Ok(read) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    read.flatten()
        .filter_map(|item| entry_from(&item.path(), &item.metadata().ok()?))
        .collect()
}

fn entry_from(path: &Path, meta: &std::fs::Metadata) -> Option<LibraryEntry> {
    if !meta.is_file() {
        return None;
    }
    let name = naming::parse_path(path)?;
    let kind = MediaKind::detect(path)?;
    Some(LibraryEntry {
        path: path.to_path_buf(),
        name: path.file_name()?.to_string_lossy().into_owned(),
        title: name.title,
        kind,
        size: meta.len(),
        modified: meta.modified().unwrap_or(UNIX_EPOCH),
        taken: name.taken,
        details: None,
    })
}

fn unchanged(a: &LibraryEntry, b: &LibraryEntry) -> bool {
    a.size == b.size && to_ms(a.modified) == to_ms(b.modified)
}

fn kind_code(kind: MediaKind) -> &'static str {
    match kind {
        MediaKind::Image => "image",
        MediaKind::Video => "video",
        MediaKind::Animation => "animation",
        MediaKind::Audio => "audio",
    }
}

fn kind_from(code: &str) -> Option<MediaKind> {
    [
        MediaKind::Image,
        MediaKind::Video,
        MediaKind::Animation,
        MediaKind::Audio,
    ]
    .into_iter()
    .find(|k| kind_code(*k) == code)
}

fn read_rows(db: &rusqlite::Connection, dir: &Path) -> rusqlite::Result<Vec<LibraryEntry>> {
    let mut q = db.prepare(
        "SELECT name, title, kind, size, modified_ms, width, height, duration, audio FROM files",
    )?;
    let rows = q.query_map([], |r| {
        let name: String = r.get(0)?;
        let kind: String = r.get(2)?;
        let width: Option<u32> = r.get(5)?;
        let details = width.map(|width| -> rusqlite::Result<Details> {
            Ok(Details {
                width,
                height: r.get::<_, Option<u32>>(6)?.unwrap_or(0),
                duration_secs: r.get(7)?,
                audio: r.get::<_, Option<bool>>(8)?.unwrap_or(false),
            })
        });
        Ok((
            name,
            r.get::<_, String>(1)?,
            kind,
            r.get::<_, i64>(3)?,
            r.get::<_, i64>(4)?,
            details.transpose()?,
        ))
    })?;
    let mut entries = Vec::new();
    for row in rows {
        let (name, title, kind, size, modified_ms, details) = row?;
        let (Some(parsed), Some(kind)) = (naming::parse_path(Path::new(&name)), kind_from(&kind))
        else {
            continue;
        };
        entries.push(LibraryEntry {
            path: dir.join(&name),
            name,
            title,
            kind,
            size: size as u64,
            modified: from_ms(modified_ms as u64),
            taken: parsed.taken,
            details,
        });
    }
    Ok(entries)
}

impl LibraryIndex {
    pub fn load(dir: &Path) -> LibraryIndex {
        let db = crate::cache_db::open(&cache_dir(dir))
            .map_err(|e| log::warn!("library index: {e:#}"))
            .ok();
        let mut entries = db.as_ref().map_or_else(Vec::new, |db| {
            read_rows(db, dir).unwrap_or_else(|e| {
                log::warn!("library index: {e:#}");
                Vec::new()
            })
        });
        entries.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.name.cmp(&b.name))
        });
        LibraryIndex {
            dir: dir.to_path_buf(),
            entries,
            version: 1,
            db,
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[LibraryEntry] {
        &self.entries
    }

    fn changed(&mut self) {
        self.version += 1;
        self.entries.sort_by(|a, b| {
            b.modified
                .cmp(&a.modified)
                .then_with(|| a.name.cmp(&b.name))
        });
    }

    fn store(&self, upsert: &[&LibraryEntry], delete: &[&str]) {
        let Some(db) = &self.db else { return };
        if upsert.is_empty() && delete.is_empty() {
            return;
        }
        let write = || -> rusqlite::Result<()> {
            let tx = db.unchecked_transaction()?;
            {
                let mut put = tx.prepare_cached(
                    "INSERT OR REPLACE INTO files (name, title, kind, size, modified_ms, width, height, duration, audio)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                )?;
                for e in upsert {
                    let d = e.details;
                    put.execute(rusqlite::params![
                        e.name,
                        e.title,
                        kind_code(e.kind),
                        e.size as i64,
                        to_ms(e.modified) as i64,
                        d.map(|d| d.width),
                        d.map(|d| d.height),
                        d.and_then(|d| d.duration_secs),
                        d.map(|d| d.audio),
                    ])?;
                }
                let mut del = tx.prepare_cached("DELETE FROM files WHERE name = ?1")?;
                for name in delete {
                    del.execute([name])?;
                }
            }
            tx.commit()
        };
        if let Err(e) = write() {
            log::warn!("library index: {e}");
        }
    }

    pub fn apply_scan(&mut self, listing: Vec<LibraryEntry>) {
        let mut known: HashMap<String, LibraryEntry> = self
            .entries
            .drain(..)
            .map(|e| (e.name.clone(), e))
            .collect();
        let mut fresh = Vec::with_capacity(listing.len());
        let mut updated = Vec::new();
        for mut entry in listing {
            match known.remove(&entry.name) {
                Some(old) if unchanged(&old, &entry) => entry = old,
                _ => updated.push(fresh.len()),
            }
            fresh.push(entry);
        }
        self.entries = fresh;
        let gone: Vec<&str> = known.keys().map(String::as_str).collect();
        self.store(
            &updated
                .iter()
                .map(|&i| &self.entries[i])
                .collect::<Vec<_>>(),
            &gone,
        );
        if updated.is_empty() && gone.is_empty() {
            self.entries.sort_by(|a, b| {
                b.modified
                    .cmp(&a.modified)
                    .then_with(|| a.name.cmp(&b.name))
            });
        } else {
            self.changed();
        }
    }

    pub fn refresh_paths(&mut self, paths: &[PathBuf]) {
        let mut upserts: Vec<LibraryEntry> = Vec::new();
        let mut deletes: Vec<String> = Vec::new();
        for path in paths {
            if path.parent() != Some(self.dir.as_path()) {
                continue;
            }
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let pos = self.entries.iter().position(|e| e.name == name);
            match (read_entry(path), pos) {
                (Some(e), Some(i)) if unchanged(&self.entries[i], &e) => {}
                (Some(e), Some(i)) => {
                    self.entries[i] = e.clone();
                    upserts.push(e);
                }
                (Some(e), None) => {
                    self.entries.push(e.clone());
                    upserts.push(e);
                }
                (None, Some(i)) => {
                    self.entries.remove(i);
                    deletes.push(name);
                }
                (None, None) => {}
            }
        }
        if upserts.is_empty() && deletes.is_empty() {
            return;
        }
        self.store(
            &upserts.iter().collect::<Vec<_>>(),
            &deletes.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        self.changed();
    }

    pub fn set_details(&mut self, path: &Path, details: Details) {
        let Some(i) = self.entries.iter().position(|e| e.path == path) else {
            return;
        };
        if self.entries[i].details != Some(details) {
            self.entries[i].details = Some(details);
            self.store(&[&self.entries[i]], &[]);
        }
    }

    pub fn query(&self, q: &Query) -> Vec<usize> {
        let search = Search::parse(&q.search);
        let mut hits: Vec<usize> = (0..self.entries.len())
            .filter(|&i| {
                let e = &self.entries[i];
                q.kind.matches(e.kind) && search.matches(e)
            })
            .collect();
        let e = &self.entries;
        match q.sort {
            Sort::Newest => {}
            Sort::Oldest => hits.reverse(),
            Sort::Title => hits.sort_by(|&a, &b| {
                e[a].title
                    .to_lowercase()
                    .cmp(&e[b].title.to_lowercase())
                    .then(e[b].modified.cmp(&e[a].modified))
            }),
            Sort::Largest => hits.sort_by(|&a, &b| e[b].size.cmp(&e[a].size)),
        }
        hits
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Change {
    Path(PathBuf),
    Rescan,
}

fn changes_of(res: notify::Result<notify::Event>) -> Vec<Change> {
    match res {
        Ok(event) if event.need_rescan() => vec![Change::Rescan],
        Ok(event) => event
            .paths
            .into_iter()
            .filter(|p| p.file_name().is_some_and(|n| n != CACHE_DIR))
            .map(Change::Path)
            .collect(),
        Err(e) => {
            log::warn!("library watcher: {e}");
            vec![Change::Rescan]
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    pub paths: Vec<PathBuf>,
    pub rescan: bool,
}

pub struct LibraryWatcher {
    _watcher: notify::RecommendedWatcher,
    rx: crossbeam_channel::Receiver<Change>,
}

impl LibraryWatcher {
    pub fn new(
        dir: &Path,
        on_change: impl Fn() + Send + 'static,
    ) -> anyhow::Result<LibraryWatcher> {
        use notify::Watcher;
        std::fs::create_dir_all(dir)?;
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                for c in changes_of(res) {
                    let _ = tx.send(c);
                }
                on_change();
            })?;
        watcher.watch(dir, notify::RecursiveMode::NonRecursive)?;
        Ok(LibraryWatcher {
            _watcher: watcher,
            rx,
        })
    }

    pub fn changes(&self) -> Changes {
        let mut out = Changes::default();
        for c in self.rx.try_iter() {
            match c {
                Change::Path(p) => out.paths.push(p),
                Change::Rescan => out.rescan = true,
            }
        }
        out.paths.sort();
        out.paths.dedup();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(dir: &Path) -> LibraryIndex {
        let mut index = LibraryIndex::load(dir);
        index.apply_scan(scan_dir(dir));
        index
    }

    fn touch(dir: &Path, name: &str, age_secs: u64, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        std::fs::File::options()
            .write(true)
            .open(&p)
            .unwrap()
            .set_modified(t)
            .unwrap();
        p
    }

    fn names(index: &LibraryIndex, q: &Query) -> Vec<String> {
        index
            .query(q)
            .into_iter()
            .map(|i| index.entries()[i].title.clone())
            .collect()
    }

    #[test]
    fn search_patterns() {
        let m = |s: &str, title: &str| simple_pattern(s).unwrap().is_match(title);
        assert!(simple_pattern("  ").is_none() && simple_pattern("*").is_none());
        assert!(m("demo", "Window DEMO 2"), "substring, any case");
        assert!(m("win*2", "Window demo 2"));
        assert!(!m("^demo", "Window demo"), "anchored start");
        assert!(m("^win", "Window demo"));
        assert!(m("demo$", "Window demo") && !m("demo$", "Window demo 2"));
        assert!(m("^*demo*$", "x demo y"));
        assert!(
            m("a.b(1)", "x a.b(1) y") && !m("a.b(1)", "axb1"),
            "other characters are literal"
        );
        assert!(m("ОТПУСК", "отпуск 2026"), "unicode case-insensitive");
    }

    #[test]
    fn index_tracks_the_folder_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Old_2026-01-01_10-00-00.png", 300, b"old");
        touch(
            dir.path(),
            "Demo clip_2026-01-02_10-00-00.mp4",
            100,
            b"video data",
        );
        touch(dir.path(), "Demo_2026-01-03_10-00-00.gif", 10, b"gif");
        touch(dir.path(), "notes.txt", 1, b"x");
        touch(dir.path(), "photo.png", 1, b"x");

        let mut index = open(dir.path());
        assert_eq!(
            names(&index, &Query::default()),
            ["Demo", "Demo clip", "Old"],
            "newest first, foreign files skipped"
        );
        let clip = &index.entries()[1];
        assert_eq!(
            (clip.kind, clip.size, clip.extension().as_str()),
            (MediaKind::Video, 10, "MP4")
        );
        let gif = dir.path().join("Demo_2026-01-03_10-00-00.gif");
        index.set_details(
            &gif,
            Details {
                width: 64,
                height: 48,
                duration_secs: Some(1.5),
                audio: false,
            },
        );
        assert!(
            dir.path().join(".cache").join("library.db").is_file(),
            "stored in the cache folder"
        );
        assert_eq!(
            names(&LibraryIndex::load(dir.path()), &Query::default()),
            ["Demo", "Demo clip", "Old"],
            "written at once, loads without a scan"
        );

        let reopened = open(dir.path());
        assert_eq!(reopened.len(), 3);
        let e = reopened.entries().iter().find(|e| e.path == gif).unwrap();
        assert_eq!(e.details.unwrap().width, 64);
        assert_eq!(reopened.version(), 1, "nothing changed on disk");

        std::fs::remove_file(dir.path().join("Old_2026-01-01_10-00-00.png")).unwrap();
        touch(dir.path(), "New_2026-01-04_10-00-00.png", 0, b"new");
        touch(
            dir.path(),
            "Demo_2026-01-03_10-00-00.gif",
            0,
            b"gif changed",
        );
        let reopened = open(dir.path());
        assert_eq!(
            names(&reopened, &Query::default()),
            ["Demo", "New", "Demo clip"]
        );
        assert!(
            reopened.entries()[0].details.is_none(),
            "changed file: details dropped"
        );
        assert!(reopened.version() > 1);
        drop(reopened);
        let stored = LibraryIndex::load(dir.path());
        assert_eq!(
            names(&stored, &Query::default()),
            ["Demo", "New", "Demo clip"],
            "scan results are stored too"
        );
        assert!(stored
            .entries()
            .iter()
            .find(|e| e.name.starts_with("Demo clip"))
            .is_some_and(|e| e.details.is_none()));
    }

    #[test]
    fn single_file_updates() {
        let dir = tempfile::tempdir().unwrap();
        let a = touch(dir.path(), "A_2026-01-01_10-00-00.png", 50, b"a");
        let mut index = open(dir.path());
        let v = index.version();
        let b = touch(dir.path(), "B_2026-01-02_10-00-00.png", 0, b"b");
        index.refresh_paths(std::slice::from_ref(&b));
        assert_eq!(index.len(), 2);
        assert!(index.version() > v);
        let renamed = naming::rename(&a, "Z").unwrap();
        index.refresh_paths(&[a.clone(), renamed.clone()]);
        assert_eq!(names(&index, &Query::default()), ["B", "Z"]);
        assert_eq!(
            names(&LibraryIndex::load(dir.path()), &Query::default()),
            ["B", "Z"],
            "single updates are stored"
        );
        let v = index.version();
        index.refresh_paths(&[dir.path().join("notes.txt"), b.clone()]);
        assert_eq!(index.version(), v, "no-op updates keep the version");
    }

    #[test]
    fn query_filters_searches_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "Beta_2026-01-01_10-00-00.png", 30, b"12345");
        touch(dir.path(), "alpha_2026-01-02_10-00-00.mp4", 20, b"1");
        touch(dir.path(), "Gamma demo_2026-01-03_10-00-00.gif", 10, b"123");
        let index = open(dir.path());
        let q = |kind, search: &str, sort| Query {
            kind,
            search: search.into(),
            sort,
        };
        assert_eq!(
            names(&index, &q(KindFilter::All, "", Sort::Oldest)),
            ["Beta", "alpha", "Gamma demo"]
        );
        assert_eq!(
            names(&index, &q(KindFilter::All, "", Sort::Title)),
            ["alpha", "Beta", "Gamma demo"]
        );
        assert_eq!(
            names(&index, &q(KindFilter::All, "", Sort::Largest)),
            ["Beta", "Gamma demo", "alpha"]
        );
        assert_eq!(
            names(
                &index,
                &q(KindFilter::Only(MediaKind::Video), "", Sort::Newest)
            ),
            ["alpha"]
        );
        assert_eq!(
            names(&index, &q(KindFilter::All, "A", Sort::Title)),
            ["alpha", "Beta", "Gamma demo"]
        );
        assert_eq!(
            names(&index, &q(KindFilter::All, "^a", Sort::Title)),
            ["alpha"]
        );
        assert_eq!(
            names(&index, &q(KindFilter::All, "*demo$", Sort::Newest)),
            ["Gamma demo"]
        );
        assert!(names(&index, &q(KindFilter::All, "2026", Sort::Newest)).is_empty());
        assert_eq!(
            names(&index, &q(KindFilter::All, "*&&2026-01-0", Sort::Title)),
            ["alpha", "Beta", "Gamma demo"]
        );
        assert_eq!(
            names(&index, &q(KindFilter::All, "*&& 2026-01-02", Sort::Newest)),
            ["alpha"]
        );
        assert_eq!(
            names(
                &index,
                &q(KindFilter::All, "a && ^2026-01-0*_10$", Sort::Title)
            )
            .len(),
            0,
            "time part anchored at end"
        );
        assert_eq!(
            names(
                &index,
                &q(KindFilter::All, "a && ^2026-01-0*00$", Sort::Title)
            ),
            ["alpha", "Beta", "Gamma demo"]
        );
        assert_eq!(
            names(&index, &q(KindFilter::All, "demo&&01-03", Sort::Title)),
            ["Gamma demo"]
        );
    }

    #[test]
    fn watcher_reports_new_files() {
        let dir = tempfile::tempdir().unwrap();
        let watcher = LibraryWatcher::new(dir.path(), || {}).unwrap();
        let p = touch(dir.path(), "X_2026-01-01_10-00-00.png", 0, b"x");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while std::time::Instant::now() < deadline && !seen.contains(&p) {
            seen.extend(watcher.changes().paths);
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            seen.iter().any(|s| s.file_name() == p.file_name()),
            "{seen:?}"
        );
    }

    #[test]
    fn lost_events_and_watch_errors_ask_for_a_rescan() {
        use notify::event::{Event, EventKind, Flag};
        let overflow = Event::new(EventKind::Other).set_flag(Flag::Rescan);
        assert_eq!(changes_of(Ok(overflow)), [Change::Rescan]);
        assert_eq!(
            changes_of(Err(notify::Error::generic("watch lost"))),
            [Change::Rescan]
        );
        let event = Event::new(EventKind::Any)
            .add_path("/m/A_2026-01-01_10-00-00.png".into())
            .add_path("/m/.cache".into());
        assert_eq!(
            changes_of(Ok(event)),
            [Change::Path("/m/A_2026-01-01_10-00-00.png".into())],
            "cache folder ignored"
        );
    }
}
