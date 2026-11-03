use crate::pages;
use crate::textures::Textures;
use crate::ui_state::{ConvertItemStatus, UiState};
use crate::view::BackendView;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use mediaa2_core::library::{Details, LibraryEntry};
use mediaa2_core::navigation::Page;
use mediaa2_core::query::LibraryPage;
use mediaa2_core::settings::{Settings, ThemePreference};
use mediaa2_core::MediaKind;
use mediaa2_protocol::state::{ConversionState, RecordingState, RecoveryState, State};
use std::path::PathBuf;
use std::time::SystemTime;

struct Snap {
    view: BackendView,
    ui: UiState,
    actions: pages::Actions,
    textures: Textures,
    monitors: Vec<mediaa2_core::screen::MonitorInfo>,
    windows: Vec<mediaa2_core::screen::WindowInfo>,
    microphones: Vec<String>,
}

impl Snap {
    fn new(page: Page, entries: Vec<LibraryEntry>) -> Self {
        let settings = Settings {
            language: Some("en".into()),
            theme: ThemePreference::Dark,
            media_dir: PathBuf::from(r"C:\Users\Test\Documents\MediaA"),
            ..Settings::default()
        };
        let mut ui = UiState::default();
        ui.select_page(page);
        ui.settings.begin(settings.clone());
        ui.convert.output_dir = settings.convert_dir();
        let mut view = BackendView::new(State {
            settings: settings.clone(),
            recording: RecordingState::Idle,
            conversion: ConversionState::Idle,
            recovery: RecoveryState { running: false },
            notices: Vec::new(),
            picker: None,
            library_revision: 1,
        });
        let total = entries.len() as u32;
        view.apply_library(LibraryPage {
            revision: 1,
            total,
            entries,
            next_offset: None,
        });
        Self {
            view,
            ui,
            actions: pages::Actions::default(),
            textures: Textures::default(),
            monitors: vec![
                mediaa2_core::screen::MonitorInfo {
                    id: mediaa2_core::screen::MonitorId::new(1),
                    device: r"\\.\DISPLAY1".into(),
                    index: 0,
                    name: "Primary".into(),
                    x: 0,
                    y: 0,
                    width: 1920,
                    height: 1080,
                    scale: 1.0,
                    primary: true,
                },
                mediaa2_core::screen::MonitorInfo {
                    id: mediaa2_core::screen::MonitorId::new(2),
                    device: r"\\.\DISPLAY2".into(),
                    index: 1,
                    name: "Secondary".into(),
                    x: 1920,
                    y: 0,
                    width: 2560,
                    height: 1440,
                    scale: 1.0,
                    primary: false,
                },
            ],
            windows: vec![mediaa2_core::screen::WindowInfo {
                id: mediaa2_core::screen::CaptureWindowId::new(1),
                title: "main.rs".into(),
                app: "Visual Studio Code".into(),
                pid: 1234,
                width: 1600,
                height: 1000,
            }],
            microphones: vec!["Microphone (USB)".into(), "Headset".into()],
        }
    }

    fn frame(&mut self, ui: &mut egui::Ui) {
        let mut ctx = pages::Ctx {
            view: &self.view,
            ui: &mut self.ui,
            actions: &mut self.actions,
            textures: &mut self.textures,
            monitors: &self.monitors,
            windows: &self.windows,
            microphones: &self.microphones,
            command_error: None,
        };
        pages::frame(&mut ctx, ui);
    }
}

fn entry(name: &str, kind: MediaKind, size: u64, taken: &str) -> LibraryEntry {
    LibraryEntry {
        path: PathBuf::from(format!(r"C:\Users\Test\Documents\MediaA\{name}")),
        name: name.into(),
        title: name
            .trim_end_matches(|c: char| c == '.' || c.is_ascii_alphanumeric())
            .into(),
        kind,
        size,
        modified: SystemTime::UNIX_EPOCH,
        taken: chrono::NaiveDateTime::parse_from_str(taken, "%Y-%m-%d %H:%M:%S")
            .expect("test timestamp"),
        details: Some(Details {
            width: 1920,
            height: 1080,
            duration_secs: matches!(kind, MediaKind::Video | MediaKind::Animation).then_some(12.5),
            audio: matches!(kind, MediaKind::Video | MediaKind::Audio),
        }),
    }
}

