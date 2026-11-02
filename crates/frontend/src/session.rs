pub fn scope() -> String {
    crate::dev_mode::mode()
        .scope
        .clone()
        .unwrap_or_else(|| "default".to_string())
}
use crate::infra::ipc::{self, Connection};
use mediaa_sync::Frame;
pub(crate) struct UiSession {
    pub(crate) reader: Connection,
    pub(crate) control: Connection,
    pub(crate) tray: bool,
    pub(crate) buffered: Vec<Frame>,
    pub(crate) sync: mediaa_sync::Client,
}

fn request_settings(
    connection: &mut Connection,
    mut client: mediaa_sync::Client,
) -> (
    Option<mediaa_core::Settings>,
    mediaa_sync::Client,
    Vec<Frame>,
) {
    let subscribe = client.subscribe(vec![
        mediaa_protocol::SETTINGS.to_string(),
        mediaa_protocol::RECORDING.to_string(),
        mediaa_protocol::LIBRARY.to_string(),
        mediaa_protocol::NOTICES.to_string(),
        mediaa_protocol::RECOVERY.to_string(),
    ]);
    if let Err(error) = connection.send_frame(&subscribe) {
        log::warn!("sync subscribe: {error}");
        return (None, client, Vec::new());
    }
    let mut buffered = Vec::new();
    loop {
        match connection.recv_frame() {
            Ok(frame) => {
                if let Frame::Snapshot { store, payload, .. } = &frame {
                    if store == mediaa_protocol::SETTINGS {
                        match mediaa_protocol::decode_settings(payload) {
                            Ok(settings) => return (Some(settings), client, buffered),
                            Err(error) => log::warn!("settings payload: {error}"),
                        }
                    }
                }
                buffered.push(frame);
            }
            Err(error) => {
                log::warn!("settings handshake: {error}");
                return (None, client, buffered);
            }
        }
    }
}
pub fn run_ui(args: &[String]) -> anyhow::Result<()> {
    let mut connection = Connection::connect(&scope())
        .map_err(|error| anyhow::anyhow!("MediaA daemon is not running: {error}"))?;
    connection.send_frame(&Frame::Hello {
        version: mediaa_sync::PROTOCOL_VERSION,
        client: ipc::CLIENT_UI.into(),
    })?;
    let mut sync = mediaa_sync::Client::new();
    let frame = connection.recv_frame()?;
    let tray = match &frame {
        Frame::Welcome { metadata, .. } => {
            metadata.get("tray").is_some_and(|value| value == "true")
        }
        Frame::Reject { reason } => anyhow::bail!("{reason}"),
        other => anyhow::bail!("unexpected handshake: {other:?}"),
    };
    sync.on_frame(frame);
    let (settings, sync, buffered) = request_settings(&mut connection, sync);
    let control = connection.try_clone()?;
    let startup = crate::app::StartupAction::from_args(args.to_vec());
    let picker_mode = matches!(
        startup,
        crate::app::StartupAction::Area | crate::app::StartupAction::Fullscreen
    );
    if !picker_mode {
        mediaa_media::init();
    }
    let page = crate::app::page_from_args(args);
    let background = crate::dev_mode::mode().background;
    let start_in_tray = startup != crate::app::StartupAction::Show
        && (picker_mode
            || background
            || startup == crate::app::StartupAction::Tray
            || mediaa_core::Settings::load().start_in_tray);
    let mut viewport = eframe::egui::ViewportBuilder::default();
    if background {
        viewport = viewport.with_position([-32000.0, -32000.0]);
    }
    if background {
        crate::dev_mode::remember_foreground(mediaa_platform::window::foreground_window());
    }
    let window_icon = if crate::dev_mode::mode().agent {
        crate::icon::rgba_agent(64)
    } else {
        crate::icon::rgba(64)
    };
    let options = eframe::NativeOptions {
        viewport: viewport
            .with_title(mediaa_core::paths::APP_IDENTITY.window_title)
            .with_app_id(mediaa_core::paths::APP_IDENTITY.application_id)
            .with_transparent(true)
            .with_visible(!start_in_tray)
            .with_inner_size([1120.0, 760.0])
            .with_min_inner_size([480.0, 480.0])
            .with_icon(std::sync::Arc::new(eframe::egui::IconData {
                rgba: window_icon,
                width: 64,
                height: 64,
            })),
        ..Default::default()
    };
    eframe::run_native(
        mediaa_core::paths::APP_IDENTITY.window_title,
        options,
        Box::new(move |cc| {
            Ok(Box::new(crate::app::App::new(
                cc,
                startup,
                page,
                None,
                settings,
                Some(UiSession {
                    reader: connection,
                    control,
                    tray,
                    buffered,
                    sync,
                }),
            )))
        }),
    )
    .map_err(|error| anyhow::anyhow!("ui: {error}"))
}
