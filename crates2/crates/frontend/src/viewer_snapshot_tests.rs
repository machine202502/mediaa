use crate::pages;
use crate::textures::Textures;
use crate::ui_state::UiState;
use crate::view::BackendView;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use mediaa2_core::library::{Details, LibraryEntry};
use mediaa2_core::navigation::Page;
use mediaa2_core::query::LibraryPage;
use mediaa2_core::settings::{Settings, ThemePreference};
use mediaa2_core::MediaKind;
use mediaa2_protocol::state::{ConversionState, RecordingState, RecoveryState, State};
use mediaa2_protocol::PreviewState;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

struct ViewerSnap {
    view: BackendView,
    ui: UiState,
    actions: pages::Actions,
    textures: Textures,
    monitors: Vec<mediaa2_core::screen::MonitorInfo>,
    windows: Vec<mediaa2_core::screen::WindowInfo>,
    microphones: Vec<String>,
    _dir: tempfile::TempDir,
}

impl ViewerSnap {
    fn new(theme: ThemePreference, kind: MediaKind, speed: f64, zoom: f64) -> Self {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = if kind == MediaKind::Image {
            let path = dir.path().join("Screenshot_2026-01-01_10-00-00.png");
            gradient_png(&path, 320, 180);
            path
        } else {
            PathBuf::from(r"C:\Users\Test\Documents\MediaA\Player_2026-01-01_10-00-00.mp4")
        };
        let settings = Settings {
            language: Some("en".into()),
            theme,
            media_dir: PathBuf::from(r"C:\Users\Test\Documents\MediaA"),
            ..Settings::default()
        };
        let mut ui = UiState::default();
        ui.select_page(Page::Library);
        ui.settings.begin(settings.clone());
        ui.convert.output_dir = settings.convert_dir();
        ui.open_viewer(path.clone());
        if let Some(viewer) = ui.viewer.as_mut() {
            viewer.paused = true;
            viewer.position = 30.0;
            viewer.speed = speed;
            viewer.zoom = zoom;
            viewer.volume = 1.0;
        }
        let mut view = BackendView::new(State {
            settings: settings.clone(),
            recording: RecordingState::Idle,
            conversion: ConversionState::Idle,
            recovery: RecoveryState { running: false },
            notices: Vec::new(),
            picker: None,
            library_revision: 1,
        });
        if kind != MediaKind::Image {
            let poster = dir.path().join("poster.png");
            solid_png(&poster, 2, 2, [20, 40, 80, 255]);
            view.set_preview(path.clone(), PreviewState::Ready(poster));
        }
        let entry = entry(path, kind);
        let total = 1;
        view.apply_library(LibraryPage {
            revision: 1,
            total,
            entries: vec![entry],
            next_offset: None,
        });
        Self {
            view,
            ui,
            actions: pages::Actions::default(),
            textures: Textures::default(),
            monitors: vec![mediaa2_core::screen::MonitorInfo {
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
            }],
            windows: Vec::new(),
            microphones: Vec::new(),
            _dir: dir,
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

fn entry(path: PathBuf, kind: MediaKind) -> LibraryEntry {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    LibraryEntry {
        path,
        title: name.clone(),
        name,
        kind,
        size: 12_400_000,
        modified: SystemTime::UNIX_EPOCH,
        taken: chrono::NaiveDateTime::parse_from_str("2026-01-01 10:00:00", "%Y-%m-%d %H:%M:%S")
            .expect("test timestamp"),
        details: Some(Details {
            width: 1920,
            height: 1080,
            duration_secs: (kind != MediaKind::Image).then_some(60.0),
            audio: matches!(kind, MediaKind::Video | MediaKind::Audio),
        }),
    }
}

fn solid_png(path: &Path, width: u32, height: u32, color: [u8; 4]) {
    image::RgbaImage::from_pixel(width, height, image::Rgba(color))
        .save(path)
        .expect("poster fixture");
}

fn gradient_png(path: &Path, width: u32, height: u32) {
    let image = image::RgbaImage::from_fn(width, height, |x, y| {
        let red = (x * 255 / width) as u8;
        let green = (y * 255 / height) as u8;
        image::Rgba([red, green, 160, 255])
    });
    image.save(path).expect("image fixture");
}

fn harness(state: ViewerSnap, light: bool) -> egui_kittest::Harness<'static, ViewerSnap> {
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1180.0, 900.0))
        .with_pixels_per_point(1.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .with_theme(if light {
            egui::Theme::Light
        } else {
            egui::Theme::Dark
        })
        .build_ui_state(|ui, state: &mut ViewerSnap| state.frame(ui), state);
    crate::ui::theme::install(&harness.ctx);
    crate::ui::theme::apply_preference(
        &harness.ctx,
        if light {
            ThemePreference::Light
        } else {
            ThemePreference::Dark
        },
    );
    harness.run_steps(3);
    harness
}

fn player(theme: ThemePreference, speed: f64) -> ViewerSnap {
    ViewerSnap::new(theme, MediaKind::Video, speed, 1.0)
}

#[test]
fn viewer_player_100() {
    let mut harness = harness(player(ThemePreference::Dark, 1.0), false);
    harness.snapshot("viewer-player-100");
}

#[test]
fn viewer_player_125() {
    let mut harness = harness(player(ThemePreference::Dark, 1.25), false);
    harness.snapshot("viewer-player-125");
}

#[test]
fn viewer_player_light() {
    let mut harness = harness(player(ThemePreference::Light, 1.25), true);
    harness.snapshot("viewer-player-light");
}

#[test]
fn viewer_player_menu() {
    let mut harness = harness(player(ThemePreference::Dark, 1.25), false);
    harness.get_by_label("1.25×").click();
    harness.run_steps(2);
    harness.snapshot("viewer-player-menu");
}

#[test]
fn viewer_player_menu_hover() {
    let mut harness = harness(player(ThemePreference::Dark, 1.25), false);
    harness.get_by_label("1.25×").click();
    harness.run_steps(2);
    let item = harness.get_by_label("0.75×").rect().center();
    harness.hover_at(item);
    harness.run_steps(2);
    harness.snapshot("viewer-player-menu-hover");
}

#[test]
fn viewer_image_zoom() {
    let mut harness = harness(
        ViewerSnap::new(ThemePreference::Dark, MediaKind::Image, 1.0, 1.6),
        false,
    );
    let path = harness
        .state_mut()
        .ui
        .viewer
        .as_ref()
        .expect("viewer")
        .path
        .clone();
    let pan_id = egui::Id::new(("mediaa2-viewer-pan", path));
    harness
        .ctx
        .data_mut(|data| data.insert_temp(pan_id, egui::vec2(80.0, -60.0)));
    harness.run_steps(2);
    harness.snapshot("viewer-image-zoom");
}
