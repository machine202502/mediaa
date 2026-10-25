pub mod app;
mod clipboard_service;
mod hotkeys;
mod hover_preview;
mod i18n;
pub mod icon;
mod latest_worker;
mod overlay;
mod pages;
mod player;
mod region_marker;
mod settings_writer;
mod theme;
mod thumbs;
mod tray;
mod viewer;

rust_i18n::i18n!("locales", fallback = "en");
