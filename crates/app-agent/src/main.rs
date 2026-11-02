#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

fn main() {
    std::env::set_var("MEDIAA_DEV_AGENT", "1");
    mediaa::run();
}
