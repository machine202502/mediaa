rust_i18n::i18n!("../i18n/locales", fallback = "en");

pub mod app;
pub mod client;
pub mod pages;
pub mod textures;
pub mod ui;
pub mod ui_state;
pub mod view;

#[cfg(test)]
mod snapshot_tests;

#[cfg(test)]
mod picker_snapshot_tests;

#[cfg(test)]
mod viewer_snapshot_tests;

#[cfg(test)]
mod notices_snapshot_tests;

pub fn dev_agent() -> bool {
    std::env::var("MEDIAA2_DEV_AGENT").is_ok_and(|value| value == "1")
}
