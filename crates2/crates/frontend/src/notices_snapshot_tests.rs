use crate::pages;
use crate::pages::notices as notices_page;
use crate::textures::Textures;
use crate::ui::theme;
use crate::ui_state::UiState;
use crate::view::BackendView;
use eframe::egui;
use egui_kittest::kittest::Queryable;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::screen::{MonitorInfo, WindowInfo};
use mediaa2_core::settings::{Settings, ThemePreference};
use mediaa2_protocol::state::{ConversionState, Notice, RecordingState, RecoveryState, State};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct Snap {
    view: BackendView,
    ui: UiState,
    actions: pages::Actions,
    textures: Textures,
    monitors: Vec<MonitorInfo>,
    windows: Vec<WindowInfo>,
    microphones: Vec<String>,
    layers: bool,
}

impl Snap {
    fn new(notices: Vec<Notice>) -> Self {
        let settings = Settings {
            language: Some("en".into()),
            theme: ThemePreference::Dark,
            ..Settings::default()
        };
        let mut ui = UiState::default();
        ui.settings.begin(settings.clone());
        let view = BackendView::new(State {
            settings,
            recording: RecordingState::Idle,
            conversion: ConversionState::Idle,
            recovery: RecoveryState { running: false },
            notices,
            picker: None,
            library_revision: 1,
        });
        Self {
            view,
            ui,
            actions: pages::Actions::default(),
            textures: Textures::default(),
            monitors: Vec::new(),
            windows: Vec::new(),
            microphones: Vec::new(),
            layers: false,
        }
    }

    fn frame(&mut self, ui: &mut egui::Ui) {
        let p = theme::palette(ui);
        ui.painter().rect_filled(ui.ctx().content_rect(), 0.0, p.bg);
        if self.layers {
            egui::Area::new(egui::Id::new("fake-stop-overlay"))
                .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0))
                .order(egui::Order::Foreground)
                .show(ui.ctx(), |ui| {
                    egui::Frame::new()
                        .fill(theme::palette(ui).card)
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            ui.set_width(300.0);
                            ui.set_min_height(220.0);
                            ui.label("Recording controls");
                        });
                });
        }
        {
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
            notices_page::show(&mut ctx, ui);
        }
        if self.layers {
            egui::Modal::new(egui::Id::new("notice-layer-modal")).show(ui.ctx(), |ui| {
                ui.set_width(320.0);
                ui.label("Confirm action");
            });
        }
    }
}

fn backend_notice(id: u64, level: NoticeLevel, summary: &str, detail: &str) -> Notice {
    Notice {
        id,
        level,
        summary: summary.into(),
        detail: detail.into(),
        action: None,
    }
}

fn harness(state: Snap) -> egui_kittest::Harness<'static, Snap> {
    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(800.0, 600.0))
        .with_pixels_per_point(1.0)
        .with_os(egui::os::OperatingSystem::Windows)
        .with_theme(egui::Theme::Dark)
        .build_ui_state(|ui, state: &mut Snap| state.frame(ui), state);
    theme::install(&harness.ctx);
    theme::apply_preference(&harness.ctx, ThemePreference::Dark);
    mediaa2_i18n::set_language(Some("en"), None);
    harness.run_steps(2);
    harness
}

fn settle(harness: &mut egui_kittest::Harness<'_, Snap>) {
    notices_page::age_notices(
        &harness.ctx,
        Duration::from_secs(1),
        Duration::from_secs(30),
    );
    harness.run_steps(2);
}

fn bottom_close(harness: &egui_kittest::Harness<'_, Snap>) -> f32 {
    harness
        .get_all_by_label(egui_phosphor::regular::X)
        .last()
        .expect("a close button")
        .rect()
        .bottom()
}

fn stack_state() -> Snap {
    let mut state = Snap::new(Vec::new());
    state.actions.notice(
        NoticeLevel::Success,
        "Screenshot copied and saved",
        "Screenshot_2026-11-01_10-13-26.png",
    );
    state.actions.notice(
        NoticeLevel::Info,
        "Files added to the conversion queue: 2",
        "Screenshot_2026-11-01_10-27-36.png\nScreenshot_2026-11-01_10-31-02.png",
    );
    state
}

#[test]
fn notices_single() {
    let _serial = serial();
    let mut harness = harness(Snap::new(vec![backend_notice(
        1,
        NoticeLevel::Success,
        "Converted",
        "The result is ready in the library",
    )]));
    notices_page::expand_notice(&harness.ctx, 0);
    settle(&mut harness);
    harness.snapshot("notices");
}