fn samples() -> Vec<LibraryEntry> {
    vec![
        entry(
            "Screenshot_2026-10-14_09-12-01.png",
            MediaKind::Image,
            1_240_000,
            "2026-10-14 09:12:01",
        ),
        entry(
            "Recording_2026-10-14_09-14-22.mkv",
            MediaKind::Video,
            84_000_000,
            "2026-10-14 09:14:22",
        ),
        entry(
            "Recording_2026-10-14_09-20-00.mp4",
            MediaKind::Video,
            12_400_000,
            "2026-10-14 09:20:00",
        ),
        entry(
            "Animation_2026-10-14_09-22-00.gif",
            MediaKind::Animation,
            4_800_000,
            "2026-10-14 09:22:00",
        ),
        entry(
            "Audio_2026-10-14_09-30-00.mp3",
            MediaKind::Audio,
            940_000,
            "2026-10-14 09:30:00",
        ),
        entry(
            "Screenshot_2026-10-14_09-31-00.png",
            MediaKind::Image,
            420_000,
            "2026-10-14 09:31:00",
        ),
    ]
}

fn harness(
    page: Page,
    entries: Vec<LibraryEntry>,
    size: egui::Vec2,
) -> egui_kittest::Harness<'static, Snap> {
    harness_state(Snap::new(page, entries), size)
}

fn harness_light(
    page: Page,
    entries: Vec<LibraryEntry>,
    size: egui::Vec2,
) -> egui_kittest::Harness<'static, Snap> {
    let mut state = Snap::new(page, entries);
    state.view.state.settings.theme = ThemePreference::Light;
    state.ui.settings.draft.theme = ThemePreference::Light;
    harness_state_theme(state, size, ThemePreference::Light)
}

fn harness_state(state: Snap, size: egui::Vec2) -> egui_kittest::Harness<'static, Snap> {
    harness_state_theme(state, size, ThemePreference::Dark)
}

fn harness_state_theme(
    state: Snap,
    size: egui::Vec2,
    preference: ThemePreference,
) -> egui_kittest::Harness<'static, Snap> {
    let theme = match preference {
        ThemePreference::Light | ThemePreference::Lime | ThemePreference::Peach => {
            egui::Theme::Light
        }
        ThemePreference::System | ThemePreference::Dark => egui::Theme::Dark,
    };
    let mut harness = egui_kittest::Harness::builder()
        .with_size(size)
        .with_pixels_per_point(1.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .with_theme(theme)
        .build_ui_state(|ui, state: &mut Snap| state.frame(ui), state);
    crate::ui::theme::install(&harness.ctx);
    crate::ui::theme::apply_preference(&harness.ctx, preference);
    harness.step();
    harness.step();
    harness
}

