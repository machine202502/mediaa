use crate::services::{
    ConversionHandle, LibraryHandle, PresenceHandle, PreviewHandle, RecordingHandle,
    ScreenshotHandle, SettingsHandle,
};
use crate::state::StateStore;
use anyhow::Context;
use mediaa2_protocol::wire::{read_frame, write_frame};
use mediaa2_protocol::{
    ClientCommand, ClientMessage, CommandError, CommandOk, ConversionCommand, ErrorCode,
    LibraryCommand, PickerCommand, PreviewReady, PreviewState, RecordingCommand, RejectReason,
    Role, ScreenshotCommand, ServerMessage, UiEvent, UiRequest, UiStartup, PROTOCOL,
};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::{broadcast, mpsc, watch};
use tokio::task::JoinSet;

const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const INCOMING_CAPACITY: usize = 16;
const UI_REQUESTS_CAPACITY: usize = 64;

#[derive(Clone)]
pub struct Shutdown {
    tx: watch::Sender<bool>,
}

impl Default for Shutdown {
    fn default() -> Self {
        Self::new()
    }
}

impl Shutdown {
    pub fn new() -> Self {
        let (tx, _) = watch::channel(false);
        Self { tx }
    }

    pub fn cancel(&self) {
        self.tx.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.tx.borrow()
    }

