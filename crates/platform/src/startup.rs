use anyhow::Context;
use auto_launch::{
    AutoLaunch, AutoLaunchBuilder, LinuxLaunchMode, MacOSLaunchMode, WindowsEnableMode,
};
use mediaa_core::paths::APP_IDENTITY;
use std::path::Path;

const TRAY_ARG: &str = "--tray";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoStart {
    Disabled,
    Window,
    Tray,
}

pub fn startup_args(mode: AutoStart) -> &'static [&'static str] {
    match mode {
        AutoStart::Tray => &[TRAY_ARG],
        AutoStart::Disabled | AutoStart::Window => &[],
    }
}

pub fn configure(mode: AutoStart) -> anyhow::Result<()> {
    let launcher = launcher(mode)?;
    match mode {
        AutoStart::Disabled => launcher
            .disable()
            .context("cannot disable MediaA autostart"),
        AutoStart::Window | AutoStart::Tray => {
            launcher.enable().context("cannot enable MediaA autostart")
        }
    }
}

fn launcher(mode: AutoStart) -> anyhow::Result<AutoLaunch> {
    let executable = std::env::current_exe().context("cannot locate the MediaA executable")?;
    launcher_for(&executable, mode)
}

fn launcher_for(executable: &Path, mode: AutoStart) -> anyhow::Result<AutoLaunch> {
    #[cfg(windows)]
    let app_path = format!("\"{}\"", executable.display());
    #[cfg(not(windows))]
    let app_path = executable.to_string_lossy().into_owned();
    AutoLaunchBuilder::new()
        .set_app_name(APP_IDENTITY.name)
        .set_app_path(&app_path)
        .set_args(startup_args(mode))
        .set_windows_enable_mode(WindowsEnableMode::CurrentUser)
        .set_linux_launch_mode(LinuxLaunchMode::XdgAutostart)
        .set_macos_launch_mode(MacOSLaunchMode::LaunchAgent)
        .build()
        .context("cannot configure MediaA autostart")
}

#[cfg(test)]
mod tests {
    use super::{launcher_for, startup_args, AutoStart, TRAY_ARG};
    use std::path::Path;

    #[test]
    fn tray_start_adds_the_tray_argument() {
        assert!(startup_args(AutoStart::Disabled).is_empty());
        assert!(startup_args(AutoStart::Window).is_empty());
        assert_eq!(startup_args(AutoStart::Tray), [TRAY_ARG]);
    }

    #[test]
    fn launcher_keeps_the_selected_start_mode() {
        let executable = Path::new("C:\\Program Files\\MediaA\\mediaa.exe");
        let normal = launcher_for(executable, AutoStart::Window).unwrap();
        let tray = launcher_for(executable, AutoStart::Tray).unwrap();
        assert!(normal.get_args().is_empty());
        assert_eq!(tray.get_args(), [TRAY_ARG]);
        assert!(tray.get_app_path().contains("mediaa.exe"));
    }
}
