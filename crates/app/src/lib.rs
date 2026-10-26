pub mod app;
mod features;
mod hotkeys;
mod i18n;
pub mod icon;
mod jobs;
mod latest_worker;
mod overlay;
mod pages;
mod region_marker;
mod services;
mod theme;
mod tray;

rust_i18n::i18n!("locales", fallback = "en");
