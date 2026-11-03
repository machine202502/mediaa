#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod launcher;

fn main() -> anyhow::Result<()> {
    std::env::set_var("MEDIAA2_DEV_AGENT", "1");
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args
        .iter()
        .any(|arg| arg == "--backend" || arg == "--daemon")
    {
        return mediaa2_backend::run::run();
    }
    if args.iter().any(|arg| arg == "--ui") {
        return mediaa2_frontend::app::run_ui(&args);
    }
    launcher::run(&args)
}
