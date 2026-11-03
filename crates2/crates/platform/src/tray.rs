use crate::tray_icon::{with_progress, Progress, StatusDot};
use crossbeam_channel::{Receiver, Sender};
use mediaa2_core::paths::APP_IDENTITY;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::thread::JoinHandle;
use tray_icon::menu::{IconMenuItem, Menu, MenuEvent, PredefinedMenuItem};
use tray_icon::{
    Icon, MouseButton, MouseButtonState, TrayIcon as PlatformTrayIcon, TrayIconBuilder,
    TrayIconEvent,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_APP,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayCommand {
    PrimaryClick,
    ShowWindow,
    RegionScreenshot,
    FullscreenScreenshot,
    StartRecording,
    StartAudio,
    PauseResume,
    StopAndSave,
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrayState {
    Idle,
    Starting,
    Recording,
    Paused,
    Saving,
}

impl TrayState {
    fn active(self) -> bool {
        matches!(self, Self::Recording | Self::Paused)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayLabels {
    pub show_window: String,
    pub region: String,
    pub fullscreen: String,
    pub start_video: String,
    pub start_audio: String,
    pub pause: String,
    pub resume: String,
    pub stop: String,
    pub quit: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayIcon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

const WAKE: u32 = WM_APP + 1;

const ITEMS: [(&str, TrayCommand); 8] = [
    ("show", TrayCommand::ShowWindow),
    ("region", TrayCommand::RegionScreenshot),
    ("fullscreen", TrayCommand::FullscreenScreenshot),
    ("start-video", TrayCommand::StartRecording),
    ("start-audio", TrayCommand::StartAudio),
    ("pause", TrayCommand::PauseResume),
    ("stop", TrayCommand::StopAndSave),
    ("quit", TrayCommand::Quit),
];

fn command_for(id: &str) -> Option<TrayCommand> {
    ITEMS
        .iter()
        .find(|(item, _)| *item == id)
        .map(|(_, command)| *command)
}

fn label(command: TrayCommand, state: TrayState, labels: &TrayLabels) -> &str {
    match command {
        TrayCommand::ShowWindow => &labels.show_window,
        TrayCommand::RegionScreenshot => &labels.region,
        TrayCommand::FullscreenScreenshot => &labels.fullscreen,
        TrayCommand::StartRecording => &labels.start_video,
        TrayCommand::StartAudio => &labels.start_audio,
        TrayCommand::PauseResume => {
            if state == TrayState::Paused {
                &labels.resume
            } else {
                &labels.pause
            }
        }
        TrayCommand::StopAndSave => &labels.stop,
        TrayCommand::Quit => &labels.quit,
        TrayCommand::PrimaryClick => "",
    }
}

fn enabled(command: TrayCommand, state: TrayState) -> bool {
    match command {
        TrayCommand::StartRecording | TrayCommand::StartAudio => state == TrayState::Idle,
        TrayCommand::PauseResume | TrayCommand::StopAndSave => state.active(),
        TrayCommand::PrimaryClick => false,
        _ => true,
    }
}

fn fraction(progress: Option<f32>) -> Option<f32> {
    progress
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 1.0))
}

fn status_dot(state: TrayState) -> Option<StatusDot> {
    match state {
        TrayState::Idle => None,
        TrayState::Starting | TrayState::Saving => Some(StatusDot::Saving),
        TrayState::Recording => Some(StatusDot::Recording),
        TrayState::Paused => Some(StatusDot::Paused),
    }
}

fn tooltip(state: TrayState, progress: Option<f32>) -> String {
    let name = APP_IDENTITY.name;
    match (state, fraction(progress)) {
        (TrayState::Idle, _) | (_, None) => name.to_string(),
        (_, Some(value)) => format!("{name} {}%", (value * 100.0).round() as u32),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct TrayView {
    state: TrayState,
    progress: Option<f32>,
    progress_enabled: bool,
}

impl TrayView {
    fn dot(self) -> Option<StatusDot> {
        if self.progress_enabled {
            status_dot(self.state)
        } else {
            None
        }
    }

    fn icon(self, base: &TrayIcon) -> TrayIcon {
        match self.dot() {
            Some(dot) => with_progress(
                base,
                Progress {
                    dot,
                    fraction: fraction(self.progress),
                },
            ),
            None => base.clone(),
        }
    }

    fn visible_progress(self) -> Option<f32> {
        if self.progress_enabled {
            self.progress
        } else {
            None
        }
    }
}

#[derive(Clone)]
struct Route {
    icon: tray_icon::TrayIconId,
    commands: Sender<TrayCommand>,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static ROUTE: OnceLock<Mutex<Option<Route>>> = OnceLock::new();
static HANDLERS: OnceLock<()> = OnceLock::new();

fn route() -> &'static Mutex<Option<Route>> {
    ROUTE.get_or_init(|| Mutex::new(None))
}

fn dispatch(command: TrayCommand) {
    let route = route().lock().clone();
    if let Some(route) = route {
        let _ = route.commands.send(command);
    }
}

fn dispatch_click(id: &tray_icon::TrayIconId) {
    let route = route().lock().clone();
    if let Some(route) = route {
        if route.icon == *id {
            let _ = route.commands.send(TrayCommand::PrimaryClick);
        }
    }
}

fn install_handlers() {
    HANDLERS.get_or_init(|| {
        MenuEvent::set_event_handler(Some(|event: MenuEvent| {
            if let Some(command) = command_for(event.id.as_ref()) {
                dispatch(command);
            }
        }));
        TrayIconEvent::set_event_handler(Some(|event: TrayIconEvent| {
            let left_click = matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } | TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            );
            if left_click {
                dispatch_click(event.id());
            }
        }));
    });
}

enum TrayRequest {
    State {
        state: TrayState,
        progress: Option<f32>,
    },
    Icon(TrayIcon),
    ProgressEnabled(bool),
    Shutdown,
}

struct TrayWorker {
    tray: PlatformTrayIcon,
    items: Vec<(TrayCommand, IconMenuItem)>,
    labels: TrayLabels,
    base: TrayIcon,
    view: TrayView,
    commands: Sender<TrayCommand>,
}

impl TrayWorker {
    fn new(labels: TrayLabels, base: TrayIcon, commands: Sender<TrayCommand>) -> io::Result<Self> {
        let view = TrayView {
            state: TrayState::Idle,
            progress: None,
            progress_enabled: true,
        };
        let items = build_items(&labels, view.state);
        let menu = build_menu(&items)?;
        let icon = platform_icon(&base).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "tray icon buffer is invalid")
        })?;
        let tray = TrayIconBuilder::new()
            .with_tooltip(APP_IDENTITY.name)
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .map_err(|error| io::Error::other(format!("tray icon: {error}")))?;
        let mut worker = Self {
            tray,
            items,
            labels,
            base,
            view,
            commands,
        };
        worker.apply_view();
        Ok(worker)
    }

    fn apply_view(&mut self) {
        for (command, item) in &self.items {
            item.set_text(label(*command, self.view.state, &self.labels));
            item.set_enabled(enabled(*command, self.view.state));
        }
        if let Some(icon) = platform_icon(&self.view.icon(&self.base)) {
            let _ = self.tray.set_icon(Some(icon));
        }
        let _ = self
            .tray
            .set_tooltip(Some(tooltip(self.view.state, self.view.visible_progress())));
    }

    fn run(&mut self, requests: Receiver<TrayRequest>) {
        // SAFETY: MSG is a plain Win32 structure and its all-zero bit pattern is valid.
        let mut message: MSG = unsafe { std::mem::zeroed() };
        loop {
            // SAFETY: message is a live MSG and a null window filter reads this thread's queue.
            let result = unsafe { GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) };
            if result <= 0 {
                break;
            }
            if message.message != WAKE {
                // SAFETY: message was filled by GetMessageW and is still alive.
                unsafe {
                    TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
                continue;
            }
            let mut shutdown = false;
            while let Ok(request) = requests.try_recv() {
                match request {
                    TrayRequest::State { state, progress } => {
                        let view = TrayView {
                            state,
                            progress: fraction(progress),
                            progress_enabled: self.view.progress_enabled,
                        };
                        if view != self.view {
                            self.view = view;
                            self.apply_view();
                        }
                    }
                    TrayRequest::Icon(icon) => {
                        self.base = icon;
                        self.apply_view();
                    }
                    TrayRequest::ProgressEnabled(enabled) => {
                        if enabled != self.view.progress_enabled {
                            self.view.progress_enabled = enabled;
                            self.apply_view();
                        }
                    }
                    TrayRequest::Shutdown => shutdown = true,
                }
            }
            if shutdown {
                break;
            }
        }
    }
}

