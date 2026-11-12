use mediaa2_core::navigation::Page;
use mediaa2_core::settings::AfterCapture;
use mediaa2_protocol::wire::{read_frame, write_frame};
use mediaa2_protocol::{
    ClientCommand, ClientMessage, Role, ScreenshotCommand, ServerMessage, PROTOCOL,
};
use std::ffi::OsString;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::windows::named_pipe::ClientOptions;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_STEP: Duration = Duration::from_millis(100);
const RESULT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn run(args: &[String]) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async_run(args))
}

async fn async_run(args: &[String]) -> anyhow::Result<()> {
    if let Err(error) = send(args).await {
        mediaa2_platform::dialogs::show_error("MediaA", &format!("{error:#}"));
    }
    Ok(())
}

async fn send(args: &[String]) -> anyhow::Result<()> {
    let mut pipe = ensure_backend().await?;
    for command in request_commands(args) {
        let id = command_id(&command);
        write_frame(
            &mut pipe,
            &ClientMessage::Command {
                id,
                command: command.clone(),
            },
        )
        .await?;
        let message =
            tokio::time::timeout(RESULT_TIMEOUT, read_frame::<_, ServerMessage>(&mut pipe))
                .await??;
        if let ServerMessage::Result {
            id: _,
            result: Err(error),
        } = message
        {
            anyhow::bail!("{} ({:?})", error.message, error.code);
        }
    }
    Ok(())
}

fn command_id(command: &ClientCommand) -> u64 {
    match command {
        ClientCommand::Screenshot(_) => 1,
        ClientCommand::ShowWindow { .. } => 2,
        _ => 0,
    }
}

async fn ensure_backend() -> anyhow::Result<impl AsyncRead + AsyncWrite + Unpin> {
    if let Some(pipe) = connect().await? {
        return Ok(pipe);
    }
    mediaa2_platform::process::spawn_detached(&[OsString::from("--backend")])?;
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    loop {
        if tokio::time::Instant::now() >= deadline {
            anyhow::bail!("the MediaA backend did not start");
        }
        if let Some(pipe) = connect().await? {
            return Ok(pipe);
        }
        tokio::time::sleep(CONNECT_STEP).await;
    }
}

async fn connect() -> anyhow::Result<Option<impl AsyncRead + AsyncWrite + Unpin>> {
    let name = mediaa2_platform::ipc::socket_name(&scope());
    let Ok(mut pipe) = ClientOptions::new().open(&name) else {
        return Ok(None);
    };
    let hello = ClientMessage::Hello {
        protocol: PROTOCOL,
        role: Role::Launcher,
        startup: None,
    };
    write_frame(&mut pipe, &hello).await?;
    let message =
        tokio::time::timeout(RESULT_TIMEOUT, read_frame::<_, ServerMessage>(&mut pipe)).await??;
    if !matches!(message, ServerMessage::State(_)) {
        anyhow::bail!("unexpected MediaA backend reply");
    }
    Ok(Some(pipe))
}

fn request_commands(args: &[String]) -> Vec<ClientCommand> {
    if args.iter().any(|arg| arg == "--area") {
        return vec![ClientCommand::Screenshot(ScreenshotCommand::StartPicker)];
    }
    if args.iter().any(|arg| arg == "--fullscreen") {
        return vec![ClientCommand::Screenshot(ScreenshotCommand::Fullscreen {
            after: AfterCapture::CopyAndSave,
        })];
    }
    vec![ClientCommand::ShowWindow {
        page: page_from_args(args),
    }]
}

fn page_from_args(args: &[String]) -> Option<Page> {
    args.iter()
        .find_map(|arg| arg.strip_prefix("--page="))
        .and_then(Page::from_arg)
}

fn scope() -> String {
    match std::env::var_os("MEDIAA2_INSTANCE_ID") {
        Some(value) if !value.is_empty() => format!(
            "{:016x}",
            mediaa2_core::hash::stable_hash(value.to_string_lossy().as_bytes())
        ),
        _ => "default".to_string(),
    }
}
