rust_i18n::i18n!("locales", fallback = "en");

pub mod app;
#[cfg(test)]
pub mod boundaries;
pub mod dev_mode;
pub mod features;
pub mod i18n;
pub mod icon;
pub mod infra;
pub mod protocol;
pub mod session;
#[cfg(test)]
pub mod test_support;
pub mod ui;

pub use mediaa_core::navigation::Page;
