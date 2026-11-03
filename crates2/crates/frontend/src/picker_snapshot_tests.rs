use crate::pages;
use crate::textures::Textures;
use crate::ui_state::{PickerUi, UiState};
use crate::view::BackendView;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use mediaa2_core::annotate::{Shape, Style, Tool};
use mediaa2_core::geometry::Rect;
use mediaa2_core::screen::{MonitorId, MonitorInfo, WindowInfo};
use mediaa2_core::settings::{Settings, ThemePreference};
use mediaa2_protocol::state::{ConversionState, PickerState, RecordingState, RecoveryState, State};
use mediaa2_protocol::{ClientCommand, PickerCommand};
use std::path::Path;
use tempfile::TempDir;

struct Snap {
    view: BackendView,
    ui: UiState,
    actions: pages::Actions,
    textures: Textures,
    monitors: Vec<MonitorInfo>,
    windows: Vec<WindowInfo>,
    microphones: Vec<String>,
    _dir: TempDir,
}

impl Snap {
    fn new(
        monitors: Vec<MonitorInfo>,
        selection: Option<Rect>,
        shapes: Vec<Shape>,
        text: Option<String>,
        picking_region: bool,
    ) -> Self {
        let dir = tempfile::tempdir().expect("a temporary directory");
        let image = dir.path().join("picker.png");
        write_image(&image);
        let settings = Settings {
            language: Some("en".into()),
            theme: ThemePreference::Dark,
            ..Settings::default()
        };
        let state = State {
            settings: settings.clone(),
            recording: RecordingState::Idle,
            conversion: ConversionState::Idle,
            recovery: RecoveryState { running: false },
            notices: Vec::new(),
            picker: Some(PickerState {
                session: 7,
                monitor: monitors[0].clone(),
                image,
                scale: 1.0,
            }),
            library_revision: 1,
        };
        let mut ui = UiState::default();
        ui.settings.begin(settings);
        ui.record.picking_region = picking_region;
        ui.open_picker(PickerUi {
            session: 7,
            tool: Tool::Rect,
            style: style(),
            shapes,
            selection,
            text,
        });
        Self {
            view: BackendView::new(state),
            ui,
            actions: pages::Actions::default(),
            textures: Textures::default(),
            monitors,
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
        pages::picker::show(&mut ctx, ui);
    }
}

fn style() -> Style {
    Style::new([255, 70, 82, 255], 4.0)
}

fn monitor(index: usize, width: u32, height: u32) -> MonitorInfo {
    MonitorInfo {
        id: MonitorId::new(index as u32 + 1),
        device: format!(r"\\.\DISPLAY{}", index + 1),
        index,
        name: if index == 0 {
            "Primary".into()
        } else {
            format!("Display {}", index + 1)
        },
        x: if index == 0 { 0 } else { 800 },
        y: 0,
        width,
        height,
        scale: 1.0,
        primary: index == 0,
    }
}

fn one_monitor() -> Vec<MonitorInfo> {
    vec![monitor(0, 800, 600)]
}

fn two_monitors() -> Vec<MonitorInfo> {
    vec![monitor(0, 800, 600), monitor(1, 1280, 720)]
}

fn arrow(selection: Rect) -> Shape {
    let mut shape = Shape::begin(
        Tool::Arrow,
        (selection.x as f32 + 60.0, selection.y as f32 + 70.0),
        style(),
    );
    shape.drag_to((selection.x as f32 + 240.0, selection.y as f32 + 160.0));
    shape
}

fn write_image(path: &Path) {
    let mut image = image::RgbaImage::new(800, 600);
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let shade = (x / 8 + y / 8) % 2;
        *pixel = image::Rgba(if shade == 0 {
            [24, 36, 56, 255]
        } else {
            [30, 44, 66, 255]
        });
    }
    for (x0, y0, width, height, color) in [
        (40, 40, 160, 120, [220, 70, 70, 255]),
        (250, 80, 200, 140, [60, 170, 110, 255]),
        (500, 60, 230, 120, [70, 120, 230, 255]),
        (120, 250, 220, 160, [240, 190, 70, 255]),
        (400, 300, 300, 180, [170, 90, 220, 255]),
        (60, 460, 260, 100, [60, 200, 210, 255]),
    ] {
        for y in y0..y0 + height {
            for x in x0..x0 + width {
                image.put_pixel(x, y, image::Rgba(color));
            }
        }
    }
    for x in (0..800).step_by(80) {
        for y in 0..600 {
            image.put_pixel(x, y, image::Rgba([60, 78, 104, 255]));
        }
    }
    for y in (0..600).step_by(80) {
        for x in 0..800 {
            image.put_pixel(x, y, image::Rgba([60, 78, 104, 255]));
        }
    }
    image.save(path).expect("the picker image is written");
}

