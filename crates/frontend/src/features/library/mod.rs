pub(crate) mod cleanup;
pub(crate) mod player;
pub mod view;
pub(crate) mod viewer;

#[cfg(test)]
use crate::features::jobs::Job;
#[cfg(test)]
use cleanup::CleanupFailure;
use cleanup::{
    cleanup_old_selected, cleanup_selected, plan_cleanup, CleanupItem, CleanupOlder, CleanupPlan,
    CleanupReport,
};
use mediaa_core::library::{human_size, Details, LibraryEntry};
use mediaa_core::wake::Wake;
use mediaa_core::{MediaKind, Settings};
#[cfg(test)]
use mediaa_platform::files;
use rust_i18n::t;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(test)]
use std::time::Duration;
use std::time::{Instant, SystemTime};
pub use view::*;
use viewer::Viewer;

pub(crate) const STRIP_PAD: i8 = 8;
pub(crate) const MIN_TILE: f32 = 200.0;
pub(crate) const TILE_BORDER: f32 = 1.0;
pub(crate) const GAP: f32 = 10.0;
pub(crate) const MEDIA_ASPECT: f32 = 0.62;
pub(crate) const SCRUB_ZONE: f32 = 26.0;
pub(crate) const CHUNK: usize = 300;
#[cfg(test)]
#[cfg(test)]
const RESCAN_EVERY: Duration = Duration::from_secs(60);

#[cfg(test)]
type TestTrash = Arc<dyn Fn(&Path) -> anyhow::Result<()> + Send + Sync>;

pub struct LibraryController {
    pub(crate) index: Option<LibraryView>,

    #[cfg(test)]
    pub(crate) scan_job: Option<Job<Vec<mediaa_core::library::LibraryEntry>>>,
    #[cfg(test)]
    pub(crate) changed_while_scanning: Vec<PathBuf>,
    pub(crate) last_scan: Instant,
    pub(crate) query: Query,
    pub(crate) hits: Option<Arc<HitList>>,
    pub(crate) limit: usize,
    pub(crate) confirm_delete: Option<PathBuf>,
    pub(crate) confirm_delete_many: Option<Vec<PathBuf>>,
    pub(crate) confirm_cleanup: Option<CleanupPlan>,
    #[cfg(test)]
    pub(crate) removal_job: Option<RemovalJob>,
    #[cfg(not(test))]
    pub(crate) pending_removal: Option<RemovalRequest>,
    pub(crate) renaming: Option<(PathBuf, String)>,
    pub(crate) renaming_many: Option<(Vec<PathBuf>, String)>,
    pub(crate) selected: Vec<PathBuf>,
    pub(crate) anchor: Option<PathBuf>,
    pub(crate) last_query: Query,
    pub(crate) hovered: Option<PathBuf>,
    pub(crate) hover_next: Option<PathBuf>,
    pub(crate) previews_enabled: bool,
    pub(crate) empty_cache_purged: bool,
    pub(crate) prewarm_done: bool,
    pub(crate) viewer: Option<Viewer>,
    pub(crate) dev_viewed: bool,
    #[cfg(test)]
    trash: Option<TestTrash>,
}

pub(crate) struct HitList {
    pub(crate) version: u64,
    pub(crate) query: Query,
    pub(crate) limit: usize,
    pub(crate) indices: Arc<[usize]>,
    pub(crate) entries: Arc<[LibraryEntry]>,
}

pub(crate) enum RemovalRequest {
    Selected { item: CleanupItem, success: String },
    SelectedMany(Vec<CleanupItem>),
    OlderThan(CleanupPlan),
}

#[cfg(test)]
pub(crate) struct RemovalJob {
    pub(crate) job: Job<CleanupReport>,
    pub(crate) success: String,
}

impl Default for LibraryController {
    fn default() -> Self {
        Self {
            index: None,

            #[cfg(test)]
            scan_job: None,
            #[cfg(test)]
            changed_while_scanning: Vec::new(),
            last_scan: Instant::now(),
            query: Query::default(),
            hits: None,
            limit: CHUNK,
            confirm_delete: None,
            confirm_delete_many: None,
            confirm_cleanup: None,
            #[cfg(test)]
            removal_job: None,
            #[cfg(not(test))]
            pending_removal: None,
            renaming: None,
            renaming_many: None,
            selected: Vec::new(),
            anchor: None,
            last_query: Query::default(),
            hovered: None,
            hover_next: None,
            previews_enabled: true,
            empty_cache_purged: false,
            prewarm_done: false,
            viewer: None,
            dev_viewed: false,
            #[cfg(test)]
            trash: None,
        }
    }
}