fn build_items(labels: &TrayLabels, state: TrayState) -> Vec<(TrayCommand, IconMenuItem)> {
    ITEMS
        .iter()
        .map(|(id, command)| {
            let item = IconMenuItem::with_id(
                *id,
                label(*command, state, labels),
                enabled(*command, state),
                None,
                None,
            );
            (*command, item)
        })
        .collect()
}

fn item(items: &[(TrayCommand, IconMenuItem)], command: TrayCommand) -> &IconMenuItem {
    &items
        .iter()
        .find(|(item, _)| *item == command)
        .expect("all tray menu items exist")
        .1
}

fn build_menu(items: &[(TrayCommand, IconMenuItem)]) -> io::Result<Menu> {
    let menu = Menu::new();
    let separator_one = PredefinedMenuItem::separator();
    let separator_two = PredefinedMenuItem::separator();
    let separator_three = PredefinedMenuItem::separator();
    menu.append_items(&[
        item(items, TrayCommand::ShowWindow),
        &separator_one,
        item(items, TrayCommand::RegionScreenshot),
        item(items, TrayCommand::FullscreenScreenshot),
        &separator_two,
        item(items, TrayCommand::StartRecording),
        item(items, TrayCommand::StartAudio),
        item(items, TrayCommand::PauseResume),
        item(items, TrayCommand::StopAndSave),
        &separator_three,
        item(items, TrayCommand::Quit),
    ])
    .map_err(|error| io::Error::other(format!("tray menu: {error}")))?;
    Ok(menu)
}

