use crate::features::agent::host::AgentHost;
use crate::features::library::LibraryStore;
use crate::features::recording::host::{self, BeginRequest, StageFlags};
use crate::features::recording::{RecordUi, RecordingController, ViewState};
use crate::infra::hotkeys::Hotkeys;
use crate::infra::ipc::{self, Connection, Listener};
use crate::infra::settings_file::SettingsWriter;
use crate::infra::tray::{Tray, TrayCommand};
use crossbeam_channel::{Receiver, Sender};
use mediaa_core::geometry::Rect;
use mediaa_core::jobs::Job;
use mediaa_core::notices::notice_summary;
use mediaa_core::settings::RecordProfile;
use mediaa_core::wake::Wake;
use mediaa_media::recorder::{recover_incomplete_sessions, RecoveryReport, StopAction};
use mediaa_platform::mask::ScreenMask;
use mediaa_platform::single_instance::SingleInstance;
use mediaa_protocol::{
    encode_settings, RecordTargetWire, RecordingCommandWire, StopSourceWire, SETTINGS,
};
use mediaa_sync::{Frame, Server, ServerAction};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod agent;
mod dispatch;
mod launch;
mod notices;
mod services;

pub use launch::launch;
use launch::{hidden_args, spawn_ui};
use mediaa_protocol::state::tray::{TrayEffect, TrayEvent as TrayUiEvent, TrayMachine};
use notices::{notice_level, NoticeRing};

const START_TIMEOUT: Duration = Duration::from_secs(5);
const TICK: Duration = Duration::from_millis(250);

pub fn scope() -> String {
    crate::dev_mode::mode()
        .scope
        .clone()
        .unwrap_or_else(|| "default".to_string())
}

pub struct UiSession {
    pub reader: Connection,
    pub control: Connection,
    pub tray: bool,
    pub buffered: Vec<Frame>,
    pub sync: mediaa_sync::Client,
}

enum UiEvent {
    Sync(Frame),
    Launcher(Frame),
    Capture(Connection, Frame),
    Closed,
}

enum Accepted {
    Ui(Connection),
    Launcher(Connection),
    Capture(Connection),
}

struct Daemon {
    settings: mediaa_core::Settings,
    settings_writer: SettingsWriter,
    tray: Option<Tray>,
    tray_rx: Option<Receiver<TrayCommand>>,
    hotkeys: Option<Hotkeys>,
    hotkey_rx: Option<Receiver<u32>>,
    ui: Option<Connection>,
    ui_tx: Sender<UiEvent>,
    tray_ui: TrayMachine,
    notices: NoticeRing,
    recovery: Option<Job<RecoveryReport>>,
    recording: RecordingController,
    conversion: crate::features::conversion::ConversionController,
    conversion_wake: Wake,
    clipboard: Option<crate::features::clipboard::ClipboardService>,
    recording_view: ViewState,
    recording_dirty: bool,
    recording_quit_when_idle: bool,
    recording_last_phase: Option<crate::features::recording::RecordingPhase>,
    sync: Server,
    settings_version: u64,
    pending_events: Vec<(String, Vec<u8>)>,
    recording_version: u64,
    library_version: u64,
    previews: Option<crate::features::preview::PreviewHandle>,
    last_ping: Instant,
    last_pong: Instant,
    notices_version: u64,
    recovery_version: u64,
    dev_record: Option<u64>,
    dev_record_at: Option<Instant>,
    dev_record_started: Option<Instant>,
    dev_tray: Option<String>,
    dev_tray_at: Option<Instant>,
    agent_host: Option<AgentHost>,
    agent_requests: crate::features::agent::requests::AgentRequests,
    last_agent_live: String,
    agent_event_cursor: usize,
    agent_snapshot_version: u64,
    last_agent_snapshot: String,
    library: Option<LibraryStore>,
    library_job: Option<Job<anyhow::Result<LibraryStore>>>,
    mask: Option<ScreenMask>,
    mask_deadline: Option<Instant>,
}

