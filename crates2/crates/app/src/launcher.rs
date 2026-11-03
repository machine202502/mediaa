use std::ffi::OsString;

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let scope = std::env::var("MEDIAA2_INSTANCE_ID").unwrap_or_else(|_| "default".into());
    let _ = scope;
    let backend_args = vec![OsString::from("--backend")];
    if args.iter().any(|arg| arg == "--tray") {
        mediaa2_platform::process::spawn_detached(&backend_args)?;
        return Ok(());
    }
    mediaa2_platform::process::spawn_detached(&backend_args)?;
    std::thread::sleep(std::time::Duration::from_millis(1200));
    let mut ui_args = vec![OsString::from("--ui")];
    ui_args.push(OsString::from("--show"));
    for arg in args {
        ui_args.push(OsString::from(arg));
    }
    let _child = mediaa2_platform::process::spawn_detached(&ui_args)?;
    Ok(())
}
