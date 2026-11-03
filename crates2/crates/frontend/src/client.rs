use eframe::egui;
use mediaa2_protocol::wire::{read_frame, write_frame};
use mediaa2_protocol::{
    ClientCommand, ClientMessage, CommandError, CommandOk, PreviewReady, Role, ServerMessage,
    UiEvent, UiRequest, UiStartup, PROTOCOL,
};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio::net::windows::named_pipe::ClientOptions;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

#[allow(
    clippy::large_enum_variant,
    reason = "client events are short-lived and boxing the state would complicate matching"
)]
pub enum ClientEvent {
    State(mediaa2_protocol::State),
    Result {
        id: u64,
        result: Result<CommandOk, CommandError>,
    },
    UiRequest(UiRequest),
    Preview(PreviewReady),
    Reject(String),
    Closed(String),
}

pub struct Client {
    commands: UnboundedSender<ClientCommand>,
    ui_events: UnboundedSender<UiEvent>,
    events: crossbeam_channel::Receiver<ClientEvent>,
    wake: Arc<OnceLock<egui::Context>>,
}

impl Client {
    pub fn connect(scope: &str, startup: UiStartup) -> anyhow::Result<Self> {
        let name = mediaa2_platform::ipc::socket_name(scope);
        let (commands_tx, commands_rx) = tokio::sync::mpsc::unbounded_channel();
        let (ui_tx, ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let (events_tx, events_rx) = crossbeam_channel::unbounded();
        let thread_events = events_tx.clone();
        let wake = Arc::new(OnceLock::new());
        let thread_wake = Arc::clone(&wake);
        std::thread::Builder::new()
            .name("mediaa2-ipc-client".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = thread_events.send(ClientEvent::Closed(error.to_string()));
                        repaint(&thread_wake);
                        return;
                    }
                };
                runtime.block_on(run(
                    name,
                    startup,
                    commands_rx,
                    ui_rx,
                    events_tx,
                    thread_wake,
                ));
            })?;
        Ok(Self {
            commands: commands_tx,
            ui_events: ui_tx,
            events: events_rx,
            wake,
        })
    }

    pub fn set_context(&self, ctx: egui::Context) {
        let _ = self.wake.set(ctx);
    }

    pub fn send(&self, command: ClientCommand) -> bool {
        self.commands.send(command).is_ok()
    }

    pub fn command_sender(&self) -> UnboundedSender<ClientCommand> {
        self.commands.clone()
    }

    pub fn send_ui(&self, event: UiEvent) -> bool {
        self.ui_events.send(event).is_ok()
    }

    pub fn drain(&self) -> Vec<ClientEvent> {
        self.events.try_iter().collect()
    }
}

fn repaint(wake: &OnceLock<egui::Context>) {
    if let Some(ctx) = wake.get() {
        ctx.request_repaint();
    }
}

fn spawn_reader<R>(
    mut reader: R,
) -> tokio::sync::mpsc::UnboundedReceiver<std::io::Result<ServerMessage>>
where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let message = read_frame::<_, ServerMessage>(&mut reader).await;
            let failed = message.is_err();
            if tx.send(message).is_err() || failed {
                break;
            }
        }
    });
    rx
}

async fn run(
    name: PathBuf,
    startup: UiStartup,
    mut commands: UnboundedReceiver<ClientCommand>,
    mut ui_events: UnboundedReceiver<UiEvent>,
    events: crossbeam_channel::Sender<ClientEvent>,
    wake: Arc<OnceLock<egui::Context>>,
) {
    let pipe = match ClientOptions::new().open(&name) {
        Ok(pipe) => pipe,
        Err(error) => {
            let _ = events.send(ClientEvent::Closed(format!(
                "MediaA backend is not running: {error}"
            )));
            repaint(&wake);
            return;
        }
    };
    let (reader, mut writer) = tokio::io::split(pipe);
    if let Err(error) = write_frame(
        &mut writer,
        &ClientMessage::Hello {
            protocol: PROTOCOL,
            role: Role::Ui,
            startup: Some(startup),
        },
    )
    .await
    {
        let _ = events.send(ClientEvent::Closed(error.to_string()));
        repaint(&wake);
        return;
    }

    let mut messages_rx = spawn_reader(reader);

    let mut next_id = 1_u64;
    loop {
        tokio::select! {
            message = messages_rx.recv() => {
                let Some(message) = message else {
                    let _ = events.send(ClientEvent::Closed("ipc reader stopped".into()));
                    repaint(&wake);
                    return;
                };
                match message {
                    Ok(ServerMessage::State(state)) => {
                        if events.send(ClientEvent::State(state)).is_err() {
                            return;
                        }
                        repaint(&wake);
                    }
                    Ok(ServerMessage::Result { id, result }) => {
                        if events.send(ClientEvent::Result { id, result }).is_err() {
                            return;
                        }
                        repaint(&wake);
                    }
                    Ok(ServerMessage::UiRequest(request)) => {
                        if events.send(ClientEvent::UiRequest(request)).is_err() {
                            return;
                        }
                        repaint(&wake);
                    }
                    Ok(ServerMessage::Preview(preview)) => {
                        if events.send(ClientEvent::Preview(preview)).is_err() {
                            return;
                        }
                        repaint(&wake);
                    }
                    Ok(ServerMessage::Reject { reason }) => {
                        let _ = events.send(ClientEvent::Reject(format!("{reason:?}")));
                        repaint(&wake);
                        return;
                    }
                    Err(error) => {
                        let _ = events.send(ClientEvent::Closed(error.to_string()));
                        repaint(&wake);
                        return;
                    }
                }
            }
            command = commands.recv() => {
                let Some(command) = command else { return };
                let id = next_id;
                next_id = next_id.saturating_add(1);
                if let Err(error) = write_frame(
                    &mut writer,
                    &ClientMessage::Command { id, command },
                )
                .await
                {
                    let _ = events.send(ClientEvent::Closed(error.to_string()));
                    repaint(&wake);
                    return;
                }
            }
            event = ui_events.recv() => {
                let Some(event) = event else { return };
                if let Err(error) = write_frame(
                    &mut writer,
                    &ClientMessage::Ui(event),
                )
                .await
                {
                    let _ = events.send(ClientEvent::Closed(error.to_string()));
                    repaint(&wake);
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_reader_task_relays_back_to_back_frames() {
        let (mut peer, client) = tokio::io::duplex(4096);
        let mut messages = spawn_reader(client);
        let message = ServerMessage::Reject {
            reason: mediaa2_protocol::RejectReason::BadHandshake,
        };
        write_frame(&mut peer, &message).await.unwrap();
        write_frame(&mut peer, &message).await.unwrap();
        assert_eq!(messages.recv().await.unwrap().unwrap(), message);
        assert_eq!(messages.recv().await.unwrap().unwrap(), message);
    }
}