pub fn run() -> anyhow::Result<()> {
    let _guard = match SingleInstance::acquire()? {
        Some(guard) => guard,
        None => return Ok(()),
    };
    let mut settings = mediaa_core::Settings::load();
    mediaa_agent::secrets::load(&mut settings.agent);
    crate::i18n::apply(settings.language.as_deref());
    let wake = Wake::default();
    let mut notices = NoticeRing::new();
    let tray = match Tray::new(&wake) {
        Ok((tray, rx)) => Some((tray, rx)),
        Err(error) => {
            log::warn!("tray unavailable: {error:#}");
            notices.push(
                ipc::Notice {
                    level: ipc::NoticeLevel::Warning,
                    summary: "tray unavailable".into(),
                    detail: format!("tray unavailable: {error:#}"),
                },
                Instant::now(),
            );
            None
        }
    };
    let hotkeys = match Hotkeys::new(&wake) {
        Ok((hotkeys, rx)) => Some((hotkeys, rx)),
        Err(error) => {
            log::warn!("global hotkeys unavailable: {error:#}");
            notices.push(
                ipc::Notice {
                    level: ipc::NoticeLevel::Warning,
                    summary: "global hotkeys unavailable".into(),
                    detail: format!("global hotkeys unavailable: {error:#}"),
                },
                Instant::now(),
            );
            None
        }
    };
    let recovery = spawn_recovery(&settings);
    let library_dir = crate::features::library::library_dir(&settings);
    let library_job = Job::spawn("library-index", Arc::default(), move || {
        LibraryStore::open(&library_dir).map_err(|error| anyhow::anyhow!("{error:#}"))
    })
    .ok();
    let dev_tray = mediaa_core::dev::tray_action();
    let dev_tray_at = dev_tray
        .as_ref()
        .map(|_| Instant::now() + Duration::from_secs(2));
    let dev_record = mediaa_core::dev::record_seconds();
    let dev_record_at = dev_record.map(|_| Instant::now() + Duration::from_secs(2));
    let mask = ScreenMask::create();
    if mask.is_none() {
        log::warn!("screen mask unavailable");
    }
    let listener = Listener::bind(&scope())?;
    log::info!(
        "daemon listening on {}",
        ipc::socket_name(&scope()).display()
    );

    let (ui_tx, ui_rx) = crossbeam_channel::unbounded();
    let (accept_tx, accept_rx) = crossbeam_channel::unbounded();
    spawn_acceptor(listener, accept_tx);

    let (tray, tray_rx) = match tray {
        Some((tray, rx)) => (Some(tray), Some(rx)),
        None => (None, None),
    };
    let (hotkeys, hotkey_rx) = match hotkeys {
        Some((hotkeys, rx)) => (Some(hotkeys), Some(rx)),
        None => (None, None),
    };
    let tray_rx = tray_rx.unwrap_or_else(closed);
    let hotkey_rx = hotkey_rx.unwrap_or_else(closed);

    let mut daemon = Daemon {
        settings,
        settings_writer: SettingsWriter::create(),
        tray,
        tray_rx: Some(tray_rx),
        hotkeys,
        hotkey_rx: Some(hotkey_rx),
        ui: None,
        ui_tx,
        tray_ui: TrayMachine::default(),
        notices,
        recovery,
        recording: RecordingController::default(),
        conversion: crate::features::conversion::ConversionController::default(),
        conversion_wake: Wake::default(),
        clipboard: None,
        recording_view: ViewState::default(),
        recording_dirty: false,
        recording_quit_when_idle: false,
        recording_last_phase: None,
        sync: Server::new(),
        settings_version: 0,
        pending_events: Vec::new(),
        recording_version: 0,
        library_version: 0,
        previews: None,
        last_ping: Instant::now(),
        last_pong: Instant::now(),
        notices_version: 0,
        recovery_version: 0,
        dev_record,
        dev_record_at,
        dev_record_started: None,
        dev_tray,
        dev_tray_at,
        agent_host: None,
        agent_requests: Default::default(),
        last_agent_live: String::new(),
        agent_event_cursor: 0,
        agent_snapshot_version: 0,
        last_agent_snapshot: String::new(),
        library: None,
        library_job,
        mask,
        mask_deadline: None,
    };
    daemon.publish_settings();
    daemon.apply_hotkeys();

    loop {
        crossbeam_channel::select! {
            recv(accept_rx) -> accepted => {
                if let Ok(accepted) = accepted {
                    daemon.handle_accept(accepted);
                }
            }
            recv(ui_rx) -> event => {
                if let Ok(event) = event {
                    daemon.handle_ui_event(event);
                }
            }
            recv(daemon.tray_rx.as_ref().expect("tray receiver")) -> command => {
                if let Ok(command) = command {
                    daemon.handle_tray_command(command);
                }
            }
            recv(daemon.hotkey_rx.as_ref().expect("hotkey receiver")) -> id => {
                if let Ok(id) = id {
                    daemon.handle_hotkey(id);
                }
            }
            default(TICK) => {
                let now = Instant::now();
                daemon.notices.expire(now);
                daemon.poll_heartbeat(now);
                daemon.poll_recovery();
                daemon.poll_recording();
                daemon.poll_dev_record();
                daemon.poll_dev_tray();
                daemon.poll_tray();
                daemon.poll_library();
                daemon.poll_previews();
                daemon.poll_conversion();
                daemon.poll_agent_host();
                daemon.poll_agent_requests();
                if let Some(mask) = daemon.mask.as_ref() {
                    mask.pump();
                }
                if daemon
                    .mask_deadline
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    daemon.hide_mask();
                }
                if let Some(error) = daemon.settings_writer.take_error() {
                    log::warn!("save settings: {error}");
                    daemon.note(
                        ipc::NoticeLevel::Warning,
                        "save settings",
                        error.to_string(),
                    );
                }
            }
        }
    }
}

