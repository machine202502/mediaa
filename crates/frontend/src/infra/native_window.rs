pub(crate) fn prepare_handle(window: isize) -> bool {
    mediaa_platform::window::prepare_handle(window).is_ok()
}
pub(crate) fn hide_window(title: &str) -> bool {
    mediaa_platform::window::hide_window(title).is_ok()
}
pub(crate) fn prepare_background(title: &str) -> bool {
    mediaa_platform::window::prepare_background(title).is_ok()
}

pub(crate) fn background_show(title: &str) -> bool {
    mediaa_platform::window::background_show(title).is_ok()
}

pub(crate) fn foreground_window() -> isize {
    mediaa_platform::window::foreground_window()
}

pub(crate) fn restore_foreground(window: isize) -> bool {
    mediaa_platform::window::restore_foreground(window).is_ok()
}
pub(crate) fn disable_open_animation(title: &str) -> bool {
    mediaa_platform::window::disable_open_animation(title).is_ok()
}

pub(crate) fn exclude_from_capture(title: &str) -> bool {
    mediaa_platform::window::exclude_from_capture(title).is_ok()
}
