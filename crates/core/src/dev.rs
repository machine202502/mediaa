use std::ffi::OsString;
use std::path::PathBuf;

const LIBRARY_ENV: &str = "MEDIAA_DEV_LIBRARY";
const INSTANCE_ENV: &str = "MEDIAA_INSTANCE_ID";
const CONVERT_ENV: &str = "MEDIAA_DEV_CONVERT";
const PICKER_ENV: &str = "MEDIAA_DEV_PICKER";
const VIEW_ENV: &str = "MEDIAA_DEV_VIEW";
const PREVIEW_ENV: &str = "MEDIAA_DEV_PREVIEW";
const NOTICES_ENV: &str = "MEDIAA_DEV_NOTICES";

fn value(name: &str) -> Option<OsString> {
    #[cfg(debug_assertions)]
    {
        std::env::var_os(name)
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = name;
        None
    }
}

fn text(name: &str) -> Option<String> {
    value(name).map(|value| value.to_string_lossy().into_owned())
}

pub fn library_dir() -> Option<PathBuf> {
    value(LIBRARY_ENV).map(PathBuf::from)
}

pub fn convert_input() -> Option<PathBuf> {
    value(CONVERT_ENV).map(PathBuf::from)
}

pub fn picker_enabled() -> bool {
    value(PICKER_ENV).is_some()
}

pub fn view_filter() -> Option<String> {
    text(VIEW_ENV)
}

pub fn preview_filter() -> Option<String> {
    text(PREVIEW_ENV)
}

pub fn notices_mode() -> Option<String> {
    text(NOTICES_ENV)
}

pub fn instance_scope() -> Option<String> {
    let value = value(INSTANCE_ENV).or_else(|| value(LIBRARY_ENV))?;
    let hash = crate::hash::stable_hash(value.to_string_lossy().as_bytes());
    Some(format!("{hash:016x}"))
}
