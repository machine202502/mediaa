pub(crate) fn copy_text(text: &str) -> Result<(), String> {
    mediaa_platform::clipboard::copy_text(text).map_err(|error| format!("{error:#}"))
}
