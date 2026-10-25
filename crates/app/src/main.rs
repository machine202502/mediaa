#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

use eframe::egui;
use mediaa::app;
use mediaa::icon;
use mediaa_core::paths;

fn main() -> eframe::Result<()> {
    mediaa_core::logging::init(&paths::logs_dir(), log::LevelFilter::Info);
    log::info!(
        "{} {} starting",
        paths::APP_IDENTITY.name,
        env!("CARGO_PKG_VERSION")
    );
    let instance = match mediaa_platform::single_instance::SingleInstance::acquire() {
        Ok(Some(instance)) => Some(instance),
        Ok(None) => {
            mediaa_platform::single_instance::SingleInstance::activate_existing();
            log::info!("{} is already running", paths::APP_IDENTITY.name);
            return Ok(());
        }
        Err(error) => {
            log::warn!("single-instance guard unavailable: {error:#}");
            None
        }
    };
    mediaa_media::init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let startup = app::StartupAction::from_args(args.clone());
    let page = app::page_from_args(&args);
    let start_in_tray =
        startup == app::StartupAction::Tray || mediaa_core::Settings::load().start_in_tray;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(paths::APP_IDENTITY.window_title)
            .with_app_id(paths::APP_IDENTITY.application_id)
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
        paths::APP_IDENTITY.window_title,
        options,
        Box::new(move |cc| Ok(Box::new(app::App::new(cc, startup, page, instance)))),
    )
}
