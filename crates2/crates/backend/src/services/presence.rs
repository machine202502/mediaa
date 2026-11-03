use crate::domain::presence::{Presence, PresenceEffect, PresenceEvent};
use crate::ipc::{Shutdown, UiMailbox, UiSignal};
use crate::services::{ConversionHandle, NoticeRing, RecordingHandle, ScreenshotHandle};
use crate::state::StateStore;
use mediaa2_core::notices::NoticeLevel;
use mediaa2_core::recording::{RecordTarget, StopAction, StopSource};
use mediaa2_core::screen::MonitorInfo;
use mediaa2_core::settings::{
    AfterCapture, HotkeyAction, HotkeySettings, RecordProfile, Settings, TrayClickAction,
};
use mediaa2_i18n::t;
use mediaa2_platform::hotkeys::Hotkeys;
use mediaa2_platform::startup::{self, AutoStart};
use mediaa2_platform::tray::{Tray, TrayCommand, TrayLabels, TrayState};
use mediaa2_platform::tray_icon::icon;
use mediaa2_platform::{cursor, screen};
use mediaa2_protocol::state::RecordingState;
use mediaa2_protocol::{CommandOk, UiEvent, UiStartup};
use std::ffi::OsString;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};

const COMMAND_CAPACITY: usize = 32;
const TICK: Duration = Duration::from_millis(250);

pub type Launch = Arc<dyn Fn(&[OsString]) -> io::Result<()> + Send + Sync>;

#[derive(Clone)]
pub struct PresenceOptions {
    pub tray: bool,
    pub hotkeys: bool,
    pub launch: Launch,
}

impl Default for PresenceOptions {
    fn default() -> Self {
        Self {
            tray: true,
            hotkeys: true,
            launch: Arc::new(|args| {
                mediaa2_platform::process::spawn_detached(args).map(|_child| ())
            }),
        }
    }
}

impl PresenceOptions {
    pub fn disabled() -> Self {
        Self {
            tray: false,
            hotkeys: false,
            launch: Arc::new(|_args| Ok(())),
        }
    }
}

pub struct PresenceDeps {
    pub state: Arc<StateStore>,
    pub mailbox: UiMailbox,
    pub signals: mpsc::UnboundedReceiver<UiSignal>,
    pub screenshots: ScreenshotHandle,
    pub recording: RecordingHandle,
    pub conversion: ConversionHandle,
    pub notices: NoticeRing,
    pub shutdown: Shutdown,
    pub options: PresenceOptions,
}

#[derive(Clone)]
pub struct PresenceHandle {
    tx: mpsc::Sender<PresenceCommand>,
}

impl PresenceHandle {
    pub fn new(deps: PresenceDeps) -> Self {
        let (tx, commands) = mpsc::channel(COMMAND_CAPACITY);
        let (platform_tx, platform_rx) = mpsc::unbounded_channel();
        let PresenceDeps {
            state,
            mailbox,
            signals,
            screenshots,
            recording,
            conversion,
            notices,
            shutdown,
            options,
        } = deps;
        let actor = Actor {
            state: Arc::clone(&state),
            presence: Presence::new(),
            tray: None,
            hotkeys: None,
            mailbox,
            screenshots,
            recording,
            conversion,
            notices,
            shutdown,
            options,
            platform_tx,
            state_rx: state.subscribe(),
            epoch: Instant::now(),
            last_tray_view: None,
            last_hotkeys: None,
            last_autostart: None,
        };
        tokio::spawn(actor.run(commands, signals, platform_rx));
        Self { tx }
    }

    pub fn open(&self) {
        self.send(PresenceCommand::Open);
    }

    pub fn quit(&self) {
        self.send(PresenceCommand::Quit);
    }

    pub fn ui_event(&self, event: UiEvent) {
        self.send(PresenceCommand::Ui(event));
    }

    pub fn ui_connected(&self, startup: UiStartup) {
        self.send(PresenceCommand::Connected(startup));
    }

    pub fn ui_disconnected(&self) {
        self.send(PresenceCommand::Disconnected);
    }