fn harness(state: Snap) -> egui_kittest::Harness<'static, Snap> {
    harness_theme(state, ThemePreference::Dark)
}

fn harness_theme(state: Snap, preference: ThemePreference) -> egui_kittest::Harness<'static, Snap> {
    let theme = match preference {
        ThemePreference::Light | ThemePreference::Lime | ThemePreference::Peach => {
            egui::Theme::Light
        }
        ThemePreference::System | ThemePreference::Dark => egui::Theme::Dark,
    };
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_pixels_per_point(1.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .with_theme(theme)
        .build_ui_state(|ui, state: &mut Snap| state.frame(ui), state);
    crate::ui::theme::install(&harness.ctx);
    crate::ui::theme::apply_preference(&harness.ctx, preference);
    mediaa2_i18n::set_language(Some("en"), None);
    harness.run_steps(4);
    harness
}

fn click_at(harness: &mut egui_kittest::Harness<'_, Snap>, pos: egui::Pos2) {
    harness.hover_at(pos);
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::default(),
    });
    harness.run_steps(1);
    harness.event(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::default(),
    });
    harness.run_steps(1);
}

#[test]
fn picker_selected() {
    mediaa2_i18n::set_language(Some("en"), None);
    let selection = Rect::new(80, 70, 340, 240);
    let mut harness = harness(Snap::new(
        one_monitor(),
        Some(selection),
        vec![arrow(selection)],
        None,
        false,
    ));
    harness.snapshot("picker-selected");
}

#[test]
fn picker_light() {
    let selection = Rect::new(80, 70, 340, 240);
    let mut state = Snap::new(
        one_monitor(),
        Some(selection),
        vec![arrow(selection)],
        None,
        false,
    );
    state.view.state.settings.theme = ThemePreference::Light;
    state.ui.settings.draft.theme = ThemePreference::Light;
    let mut harness = harness_theme(state, ThemePreference::Light);
    harness.snapshot("picker-light");
}

#[test]
fn picker_text() {
    let selection = Rect::new(100, 90, 360, 250);
    let mut harness = harness(Snap::new(
        one_monitor(),
        Some(selection),
        Vec::new(),
        None,
        false,
    ));
    harness.get_by_label(egui_phosphor::regular::TEXT_T).click();
    harness.run_steps(3);
    click_at(&mut harness, egui::pos2(40.0, 40.0));
    harness.run_steps(2);
    harness.event(egui::Event::Text("Label".into()));
    harness.run_steps(2);
    assert_eq!(
        harness
            .state()
            .ui
            .picker
            .as_ref()
            .expect("picker is open")
            .text
            .as_deref(),
        Some("Label")
    );
    harness.snapshot("picker-text");
}

#[test]
fn picker_palette() {
    let selection = Rect::new(120, 100, 400, 260);
    let mut harness = harness(Snap::new(
        one_monitor(),
        Some(selection),
        Vec::new(),
        None,
        false,
    ));
    harness.get_by_label("Color").click();
    harness.run_steps(3);
    harness.snapshot("picker-palette");
}

#[test]
fn picker_edge() {
    let selection = Rect::new(520, 390, 270, 200);
    let mut harness = harness(Snap::new(
        one_monitor(),
        Some(selection),
        Vec::new(),
        None,
        false,
    ));
    harness.snapshot("picker-edge");
}

#[test]
fn picker_monitor() {
    let mut harness = harness(Snap::new(two_monitors(), None, Vec::new(), None, false));
    harness.snapshot("picker-monitor");
    harness.key_press(egui::Key::Tab);
    harness.run_steps(2);
    let commands = harness.state_mut().actions.take_commands();
    assert!(
        commands.iter().any(|command| matches!(
            command,
            ClientCommand::Picker(PickerCommand::Monitor {
                session: 7,
                index: 1
            })
        )),
        "Tab asks the backend for the next monitor: {commands:?}"
    );
}