fn platform_icon(icon: &TrayIcon) -> Option<Icon> {
    Icon::from_rgba(icon.rgba.clone(), icon.width, icon.height).ok()
}

fn valid_icon(icon: &TrayIcon) -> bool {
    let Some(pixels) = (icon.width as usize).checked_mul(icon.height as usize) else {
        return false;
    };
    let Some(bytes) = pixels.checked_mul(4) else {
        return false;
    };
    icon.width > 0 && icon.height > 0 && icon.rgba.len() == bytes
}

fn run_tray(
    labels: TrayLabels,
    base: TrayIcon,
    commands: Sender<TrayCommand>,
    requests: Receiver<TrayRequest>,
    init: Sender<io::Result<u32>>,
) {
    let mut worker = match TrayWorker::new(labels, base, commands) {
        Ok(worker) => worker,
        Err(error) => {
            let _ = init.send(Err(error));
            return;
        }
    };
    // SAFETY: GetCurrentThreadId has no preconditions and returns the calling thread id.
    let thread_id = unsafe { GetCurrentThreadId() };
    *route().lock() = Some(Route {
        icon: worker.tray.id().clone(),
        commands: worker.commands.clone(),
    });
    let _ = init.send(Ok(thread_id));
    worker.run(requests);
    drop(worker);
    *route().lock() = None;
}

pub struct Tray {
    requests: Sender<TrayRequest>,
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
}

