pub(crate) fn disable_open_animation(title: &str) -> bool {
    mediaa_platform::window::disable_open_animation(title).is_ok()
}

pub(crate) fn exclude_from_capture(title: &str) -> bool {
    mediaa_platform::window::exclude_from_capture(title).is_ok()
}
