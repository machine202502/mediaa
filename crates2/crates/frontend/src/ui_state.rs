use mediaa2_core::annotate::{Shape, Style, Tool};
use mediaa2_core::convert::ConversionOptions;
use mediaa2_core::formats::AudioFormat;
use mediaa2_core::geometry::Rect;
use mediaa2_core::navigation::Page;
use mediaa2_core::query::LibraryQuery;
use mediaa2_core::settings::{AudioRecordSettings, Settings};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct UiState {
    pub page: Page,
    pub last_capture_page: Page,
    pub last_main_page: Page,
    pub settings: SettingsPage,
    pub record: RecordPage,
    pub audio: AudioPage,
    pub convert: ConvertPage,
    pub capture: CapturePage,
    pub library: LibraryPageState,
    pub picker: Option<PickerUi>,
    pub viewer: Option<ViewerUi>,
    pub transcript: Option<mediaa2_protocol::TranscriptView>,
    pub chat_timeline: Option<mediaa2_core::chat::Timeline>,
    pub topics: Option<Vec<mediaa2_protocol::TopicChapter>>,
    pub topics_path: Option<String>,
    pub selected_task: Option<i64>,
    pub renaming_speaker: Option<u32>,
    pub rename_value: String,
    pub chat_store: Option<String>,
    pub chat_id: Option<i64>,
    /// the model pinned to the open chat (from the last Chat/ChatView reply)
    pub chat_model: Option<mediaa2_protocol::ChatModelInfo>,
    /// a question waiting for the user to pick the chat's model
    pub chat_confirm: Option<String>,
    pub transcription_tab: TranscriptionTab,
    pub transcription: TranscriptionUi,
}