pub fn library_dir(settings: &Settings) -> PathBuf {
    crate::dev_mode::mode()
        .library
        .clone()
        .unwrap_or_else(|| settings.media_dir.clone())
}

#[cfg(test)]
pub(crate) fn scan_test_dir(dir: &Path) -> Vec<LibraryEntry> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let kind = MediaKind::detect(&path)?;
            let name = mediaa_core::naming::parse_path(&path)?;
            Some(LibraryEntry {
                path: path.clone(),
                name: path.file_name()?.to_string_lossy().into_owned(),
                title: name.title,
                kind,
                size: metadata.len(),
                modified: metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
                taken: name.taken,
                details: None,
            })
        })
        .collect()
}

impl LibraryController {
    #[cfg(test)]
    pub(crate) fn set_test_entries(&mut self, dir: &Path) {
        let entries = scan_test_dir(dir);
        self.index = Some(LibraryView::from_entries(dir, entries, 1));
        self.hits = None;
        self.last_scan = Instant::now();
    }

    #[cfg(test)]
    pub(crate) fn set_test_trash(&mut self) {
        self.trash = Some(Arc::new(|path| {
            std::fs::remove_file(path)?;
            Ok(())
        }));
    }

    pub(crate) fn selected_count(&self) -> usize {
        self.selected.len()
    }

    pub(crate) fn select_click(&mut self, path: &Path, ctrl: bool, shift: bool) {
        if shift {
            let entries: &[LibraryEntry] = self.hits.as_ref().map_or(&[], |h| &h.entries);
            let anchor = self.anchor.clone().unwrap_or_else(|| path.to_path_buf());
            let (Some(a), Some(b)) = (
                entries.iter().position(|e| e.path == anchor),
                entries.iter().position(|e| e.path == path),
            ) else {
                self.selected = vec![path.to_path_buf()];
                self.anchor = Some(path.to_path_buf());
                return;
            };
            let (from, to) = if a <= b { (a, b) } else { (b, a) };
            self.selected = entries[from..=to].iter().map(|e| e.path.clone()).collect();
            return;
        }
        if ctrl {
            if let Some(index) = self.selected.iter().position(|p| p == path) {
                self.selected.remove(index);
            } else {
                self.selected.push(path.to_path_buf());
            }
            self.anchor = Some(path.to_path_buf());
            return;
        }
        self.selected = vec![path.to_path_buf()];
        self.anchor = Some(path.to_path_buf());
    }

    pub(crate) fn toggle_selected(&mut self, path: &Path) {
        if let Some(index) = self.selected.iter().position(|p| p == path) {
            self.selected.remove(index);
        } else {
            self.selected.push(path.to_path_buf());
        }
        self.anchor = Some(path.to_path_buf());
    }

    pub(crate) fn select_day(&mut self, entries: &[&LibraryEntry]) {
        let all_selected = !entries.is_empty()
            && entries
                .iter()
                .all(|entry| self.selected.iter().any(|path| path == &entry.path));
        if all_selected {
            let paths: Vec<&PathBuf> = entries.iter().map(|entry| &entry.path).collect();
            self.selected.retain(|path| !paths.contains(&path));
            self.anchor = None;
        } else {
            for entry in entries {
                if !self.selected.contains(&entry.path) {
                    self.selected.push(entry.path.clone());
                }
            }
            self.anchor = entries.last().map(|entry| entry.path.clone());
        }
    }

    pub(crate) fn clear_selection(&mut self) {
        self.selected.clear();
        self.anchor = None;
    }

    pub(crate) fn rename_selected(&mut self, old: &Path, new: &Path) {
        for path in &mut self.selected {
            if path == old {
                *path = new.to_path_buf();
            }
        }
        if self.anchor.as_deref() == Some(old) {
            self.anchor = Some(new.to_path_buf());
        }
    }

    pub(crate) fn remove_selected(&mut self, removed: &[PathBuf]) {
        self.selected.retain(|path| !removed.contains(path));
        if self
            .anchor
            .as_ref()
            .is_some_and(|path| removed.contains(path))
        {
            self.anchor = None;
        }
    }
    #[cfg(test)]
    pub(crate) fn selected_for_test(&self) -> Vec<PathBuf> {
        self.selected.clone()
    }