    pub async fn cancelled(&self) {
        let mut rx = self.tx.subscribe();
        loop {
            if *rx.borrow_and_update() {
                return;
            }
            if rx.changed().await.is_err() {
                return;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiSignal {
    Connected(UiStartup),
    Event(UiEvent),
    Disconnected,
}

#[derive(Clone)]
pub struct UiMailbox {
    requests: broadcast::Sender<UiRequest>,
    signals: mpsc::UnboundedSender<UiSignal>,
}

impl UiMailbox {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<UiSignal>) {
        let (requests, _) = broadcast::channel(UI_REQUESTS_CAPACITY);
        let (signals, receiver) = mpsc::unbounded_channel();
        (Self { requests, signals }, receiver)
    }

    pub fn send(&self, request: UiRequest) {
        if self.requests.send(request).is_err() {
            log::debug!("ipc: no ui is subscribed to requests");
        }
    }

    pub fn event(&self, event: UiEvent) {
        let _ = self.signals.send(UiSignal::Event(event));
    }

    pub fn connected(&self, startup: UiStartup) {
        let _ = self.signals.send(UiSignal::Connected(startup));
    }

    pub fn disconnected(&self) {
        let _ = self.signals.send(UiSignal::Disconnected);
    }

    pub fn subscribe(&self) -> broadcast::Receiver<UiRequest> {
        self.requests.subscribe()
    }
}

struct UiLifecycle {
    lease: Option<UiLease>,
    mailbox: UiMailbox,
}

impl Drop for UiLifecycle {
    fn drop(&mut self) {
        let _ = self.lease.take();
        self.mailbox.disconnected();
    }
}

#[derive(Clone, Default)]
struct UiGate {
    open: Arc<AtomicBool>,
}

struct UiLease {
    open: Arc<AtomicBool>,
}

impl UiGate {
    fn new() -> Self {
        Self::default()
    }

    fn claim(&self) -> Option<UiLease> {
        self.open
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| UiLease {
                open: Arc::clone(&self.open),
            })
    }
}

impl Drop for UiLease {
    fn drop(&mut self) {
        self.open.store(false, Ordering::Release);
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the composition root passes every daemon service explicitly"
)]
pub async fn serve(
    state: Arc<StateStore>,
    settings: SettingsHandle,
    library: LibraryHandle,
    recording: RecordingHandle,
    conversion: ConversionHandle,
    screenshots: ScreenshotHandle,
    previews: PreviewHandle,
    presence: PresenceHandle,
    mailbox: UiMailbox,
    shutdown: Shutdown,
) -> anyhow::Result<()> {
    let scope = scope();
    mediaa2_platform::ipc::prepare(&scope);
    let name = mediaa2_platform::ipc::socket_name(&scope);
    let mut server = Some(
        ServerOptions::new()
            .create(&name)
            .with_context(|| format!("cannot create {}", name.display()))?,
    );
    let gate = UiGate::new();
    let mut connections = JoinSet::new();
    log::info!("ipc: listening on {}", name.display());
    loop {
        let accepted = {
            let current = server.as_ref().expect("pipe server");
            tokio::select! {
                result = current.connect() => result,
                () = shutdown.cancelled() => break,
                joined = connections.join_next(), if !connections.is_empty() => {
                    if let Some(Err(error)) = joined {
                        log::debug!("ipc: connection task failed: {error}");
                    }
                    continue;
                }
            }
        };
        accepted.with_context(|| format!("cannot accept on {}", name.display()))?;
        let connected = server.take().expect("pipe server");
        server = Some(
            ServerOptions::new()
                .create(&name)
                .with_context(|| format!("cannot create {}", name.display()))?,
        );
        if shutdown.is_cancelled() {
            break;
        }
        let state = Arc::clone(&state);
        let settings = settings.clone();
        let library = library.clone();
        let recording = recording.clone();
        let conversion = conversion.clone();
        let screenshots = screenshots.clone();
        let previews = previews.clone();
        let presence = presence.clone();
        let mailbox = mailbox.clone();
        let shutdown = shutdown.clone();
        let gate = gate.clone();
        connections.spawn(async move {
            if let Err(error) = handle_connection(
                connected,
                state,
                settings,
                library,
                recording,
                conversion,
                screenshots,
                previews,
                presence,
                mailbox,
                shutdown,
                gate,
            )
            .await
            {
                log::debug!("ipc: connection ended: {error:#}");
            }
        });
    }
    let drain = async {
        while let Some(result) = connections.join_next().await {
            if let Err(error) = result {
                log::debug!("ipc: connection task failed: {error}");
            }
        }
    };
    if tokio::time::timeout(Duration::from_secs(5), drain)
        .await
        .is_err()
    {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "the connection owns the daemon handles it dispatches commands to"
)]
async fn handle_connection<S>(
    mut stream: S,
    state: Arc<StateStore>,
    settings: SettingsHandle,
    library: LibraryHandle,
    recording: RecordingHandle,
    conversion: ConversionHandle,
    screenshots: ScreenshotHandle,
    previews: PreviewHandle,
    presence: PresenceHandle,
    mailbox: UiMailbox,
    shutdown: Shutdown,
    gate: UiGate,
) -> anyhow::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let hello = match read_frame::<_, ClientMessage>(&mut stream).await {
        Ok(message) => message,
        Err(error) if is_disconnect(&error) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let ClientMessage::Hello {
        protocol,
        role,
        startup,
    } = hello
    else {
        let _ = send(
            &mut stream,
            &ServerMessage::Reject {
                reason: RejectReason::BadHandshake,
            },
        )
        .await;
        return Ok(());
    };
    if protocol != PROTOCOL {
        let _ = send(
            &mut stream,
            &ServerMessage::Reject {
                reason: RejectReason::ProtocolVersion { client: protocol },
            },
        )
        .await;
        return Ok(());
    }
    let is_ui = matches!(role, Role::Ui);
    let gate_lease = match role {
        Role::Ui => match gate.claim() {
            Some(lease) => Some(lease),
            None => {
                let _ = send(
                    &mut stream,
                    &ServerMessage::Reject {
                        reason: RejectReason::UiAlreadyOpen,
                    },
                )
                .await;
                return Ok(());
            }
        },
        Role::Launcher => None,
    };
    let mut requests = mailbox.subscribe();
    let _lifecycle = is_ui.then(|| {
        mailbox.connected(startup.unwrap_or(UiStartup::Show));
        UiLifecycle {
            lease: gate_lease,
            mailbox: mailbox.clone(),
        }
    });

    let mut state_rx = state.subscribe();
    let snapshot = state.snapshot();
    send(&mut stream, &ServerMessage::State((*snapshot).clone())).await?;

    let (mut reader, mut writer) = tokio::io::split(stream);
    let (preview_tx, mut preview_rx) = mpsc::unbounded_channel();
    let (tx, mut rx) = mpsc::channel(INCOMING_CAPACITY);
    let reader_task = tokio::spawn(async move {
        loop {
            match read_frame::<_, ClientMessage>(&mut reader).await {
                Ok(message) => {
                    if tx.send(Ok(message)).await.is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = tx.send(Err(error)).await;
                    break;
                }
            }
        }
    });

    let outcome: anyhow::Result<()> = loop {
        tokio::select! {
            biased;
            () = shutdown.cancelled() => break Ok(()),
            changed = state_rx.changed() => {
                if changed.is_err() {
                    break Ok(());
                }
                let current = state.snapshot();
                if let Err(error) = send(
                    &mut writer,
                    &ServerMessage::State((*current).clone()),
                )
                .await
                {
                    break Err(error.into());
                }
            }
            request = next_request(&mut requests, is_ui) => {
                match request {
                    Ok(request) => {
                        if let Err(error) = send(
                            &mut writer,
                            &ServerMessage::UiRequest(request),
                        )
                        .await
                        {
                            break Err(error.into());
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(skipped)) => {
                        log::warn!("ipc: ui request lag: {skipped}");
                    }
                    Err(broadcast::error::RecvError::Closed) => break Ok(()),
                }
            }
            preview = preview_rx.recv() => {
                if let Some(preview) = preview {
                    if let Err(error) = send(
                        &mut writer,
                        &ServerMessage::Preview(preview),
                    )
                    .await
                    {
                        break Err(error.into());
                    }
                }
            }
            incoming = rx.recv() => {
                match incoming {
                    Some(Ok(ClientMessage::Command { id, command })) => {
                        log::info!("ipc: command id={id} {command:?}");
                        let result = dispatch(
                            command,
                            &state,
                            &settings,
                            &library,
                            &recording,
                            &conversion,
                            &screenshots,
                            &previews,
                            &presence,
                            &shutdown,
                            &preview_tx,
                        )
                        .await;
                        log::info!(
                            "ipc: result id={id} ok={} error={:?}",
                            result.is_ok(),
                            result.as_ref().err().map(|error| (&error.code, &error.message))
                        );
                        if let Err(error) = send(
                            &mut writer,
                            &ServerMessage::Result { id, result },
                        )
                        .await
                        {
                            break Err(error.into());
                        }
                        if shutdown.is_cancelled() {
                            break Ok(());
                        }
                    }
                    Some(Ok(ClientMessage::Ui(event))) => {
                        if is_ui {
                            mailbox.event(event);
                        } else {
                            log::debug!("ipc: launcher sent a ui event {event:?}");
                        }
                    }
                    Some(Ok(ClientMessage::Hello { .. })) => {
                        let _ = send(
                            &mut writer,
                            &ServerMessage::Reject {
                                reason: RejectReason::BadHandshake,
                            },
                        )
                        .await;
                        break Ok(());
                    }
                    Some(Err(error)) => {
                        if is_disconnect(&error) {
                            break Ok(());
                        }
                        break Err(error.into());
                    }
                    None => break Ok(()),
                }
            }
        }
    };

    reader_task.abort();
    outcome
}

async fn next_request(
    requests: &mut broadcast::Receiver<UiRequest>,
    is_ui: bool,
) -> Result<UiRequest, broadcast::error::RecvError> {
    if !is_ui {
        return std::future::pending().await;
    }
    requests.recv().await
}

#[allow(
    clippy::too_many_arguments,
    reason = "one command dispatcher receives every daemon handle"
)]
async fn dispatch(
    command: ClientCommand,
    state: &StateStore,
    settings: &SettingsHandle,
    library: &LibraryHandle,
    recording: &RecordingHandle,
    conversion: &ConversionHandle,
    screenshots: &ScreenshotHandle,
    previews: &PreviewHandle,
    presence: &PresenceHandle,
    shutdown: &Shutdown,
    preview_tx: &mpsc::UnboundedSender<PreviewReady>,
) -> Result<CommandOk, CommandError> {
    match command {
        ClientCommand::SetSettings(value) => {
            settings.apply(value).await?;
            Ok(CommandOk::Unit)
        }
        ClientCommand::ShowWindow => {
            presence.open();
            Ok(CommandOk::Unit)
        }
        ClientCommand::Quit => {
            presence.quit();
            shutdown.cancel();
            Ok(CommandOk::Unit)
        }
        ClientCommand::Library(command) => handle_library(command, state, library).await,
        ClientCommand::Recording(command) => handle_recording(command, recording).await,
        ClientCommand::Conversion(command) => handle_conversion(command, conversion).await,
        ClientCommand::Screenshot(command) => handle_screenshot(command, screenshots).await,
        ClientCommand::Picker(command) => handle_picker(command, screenshots, presence).await,
        ClientCommand::HotkeysPaused(_) => Ok(CommandOk::Unit),
        ClientCommand::Preview { path, kind } => {
            previews.request(path, kind, preview_tx.clone());
            Ok(CommandOk::Preview(PreviewState::Pending))
        }
        ClientCommand::Probe { .. } => Err(unavailable()),
    }
}

async fn handle_recording(
    command: RecordingCommand,
    recording: &RecordingHandle,
) -> Result<CommandOk, CommandError> {
    match command {
        RecordingCommand::Start {
            target,
            profile,
            hide,
        } => recording.start(target, profile, hide).await,
        RecordingCommand::Stop { action, source } => recording.stop(action, source).await,
        RecordingCommand::TogglePause => recording.pause().await,
        RecordingCommand::CancelStart => recording.cancel_start().await,
        RecordingCommand::CancelFinalize => recording.cancel_finalize().await,
    }
}

async fn handle_conversion(
    command: ConversionCommand,
    conversion: &ConversionHandle,
) -> Result<CommandOk, CommandError> {
    match command {
        ConversionCommand::Start {
            inputs,
            target,
            output_dir,
        } => conversion.start(inputs, target, output_dir).await,
        ConversionCommand::Cancel => conversion.cancel().await,
    }
}

async fn handle_screenshot(
    command: ScreenshotCommand,
    screenshots: &ScreenshotHandle,
) -> Result<CommandOk, CommandError> {
    match command {
        ScreenshotCommand::Monitor { monitor, after } => {
            screenshots.capture_monitor(monitor, after).await
        }
        ScreenshotCommand::Window { window, after } => {
            screenshots.capture_window(window, after).await
        }
        ScreenshotCommand::StartPicker => screenshots.start_picker().await,
    }
}

async fn handle_picker(
    command: PickerCommand,
    screenshots: &ScreenshotHandle,
    presence: &PresenceHandle,
) -> Result<CommandOk, CommandError> {
    match command {
        PickerCommand::Commit {
            session,
            selection,
            shapes,
            format,
            action,
            output,
        } => {
            let result = screenshots
                .picker_commit(session, selection, shapes, format, action, output)
                .await;
            if result.is_ok() {
                presence.picker_done();
            }
            result
        }
        PickerCommand::Cancel { session } => {
            let result = screenshots.picker_cancel(session).await;
            if result.is_ok() {
                presence.picker_done();
            }
            result
        }
        PickerCommand::Monitor { session, index } => {
            screenshots.picker_monitor(session, index).await
        }
    }
}

async fn handle_library(
    command: LibraryCommand,
    state: &StateStore,
    library: &LibraryHandle,
) -> Result<CommandOk, CommandError> {
    match command {
        LibraryCommand::Query(query) => library
            .query(&query)
            .map(CommandOk::LibraryPage)
            .map_err(internal),
        LibraryCommand::Refresh => {
            library.refresh().map_err(internal)?;
            sync_revision(state, library);
            Ok(CommandOk::Unit)
        }
        LibraryCommand::Rename { from, to, title } => {
            library.rename(&from, &to, &title).map_err(internal)?;
            sync_revision(state, library);
            Ok(CommandOk::Unit)
        }
        LibraryCommand::Delete { paths, permanent } => {
            library.delete(&paths, permanent).map_err(internal)?;
            sync_revision(state, library);
            Ok(CommandOk::Unit)
        }
        LibraryCommand::Cleanup { older_days } => {
            let handle = library.clone();
            let report = tokio::task::spawn_blocking(move || handle.cleanup(older_days))
                .await
                .map_err(|error| CommandError {
                    code: ErrorCode::Internal,
                    message: error.to_string(),
                })?
                .map_err(internal)?;
            sync_revision(state, library);
            Ok(CommandOk::Cleanup(report))
        }
    }
}

fn sync_revision(state: &StateStore, library: &LibraryHandle) {
    state.update(|current| current.library_revision = library.revision());
}

fn internal(error: anyhow::Error) -> CommandError {
    CommandError {
        code: ErrorCode::Internal,
        message: error.to_string(),
    }
}

fn unavailable() -> CommandError {
    CommandError {
        code: ErrorCode::Unavailable,
        message: "not wired yet".into(),
    }
}

async fn send<W>(writer: &mut W, message: &ServerMessage) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    match tokio::time::timeout(WRITE_TIMEOUT, write_frame(writer, message)).await {
        Ok(result) => result,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "message write timed out",
        )),
    }
}

fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
    )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::presence::{PresenceDeps, PresenceOptions};
    use crate::services::test_state;
    use mediaa2_core::settings::{Settings, ThemePreference};
    use tokio::io::AsyncRead;

    async fn next_message<R>(reader: &mut R) -> ServerMessage
    where
        R: AsyncRead + Unpin,
    {
        read_frame(reader).await.expect("server message")
    }

    fn mailbox() -> (UiMailbox, mpsc::UnboundedReceiver<UiSignal>) {
        UiMailbox::new()
    }

    #[test]
    fn ui_mailbox_broadcasts_requests_and_forwards_signals() {
        let (mailbox, mut signals) = mailbox();
        let mut requests = mailbox.subscribe();
        mailbox.send(UiRequest::ShowWindow);
        assert_eq!(requests.try_recv(), Ok(UiRequest::ShowWindow));
        mailbox.connected(UiStartup::Tray);
        mailbox.event(UiEvent::Ready);
        mailbox.disconnected();
        assert_eq!(signals.try_recv(), Ok(UiSignal::Connected(UiStartup::Tray)));
        assert_eq!(signals.try_recv(), Ok(UiSignal::Event(UiEvent::Ready)));
        assert_eq!(signals.try_recv(), Ok(UiSignal::Disconnected));
        assert!(signals.try_recv().is_err());
    }

    #[test]
    fn a_request_without_a_subscriber_is_dropped() {
        let (mailbox, _signals) = mailbox();
        mailbox.send(UiRequest::Quit);
    }

    #[tokio::test]
    async fn a_ui_connection_applies_settings_and_quits() {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateStore::new(test_state()));
        let settings_path = dir.path().join("settings.toml");
        let settings = SettingsHandle::new(Arc::clone(&state), settings_path.clone());
        let library = LibraryHandle::open(dir.path().join("media")).unwrap();
        let notices = crate::services::NoticeRing::new();
        let recording = RecordingHandle::new(Arc::clone(&state), library.clone(), notices.clone());
        let conversion =
            ConversionHandle::new(Arc::clone(&state), library.clone(), notices.clone());
        let screenshots =
            ScreenshotHandle::new(Arc::clone(&state), library.clone(), notices.clone());
        let previews = PreviewHandle::new();
        let shutdown = Shutdown::new();
        let (mailbox, signals) = mailbox();
        let presence = PresenceHandle::new(PresenceDeps {
            state: Arc::clone(&state),
            mailbox: mailbox.clone(),
            signals,
            screenshots: screenshots.clone(),
            recording: recording.clone(),
            conversion: conversion.clone(),
            notices,
            shutdown: shutdown.clone(),
            options: PresenceOptions::disabled(),
        });
        let client_mailbox = mailbox.clone();
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let server_state = Arc::clone(&state);
        let server_shutdown = shutdown.clone();
        let task = tokio::spawn(async move {
            handle_connection(
                server,
                server_state,
                settings,
                library,
                recording,
                conversion,
                screenshots,
                previews,
                presence,
                mailbox,
                server_shutdown,
                UiGate::new(),
            )
            .await
        });

        write_frame(
            &mut client,
            &ClientMessage::Hello {
                protocol: PROTOCOL,
                role: Role::Ui,
                startup: None,
            },
        )
        .await
        .unwrap();
        let ServerMessage::State(initial) = next_message(&mut client).await else {
            panic!("expected the initial state");
        };
        assert_eq!(initial.library_revision, 0);

        client_mailbox.send(UiRequest::HideWindow);
        let ServerMessage::UiRequest(request) = next_message(&mut client).await else {
            panic!("expected a ui request");
        };
        assert_eq!(request, UiRequest::HideWindow);

        let mut next = initial.settings.clone();
        next.theme = ThemePreference::Dark;
        next.record.fps = 30;
        write_frame(
            &mut client,
            &ClientMessage::Command {
                id: 7,
                command: ClientCommand::SetSettings(next),
            },
        )
        .await
        .unwrap();
        let ServerMessage::Result { id, result } = next_message(&mut client).await else {
            panic!("expected a settings result");
        };
        assert_eq!(id, 7);
        assert_eq!(result, Ok(CommandOk::Unit));
        let ServerMessage::State(applied) = next_message(&mut client).await else {
            panic!("expected the applied state");
        };
        assert_eq!(applied.settings.theme, ThemePreference::Dark);
        assert_eq!(applied.settings.record.fps, 30);

        write_frame(
            &mut client,
            &ClientMessage::Command {
                id: 8,
                command: ClientCommand::Quit,
            },
        )
        .await
        .unwrap();
        let ServerMessage::Result { id, result } = next_message(&mut client).await else {
            panic!("expected a quit result");
        };
        assert_eq!(id, 8);
        assert_eq!(result, Ok(CommandOk::Unit));

        task.await.unwrap().unwrap();
        assert!(shutdown.is_cancelled());
        assert_eq!(
            Settings::load_from(&settings_path).theme,
            ThemePreference::Dark
        );
    }
}
