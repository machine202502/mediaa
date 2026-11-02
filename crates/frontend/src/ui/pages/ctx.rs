use crate::features::capture::AppCapture;
use crate::features::command::AppCommand;
use crate::features::conversion::{ConversionController, ConversionUiAction};
use crate::features::intent::Intent;
use crate::features::library::LibraryController;
use crate::features::navigation::Page;
use crate::features::notices::{NoticeLevel, Notices};
use crate::features::recording::RecordTarget;
use crate::infra::clipboard_worker::ClipboardService;
use crate::ui::caches::Caches;
use crate::ui::pages::agent::AgentUi;
use crate::ui::pages::library::hover_preview::Previews;
use crate::ui::pages::library::thumbs::Thumbnails;
use mediaa_core::settings::{RecordProfile, Settings};
use mediaa_core::wake::Wake;
use mediaa_platform::screen::{MonitorInfo, WindowInfo};
use std::path::{Path, PathBuf};

pub(crate) struct Ctx<'a> {
    pub page: Page,
    pub page_scroll: Vec<(Page, f32)>,
    pub last_capture_page: Page,
    pub last_main_page: Page,
    pub last_agent_page: Page,
    pub settings: &'a mut Settings,
    pub lib: &'a mut LibraryController,
    pub recording: &'a mut crate::features::recording::ViewState,
    pub capture: &'a mut AppCapture,
    pub conversion: &'a mut ConversionController,
    pub agent_ui: &'a mut AgentUi,
    pub thumbs: &'a mut Thumbnails,
    pub previews: &'a mut Previews,
    pub caches: &'a mut Caches,
    pub notices: &'a mut Notices,
    pub clipboard: &'a mut ClipboardService,
    pub wake: &'a Wake,
    pub monitors: &'a [MonitorInfo],
    pub windows: &'a [WindowInfo],
    pub mic_devices: &'a [String],
    pub hotkey_problems: &'a [String],
    pub recovery_active: bool,
    pub viewer_open: bool,
    pub intents: Vec<Intent>,
}

impl Ctx<'_> {
    pub(crate) fn settings_changed(&mut self) {
        self.intents.push(Intent::SettingsEdited);
    }

    pub(crate) fn edit_settings(&mut self, edit: impl FnOnce(&mut Settings)) {
        edit(self.settings);
        self.settings_changed();
    }

    pub(crate) fn preview_options_changed(&mut self) {
        self.intents.push(Intent::PreviewOptionsChanged {
            animated: self.settings.animated_previews,
            full: self.settings.full_preview_videos,
        });
    }

    pub(crate) fn refresh_catalogs(&mut self) {
        self.intents.push(Intent::RefreshCatalogs);
    }

    pub(crate) fn reapply_hotkeys(&mut self) {
        self.intents.push(Intent::ReapplyHotkeys);
    }

    pub(crate) fn suspend_hotkeys(&mut self) {
        self.intents.push(Intent::SuspendHotkeys);
    }

    pub(crate) fn error(&mut self, text: impl Into<String>) {
        self.notice(NoticeLevel::Error, text);
    }

    pub(crate) fn warning(&mut self, text: impl Into<String>) {
        self.notice(NoticeLevel::Warning, text);
    }

    pub(crate) fn info(&mut self, text: impl Into<String>) {
        self.notice(NoticeLevel::Info, text);
    }

    pub(crate) fn success(&mut self, text: impl Into<String>) {
        self.notice(NoticeLevel::Success, text);
    }

    fn notice(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.intents.push(Intent::Notice {
            level,
            text: text.into(),
        });
    }

    pub(crate) fn open_path(&mut self, path: &Path) {
        self.intents.push(Intent::OpenPath(path.to_path_buf()));
    }

    pub(crate) fn reveal_path(&mut self, path: PathBuf) {
        self.intents.push(Intent::Reveal(path));
    }

    pub(crate) fn rename_media(&mut self, path: PathBuf, title: String) {
        self.intents.push(Intent::RenameMedia { path, title });
    }

    pub(crate) fn open_in_convert(&mut self, path: PathBuf) {
        self.intents.push(Intent::OpenInConvert(vec![path]));
    }

    pub(crate) fn open_in_convert_many(&mut self, paths: Vec<PathBuf>) {
        self.intents.push(Intent::OpenInConvert(paths));
    }

    pub(crate) fn apply_conversion_ui_action(&mut self, action: ConversionUiAction) {
        self.intents.push(Intent::ConversionUiAction(action));
    }

    pub(crate) fn dispatch_command(&mut self, command: AppCommand) {
        self.intents.push(Intent::Command(command));
    }

    pub(crate) fn record_target_from_ui(&self) -> Result<RecordTarget, String> {
        crate::features::recording::plan::target_from_ui(
            self.settings,
            &self.recording.ui,
            self.monitors,
            self.windows,
        )
    }

    pub(crate) fn start_region_screenshot(
        &mut self,
        purpose: crate::features::region_editor::Purpose,
    ) {
        self.intents.push(Intent::RegionEditor { purpose });
    }

    pub(crate) fn start_recording_from_ui(&mut self) {
        self.intents.push(Intent::StartRecording);
    }

    pub(crate) fn start_audio_recording(&mut self, profile: RecordProfile, hide: Option<bool>) {
        self.intents
            .push(Intent::StartAudioRecording { profile, hide });
    }

    pub(crate) fn recording_page(&self) -> Page {
        self.recording.page()
    }

    pub(crate) fn recovery_active(&self) -> bool {
        self.recovery_active
    }

    pub(crate) fn image_io_busy(&self) -> bool {
        self.capture.busy()
    }

    pub(crate) fn handle_intents(&mut self, intents: Vec<Intent>) {
        self.intents.extend(intents);
    }
}
