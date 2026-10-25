pub mod app;
mod hotkeys;
mod hover_preview;
mod i18n;
pub mod icon;
mod overlay;
mod pages;
mod player;
mod region_marker;
mod theme;
mod thumbs;
mod tray;
mod viewer;

rust_i18n::i18n!("locales", fallback = "en");