#[test]
fn design_library_empty() {
    let mut harness = harness(Page::Library, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("library-empty");
}

#[test]
fn design_library_populated() {
    let mut harness = harness(Page::Library, samples(), egui::vec2(1180.0, 900.0));
    harness.snapshot("library-populated");
}

#[test]
fn design_library_narrow() {
    let mut harness = harness(Page::Library, samples(), egui::vec2(760.0, 760.0));
    harness.snapshot("library-narrow");
}

#[test]
fn design_library_hover() {
    let media = std::env::temp_dir().join("mediaa-hover-frame.png");
    let image = image::RgbaImage::from_fn(320, 180, |x, y| {
        image::Rgba([(x % 256) as u8, (y % 256) as u8, 140, 255])
    });
    image.save(&media).expect("write hover frame");
    let entry = LibraryEntry {
        path: media.clone(),
        name: "hover-preview.png".into(),
        title: "Hover preview".into(),
        kind: MediaKind::Animation,
        size: 41_458,
        modified: SystemTime::UNIX_EPOCH,
        taken: chrono::NaiveDateTime::parse_from_str("2026-10-14 09:14:22", "%Y-%m-%d %H:%M:%S")
            .expect("test timestamp"),
        details: Some(Details {
            width: 1920,
            height: 1080,
            duration_secs: Some(2.0),
            audio: false,
        }),
    };
    let mut state = Snap::new(Page::Library, Vec::new());
    state.view.library.entries = vec![entry];
    state.view.library.total = 1;
    state.ui.library.hovered = Some(media.clone());
    state.view.hover_previews.insert(
        media.clone(),
        mediaa2_protocol::PreviewState::Ready(media.clone()),
    );
    state.textures.seed_hover_static(&media, &media);
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.hover_at(egui::pos2(120.0, 260.0));
    harness.run_steps(2);
    harness.snapshot("library-hover");
}

#[test]
fn design_library_light() {
    let mut harness = harness_light(Page::Library, samples(), egui::vec2(1180.0, 900.0));
    harness.snapshot("library-light");
}

#[test]
fn design_about() {
    let mut harness = harness(Page::About, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("about");
}

#[test]
fn design_settings() {
    let mut harness = harness(Page::Settings, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("settings");
}

#[test]
fn design_settings_advanced() {
    let mut state = Snap::new(Page::Settings, Vec::new());
    state.ui.settings.advanced = true;
    state.ui.settings.draft.advanced_settings = true;
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("settings-advanced");
}

#[test]
fn design_settings_light() {
    let mut harness = harness_light(Page::Settings, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("settings-light");
}

#[test]
fn design_settings_narrow() {
    let mut harness = harness(Page::Settings, Vec::new(), egui::vec2(760.0, 900.0));
    harness.snapshot("settings-narrow");
}

#[test]
fn design_settings_advanced_off() {
    let mut state = Snap::new(Page::Settings, Vec::new());
    state.ui.settings.advanced = true;
    state.ui.settings.draft.advanced_settings = true;
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new("settings-advanced-off"), true);
    });
    harness.run_steps(2);
    harness.snapshot("settings-advanced-off");
}

#[test]
fn design_narrow_nav_returns_to_last_section() {
    let mut harness = harness(Page::Settings, Vec::new(), egui::vec2(760.0, 900.0));
    harness.get_by_label_contains("Capture").click();
    harness.run_steps(2);
    assert_eq!(harness.state().ui.page, Page::Screenshot);
    harness.snapshot("settings-narrow-return");
}

#[test]
fn design_settings_reset() {
    let mut harness = harness(Page::Settings, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new("settings-reset-open"), true);
    });
    harness.step();
    harness.snapshot("settings-reset");
}