/// Transient editing/check state of the transcription settings tabs
/// (providers, models, budget). Not mirrored from the backend.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TranscriptionUi {
    pub add_kind: mediaa2_core::agent::ConnectionKind,
    /// provider id -> last check result ("ok"/error)
    pub checks: BTreeMap<String, Result<String, String>>,
    /// provider ids with a check in flight
    pub checking: BTreeSet<String>,
    /// connection id -> known model ids (from ModelsCheck)
    pub models: BTreeMap<String, Vec<String>>,
    pub models_checking: bool,
    /// (connection id, model) waiting for the download confirmation
    pub pull_confirm: Option<(String, String)>,
    pub web_status: Option<Result<String, String>>,
    pub pool_name: String,
    pub pool_limit: u32,
    pub pool_error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TranscriptionTab {
    #[default]
    Assistant,
    Providers,
    Models,
    Budget,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SettingsPage {
    pub draft: Settings,
    pub dirty: bool,
    pub advanced: bool,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RecordPage {
    pub target: TargetKind,
    pub selected_monitors: BTreeSet<u32>,
    pub selected_windows: BTreeSet<u32>,
    pub region: Option<(u32, Rect)>,
    pub picking_region: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TargetKind {
    #[default]
    Screen,
    Region,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioPage {
    pub format: AudioFormat,
    pub kbps: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConvertItemStatus {
    #[default]
    Pending,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ConvertPage {
    pub inputs: Vec<PathBuf>,
    pub options: ConversionOptions,
    pub output_dir: PathBuf,
    pub statuses: Vec<ConvertItemStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CapturePage {
    pub monitor: Option<u32>,
    pub window: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LibraryPageState {
    pub selection: BTreeSet<PathBuf>,
    pub scroll: f32,
    pub query: LibraryQuery,
    pub hovered: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickerUi {
    pub session: u64,
    pub tool: Tool,
    pub style: Style,
    pub shapes: Vec<Shape>,
    pub selection: Option<Rect>,
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PickerCommit {
    pub session: u64,
    pub selection: Rect,
    pub shapes: Vec<Shape>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ViewerUi {
    pub path: PathBuf,
    pub volume: f32,
    pub paused: bool,
    pub position: f64,
    pub speed: f64,
    pub zoom: f64,
}

impl Default for UiState {
    fn default() -> Self {
        let page = Page::Screenshot;
        Self {
            page,
            last_capture_page: page,
            last_main_page: page,
            settings: SettingsPage::default(),
            record: RecordPage::default(),
            audio: AudioPage::default(),
            convert: ConvertPage::default(),
            capture: CapturePage::default(),
            library: LibraryPageState::default(),
            picker: None,
            viewer: None,
            transcript: None,
            chat_timeline: None,
            topics: None,
            topics_path: None,
            selected_task: None,
            renaming_speaker: None,
            rename_value: String::new(),
            chat_store: None,
            chat_id: None,
            chat_model: None,
            chat_confirm: None,
            transcription_tab: TranscriptionTab::Assistant,
            transcription: TranscriptionUi::default(),
        }
    }
}

impl UiState {
    pub fn select_page(&mut self, page: Page) {
        self.page = page;
        match page {
            Page::Screenshot | Page::Record | Page::RecordAudio => {
                self.last_capture_page = page;
                self.last_main_page = page;
            }
            Page::Library | Page::Transcription | Page::Convert => self.last_main_page = page,
            Page::Settings | Page::About => {}
        }
    }

    pub fn settings_draft_changed(&mut self) {
        self.settings.dirty = true;
        self.settings.advanced = self.settings.draft.advanced_settings;
    }

    pub fn clear_settings_dirty(&mut self) {
        self.settings.dirty = false;
    }

    pub fn open_picker(&mut self, picker: PickerUi) {
        self.picker = Some(picker);
    }

    pub fn close_picker(&mut self) {
        self.picker = None;
    }

    pub fn open_viewer(&mut self, path: PathBuf) {
        self.viewer = Some(ViewerUi::new(path));
    }

    pub fn close_viewer(&mut self) {
        self.viewer = None;
    }
}

impl Default for SettingsPage {
    fn default() -> Self {
        let draft = Settings::default();
        Self {
            advanced: draft.advanced_settings,
            draft,
            dirty: false,
        }
    }
}

impl SettingsPage {
    pub fn begin(&mut self, settings: Settings) {
        self.advanced = settings.advanced_settings;
        self.draft = settings;
        self.dirty = false;
    }

    pub fn take_change(&mut self) -> Option<Settings> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        self.advanced = self.draft.advanced_settings;
        Some(self.draft.clone())
    }
}

impl Default for AudioPage {
    fn default() -> Self {
        let audio = AudioRecordSettings::default();
        Self {
            format: audio.format,
            kbps: audio.kbps,
        }
    }
}

impl LibraryPageState {
    pub fn toggle(&mut self, path: &Path) {
        if !self.selection.remove(path) {
            self.selection.insert(path.to_path_buf());
        }
    }

    pub fn clear(&mut self) {
        self.selection.clear();
    }

    pub fn select_only(&mut self, path: &Path) {
        self.selection.clear();
        self.selection.insert(path.to_path_buf());
    }

    pub fn is_selected(&self, path: &Path) -> bool {
        self.selection.contains(path)
    }
}

impl PickerUi {
    pub fn new(session: u64) -> Self {
        Self {
            session,
            tool: Tool::Pen,
            style: Style::new([255, 0, 0, 255], 4.0),
            shapes: Vec::new(),
            selection: None,
            text: None,
        }
    }

    pub fn commit(&self) -> Option<PickerCommit> {
        let selection = self.selection?;
        if selection.is_empty() {
            return None;
        }
        Some(PickerCommit {
            session: self.session,
            selection,
            shapes: self.shapes.clone(),
        })
    }
}

impl ViewerUi {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            volume: 1.0,
            paused: false,
            position: 0.0,
            speed: 1.0,
            zoom: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn select_page_tracks_capture_and_main_pages() {
        let mut ui = UiState::default();

        ui.select_page(Page::Record);
        assert_eq!(ui.page, Page::Record);
        assert_eq!(ui.last_capture_page, Page::Record);
        assert_eq!(ui.last_main_page, Page::Record);

        ui.select_page(Page::Settings);
        assert_eq!(ui.page, Page::Settings);
        assert_eq!(ui.last_capture_page, Page::Record);
        assert_eq!(ui.last_main_page, Page::Record);

        ui.select_page(Page::Library);
        assert_eq!(ui.last_capture_page, Page::Record);
        assert_eq!(ui.last_main_page, Page::Library);

        ui.select_page(Page::RecordAudio);
        assert_eq!(ui.last_capture_page, Page::RecordAudio);
        assert_eq!(ui.last_main_page, Page::RecordAudio);

        ui.select_page(Page::About);
        assert_eq!(ui.last_capture_page, Page::RecordAudio);
        assert_eq!(ui.last_main_page, Page::RecordAudio);
    }

    #[test]
    fn library_selection_toggles_and_replaces() {
        let mut library = LibraryPageState::default();
        let a = Path::new("a.png");
        let b = Path::new("b.png");

        library.toggle(a);
        assert!(library.is_selected(a));
        assert!(!library.is_selected(b));

        library.toggle(b);
        assert_eq!(library.selection.len(), 2);

        library.toggle(a);
        assert!(!library.is_selected(a));
        assert!(library.is_selected(b));

        library.select_only(a);
        assert_eq!(library.selection.len(), 1);
        assert!(library.is_selected(a));
        assert!(!library.is_selected(b));

        library.clear();
        assert!(library.selection.is_empty());
        assert!(!library.is_selected(a));
    }

    #[test]
    fn settings_draft_is_dirty_until_taken_once() {
        let mut ui = UiState::default();
        let settings = Settings {
            advanced_settings: true,
            ..Settings::default()
        };
        ui.settings.begin(settings.clone());

        assert!(!ui.settings.dirty);
        assert!(ui.settings.advanced);
        assert_eq!(ui.settings.draft, settings);

        ui.settings.draft.viewer_volume = 0.4;
        ui.settings_draft_changed();
        assert!(ui.settings.dirty);

        let change = ui
            .settings
            .take_change()
            .expect("a dirty draft produces a change");
        assert_eq!(change.viewer_volume, 0.4);
        assert!(!ui.settings.dirty);
        assert!(ui.settings.take_change().is_none());
    }

    #[test]
    fn clearing_the_settings_dirty_flag_keeps_the_draft() {
        let mut ui = UiState::default();
        ui.settings.draft.close_to_tray = true;
        ui.settings_draft_changed();

        ui.clear_settings_dirty();

        assert!(!ui.settings.dirty);
        assert!(ui.settings.draft.close_to_tray);
    }

    #[test]
    fn picker_commit_carries_selection_and_shapes() {
        let mut picker = PickerUi::new(9);
        picker.selection = Some(Rect::new(4, 5, 30, 40));
        let mut shape = Shape::begin(Tool::Arrow, (1.0, 2.0), picker.style);
        shape.drag_to((20.0, 30.0));
        picker.shapes.push(shape.clone());
        picker.text = Some("note".into());

        let commit = picker.commit().expect("a non-empty selection commits");

        assert_eq!(commit.session, 9);
        assert_eq!(commit.selection, Rect::new(4, 5, 30, 40));
        assert_eq!(commit.shapes, vec![shape]);
        assert_eq!(picker.shapes.len(), 1);
    }

    #[test]
    fn picker_without_a_usable_selection_cannot_commit() {
        let mut picker = PickerUi::new(1);
        assert!(picker.commit().is_none());

        picker.selection = Some(Rect::new(0, 0, 0, 10));
        assert!(picker.commit().is_none());

        picker.selection = Some(Rect::new(0, 0, 10, 10));
        assert!(picker.commit().is_some());
    }

    #[test]
    fn viewer_starts_at_normal_playback_and_closes() {
        let mut ui = UiState::default();
        ui.open_viewer(PathBuf::from("clip.mp4"));

        let viewer = ui.viewer.as_ref().expect("viewer is open");
        assert_eq!(viewer.path, PathBuf::from("clip.mp4"));
        assert_eq!(viewer.volume, 1.0);
        assert!(!viewer.paused);
        assert_eq!(viewer.position, 0.0);
        assert_eq!(viewer.speed, 1.0);
        assert_eq!(viewer.zoom, 1.0);

        ui.close_viewer();
        assert!(ui.viewer.is_none());
    }

    #[test]
    fn ui_defaults_are_sane() {
        let ui = UiState::default();
        assert_eq!(ui.page, Page::Screenshot);
        assert_eq!(ui.audio.format, AudioFormat::Mp3);
        assert_eq!(ui.audio.kbps, AudioRecordSettings::default().kbps);
        assert_eq!(ui.convert.options, ConversionOptions::default());
        assert!(ui.picker.is_none());
        assert!(ui.viewer.is_none());
    }
}
