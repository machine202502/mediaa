#[macro_use]
extern crate rust_i18n;

pub use rust_i18n::t;

rust_i18n::i18n!("../frontend/locales", fallback = "en");

#[cfg(test)]
pub mod boundaries;
pub mod daemon;
pub mod dev_mode;
pub mod features;
pub mod i18n;
pub mod icon;
#[macro_use]
pub mod infra;
pub mod runtime;