#[test]
fn design_capture() {
    let mut harness = harness(Page::Screenshot, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("capture");
}

#[test]
fn design_record() {
    let mut harness = harness(Page::Record, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("record");
}

#[test]
fn design_record_narrow() {
    let mut harness = harness(Page::Record, Vec::new(), egui::vec2(760.0, 760.0));
    harness.snapshot("record-narrow");
}

#[test]
fn design_record_light() {
    let mut harness = harness_light(Page::Record, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("record-light");
}

#[test]
fn design_record_starting() {
    let mut state = Snap::new(Page::Record, Vec::new());
    state.ui.settings.draft.hide_recording_preparation = false;
    let monitor = state.monitors[0].clone();
    state.view.state.recording = RecordingState::Starting {
        started_ms: 0,
        target: mediaa2_core::recording::RecordTarget::Screens {
            monitors: vec![monitor],
        },
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("record-starting");
}

#[test]
fn design_record_recording() {
    let mut state = Snap::new(Page::Record, Vec::new());
    let monitor = state.monitors[0].clone();
    state.view.state.recording = RecordingState::Active {
        job: 1,
        target: mediaa2_core::recording::RecordTarget::Screens {
            monitors: vec![monitor],
        },
        started_ms: 0,
        paused: false,
        elapsed_ms: 12_400,
        sources: vec!["Primary (1920×1080)".into()],
        has_audio: true,
        output: mediaa2_core::formats::OutputKind::Video(
            mediaa2_core::formats::VideoContainer::Mkv,
        ),
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("record-recording");
}

#[test]
fn design_record_paused() {
    let mut state = Snap::new(Page::Record, Vec::new());
    let monitor = state.monitors[0].clone();
    state.view.state.recording = RecordingState::Active {
        job: 1,
        target: mediaa2_core::recording::RecordTarget::Screens {
            monitors: vec![monitor],
        },
        started_ms: 0,
        paused: true,
        elapsed_ms: 12_400,
        sources: vec!["Primary (1920×1080)".into()],
        has_audio: true,
        output: mediaa2_core::formats::OutputKind::Video(
            mediaa2_core::formats::VideoContainer::Mkv,
        ),
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("record-paused");
}

#[test]
fn design_record_saving() {
    let mut state = Snap::new(Page::Record, Vec::new());
    state.view.state.recording = RecordingState::Finalizing {
        job: 1,
        recorded_ms: 12_400,
        progress: Some(0.55),
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("record-saving");
}

#[test]
fn design_record_cancel_modal() {
    let mut state = Snap::new(Page::Record, Vec::new());
    let monitor = state.monitors[0].clone();
    state.view.state.recording = RecordingState::Active {
        job: 1,
        target: mediaa2_core::recording::RecordTarget::Screens {
            monitors: vec![monitor],
        },
        started_ms: 0,
        paused: false,
        elapsed_ms: 12_400,
        sources: vec!["Primary (1920×1080)".into()],
        has_audio: true,
        output: mediaa2_core::formats::OutputKind::Video(
            mediaa2_core::formats::VideoContainer::Mkv,
        ),
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(("record-cancel-confirm", "open")), true);
    });
    harness.step();
    harness.step();
    harness.snapshot("record-cancel-modal");
}

#[test]
fn design_record_start_cancel_modal() {
    let mut state = Snap::new(Page::Record, Vec::new());
    state.ui.settings.draft.hide_recording_preparation = false;
    let monitor = state.monitors[0].clone();
    state.view.state.recording = RecordingState::Starting {
        started_ms: 0,
        target: mediaa2_core::recording::RecordTarget::Screens {
            monitors: vec![monitor],
        },
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(("record-start-cancel", "open")), true);
    });
    harness.step();
    harness.step();
    harness.snapshot("record-start-cancel-modal");
}

#[test]
fn design_record_abort_modal() {
    let mut state = Snap::new(Page::Record, Vec::new());
    state.view.state.recording = RecordingState::Finalizing {
        job: 1,
        recorded_ms: 12_400,
        progress: Some(0.55),
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new(("record-abort-save", "open")), true);
    });
    harness.step();
    harness.step();
    harness.snapshot("record-abort-modal");
}

#[test]
fn design_audio() {
    let mut harness = harness(Page::RecordAudio, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("audio");
}

#[test]
fn design_audio_narrow() {
    let mut harness = harness(Page::RecordAudio, Vec::new(), egui::vec2(760.0, 760.0));
    harness.snapshot("audio-narrow");
}

#[test]
fn design_audio_recording() {
    let mut state = Snap::new(Page::RecordAudio, Vec::new());
    state.view.state.recording = RecordingState::Active {
        job: 1,
        target: mediaa2_core::recording::RecordTarget::Audio,
        started_ms: 0,
        paused: false,
        elapsed_ms: 18_300,
        sources: vec!["Microphone (USB)".into(), "System sound".into()],
        has_audio: true,
        output: mediaa2_core::formats::OutputKind::Audio(mediaa2_core::formats::AudioFormat::Mp3),
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("audio-recording");
}

#[test]
fn design_convert() {
    let mut harness = harness(Page::Convert, Vec::new(), egui::vec2(1180.0, 900.0));
    harness.snapshot("convert");
}

#[test]
fn design_convert_selected() {
    let mut state = Snap::new(Page::Convert, Vec::new());
    state.ui.convert.inputs = vec![
        PathBuf::from(r"C:\Users\Test\Documents\MediaA\Recording_2026-10-14_09-14-22.mkv"),
        PathBuf::from(r"C:\Users\Test\Documents\MediaA\Screenshot_2026-10-14_09-12-01.png"),
    ];
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("convert-selected");
}

#[test]
fn design_convert_statuses() {
    let mut state = Snap::new(Page::Convert, Vec::new());
    state.ui.convert.inputs = vec![
        PathBuf::from(r"C:\Users\Test\Documents\MediaA\Done_2026-01-03_10-00-00.mp4"),
        PathBuf::from(r"C:\Users\Test\Documents\MediaA\Failed_2026-01-02_10-00-00.mp4"),
        PathBuf::from(r"C:\Users\Test\Documents\MediaA\Pending_2026-01-01_10-00-00.mp4"),
        PathBuf::from(r"C:\Users\Test\Documents\MediaA\Cancelled_2026-01-01_09-00-00.mp4"),
    ];
    state.ui.convert.statuses = vec![
        ConvertItemStatus::Done,
        ConvertItemStatus::Failed,
        ConvertItemStatus::Pending,
        ConvertItemStatus::Cancelled,
    ];
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("convert-statuses");
}

#[test]
fn design_convert_running() {
    let mut state = Snap::new(Page::Convert, Vec::new());
    state.ui.convert.inputs = vec![PathBuf::from(
        r"C:\Users\Test\Documents\MediaA\Recording_2026-10-14_09-14-22.mkv",
    )];
    state.view.state.conversion = ConversionState::Running {
        job: 1,
        done: 1,
        total: 3,
        progress: Some(0.42),
        current: Some("Recording_2026-10-14_09-14-22.mkv".into()),
    };
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("convert-running");
}

#[test]
fn design_convert_cancelling() {
    let mut state = Snap::new(Page::Convert, Vec::new());
    state.view.state.conversion = ConversionState::Cancelling;
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("convert-cancelling");
}

#[test]
fn design_library_bulk_selected() {
    let mut state = Snap::new(Page::Library, samples());
    for entry in state.view.library.entries.iter().take(2) {
        state.ui.library.selection.insert(entry.path.clone());
    }
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    harness.snapshot("library-bulk-selected");
}

#[test]
fn design_library_cleanup() {
    let mut harness = harness(Page::Library, samples(), egui::vec2(1180.0, 900.0));
    harness.ctx.data_mut(|data| {
        data.insert_temp(egui::Id::new("mediaa2-library-cleanup"), 7u32);
    });
    harness.run_steps(2);
    harness.snapshot("library-cleanup");
}

#[test]
fn design_library_bulk_menu() {
    let mut state = Snap::new(Page::Library, samples());
    for entry in state.view.library.entries.iter().take(2) {
        state.ui.library.selection.insert(entry.path.clone());
    }
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    let label = format!("Selected: {}", 2);
    harness.get_by_label_contains(&label).click();
    harness.run_steps(2);
    harness.snapshot("library-bulk-menu");
}

#[test]
fn design_library_selection_menu() {
    let mut state = Snap::new(Page::Library, samples());
    for entry in state.view.library.entries.iter().take(2) {
        state.ui.library.selection.insert(entry.path.clone());
    }
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    let label = format!("Selected: {}", 2);
    harness.get_by_label_contains(&label).click();
    harness.run_steps(2);
    harness.snapshot("library-selection-menu");
}

#[test]
fn design_library_card_menu() {
    let mut harness = harness(
        Page::Library,
        samples().into_iter().take(1).collect(),
        egui::vec2(1180.0, 900.0),
    );
    harness.get_by_label("All actions").click();
    harness.run_steps(2);
    harness.snapshot("library-card-menu");
}

#[test]
fn design_library_delete() {
    let state = Snap::new(Page::Library, samples());
    let paths = state
        .view
        .library
        .entries
        .iter()
        .take(1)
        .map(|entry| entry.path.clone())
        .collect();
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    crate::pages::library::seed_delete(&harness.ctx, paths);
    harness.run_steps(2);
    harness.snapshot("library-delete");
}

#[test]
fn design_library_delete_many() {
    let state = Snap::new(Page::Library, samples());
    let paths = state
        .view
        .library
        .entries
        .iter()
        .take(3)
        .map(|entry| entry.path.clone())
        .collect();
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    crate::pages::library::seed_delete(&harness.ctx, paths);
    harness.run_steps(2);
    harness.snapshot("library-delete-many");
}

#[test]
fn design_library_rename() {
    let state = Snap::new(Page::Library, samples());
    let entry = state.view.library.entries[0].clone();
    let title = mediaa2_core::naming::parse_path(&entry.path)
        .map(|name| name.title)
        .unwrap_or_else(|| entry.title.clone());
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    crate::pages::library::seed_rename(&harness.ctx, vec![entry.path], title);
    harness.run_steps(2);
    harness.snapshot("library-rename");
}

#[test]
fn design_library_rename_many() {
    let state = Snap::new(Page::Library, samples());
    let paths = state
        .view
        .library
        .entries
        .iter()
        .take(3)
        .map(|entry| entry.path.clone())
        .collect();
    let mut harness = harness_state(state, egui::vec2(1180.0, 900.0));
    crate::pages::library::seed_rename(&harness.ctx, paths, String::new());
    harness.run_steps(2);
    harness.snapshot("library-rename-many");
}

#[test]
fn design_capture_narrow() {
    let mut harness = harness(Page::Screenshot, Vec::new(), egui::vec2(760.0, 760.0));
    harness.snapshot("capture-narrow");
}