    #[cfg(test)]
    pub(crate) fn select_for_test(&mut self, paths: Vec<PathBuf>) {
        self.selected = paths;
        self.anchor = self.selected.last().cloned();
    }

    #[cfg(test)]
    pub(crate) fn begin_bulk_rename_for_test(&mut self, paths: Vec<PathBuf>) {
        self.renaming_many = Some((paths, String::new()));
    }

    #[cfg(test)]
    pub(crate) fn bulk_rename_title_for_test(&self) -> Option<&str> {
        self.renaming_many.as_ref().map(|(_, title)| title.as_str())
    }

    pub(crate) fn delete_many(&mut self, paths: Vec<PathBuf>) -> anyhow::Result<()> {
        let items = paths
            .into_iter()
            .map(|path| {
                let known = self.index.as_ref().and_then(|index| {
                    index
                        .entries()
                        .iter()
                        .find(|entry| entry.path == path)
                        .map(|entry| (entry.size, entry.modified))
                });
                let metadata = std::fs::metadata(&path).ok();
                CleanupItem {
                    size: known
                        .map(|(size, _)| size)
                        .or_else(|| metadata.as_ref().map(|m| m.len()))
                        .unwrap_or(0),
                    modified: known
                        .map(|(_, modified)| modified)
                        .or_else(|| metadata.and_then(|m| m.modified().ok())),
                    path,
                }
            })
            .collect();
        self.start_removal(RemovalRequest::SelectedMany(items))
    }

    pub fn viewer_open(&self) -> bool {
        self.viewer.is_some()
    }

    pub(crate) fn refresh(&mut self, settings: &Settings) {
        let dir = library_dir(settings);
        #[cfg(test)]
        if self.scan_job.is_some() {
            if self.index.as_ref().is_some_and(|index| index.dir() == dir) {
                return;
            }
            if let Some(job) = self.scan_job.take() {
                job.cancel();
            }
        }
        self.prewarm_done = false;
        if self.index.as_ref().is_none_or(|i| i.dir() != dir) {
            self.index = Some(LibraryView::from_entries(&dir, Vec::new(), 0));
            return;
        }
        #[cfg(not(test))]
        {
            self.last_scan = Instant::now();
        }
        #[cfg(test)]
        {
            let worker_dir = dir.clone();
            let spawned = Job::spawn("library-scan", Arc::default(), move || {
                scan_test_dir(&worker_dir)
            });
            self.scan_job = match spawned {
                Ok(job) => Some(job),
                Err(error) => {
                    log::warn!("library scan thread: {error}");
                    None
                }
            };
            self.changed_while_scanning.clear();
            self.last_scan = Instant::now();
        }
    }

    pub(crate) fn poll_background(
        &mut self,
        settings: &Settings,
        wake: &Wake,
    ) -> Option<(PathBuf, HashMap<String, SystemTime>)> {
        let sync = self.poll(settings, wake);
        if sync.is_some() {
            self.prewarm_done = false;
        }
        sync
    }

    pub(crate) fn prewarm(&mut self) -> Option<Vec<mediaa_core::library::LibraryEntry>> {
        if self.prewarm_done {
            return None;
        }
        let entries = self.index.as_ref()?.entries().to_vec();
        self.prewarm_done = true;
        Some(entries)
    }

    pub(crate) fn poll(
        &mut self,
        settings: &Settings,
        wake: &Wake,
    ) -> Option<(PathBuf, HashMap<String, SystemTime>)> {
        if self
            .index
            .as_ref()
            .is_none_or(|i| i.dir() != library_dir(settings))
        {
            self.refresh(settings);
            return None;
        }
        #[cfg(test)]
        {
            let index = self.index.as_mut()?;
            index.flush_cache();
            let mut scanned = None;
            if self.last_scan.elapsed() > RESCAN_EVERY && self.scan_job.is_none() {
                self.refresh(settings);
                return None;
            }
            let index = self.index.as_mut()?;
            if let Some(job) = self.scan_job.as_mut() {
                match job.poll() {
                    Some(Ok(listing)) => {
                        index.apply_scan(listing);
                        index.refresh_paths(&std::mem::take(&mut self.changed_while_scanning));
                        self.scan_job = None;
                        scanned = Some((
                            index.dir().to_path_buf(),
                            index
                                .entries()
                                .iter()
                                .map(|entry| (entry.name.clone(), entry.modified))
                                .collect(),
                        ));
                    }
                    None => wake.after(Duration::from_millis(50)),
                    Some(Err(error)) => {
                        self.scan_job = None;
                        log::warn!("library scan stopped: {error:#}");
                    }
                }
            }
            scanned
        }
        #[cfg(not(test))]
        {
            let _ = wake;
            None
        }
    }