fn closed<T>() -> Receiver<T> {
    let (_tx, rx) = crossbeam_channel::bounded(1);
    rx
}

fn spawn_acceptor(listener: Listener, tx: Sender<Accepted>) {
    std::thread::spawn(move || loop {
        let connection = match listener.accept() {
            Ok(connection) => connection,
            Err(error) => {
                log::warn!("ipc accept: {error}");
                continue;
            }
        };
        let tx = tx.clone();
        std::thread::spawn(move || {
            let mut connection = connection;
            let hello = match connection.recv_frame() {
                Ok(Frame::Hello { version, client }) => {
                    if version != mediaa_sync::PROTOCOL_VERSION {
                        let _ = connection.send_frame(&Frame::Reject {
                            reason: format!(
                                "protocol version {version} is not {}",
                                mediaa_sync::PROTOCOL_VERSION
                            ),
                        });
                        return;
                    }
                    client
                }
                Ok(_) => {
                    let _ = connection.send_frame(&Frame::Reject {
                        reason: "expected hello".into(),
                    });
                    return;
                }
                Err(error) => {
                    log::warn!("ipc handshake: {error}");
                    return;
                }
            };
            match hello.as_str() {
                ipc::CLIENT_UI => {
                    if tx.send(Accepted::Ui(connection)).is_err() {
                        log::warn!("daemon loop is gone");
                    }
                }
                ipc::CLIENT_LAUNCHER => {
                    if tx.send(Accepted::Launcher(connection)).is_err() {
                        log::warn!("daemon loop is gone");
                    }
                }
                ipc::CLIENT_CAPTURE => {
                    if tx.send(Accepted::Capture(connection)).is_err() {
                        log::warn!("daemon loop is gone");
                    }
                }
                other => log::warn!("ipc handshake: unknown client {other}"),
            }
        });
    });
}

impl Daemon {
    fn handle_tray_command(&mut self, command: TrayCommand) {
        log::info!("tray command: {command:?}, ui={}", self.ui.is_some());
        if command == TrayCommand::ShowWindow {
            self.tray_event(TrayUiEvent::Show, Instant::now());
            return;
        }
        if command == TrayCommand::Quit {
            let immediate = self.recording_view.is_idle() && !self.conversion.is_busy();
            log::info!("tray quit; immediate={immediate}");
            self.tray_event(TrayUiEvent::Quit { immediate }, Instant::now());
            return;
        }
        if tray_starts_picker(
            command,
            self.settings.tray_left_click,
            self.settings.stop_recording_on_left_click,
        ) {
            self.show_mask();
        }
        self.forward_event(
            mediaa_protocol::TRAY_EVENT,
            mediaa_protocol::encode_tray_command(wire_tray_command(command)),
        );
    }

    fn tray_event(&mut self, event: TrayUiEvent, now: Instant) {
        let before = self.tray_ui.state();
        match self.tray_ui.on(event, now) {
            Ok(transition) => {
                if before != transition.state || !transition.effects.is_empty() {
                    log::info!(
                        "tray state: {before:?} -> {:?} ({:?})",
                        transition.state,
                        transition.effects
                    );
                }
                self.apply_tray_effects(transition.effects);
            }
            Err(rejected) => {
                log::warn!("tray transition rejected: {rejected:?}");
            }
        }
    }

    fn apply_tray_effects(&mut self, effects: Vec<TrayEffect>) {
        for effect in effects {
            match effect {
                TrayEffect::SpawnVisible => self.ensure_ui_visible(),
                TrayEffect::SendShow => self.forward_event(
                    mediaa_protocol::TRAY_EVENT,
                    mediaa_protocol::encode_tray_command(
                        mediaa_protocol::TrayCommandWire::ShowWindow,
                    ),
                ),
                TrayEffect::SendHide => {
                    log::info!("tray hide is initiated by the ui");
                }
                TrayEffect::SendQuit => self.forward_event(
                    mediaa_protocol::TRAY_EVENT,
                    mediaa_protocol::encode_tray_command(mediaa_protocol::TrayCommandWire::Quit),
                ),
                TrayEffect::RestartVisible => {
                    self.ui = None;
                    self.hide_mask();
                    self.ensure_ui_visible();
                }
                TrayEffect::FinishQuit => {
                    log::info!("tray quit; stopping the daemon");
                    self.quit();
                }
            }
        }
    }

