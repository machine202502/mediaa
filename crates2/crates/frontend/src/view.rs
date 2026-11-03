use mediaa2_core::library::LibraryEntry;
use mediaa2_core::query::{LibraryPage, LibraryQuery};
use mediaa2_core::settings::Settings;
use mediaa2_protocol::command::PreviewState;
use mediaa2_protocol::state::{
    ConversionState, Notice, PickerState, RecordingState, RecoveryState, State,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct BackendView {
    pub state: State,
    pub library: LibraryView,
    pub previews: HashMap<PathBuf, PreviewState>,
    pub hover_previews: HashMap<PathBuf, PreviewState>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LibraryView {
    pub query: LibraryQuery,
    pub entries: Vec<LibraryEntry>,
    pub total: u32,
    pub next_offset: Option<u32>,
    pub loaded_revision: Option<u64>,
    pub loading: bool,
}

impl BackendView {
    pub fn new(state: State) -> Self {
        Self {
            state,
            library: LibraryView::default(),
            previews: HashMap::new(),
            hover_previews: HashMap::new(),
            last_error: None,
        }
    }

    pub fn apply(&mut self, state: State) {
        if state.library_revision != self.state.library_revision {
            self.library.reset_pages();
        }
        self.state = state;
    }

    pub fn apply_library(&mut self, page: LibraryPage) {
        if page.revision != self.state.library_revision {
            return;
        }
        let replace =
            self.library.query.offset == 0 || self.library.loaded_revision != Some(page.revision);
        if replace {
            self.library.entries.clear();
        }
        for entry in page.entries {
            self.library.upsert_entry(entry);
        }
        self.library.total = page.total;
        self.library.next_offset = page.next_offset;
        self.library.loaded_revision = Some(page.revision);
        self.library.loading = false;
    }

    pub fn set_preview(&mut self, path: PathBuf, preview: PreviewState) {
        self.previews.insert(path, preview);
    }

    pub fn set_hover_preview(&mut self, path: PathBuf, preview: PreviewState) {
        self.hover_previews.insert(path, preview);
    }

    pub fn clear(&mut self, path: &Path) {
        self.previews.remove(path);
    }

    pub fn recording(&self) -> &RecordingState {
        &self.state.recording
    }

    pub fn conversion(&self) -> &ConversionState {
        &self.state.conversion
    }

    pub fn recovery(&self) -> RecoveryState {
        self.state.recovery
    }

    pub fn picker(&self) -> Option<&PickerState> {
        self.state.picker.as_ref()
    }

    pub fn settings(&self) -> &Settings {
        &self.state.settings
    }

    pub fn notices(&self) -> &[Notice] {
        &self.state.notices
    }

    pub fn library_revision(&self) -> u64 {
        self.state.library_revision
    }

    pub fn recording_active(&self) -> bool {
        !matches!(self.state.recording, RecordingState::Idle)
    }

    pub fn conversion_busy(&self) -> bool {
        !matches!(self.state.conversion, ConversionState::Idle)
    }
}

impl LibraryView {
    fn reset_pages(&mut self) {
        self.entries.clear();
        self.total = 0;
        self.next_offset = None;
        self.loaded_revision = None;
        self.loading = false;
    }

    fn upsert_entry(&mut self, entry: LibraryEntry) {
        match self
            .entries
            .iter_mut()
            .find(|existing| existing.path == entry.path)
        {
            Some(existing) => *existing = entry,
            None => self.entries.push(entry),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::formats::MediaKind;
    use std::time::SystemTime;

    fn state(revision: u64) -> State {
        State {
            settings: Settings::default(),
            recording: RecordingState::Idle,
            conversion: ConversionState::Idle,
            recovery: RecoveryState { running: false },
            notices: Vec::new(),
            picker: None,
            library_revision: revision,
        }
    }

    fn entry(path: &str) -> LibraryEntry {
        LibraryEntry {
            path: PathBuf::from(path),
            name: path.into(),
            title: path.into(),
            kind: MediaKind::Image,
            size: 1,
            modified: SystemTime::UNIX_EPOCH,
            taken: Default::default(),
            details: None,
        }
    }

    fn page(
        revision: u64,
        total: u32,
        entries: Vec<LibraryEntry>,
        next_offset: Option<u32>,
    ) -> LibraryPage {
        LibraryPage {
            revision,
            total,
            entries,
            next_offset,
        }
    }

    fn paths(view: &LibraryView) -> Vec<String> {
        view.entries
            .iter()
            .map(|entry| entry.path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn apply_replaces_the_state_and_keeps_pages_for_the_same_revision() {
        let mut view = BackendView::new(state(1));
        view.library.entries = vec![entry("a.png")];
        view.library.total = 1;
        view.library.next_offset = Some(1);
        view.library.loaded_revision = Some(1);

        let mut next = state(1);
        next.recording = RecordingState::Finalizing {
            job: 4,
            recorded_ms: 2_000,
            progress: Some(0.5),
        };
        next.conversion = ConversionState::Running {
            job: 5,
            done: 1,
            total: 3,
            progress: Some(0.25),
            current: Some("a.png".into()),
        };
        next.recovery = RecoveryState { running: true };
        next.settings.viewer_volume = 0.5;

        view.apply(next.clone());

        assert_eq!(view.state, next);
        assert_eq!(view.recording(), &next.recording);
        assert_eq!(view.conversion(), &next.conversion);
        assert_eq!(view.recovery(), RecoveryState { running: true });
        assert_eq!(view.settings().viewer_volume, 0.5);
        assert_eq!(view.library_revision(), 1);
        assert_eq!(paths(&view.library), ["a.png"]);
    }

    #[test]
    fn a_new_library_revision_clears_loaded_pages() {
        let mut view = BackendView::new(state(1));
        view.library.entries = vec![entry("a.png")];
        view.library.total = 1;
        view.library.next_offset = Some(1);
        view.library.loaded_revision = Some(1);
        view.library.loading = true;

        view.apply(state(2));

        assert!(view.library.entries.is_empty());
        assert_eq!(view.library.total, 0);
        assert_eq!(view.library.next_offset, None);
        assert_eq!(view.library.loaded_revision, None);
        assert!(!view.library.loading);
        assert_eq!(view.library_revision(), 2);
    }

    #[test]
    fn library_pages_append_without_duplicate_paths() {
        let mut view = BackendView::new(state(5));
        view.library.query.offset = 0;
        view.apply_library(page(5, 3, vec![entry("a.png"), entry("b.png")], Some(2)));

        assert_eq!(paths(&view.library), ["a.png", "b.png"]);
        assert_eq!(view.library.total, 3);
        assert_eq!(view.library.next_offset, Some(2));
        assert_eq!(view.library.loaded_revision, Some(5));

        view.library.query.offset = 2;
        view.apply_library(page(5, 4, vec![entry("b.png"), entry("c.png")], None));

        assert_eq!(paths(&view.library), ["a.png", "b.png", "c.png"]);
        assert_eq!(view.library.total, 4);
        assert_eq!(view.library.next_offset, None);
        assert_eq!(view.library.loaded_revision, Some(5));
    }

    #[test]
    fn loading_offset_zero_again_replaces_the_page() {
        let mut view = BackendView::new(state(2));
        view.apply_library(page(2, 2, vec![entry("a.png"), entry("b.png")], Some(2)));
        assert_eq!(paths(&view.library), ["a.png", "b.png"]);

        view.apply_library(page(2, 1, vec![entry("c.png")], None));

        assert_eq!(paths(&view.library), ["c.png"]);
        assert_eq!(view.library.total, 1);
        assert_eq!(view.library.next_offset, None);
    }

    #[test]
    fn a_page_from_another_revision_is_ignored() {
        let mut view = BackendView::new(state(5));
        view.apply_library(page(5, 1, vec![entry("a.png")], None));
        let before = view.library.clone();

        view.apply_library(page(4, 9, vec![entry("stale.png")], Some(9)));

        assert_eq!(view.library, before);
    }

    #[test]
    fn duplicate_paths_inside_one_page_are_collapsed() {
        let mut view = BackendView::new(state(3));
        view.apply_library(page(3, 2, vec![entry("a.png"), entry("a.png")], None));

        assert_eq!(paths(&view.library), ["a.png"]);
        assert_eq!(view.library.total, 2);
    }

    #[test]
    fn previews_are_stored_and_cleared_by_path() {
        let mut view = BackendView::new(state(1));
        let path = PathBuf::from("thumb.png");
        let ready = PreviewState::Ready(PathBuf::from("thumb-cache.png"));

        view.set_preview(path.clone(), ready.clone());
        assert_eq!(view.previews.get(&path), Some(&ready));

        view.clear(&path);
        assert!(view.previews.is_empty());
    }
}
