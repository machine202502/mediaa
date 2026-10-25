use anyhow::Context;
use auto_launch::{
    AutoLaunch, AutoLaunchBuilder, LinuxLaunchMode, MacOSLaunchMode, WindowsEnableMode,
};
use std::path::Path;

const APP_NAME: &str = "MediaA";
const TRAY_ARG: &str = "--tray";

pub fn startup_args(start_in_tray: bool) -> &'static [&'static str] {
    if start_in_tray {
        &[TRAY_ARG]
    } else {
        &[]
    }
}

pub fn set_enabled(enabled: bool, start_in_tray: bool) -> anyhow::Result<()> {
    let launcher = launcher(start_in_tray)?;
    if enabled {
        launcher
            .enable()
            .context("cannot enable MediaA autostart")?;
    } else {
        launcher
            .disable()
            .context("cannot disable MediaA autostart")?;
    }
    Ok(())
}

fn launcher(start_in_tray: bool) -> anyhow::Result<AutoLaunch> {
    let executable = std::env::current_exe().context("cannot locate the MediaA executable")?;
    launcher_for(&executable, start_in_tray)
}

fn launcher_for(executable: &Path, start_in_tray: bool) -> anyhow::Result<AutoLaunch> {
    #[cfg(windows)]
    let app_path = format!("\"{}\"", executable.display());
    #[cfg(not(windows))]
    let app_path = executable.to_string_lossy().into_owned();
    AutoLaunchBuilder::new()
        .set_app_name(APP_NAME)
        .set_app_path(&app_path)
        .set_args(startup_args(start_in_tray))
        .set_windows_enable_mode(WindowsEnableMode::CurrentUser)
        .set_linux_launch_mode(LinuxLaunchMode::XdgAutostart)
        .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
        .build()
        .context("cannot configure MediaA autostart")
}

#[cfg(test)]
mod tests {
    use super::{launcher_for, startup_args, TRAY_ARG};
    use std::path::Path;

    #[test]
    fn tray_start_adds_the_tray_argument() {
        assert!(startup_args(false).is_empty());
        assert_eq!(startup_args(true), [TRAY_ARG]);
    }

    #[test]
    fn launcher_keeps_the_selected_start_mode() {
        let executable = Path::new("C:\\Program Files\\MediaA\\mediaa.exe");
        let normal = launcher_for(executable, false).unwrap();
        let tray = launcher_for(executable, true).unwrap();
        assert!(normal.get_args().is_empty());
        assert_eq!(tray.get_args(), [TRAY_ARG]);
        assert!(tray.get_app_path().contains("mediaa.exe"));
    }
}