impl Tray {
    pub fn new(labels: TrayLabels, icon: TrayIcon) -> io::Result<(Self, Receiver<TrayCommand>)> {
        if !valid_icon(&icon) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "tray icon buffer is invalid",
            ));
        }
        if ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "a tray icon is already running",
            ));
        }
        install_handlers();
        let (commands, events) = crossbeam_channel::unbounded();
        let (requests, request_rx) = crossbeam_channel::unbounded();
        let (init_tx, init_rx) = crossbeam_channel::bounded(1);
        let worker = match std::thread::Builder::new()
            .name("mediaa2-tray".into())
            .spawn(move || run_tray(labels, icon, commands, request_rx, init_tx))
        {
            Ok(worker) => worker,
            Err(error) => {
                ACTIVE.store(false, Ordering::SeqCst);
                return Err(error);
            }
        };
        match init_rx.recv() {
            Ok(Ok(thread_id)) => Ok((
                Self {
                    requests,
                    thread_id,
                    worker: Some(worker),
                },
                events,
            )),
            Ok(Err(error)) => {
                let _ = worker.join();
                ACTIVE.store(false, Ordering::SeqCst);
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                ACTIVE.store(false, Ordering::SeqCst);
                Err(io::Error::other("tray thread stopped during startup"))
            }
        }
    }

    pub fn set_state(&self, state: TrayState, progress: Option<f32>) {
        self.post(TrayRequest::State {
            state,
            progress: fraction(progress),
        });
    }

    pub fn set_icon(&self, icon: TrayIcon) {
        if valid_icon(&icon) {
            self.post(TrayRequest::Icon(icon));
        }
    }

    pub fn set_progress_enabled(&self, enabled: bool) {
        self.post(TrayRequest::ProgressEnabled(enabled));
    }

    fn post(&self, request: TrayRequest) {
        if self.requests.send(request).is_ok() {
            // SAFETY: thread_id names a live worker thread; a failed post only skips the wake.
            unsafe {
                PostThreadMessageW(self.thread_id, WAKE, 0, 0);
            }
        }
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        if self.requests.send(TrayRequest::Shutdown).is_ok() {
            // SAFETY: thread_id names the worker thread that owns the tray; a failed post is harmless.
            unsafe {
                PostThreadMessageW(self.thread_id, WAKE, 0, 0);
            }
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        ACTIVE.store(false, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels() -> TrayLabels {
        TrayLabels {
            show_window: "Show window".into(),
            region: "Region".into(),
            fullscreen: "Fullscreen".into(),
            start_video: "Start video".into(),
            start_audio: "Start audio".into(),
            pause: "Pause".into(),
            resume: "Resume".into(),
            stop: "Stop".into(),
            quit: "Quit".into(),
        }
    }

    #[test]
    fn every_menu_id_maps_to_a_command() {
        for (id, command) in ITEMS {
            assert_eq!(command_for(id), Some(command));
        }
        assert_eq!(command_for("cancel"), None);
        assert_eq!(command_for("nope"), None);
    }

    #[test]
    fn menu_ids_are_unique() {
        let mut ids: Vec<&str> = ITEMS.iter().map(|(id, _)| *id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ITEMS.len());
    }

    #[test]
    fn idle_menu_allows_starting_and_disables_pause_and_stop() {
        let labels = labels();
        assert!(enabled(TrayCommand::StartRecording, TrayState::Idle));
        assert!(enabled(TrayCommand::StartAudio, TrayState::Idle));
        assert!(!enabled(TrayCommand::PauseResume, TrayState::Idle));
        assert!(!enabled(TrayCommand::StopAndSave, TrayState::Idle));
        assert_eq!(
            label(TrayCommand::PauseResume, TrayState::Idle, &labels),
            "Pause"
        );
        for command in [
            TrayCommand::ShowWindow,
            TrayCommand::RegionScreenshot,
            TrayCommand::FullscreenScreenshot,
            TrayCommand::Quit,
        ] {
            assert!(enabled(command, TrayState::Idle));
        }
    }

    #[test]
    fn recording_and_paused_swap_the_pause_item() {
        let labels = labels();
        assert!(!enabled(TrayCommand::StartRecording, TrayState::Recording));
        assert!(!enabled(TrayCommand::StartAudio, TrayState::Paused));
        assert!(enabled(TrayCommand::PauseResume, TrayState::Recording));
        assert!(enabled(TrayCommand::StopAndSave, TrayState::Recording));
        assert_eq!(
            label(TrayCommand::PauseResume, TrayState::Recording, &labels),
            "Pause"
        );
        assert_eq!(
            label(TrayCommand::PauseResume, TrayState::Paused, &labels),
            "Resume"
        );
    }

    #[test]
    fn busy_states_do_not_start_or_stop() {
        for state in [TrayState::Starting, TrayState::Saving] {
            assert!(!enabled(TrayCommand::StartRecording, state));
            assert!(!enabled(TrayCommand::StartAudio, state));
            assert!(!enabled(TrayCommand::PauseResume, state));
            assert!(!enabled(TrayCommand::StopAndSave, state));
        }
    }

    #[test]
    fn progress_is_hidden_while_disabled_or_idle() {
        let base = crate::tray_icon::icon(false, 1);
        let idle = TrayView {
            state: TrayState::Idle,
            progress: Some(0.5),
            progress_enabled: true,
        };
        assert_eq!(idle.dot(), None);
        assert_eq!(idle.icon(&base), base);
        assert_eq!(idle.visible_progress(), Some(0.5));
        let disabled = TrayView {
            state: TrayState::Recording,
            progress: None,
            progress_enabled: false,
        };
        assert_eq!(disabled.dot(), None);
        assert_eq!(disabled.icon(&base), base);
        assert_eq!(disabled.visible_progress(), None);
        let recording = TrayView {
            state: TrayState::Recording,
            progress: None,
            progress_enabled: true,
        };
        assert_eq!(recording.dot(), Some(StatusDot::Recording));
        assert_ne!(recording.icon(&base), base);
    }

    #[test]
    fn tooltip_keeps_the_name_and_adds_a_percentage() {
        assert_eq!(tooltip(TrayState::Idle, Some(0.5)), "MediaA");
        assert_eq!(tooltip(TrayState::Recording, None), "MediaA");
        assert_eq!(tooltip(TrayState::Saving, Some(0.42)), "MediaA 42%");
        assert_eq!(tooltip(TrayState::Saving, Some(1.5)), "MediaA 100%");
    }

    #[test]
    fn progress_fraction_is_finite_and_clamped() {
        assert_eq!(fraction(Some(-1.0)), Some(0.0));
        assert_eq!(fraction(Some(2.0)), Some(1.0));
        assert_eq!(fraction(Some(f32::NAN)), None);
        assert_eq!(fraction(None), None);
    }

    #[test]
    fn tray_is_send() {
        fn assert_send<T: Send>() {}
        assert_send::<Tray>();
    }

    #[test]
    fn invalid_icon_buffers_are_rejected() {
        let bad = TrayIcon {
            width: 2,
            height: 2,
            rgba: vec![0; 3],
        };
        assert!(!valid_icon(&bad));
        let good = TrayIcon {
            width: 2,
            height: 2,
            rgba: vec![0; 16],
        };
        assert!(valid_icon(&good));
        let empty = TrayIcon {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
        assert!(!valid_icon(&empty));
    }
}