    pub(crate) fn hits(&mut self) -> Option<Arc<HitList>> {
        let index = self.index.as_ref()?;
        let query_changed = self
            .hits
            .as_ref()
            .is_none_or(|hits| hits.query != self.query);
        if query_changed {
            self.limit = CHUNK;
        }
        let rebuild = self.hits.as_ref().is_none_or(|hits| {
            hits.version != index.version() || hits.query != self.query || hits.limit != self.limit
        });
        if !rebuild {
            return self.hits.clone();
        }
        let indices: Arc<[usize]> = if query_changed
            || self
                .hits
                .as_ref()
                .is_none_or(|hits| hits.version != index.version())
        {
            index.query(&self.query).into()
        } else {
            self.hits
                .as_ref()
                .map(|hits| hits.indices.clone())
                .unwrap_or_default()
        };
        let entries: Arc<[LibraryEntry]> = indices
            .iter()
            .take(self.limit)
            .filter_map(|&entry| index.entries().get(entry).cloned())
            .collect::<Vec<_>>()
            .into();
        self.hits = Some(Arc::new(HitList {
            version: index.version(),
            query: self.query.clone(),
            limit: self.limit,
            indices,
            entries,
        }));
        self.hits.clone()
    }

    pub(crate) fn set_details(&mut self, path: &Path, details: Details) {
        if let Some(i) = self.index.as_mut() {
            i.set_details(path, details);
        }
    }

    pub(crate) fn apply_snapshot(
        &mut self,
        dir: PathBuf,
        entries: Vec<LibraryEntry>,
        version: u64,
    ) {
        self.index = Some(LibraryView::from_entries(&dir, entries, version));
        self.hits = None;
        self.last_scan = Instant::now();
    }

    pub fn flush_details(&mut self) {
        if let Some(index) = self.index.as_mut() {
            index.flush_details();
        }
    }

    pub fn flush(&mut self) {
        #[cfg(test)]
        if let Some(index) = self.index.as_mut() {
            index.flush();
        }
    }

    pub(crate) fn cleanup_plan(&self, older: CleanupOlder) -> CleanupPlan {
        let entries = self
            .index
            .as_ref()
            .map(LibraryView::entries)
            .unwrap_or_default();
        plan_cleanup(entries, older, SystemTime::now())
    }

    pub(crate) fn prepare_for_removal(&mut self, items: &[CleanupItem]) {
        if self
            .viewer
            .as_ref()
            .is_some_and(|viewer| items.iter().any(|item| item.path == viewer.path()))
        {
            self.viewer = None;
        }
    }

    pub fn cleanup_active(&self) -> bool {
        #[cfg(not(test))]
        if self.pending_removal.is_some() {
            return true;
        }
        #[cfg(test)]
        if self.removal_job.is_some() {
            return true;
        }
        false
    }

    pub(crate) fn start_cleanup(&mut self, plan: &CleanupPlan) -> anyhow::Result<()> {
        self.start_removal(RemovalRequest::OlderThan(plan.clone()))
    }