#[test]
fn notices_stack_two() {
    let _serial = serial();
    let mut harness = harness(stack_state());
    settle(&mut harness);
    harness.snapshot("notices-stack-two");
}

#[test]
fn notices_stack_after_close() {
    let _serial = serial();
    let mut harness = harness(stack_state());
    settle(&mut harness);
    let before = bottom_close(&harness);
    harness
        .get_all_by_label(egui_phosphor::regular::X)
        .next()
        .expect("the top notice")
        .click();
    harness.run_steps(5);
    assert_eq!(notices_page::notice_count(&harness.ctx), 1);
    let after = bottom_close(&harness);
    assert!(
        (after - before).abs() < 0.5,
        "the remaining notice keeps its place: {before} -> {after}"
    );
    harness.hover_at(egui::pos2(20.0, 20.0));
    harness.run_steps(2);
    harness.snapshot("notices-stack-after-close");
}

#[test]
fn notices_stack_after_expiry() {
    let _serial = serial();
    let mut harness = harness(stack_state());
    settle(&mut harness);
    let before = bottom_close(&harness);
    notices_page::expire_notice(&harness.ctx, 0);
    harness.run_steps(5);
    assert_eq!(notices_page::notice_count(&harness.ctx), 1);
    let after = bottom_close(&harness);
    assert!(
        (after - before).abs() < 0.5,
        "the remaining notice keeps its place: {before} -> {after}"
    );
    harness.snapshot("notices-stack-after-expiry");
}

#[test]
fn notices_layers() {
    let _serial = serial();
    let mut state = Snap::new(vec![backend_notice(
        1,
        NoticeLevel::Info,
        "Saved",
        "The screenshot is ready",
    )]);
    state.layers = true;
    let mut harness = harness(state);
    settle(&mut harness);
    harness.snapshot("notices-layers");
}

#[test]
fn notices_keep_at_most_five_cards() {
    let _serial = serial();
    let mut state = Snap::new(Vec::new());
    for index in 0..7 {
        state
            .actions
            .notice(NoticeLevel::Info, format!("Notice {index}"), String::new());
    }
    let mut harness = harness(state);
    settle(&mut harness);
    assert_eq!(
        harness.get_all_by_label(egui_phosphor::regular::X).count(),
        5
    );
    assert!(harness.query_by_label("Notice 0").is_none());
    assert!(harness.query_by_label("Notice 6").is_some());
}

#[test]
fn notices_backend_items_are_not_replayed_after_expiry() {
    let _serial = serial();
    let mut harness = harness(Snap::new(vec![backend_notice(
        1,
        NoticeLevel::Info,
        "Transient",
        "Detail",
    )]));
    settle(&mut harness);
    notices_page::expire_notice(&harness.ctx, 0);
    harness.run_steps(3);
    assert_eq!(notices_page::notice_count(&harness.ctx), 0);
    harness.run_steps(3);
    assert_eq!(notices_page::notice_count(&harness.ctx), 0);
}

#[test]
fn notices_hover_pauses_the_remaining_lifetime() {
    let _serial = serial();
    let mut harness = harness(Snap::new(vec![backend_notice(
        1,
        NoticeLevel::Info,
        "Hover me",
        "Detail",
    )]));
    settle(&mut harness);
    let at = harness.get_by_label("Hover me").rect().center();
    harness.hover_at(at);
    harness.run_steps(2);
    assert!(notices_page::notice_hovered(&harness.ctx, 0));
    notices_page::expire_notice(&harness.ctx, 0);
    harness.run_steps(2);
    assert_eq!(notices_page::notice_count(&harness.ctx), 1);
    harness.hover_at(egui::pos2(20.0, 20.0));
    harness.run_steps(3);
    assert!(!notices_page::notice_hovered(&harness.ctx, 0));
    assert_eq!(notices_page::notice_count(&harness.ctx), 0);
}

#[test]
fn notices_render_every_level_icon() {
    let _serial = serial();
    let mut harness = harness(Snap::new(vec![
        backend_notice(1, NoticeLevel::Info, "Info card", "Detail"),
        backend_notice(2, NoticeLevel::Success, "Success card", "Detail"),
        backend_notice(3, NoticeLevel::Warning, "Warning card", "Detail"),
        backend_notice(4, NoticeLevel::Error, "Error card", "Detail"),
    ]));
    settle(&mut harness);
    for icon in [
        egui_phosphor::regular::INFO,
        egui_phosphor::regular::CHECK_CIRCLE,
        egui_phosphor::regular::WARNING,
        egui_phosphor::regular::WARNING_OCTAGON,
    ] {
        assert!(harness.query_by_label(icon).is_some(), "{icon}");
    }
}