    fn handle_hotkey(&mut self, id: u32) {
        let actions = crate::features::hotkeys::ordered_actions(
            self.hotkeys
                .as_ref()
                .map(|hotkeys| hotkeys.actions_for(id))
                .unwrap_or_default(),
        );
        if actions.is_empty() {
            return;
        }
        if actions.contains(&mediaa_core::settings::HotkeyAction::RegionScreenshot) {
            self.show_mask();
        }
        self.forward_event(
            mediaa_protocol::HOTKEYS_EVENT,
            mediaa_protocol::encode_hotkeys(&actions),
        );
    }

    fn show_mask(&mut self) {
        if crate::dev_mode::mode().background {
            return;
        }
        if let Some(mask) = self.mask.as_ref() {
            mask.pump();
            mask.show();
            self.mask_deadline = Some(Instant::now() + Duration::from_secs(6));
        }
    }

    fn hide_mask(&mut self) {
        if let Some(mask) = self.mask.as_ref() {
            mask.hide();
        }
        self.mask_deadline = None;
    }

    fn apply_settings(&mut self, mut settings: mediaa_core::Settings) {
        let mut secrets_changed = false;
        for connection in &mut settings.agent.connections {
            let Some(old) = self.settings.agent.connection(&connection.id) else {
                secrets_changed |= !connection.api_key.is_empty();
                continue;
            };
            if connection.api_key.is_empty() {
                connection.api_key = old.api_key.clone();
            } else if connection.api_key != old.api_key {
                secrets_changed = true;
            }
        }
        if settings.agent.web.api_key.is_empty() {
            settings.agent.web.api_key = self.settings.agent.web.api_key.clone();
        } else if settings.agent.web.api_key != self.settings.agent.web.api_key {
            secrets_changed = true;
        }
        self.settings = settings;
        crate::i18n::apply(self.settings.language.as_deref());
        if let Some(tray) = self.tray.as_mut() {
            tray.refresh();
        }
        if secrets_changed {
            if let Err(error) = mediaa_agent::secrets::save(&self.settings.agent) {
                log::warn!("save agent secrets: {error:#}");
            }
        }
        self.publish_settings();
        self.apply_hotkeys();
        match self.settings_writer.request(self.settings.clone()) {
            Ok(generation) => log::debug!("settings queued for save: {generation}"),
            Err(error) => log::warn!("save settings: {error:#}"),
        }
    }
    fn publish_notices(&mut self) {
        let notices: Vec<mediaa_protocol::NoticeWire> = self
            .notices
            .entries()
            .map(|(id, notice)| mediaa_protocol::NoticeWire {
                id,
                level: match notice.level {
                    ipc::NoticeLevel::Info => mediaa_protocol::NoticeLevelWire::Info,
                    ipc::NoticeLevel::Warning => mediaa_protocol::NoticeLevelWire::Warning,
                    ipc::NoticeLevel::Error => mediaa_protocol::NoticeLevelWire::Error,
                },
                summary: notice.summary.clone(),
                detail: notice.detail.clone(),
            })
            .collect();
        self.notices_version += 1;
        let payload = mediaa_protocol::encode_notices(&notices);
        self.sync
            .hub_mut()
            .publish(mediaa_protocol::NOTICES, self.notices_version, payload);
        self.flush_sync();
    }

    fn publish_recovery(&mut self) {
        self.recovery_version += 1;
        let state = mediaa_protocol::RecoveryStateWire {
            running: self.recovery.is_some(),
        };
        let payload = mediaa_protocol::encode_recovery_state(&state);
        self.sync
            .hub_mut()
            .publish(mediaa_protocol::RECOVERY, self.recovery_version, payload);
        self.flush_sync();
    }

    fn push_recovery_report(&mut self, report: RecoveryReport) {
        let report = mediaa_protocol::RecoveryReportWire {
            files: report
                .files
                .iter()
                .map(|path| path.display().to_string())
                .collect(),
            failures: report.failures,
        };
        let payload = mediaa_protocol::encode_recovery_report(&report);
        self.sync
            .events_mut()
            .push(mediaa_protocol::RECOVERY_EVENT, payload);
        self.flush_sync();
    }
    fn publish_settings(&mut self) {
        self.settings_version += 1;
        let payload = encode_settings(&self.settings);
        self.sync
            .hub_mut()
            .publish(SETTINGS, self.settings_version, payload);
    }

