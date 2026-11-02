use super::*;
use std::process::{Command, Stdio};

pub(super) fn hidden_args() -> Vec<String> {
    vec!["--tray".to_string()]
}

enum Attempt {
    Ready(Connection, bool),
    Unreachable(String),
}

pub fn launch(args: &[String]) -> anyhow::Result<()> {
    let daemon_only = args.iter().any(|arg| arg == "--tray");
    match try_launcher() {
        Attempt::Ready(connection, ui_open) => launch_ready(connection, ui_open, daemon_only, args),
        Attempt::Unreachable(error) => {
            log::info!("daemon is not running ({error}); starting it");
            spawn_daemon()?;
            let deadline = Instant::now() + START_TIMEOUT;
            loop {
                match try_launcher() {
                    Attempt::Ready(connection, ui_open) => {
                        return launch_ready(connection, ui_open, daemon_only, args);
                    }
                    Attempt::Unreachable(error) => {
                        if Instant::now() >= deadline {
                            anyhow::bail!("the daemon did not start: {error}");
                        }
                        std::thread::sleep(Duration::from_millis(50));
                    }
                }
            }
        }
    }
}

fn launch_ready(
    mut connection: Connection,
    ui_open: bool,
    daemon_only: bool,
    args: &[String],
) -> anyhow::Result<()> {
    if daemon_only {
        return Ok(());
    }
    let area = args.iter().any(|arg| arg == "--area");
    if area {
        announce_picker(&mut connection);
    }
    if ui_open {
        if !area {
            log::info!("second launch: asking the running ui to show its window");
            if let Err(error) = connection.send_frame(&Frame::Command {
                id: 0,
                name: mediaa_protocol::SHOW_WINDOW.into(),
                payload: Vec::new(),
            }) {
                log::warn!("show window: {error}");
            }
        }
        return Ok(());
    }
    spawn_ui(args)
}

fn try_launcher() -> Attempt {
    let mut connection = match Connection::connect(&scope()) {
        Ok(connection) => connection,
        Err(error) => return Attempt::Unreachable(error.to_string()),
    };
    let hello = Frame::Hello {
        version: mediaa_sync::PROTOCOL_VERSION,
        client: ipc::CLIENT_LAUNCHER.into(),
    };
    if let Err(error) = connection.send_frame(&hello) {
        return Attempt::Unreachable(error.to_string());
    }
    match connection.recv_frame() {
        Ok(Frame::Welcome { metadata, .. }) => Attempt::Ready(
            connection,
            metadata.get("ui").is_some_and(|value| value == "true"),
        ),
        Ok(other) => Attempt::Unreachable(format!("unexpected handshake: {other:?}")),
        Err(error) => Attempt::Unreachable(error.to_string()),
    }
}

fn spawn_daemon() -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    let mut command = hidden_command(&exe);
    command.arg("--backend");
    command.spawn()?;
    Ok(())
}

fn announce_picker(connection: &mut Connection) {
    if let Err(error) = connection.send_frame(&Frame::Command {
        id: 0,
        name: mediaa_protocol::PICKER_STARTING.into(),
        payload: Vec::new(),
    }) {
        log::warn!("picker announce: {error}");
    }
}
pub(super) fn spawn_ui(args: &[String]) -> anyhow::Result<()> {
    let exe = std::env::current_exe()?;
    ui_command(&exe, args).spawn()?;
    Ok(())
}

pub(super) fn hidden_command(exe: &Path) -> Command {
    let mut command = Command::new(exe);
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    command
}

fn ui_command(exe: &Path, args: &[String]) -> Command {
    let mut command = hidden_command(exe);
    command.arg("--ui").args(args);
    command
}
