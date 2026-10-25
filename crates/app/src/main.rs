#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod hotkeys;
mod hover_preview;
mod i18n;
mod icon;
mod overlay;
mod pages;
mod player;
mod region_marker;
mod theme;
mod thumbs;
mod tray;
mod viewer;

rust_i18n::i18n!("locales", fallback = "en");

use eframe::egui;
use mediaa_core::paths;
use std::time::Duration;

fn main() -> eframe::Result<()> {
    mediaa_core::logging::init(&paths::logs_dir(), log::LevelFilter::Info);
    log::info!("MediaA {} starting", env!("CARGO_PKG_VERSION"));
    mediaa_media::init();
    let removed = paths::remove_stale_entries(&paths::work_dir(), Duration::from_secs(24 * 3600));
    if removed > 0 {
        log::info!("removed {removed} stale scratch entries");
    }

    let args: Vec<String> = std::env::args().skip(1).collect();
    let startup = app::StartupAction::from_args(args.clone());
    let page = app::page_from_args(&args);
    let start_in_tray =
        startup == app::StartupAction::Tray || mediaa_core::Settings::load().start_in_tray;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MediaA")
            .with_app_id("mediaa")
            .with_transparent(true)
            .with_visible(!start_in_tray)
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([480.0, 480.0])
            .with_icon(std::sync::Arc::new(egui::IconData {
                rgba: icon::rgba(64),
                width: 64,
                height: 64,
            })),
        ..Default::default()
    };
    eframe::run_native(
        "MediaA",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, startup, page)))),
    )
}
