pub fn print_screen_down() -> bool {
    #[cfg(windows)]
    {
        crate::windows::keys::print_screen_down()
    }
    #[cfg(not(windows))]
    {
        false
    }
}