    pub(crate) fn start_delete(&mut self, path: PathBuf) -> anyhow::Result<()> {
        let modified = std::fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok());
        let item = CleanupItem {
            path,
            size: 0,
            modified,
        };
        let success = t!(
            "lib.deleted",
            name = item.path.file_name().unwrap_or_default().to_string_lossy()
        )
        .into_owned();
        self.start_removal(RemovalRequest::Selected { item, success })
    }

    pub(crate) fn start_removal(&mut self, request: RemovalRequest) -> anyhow::Result<()> {
        if self.cleanup_active() {
            anyhow::bail!("cleanup is already running");
        }
        let (items, success) = match &request {
            RemovalRequest::Selected { item, success } => (vec![item.clone()], success.clone()),
            RemovalRequest::SelectedMany(items) => (
                items.clone(),
                t!(
                    "lib.deleted_many",
                    count = items.len(),
                    files = crate::i18n::files(items.len())
                )
                .into_owned(),
            ),
            RemovalRequest::OlderThan(plan) => (
                plan.items.clone(),
                t!(
                    "lib.cleanup_done",
                    count = plan.items.len(),
                    size = human_size(plan.bytes())
                )
                .into_owned(),
            ),
        };
        self.prepare_for_removal(&items);
        #[cfg(not(test))]
        {
            let _ = &success;
            self.pending_removal = Some(request);
            Ok(())
        }
        #[cfg(test)]
        {
            let worker_request = request;
            let trash = self.trash.clone();
            let job = Job::spawn("library-removal", Arc::default(), move || {
                let delete = move |path: &Path| -> anyhow::Result<()> {
                    if !path.exists() {
                        return Ok(());
                    }
                    if let Some(trash) = &trash {
                        return trash(path);
                    }
                    files::move_to_trash(path)
                };
                match worker_request {
                    RemovalRequest::Selected { item, .. } => cleanup_selected(&[item], delete),
                    RemovalRequest::SelectedMany(items) => cleanup_selected(&items, delete),
                    RemovalRequest::OlderThan(plan) => {
                        cleanup_old_selected(&plan.items, plan.older, SystemTime::now(), delete)
                    }
                }
            })?;
            self.removal_job = Some(RemovalJob { job, success });
            Ok(())
        }
    }

    #[cfg(not(test))]
    pub(crate) fn take_removal(&mut self) -> Option<RemovalRequest> {
        self.pending_removal.take()
    }

    pub(crate) fn poll_cleanup(&mut self, wake: &Wake) -> Option<(CleanupReport, String)> {
        #[cfg(not(test))]
        {
            let request = self.pending_removal.take()?;
            let (items, success) = match &request {
                RemovalRequest::Selected { item, success } => (vec![item.clone()], success.clone()),
                RemovalRequest::SelectedMany(items) => (
                    items.clone(),
                    t!(
                        "lib.deleted_many",
                        count = items.len(),
                        files = crate::i18n::files(items.len())
                    )
                    .into_owned(),
                ),
                RemovalRequest::OlderThan(plan) => (
                    plan.items.clone(),
                    t!(
                        "lib.cleanup_done",
                        count = plan.items.len(),
                        size = human_size(plan.bytes())
                    )
                    .into_owned(),
                ),
            };
            let report = match request {
                RemovalRequest::Selected { item, .. } => cleanup_selected(&[item], |_| Ok(())),
                RemovalRequest::SelectedMany(items) => cleanup_selected(&items, |_| Ok(())),
                RemovalRequest::OlderThan(plan) => {
                    cleanup_old_selected(&plan.items, plan.older, SystemTime::now(), |_| Ok(()))
                }
            };
            let _ = items;
            let _ = wake;
            Some((report, success))
        }
        #[cfg(test)]
        {
            let job = self.removal_job.as_mut()?;
            let Some(result) = job.job.poll() else {
                wake.after(Duration::from_millis(100));
                return None;
            };
            let job = self.removal_job.take().expect("removal job checked");
            Some((
                result.unwrap_or_else(|_| CleanupReport {
                    failures: vec![CleanupFailure {
                        path: PathBuf::new(),
                        error: "cleanup worker stopped".into(),
                    }],
                    ..CleanupReport::default()
                }),
                job.success,
            ))
        }
    }
}

#[derive(Clone)]
pub(crate) enum LibraryAction {
    Open(PathBuf),
    View(PathBuf, MediaKind, Option<Details>),
    Reveal(PathBuf),
    CopyFile(PathBuf),
    CopyFiles(Vec<PathBuf>),
    CopyImage(PathBuf),
    CopyPath(PathBuf),
    CopyPaths(Vec<PathBuf>),
    CopyLink(String),
    Convert(PathBuf),
    ConvertMany(Vec<PathBuf>),
    AskDelete(PathBuf),
    AskDeleteMany(Vec<PathBuf>),
    AskRename(PathBuf, String),
    AskRenameMany(Vec<PathBuf>),
    ToggleSelect(PathBuf),
    SetViewerVolume(f32),
    AskCleanup(CleanupPlan),
}

pub(crate) struct Tool {
    pub(crate) icon: &'static str,
    pub(crate) label: String,
    pub(crate) action: LibraryAction,
    pub(crate) main: bool,
    pub(crate) danger: bool,
}

#[cfg(test)]
mod tests;
