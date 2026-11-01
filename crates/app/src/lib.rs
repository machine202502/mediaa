mod adapters;
mod i18n;
pub mod icon;
#[cfg(test)]
mod layer_check;
mod logic;
pub mod shell;
#[cfg(test)]
mod test_support;
mod ui;
mod wake;

rust_i18n::i18n!("locales", fallback = "en");