    fn poll_heartbeat(&mut self, now: Instant) {
        if self.ui.is_none() {
            return;
        }
        if now.duration_since(self.last_pong) > Duration::from_secs(15) {
            log::warn!("ui heartbeat timed out; dropping the ui");
            self.ui = None;
            self.hide_mask();
            return;
        }
        if now.duration_since(self.last_ping) >= Duration::from_secs(5) {
            self.last_ping = now;
            if let Some(ui) = self.ui.as_mut() {
                if let Err(error) = ui.send_frame(&Frame::Ping) {
                    log::warn!("ui heartbeat: {error}");
                    self.ui = None;
                }
            }
        }
    }
    fn note(
        &mut self,
        level: ipc::NoticeLevel,
        summary: impl Into<String>,
        detail: impl Into<String>,
    ) {
        let notice = ipc::Notice {
            level,
            summary: summary.into(),
            detail: detail.into(),
        };
        self.notices.push(notice, Instant::now());
        self.publish_notices();
    }

    fn quit(&mut self) -> ! {
        self.hide_mask();
        log::info!("daemon quit: flushing settings");
        if let Err(error) = self
            .settings_writer
            .flush(self.settings.clone(), Duration::from_secs(2))
        {
            log::error!("saving settings on exit: {error:#}");
        }
        log::info!("daemon quit: settings flushed");
        std::process::exit(0);
    }

    fn apply_hotkeys(&mut self) {
        let Some(hotkeys) = self.hotkeys.as_mut() else {
            return;
        };
        let (groups, mut problems) =
            crate::features::hotkeys::grouped_actions(&self.settings.hotkeys);
        problems.append(&mut hotkeys.apply(groups));
        for problem in problems {
            log::warn!("hotkey: {problem}");
            self.note(ipc::NoticeLevel::Warning, "hotkey", problem);
        }
    }
}

fn recording_state_due(
    dirty: bool,
    phase: crate::features::recording::RecordingPhase,
    last: Option<crate::features::recording::RecordingPhase>,
) -> bool {
    use crate::features::recording::RecordingPhase;
    dirty || last != Some(phase) || !matches!(phase, RecordingPhase::Idle)
}

fn recovery_candidate(work: &Path) -> bool {
    std::fs::read_dir(work).is_ok_and(|entries| {
        entries
            .flatten()
            .any(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
    })
}

fn spawn_recovery(settings: &mediaa_core::Settings) -> Option<Job<RecoveryReport>> {
    let work = mediaa_core::paths::work_dir();
    if !recovery_candidate(&work) {
        return None;
    }
    let media = crate::features::library::library_dir(settings);
    Job::spawn("recording-recovery", Arc::default(), move || {
        recover_incomplete_sessions(&work, &media)
    })
    .ok()
}

#[allow(dead_code, reason = "used by the UI start handshake")]
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
fn wire_tray_command(command: TrayCommand) -> mediaa_protocol::TrayCommandWire {
    match command {
        TrayCommand::PrimaryClick => mediaa_protocol::TrayCommandWire::PrimaryClick,
        TrayCommand::ShowWindow => mediaa_protocol::TrayCommandWire::ShowWindow,
        TrayCommand::RegionScreenshot => mediaa_protocol::TrayCommandWire::RegionScreenshot,
        TrayCommand::FullscreenScreenshot => mediaa_protocol::TrayCommandWire::FullscreenScreenshot,
        TrayCommand::StartRecording => mediaa_protocol::TrayCommandWire::StartRecording,
        TrayCommand::StartAudio => mediaa_protocol::TrayCommandWire::StartAudio,
        TrayCommand::PauseResume => mediaa_protocol::TrayCommandWire::PauseResume,
        TrayCommand::StopAndSave => mediaa_protocol::TrayCommandWire::StopAndSave,
        TrayCommand::Quit => mediaa_protocol::TrayCommandWire::Quit,
    }
}

fn tray_starts_picker(
    command: TrayCommand,
    left_click: mediaa_core::settings::TrayClickAction,
    stop_on_left_click: bool,
) -> bool {
    match command {
        TrayCommand::RegionScreenshot => true,
        TrayCommand::PrimaryClick => {
            left_click == mediaa_core::settings::TrayClickAction::ScreenArea && !stop_on_left_click
        }
        _ => false,
    }
}
