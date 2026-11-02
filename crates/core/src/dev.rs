use std::ffi::OsString;
use std::path::PathBuf;

const LIBRARY_ENV: &str = "MEDIAA_DEV_LIBRARY";
const INSTANCE_ENV: &str = "MEDIAA_INSTANCE_ID";
const CONVERT_ENV: &str = "MEDIAA_DEV_CONVERT";
const PICKER_ENV: &str = "MEDIAA_DEV_PICKER";
const VIEW_ENV: &str = "MEDIAA_DEV_VIEW";
const PREVIEW_ENV: &str = "MEDIAA_DEV_PREVIEW";
const BACKGROUND_ENV: &str = "MEDIAA_DEV_BACKGROUND";
const AGENT_ENV: &str = "MEDIAA_DEV_AGENT";
const RECORD_ENV: &str = "MEDIAA_DEV_RECORD";
const TRAY_ENV: &str = "MEDIAA_DEV_TRAY";
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

pub fn tray_action() -> Option<String> {
    text(TRAY_ENV)
}

pub fn record_seconds() -> Option<u64> {
    text(RECORD_ENV).and_then(|value| {
        if value == "1" || value.eq_ignore_ascii_case("true") {
            Some(3)
        } else {
            value.parse().ok().filter(|seconds| *seconds > 0)
        }
    })
}

pub fn agent_copy() -> bool {
    text(AGENT_ENV).is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub fn input_file() -> Option<std::path::PathBuf> {
    value("MEDIAA_DEV_INPUT").map(std::path::PathBuf::from)
}

pub fn background() -> bool {
    text(BACKGROUND_ENV).is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

pub fn preview_filter() -> Option<String> {
    text(PREVIEW_ENV)
}

pub fn instance_scope() -> Option<String> {
    let value = value(INSTANCE_ENV).or_else(|| value(LIBRARY_ENV))?;
    let hash = crate::hash::stable_hash(value.to_string_lossy().as_bytes());
    Some(format!("{hash:016x}"))
}