    pub fn picker_done(&self) {
        self.send(PresenceCommand::PickerDone);
    }

    fn send(&self, command: PresenceCommand) {
        if self.tx.try_send(command).is_err() {
            log::warn!("presence: command queue is full");
        }
    }
}

enum PresenceCommand {
    Open,
    Quit,
    Ui(UiEvent),
    Connected(UiStartup),
    Disconnected,
    PickerDone,
}

enum PlatformEvent {
    Tray(TrayCommand),
    Hotkey(u32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrayView {
    pub state: TrayState,
    pub progress: Option<f32>,
    pub progress_enabled: bool,
}

pub fn tray_state_for(recording: &RecordingState, settings: &Settings) -> TrayView {
    let (state, progress) = match recording {
        RecordingState::Idle => (TrayState::Idle, None),
        RecordingState::Starting { .. } => (TrayState::Starting, None),
        RecordingState::Active { paused, .. } => (
            if *paused {
                TrayState::Paused
            } else {
                TrayState::Recording
            },
            None,
        ),
        RecordingState::Finalizing { progress, .. } => (TrayState::Saving, *progress),
    };
    TrayView {
        state,
        progress,
        progress_enabled: settings.tray_progress,
    }
}

fn hotkey_groups(settings: &HotkeySettings) -> Vec<(String, Vec<HotkeyAction>)> {
    if !settings.enabled {
        return Vec::new();
    }
    let mut groups: Vec<(String, Vec<HotkeyAction>)> = Vec::new();
    for (text, action) in [
        (&settings.region_screenshot, HotkeyAction::RegionScreenshot),
        (
            &settings.fullscreen_screenshot,
            HotkeyAction::FullscreenScreenshot,
        ),
        (&settings.start_video, HotkeyAction::StartVideo),
        (&settings.start_audio, HotkeyAction::StartAudio),
        (&settings.stop_recording, HotkeyAction::StopRecording),
    ] {
        let text = text.trim();
        if text.is_empty() {
            continue;
        }
        if let Some(group) = groups
            .iter_mut()
            .find(|(existing, _)| existing.eq_ignore_ascii_case(text))
        {
            group.1.push(action);
        } else {
            groups.push((text.to_string(), vec![action]));
        }
    }
    for (_, actions) in &mut groups {
        actions.sort_by_key(|action| u8::from(*action != HotkeyAction::StopRecording));
    }
    groups
}

fn autostart_mode(settings: &Settings) -> AutoStart {
    match (settings.launch_at_login, settings.start_in_tray) {
        (false, _) => AutoStart::Disabled,
        (true, false) => AutoStart::Window,
        (true, true) => AutoStart::Tray,
    }
}

fn tray_labels() -> TrayLabels {
    TrayLabels {
        show_window: t!("tray.show").to_string(),
        region: t!("tray.region").to_string(),
        fullscreen: t!("tray.fullscreen").to_string(),
        start_video: t!("tray.start").to_string(),
        start_audio: t!("tray.audio").to_string(),
        pause: t!("tray.pause").to_string(),
        resume: t!("tray.resume").to_string(),
        stop: t!("tray.stop").to_string(),
        quit: t!("tray.quit").to_string(),
    }
}

fn ui_args(startup: &UiStartup) -> Vec<OsString> {
    let mut args = vec![OsString::from("--ui")];
    match startup {
        UiStartup::Show => args.push(OsString::from("--show")),
        UiStartup::Tray => args.push(OsString::from("--tray")),
        UiStartup::Picker { session } => {
            args.push(OsString::from("--picker"));
            args.push(OsString::from(session.to_string()));
        }
    }
    args
}

fn monitor_under_cursor() -> Option<MonitorInfo> {
    let monitors = screen::monitors().ok()?;
    if monitors.is_empty() {
        return None;
    }
    cursor::position()
        .and_then(|(x, y)| screen::monitor_at(x, y))
        .and_then(|id| monitors.iter().find(|monitor| monitor.id == id).cloned())
        .or_else(|| monitors.into_iter().next())
}

struct Actor {
    state: Arc<StateStore>,
    presence: Presence,
    tray: Option<Tray>,
    hotkeys: Option<Hotkeys>,
    mailbox: UiMailbox,
    screenshots: ScreenshotHandle,
    recording: RecordingHandle,
    conversion: ConversionHandle,
    notices: NoticeRing,
    shutdown: Shutdown,
    options: PresenceOptions,
    platform_tx: mpsc::UnboundedSender<PlatformEvent>,
    state_rx: watch::Receiver<Arc<mediaa2_protocol::State>>,
    epoch: Instant,
    last_tray_view: Option<TrayView>,
    last_hotkeys: Option<HotkeySettings>,
    last_autostart: Option<AutoStart>,
}

impl Actor {
    async fn run(
        mut self,
        mut commands: mpsc::Receiver<PresenceCommand>,
        mut signals: mpsc::UnboundedReceiver<UiSignal>,
        mut platform: mpsc::UnboundedReceiver<PlatformEvent>,
    ) {
        self.install().await;
        let initial = self.state.snapshot();
        self.sync_tray(&initial);
        self.sync_settings(&initial.settings);
        let mut ticker = tokio::time::interval(TICK);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                command = commands.recv() => {
                    let Some(command) = command else { break };
                    self.handle(command).await;
                }
                signal = signals.recv() => {
                    let Some(signal) = signal else { break };
                    self.handle_signal(signal).await;
                }
                event = platform.recv() => {
                    let Some(event) = event else { break };
                    self.handle_platform(event).await;
                }
                changed = self.state_rx.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let state = self.state.snapshot();
                    self.sync_tray(&state);
                    self.sync_settings(&state.settings);
                }
                _ = ticker.tick() => self.on_tick().await,
            }
        }
        self.tray = None;
        self.hotkeys = None;
    }

    async fn install(&mut self) {
        if self.options.tray {
            let labels = tray_labels();
            let agent = std::env::var("MEDIAA2_DEV_AGENT").is_ok_and(|value| value == "1");
            let base = icon(agent, 1);
            match tokio::task::spawn_blocking(move || Tray::new(labels, base)).await {
                Ok(Ok((tray, events))) => {
                    self.tray = Some(tray);
                    self.bridge_tray(events);
                }
                Ok(Err(error)) => self.notice("Tray is unavailable", error.to_string()),
                Err(error) => {
                    self.notice("Tray is unavailable", format!("tray task failed: {error}"))
                }
            }
        }
        if self.options.hotkeys {
            match tokio::task::spawn_blocking(Hotkeys::new).await {
                Ok(Ok((hotkeys, events))) => {
                    self.hotkeys = Some(hotkeys);
                    self.bridge_hotkeys(events);
                }
                Ok(Err(error)) => self.notice("Global hotkeys are unavailable", error.to_string()),
                Err(error) => self.notice(
                    "Global hotkeys are unavailable",
                    format!("hotkey task failed: {error}"),
                ),
            }
        }
    }

    fn bridge_tray(&self, events: crossbeam_channel::Receiver<TrayCommand>) {
        let tx = self.platform_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("mediaa2-tray-bridge".into())
            .spawn(move || {
                for command in events {
                    if tx.send(PlatformEvent::Tray(command)).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            self.notice("Tray is unavailable", error.to_string());
        }
    }

    fn bridge_hotkeys(&self, events: crossbeam_channel::Receiver<u32>) {
        let tx = self.platform_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("mediaa2-hotkey-bridge".into())
            .spawn(move || {
                for id in events {
                    if tx.send(PlatformEvent::Hotkey(id)).is_err() {
                        break;
                    }
                }
            });
        if let Err(error) = spawned {
            self.notice("Global hotkeys are unavailable", error.to_string());
        }
    }

    fn sync_tray(&mut self, state: &mediaa2_protocol::State) {
        let view = tray_state_for(&state.recording, &state.settings);
        if self.last_tray_view == Some(view) {
            return;
        }
        self.last_tray_view = Some(view);
        if let Some(tray) = &self.tray {
            tray.set_progress_enabled(view.progress_enabled);
            tray.set_state(view.state, view.progress);
        }
    }

    fn sync_settings(&mut self, settings: &Settings) {
        let mode = autostart_mode(settings);
        if self.last_autostart != Some(mode) {
            self.last_autostart = Some(mode);
            if let Err(error) = startup::configure(mode) {
                self.notice("Startup is unavailable", format!("{error:#}"));
            }
        }
        if self.last_hotkeys.as_ref() != Some(&settings.hotkeys) {
            self.last_hotkeys = Some(settings.hotkeys.clone());
            let groups = hotkey_groups(&settings.hotkeys);
            if let Some(hotkeys) = self.hotkeys.as_mut() {
                for problem in hotkeys.apply(groups) {
                    self.notice("Global hotkeys are unavailable", problem);
                }
            }
        }
    }

    async fn handle(&mut self, command: PresenceCommand) {
        match command {
            PresenceCommand::Open => self.apply(PresenceEvent::OpenRequested).await,
            PresenceCommand::Quit => self.apply(PresenceEvent::Quit { immediate: false }).await,
            PresenceCommand::Ui(event) => self.on_ui_event(event).await,
            PresenceCommand::Connected(startup) => self.on_connected(startup).await,
            PresenceCommand::Disconnected => self.apply(PresenceEvent::UiGone).await,
            PresenceCommand::PickerDone => self.apply(PresenceEvent::PickerDone).await,
        }
    }

    async fn handle_signal(&mut self, signal: UiSignal) {
        match signal {
            UiSignal::Connected(startup) => self.on_connected(startup).await,
            UiSignal::Event(event) => self.on_ui_event(event).await,
            UiSignal::Disconnected => self.apply(PresenceEvent::UiGone).await,
        }
    }

    async fn handle_platform(&mut self, event: PlatformEvent) {
        match event {
            PlatformEvent::Tray(command) => self.on_tray(command).await,
            PlatformEvent::Hotkey(id) => self.on_hotkey(id).await,
        }
    }

    async fn on_tick(&mut self) {
        let now = self.now_ms();
        self.apply(PresenceEvent::Tick { now_ms: now }).await;
    }

    async fn on_ui_event(&mut self, event: UiEvent) {
        let event = match event {
            UiEvent::Ready => PresenceEvent::UiReady,
            UiEvent::WindowShown => PresenceEvent::WindowShown,
            UiEvent::WindowHidden | UiEvent::Closing => PresenceEvent::WindowHidden,
        };
        self.apply(event).await;
    }

    async fn on_connected(&mut self, startup: UiStartup) {
        let startup = self.resolve_startup(startup);
        if matches!(self.presence, Presence::NoUi) {
            let now = self.now_ms();
            let _ = self.presence.apply(PresenceEvent::OpenRequested, now);
        }
        self.apply(PresenceEvent::UiConnected { startup }).await;
    }

    fn resolve_startup(&self, startup: UiStartup) -> UiStartup {
        match startup {
            UiStartup::Picker { .. } => match self.state.snapshot().picker.as_ref() {
                Some(picker) => UiStartup::Picker {
                    session: picker.session,
                },
                None => startup,
            },
            other => other,
        }
    }

    async fn on_tray(&mut self, command: TrayCommand) {
        match command {
            TrayCommand::ShowWindow => self.apply(PresenceEvent::OpenRequested).await,
            TrayCommand::RegionScreenshot => self.start_picker().await,
            TrayCommand::FullscreenScreenshot => self.fullscreen().await,
            TrayCommand::StartRecording => self.start_video().await,
            TrayCommand::StartAudio => self.start_audio().await,
            TrayCommand::PauseResume => self.toggle_pause().await,
            TrayCommand::StopAndSave => self.stop(StopSource::Tray).await,
            TrayCommand::Quit => self.apply(PresenceEvent::Quit { immediate: false }).await,
            TrayCommand::PrimaryClick => self.primary_click().await,
        }
    }

    async fn on_hotkey(&mut self, id: u32) {
        let actions = self
            .hotkeys
            .as_ref()
            .map(|hotkeys| hotkeys.actions_for(id))
            .unwrap_or_default();
        for action in actions {
            match action {
                HotkeyAction::RegionScreenshot => self.start_picker().await,
                HotkeyAction::FullscreenScreenshot => self.fullscreen().await,
                HotkeyAction::StartVideo => self.start_video().await,
                HotkeyAction::StartAudio => self.start_audio().await,
                HotkeyAction::StopRecording => self.stop(StopSource::Hotkey).await,
            }
        }
    }

    async fn primary_click(&mut self) {
        match self.state.snapshot().settings.tray_left_click {
            TrayClickAction::OpenWindow => self.apply(PresenceEvent::OpenRequested).await,
            TrayClickAction::ScreenArea => self.start_picker().await,
        }
    }

    async fn start_picker(&mut self) {
        match self.screenshots.start_picker().await {
            Ok(CommandOk::PickerStarted { session }) => {
                self.apply(PresenceEvent::PickerRequested { session }).await
            }
            Ok(_) => self.notice("Screen area", "the picker did not start"),
            Err(error) => self.notice("Screen area", error.message),
        }
    }

    async fn fullscreen(&mut self) {
        let monitor = tokio::task::spawn_blocking(monitor_under_cursor).await;
        let monitor = match monitor {
            Ok(Some(monitor)) => monitor,
            Ok(None) => {
                self.notice("Screenshot", "No monitors are available");
                return;
            }
            Err(error) => {
                self.notice("Screenshot", format!("monitor task failed: {error}"));
                return;
            }
        };
        if let Err(error) = self
            .screenshots
            .capture_monitor(monitor, AfterCapture::CopyAndSave)
            .await
        {
            self.notice("Screenshot", error.message);
        }
    }

    async fn start_video(&mut self) {
        let monitors = tokio::task::spawn_blocking(screen::monitors).await;
        let monitors = match monitors {
            Ok(Ok(monitors)) => monitors,
            Ok(Err(error)) => {
                self.notice("Recording", format!("{error:#}"));
                return;
            }
            Err(error) => {
                self.notice("Recording", format!("monitor task failed: {error}"));
                return;
            }
        };
        if monitors.is_empty() {
            self.notice("Recording", "No monitors are available");
            return;
        }
        self.start_target(RecordTarget::Screens { monitors }, RecordProfile::Hotkey)
            .await;
    }

    async fn start_audio(&mut self) {
        self.start_target(RecordTarget::Audio, RecordProfile::Hotkey)
            .await;
    }

    async fn start_target(&mut self, target: RecordTarget, profile: RecordProfile) {
        if let Err(error) = self.recording.start(target, profile, None).await {
            self.notice("Recording", error.message);
        }
    }

    async fn toggle_pause(&mut self) {
        if let Err(error) = self.recording.pause().await {
            self.notice("Recording", error.message);
        }
    }

    async fn stop(&mut self, source: StopSource) {
        if let Err(error) = self.recording.stop(StopAction::Save, source).await {
            self.notice("Recording", error.message);
        }
    }

    async fn apply(&mut self, event: PresenceEvent) {
        let now = self.now_ms();
        let effects = self.presence.apply(event, now);
        self.run_effects(effects).await;
    }

    async fn run_effects(&mut self, effects: Vec<PresenceEffect>) {
        for effect in effects {
            self.run_effect(effect).await;
        }
    }

    async fn run_effect(&mut self, effect: PresenceEffect) {
        match effect {
            PresenceEffect::Spawn(startup) => self.spawn_ui(startup).await,
            PresenceEffect::Send(request) => self.mailbox.send(request),
            PresenceEffect::CancelPicker => self.cancel_picker().await,
            PresenceEffect::FinishQuit => self.finish_quit().await,
            PresenceEffect::Notice(message) => self.notice("Window", message),
        }
    }

    async fn spawn_ui(&mut self, startup: UiStartup) {
        let args = ui_args(&startup);
        if let Err(error) = (self.options.launch)(&args) {
            log::warn!("presence: cannot start the ui: {error}");
            match startup {
                UiStartup::Picker { .. } => Box::pin(self.apply(PresenceEvent::PickerFailed)).await,
                _ => Box::pin(self.apply(PresenceEvent::UiGone)).await,
            }
        }
    }

    async fn cancel_picker(&mut self) {
        let session = self
            .state
            .snapshot()
            .picker
            .as_ref()
            .map(|picker| picker.session);
        if let Some(session) = session {
            if let Err(error) = self.screenshots.picker_cancel(session).await {
                self.notice("Screen area", error.message);
            }
        }
    }

    async fn finish_quit(&mut self) {
        if let Err(error) = self.conversion.cancel().await {
            log::warn!("presence: cannot cancel conversion: {}", error.message);
        }
        self.shutdown.cancel();
    }

    fn notice(&self, summary: &str, detail: impl Into<String>) {
        self.notices
            .push(&self.state, NoticeLevel::Warning, summary, detail);
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.epoch.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::test_state;
    use mediaa2_core::formats::{OutputKind, VideoContainer};
    use mediaa2_core::recording::RecordTarget;
    use mediaa2_protocol::state::RecordingState;
    use mediaa2_protocol::{UiEvent, UiRequest, UiStartup};
    use std::sync::Arc;
    use std::time::Duration;

    use crate::services::{LibraryHandle, NoticeRing};

    fn fixture(
        options: PresenceOptions,
    ) -> (
        PresenceHandle,
        UiMailbox,
        Arc<StateStore>,
        Shutdown,
        tempfile::TempDir,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(StateStore::new(test_state()));
        let library = LibraryHandle::open(dir.path().join("media")).unwrap();
        let notices = NoticeRing::new();
        let recording = RecordingHandle::new(Arc::clone(&state), library.clone(), notices.clone());
        let conversion =
            ConversionHandle::new(Arc::clone(&state), library.clone(), notices.clone());
        let screenshots = ScreenshotHandle::new(Arc::clone(&state), library, notices.clone());
        let shutdown = Shutdown::new();
        let (mailbox, signals) = UiMailbox::new();
        let handle = PresenceHandle::new(PresenceDeps {
            state: Arc::clone(&state),
            mailbox: mailbox.clone(),
            signals,
            screenshots,
            recording,
            conversion,
            notices,
            shutdown: shutdown.clone(),
            options,
        });
        (handle, mailbox, state, shutdown, dir)
    }

    fn active(paused: bool) -> RecordingState {
        RecordingState::Active {
            job: 1,
            target: RecordTarget::Audio,
            started_ms: 0,
            paused,
            elapsed_ms: 0,
            sources: Vec::new(),
            has_audio: false,
            output: OutputKind::Video(VideoContainer::Mkv),
        }
    }

    #[test]
    fn tray_state_follows_the_recording_phase() {
        let settings = Settings::default();
        assert_eq!(
            tray_state_for(&RecordingState::Idle, &settings),
            TrayView {
                state: TrayState::Idle,
                progress: None,
                progress_enabled: true,
            }
        );
        assert_eq!(
            tray_state_for(
                &RecordingState::Starting {
                    started_ms: 0,
                    target: RecordTarget::Audio,
                },
                &settings
            )
            .state,
            TrayState::Starting
        );
        assert_eq!(
            tray_state_for(&active(false), &settings).state,
            TrayState::Recording
        );
        assert_eq!(
            tray_state_for(&active(true), &settings).state,
            TrayState::Paused
        );
        let saving = tray_state_for(
            &RecordingState::Finalizing {
                job: 1,
                recorded_ms: 1000,
                progress: Some(0.4),
            },
            &settings,
        );
        assert_eq!(saving.state, TrayState::Saving);
        assert_eq!(saving.progress, Some(0.4));
        assert!(
            !tray_state_for(
                &active(false),
                &Settings {
                    tray_progress: false,
                    ..Settings::default()
                }
            )
            .progress_enabled
        );
    }

    #[test]
    fn hotkeys_group_by_combo_and_stop_runs_first() {
        let settings = HotkeySettings {
            enabled: true,
            region_screenshot: "Ctrl+Shift+S".into(),
            start_video: "F9".into(),
            stop_recording: "F9".into(),
            ..HotkeySettings::default()
        };
        let groups = hotkey_groups(&settings);
        assert_eq!(groups.len(), 2);
        let f9 = groups.iter().find(|(text, _)| text == "F9").unwrap();
        assert_eq!(
            f9.1,
            vec![HotkeyAction::StopRecording, HotkeyAction::StartVideo]
        );
        let region = groups
            .iter()
            .find(|(text, _)| text == "Ctrl+Shift+S")
            .unwrap();
        assert_eq!(region.1, vec![HotkeyAction::RegionScreenshot]);
        let disabled = HotkeySettings {
            enabled: false,
            region_screenshot: "F9".into(),
            ..HotkeySettings::default()
        };
        assert!(hotkey_groups(&disabled).is_empty());
    }

    #[tokio::test]
    async fn open_spawns_one_ui_process() {
        let (args_tx, mut args_rx) = mpsc::unbounded_channel();
        let options = PresenceOptions {
            tray: false,
            hotkeys: false,
            launch: Arc::new(move |args| {
                let _ = args_tx.send(args.to_vec());
                Ok(())
            }),
        };
        let (handle, _mailbox, _state, _shutdown, _dir) = fixture(options);
        handle.open();
        let args = tokio::time::timeout(Duration::from_secs(2), args_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(args, vec![OsString::from("--ui"), OsString::from("--show")]);
        assert!(
            tokio::time::timeout(Duration::from_millis(100), args_rx.recv())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn ready_after_connection_sends_show_window() {
        let (handle, mailbox, _state, _shutdown, _dir) = fixture(PresenceOptions::disabled());
        let mut requests = mailbox.subscribe();
        handle.open();
        mailbox.connected(UiStartup::Show);
        mailbox.event(UiEvent::Ready);
        let request = tokio::time::timeout(Duration::from_secs(2), requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request, UiRequest::ShowWindow);
    }

    #[tokio::test]
    async fn quit_without_a_ui_cancels_shutdown() {
        let (handle, _mailbox, _state, shutdown, _dir) = fixture(PresenceOptions::disabled());
        handle.quit();
        tokio::time::timeout(Duration::from_secs(2), shutdown.cancelled())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_connected_ui_does_not_spawn_another_process() {
        let (args_tx, mut args_rx) = mpsc::unbounded_channel();
        let options = PresenceOptions {
            tray: false,
            hotkeys: false,
            launch: Arc::new(move |args| {
                let _ = args_tx.send(args.to_vec());
                Ok(())
            }),
        };
        let (_handle, mailbox, _state, _shutdown, _dir) = fixture(options);
        mailbox.connected(UiStartup::Tray);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), args_rx.recv())
                .await
                .is_err()
        );
    }

    #[test]
    fn spawned_picker_arguments_carry_the_session() {
        assert_eq!(
            ui_args(&UiStartup::Picker { session: 7 }),
            vec![
                OsString::from("--ui"),
                OsString::from("--picker"),
                OsString::from("7"),
            ]
        );
        assert_eq!(
            ui_args(&UiStartup::Tray),
            vec![OsString::from("--ui"), OsString::from("--tray")]
        );
    }

    #[test]
    fn autostart_mode_uses_both_settings() {
        let settings = Settings::default();
        assert_eq!(autostart_mode(&settings), AutoStart::Disabled);
        assert_eq!(
            autostart_mode(&Settings {
                launch_at_login: true,
                ..Settings::default()
            }),
            AutoStart::Window
        );
        assert_eq!(
            autostart_mode(&Settings {
                launch_at_login: true,
                start_in_tray: true,
                ..Settings::default()
            }),
            AutoStart::Tray
        );
    }

    #[test]
    fn tray_labels_are_localized() {
        let labels = tray_labels();
        assert!(!labels.show_window.is_empty());
        assert!(!labels.region.is_empty());
        assert!(!labels.fullscreen.is_empty());
        assert!(!labels.start_video.is_empty());
        assert!(!labels.start_audio.is_empty());
        assert!(!labels.pause.is_empty());
        assert!(!labels.resume.is_empty());
        assert!(!labels.stop.is_empty());
        assert!(!labels.quit.is_empty());
    }
}
