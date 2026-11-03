use crossbeam_channel::{bounded, Receiver, Sender};
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use mediaa2_core::settings::HotkeyAction;
use parking_lot::Mutex;
use std::io;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::thread::JoinHandle;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostThreadMessageW, TranslateMessage, MSG, WM_APP,
};

const WAKE: u32 = WM_APP + 2;

type Group = (String, HotKey, Vec<HotkeyAction>);
type ActionMap = Arc<Mutex<Vec<(u32, Vec<HotkeyAction>)>>>;

static HANDLER: OnceLock<()> = OnceLock::new();
static EVENT_SINK: OnceLock<Mutex<Option<Sender<u32>>>> = OnceLock::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn event_sink() -> &'static Mutex<Option<Sender<u32>>> {
    EVENT_SINK.get_or_init(|| Mutex::new(None))
}

fn install_handler() {
    HANDLER.get_or_init(|| {
        GlobalHotKeyEvent::set_event_handler(Some(|event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed {
                return;
            }
            let sink = event_sink().lock().clone();
            if let Some(sink) = sink {
                let _ = sink.try_send(event.id);
            }
        }));
    });
}

fn parse_groups(groups: Vec<(String, Vec<HotkeyAction>)>) -> (Vec<Group>, Vec<String>) {
    let mut parsed = Vec::new();
    let mut problems = Vec::new();
    for (text, actions) in groups {
        match HotKey::from_str(text.trim()) {
            Ok(hotkey) => parsed.push((text, hotkey, actions)),
            Err(error) => problems.push(format!("{text}: {error}")),
        }
    }
    (parsed, problems)
}

enum Request {
    Apply(Vec<(String, Vec<HotkeyAction>)>, Sender<Vec<String>>),
    Pause(Sender<()>),
    Resume(Sender<()>),
    Shutdown,
}

struct State {
    manager: GlobalHotKeyManager,
    registered: Vec<HotKey>,
    groups: Vec<Group>,
    paused: bool,
    actions: ActionMap,
}

impl State {
    fn unregister_all(&mut self) {
        for hotkey in self.registered.drain(..) {
            if let Err(error) = self.manager.unregister(hotkey) {
                log::warn!("hotkey unregister: {error}");
            }
        }
    }

    fn rewrite_actions(&self) {
        let mapping = self
            .groups
            .iter()
            .map(|(_, hotkey, actions)| (hotkey.id(), actions.clone()))
            .collect();
        *self.actions.lock() = mapping;
    }

    fn register_all(&mut self, problems: &mut Vec<String>) {
        for (text, hotkey, _) in &self.groups {
            match self.manager.register(*hotkey) {
                Ok(()) => self.registered.push(*hotkey),
                Err(error) => problems.push(format!("{text}: {error}")),
            }
        }
    }

    fn apply(&mut self, groups: Vec<(String, Vec<HotkeyAction>)>) -> Vec<String> {
        self.unregister_all();
        let (parsed, mut problems) = parse_groups(groups);
        self.groups = parsed;
        self.rewrite_actions();
        if !self.paused {
            self.register_all(&mut problems);
        }
        problems
    }

    fn pause(&mut self) {
        self.paused = true;
        self.unregister_all();
    }

    fn resume(&mut self) {
        self.paused = false;
        self.unregister_all();
        let mut problems = Vec::new();
        self.register_all(&mut problems);
        for problem in problems {
            log::warn!("hotkey {problem}");
        }
    }
}

pub struct Hotkeys {
    requests: Sender<Request>,
    thread_id: u32,
    worker: Option<JoinHandle<()>>,
    events: Sender<u32>,
    actions: ActionMap,
}

impl Hotkeys {
    pub fn new() -> io::Result<(Self, Receiver<u32>)> {
        if ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "global hotkeys are already running",
            ));
        }
        install_handler();
        let (events, event_rx) = crossbeam_channel::unbounded();
        *event_sink().lock() = Some(events.clone());
        let (requests, request_rx) = crossbeam_channel::unbounded();
        let (init_tx, init_rx) = bounded(1);
        let actions = Arc::new(Mutex::new(Vec::new()));
        let worker_actions = actions.clone();
        let worker = match std::thread::Builder::new()
            .name("mediaa2-hotkeys".into())
            .spawn(move || run(request_rx, init_tx, worker_actions))
        {
            Ok(worker) => worker,
            Err(error) => {
                *event_sink().lock() = None;
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
                    events,
                    actions,
                },
                event_rx,
            )),
            Ok(Err(error)) => {
                let _ = worker.join();
                *event_sink().lock() = None;
                ACTIVE.store(false, Ordering::SeqCst);
                Err(error)
            }
            Err(_) => {
                let _ = worker.join();
                *event_sink().lock() = None;
                ACTIVE.store(false, Ordering::SeqCst);
                Err(io::Error::other("hotkey thread stopped during startup"))
            }
        }
    }

    pub fn apply(&mut self, groups: Vec<(String, Vec<HotkeyAction>)>) -> Vec<String> {
        let (reply, answer) = bounded(1);
        if self.requests.send(Request::Apply(groups, reply)).is_err() || !self.wake() {
            return vec!["global hotkeys are unavailable".to_string()];
        }
        match answer.recv() {
            Ok(problems) => problems,
            Err(_) => vec!["global hotkeys stopped".to_string()],
        }
    }

    pub fn pause(&mut self) {
        let (done, answer) = bounded(1);
        if self.requests.send(Request::Pause(done)).is_ok() && self.wake() {
            let _ = answer.recv();
        }
    }

    pub fn resume(&mut self) {
        let (done, answer) = bounded(1);
        if self.requests.send(Request::Resume(done)).is_ok() && self.wake() {
            let _ = answer.recv();
        }
    }

    pub fn actions_for(&self, id: u32) -> Vec<HotkeyAction> {
        self.actions
            .lock()
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, actions)| actions.clone())
            .unwrap_or_default()
    }

    fn wake(&self) -> bool {
        // SAFETY: thread_id names a live worker thread; a failed post only skips the wake.
        unsafe { PostThreadMessageW(self.thread_id, WAKE, 0, 0) != 0 }
    }
}

impl Drop for Hotkeys {
    fn drop(&mut self) {
        if self.requests.send(Request::Shutdown).is_ok() {
            self.wake();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let mut sink = event_sink().lock();
        if sink
            .as_ref()
            .is_some_and(|current| current.same_channel(&self.events))
        {
            *sink = None;
        }
        ACTIVE.store(false, Ordering::SeqCst);
    }
}

fn run(requests: Receiver<Request>, init: Sender<io::Result<u32>>, actions: ActionMap) {
    let manager = match GlobalHotKeyManager::new() {
        Ok(manager) => manager,
        Err(error) => {
            let _ = init.send(Err(io::Error::other(format!(
                "global hotkeys unavailable: {error}"
            ))));
            return;
        }
    };
    // SAFETY: GetCurrentThreadId has no preconditions and returns the calling thread id.
    let thread_id = unsafe { GetCurrentThreadId() };
    if init.send(Ok(thread_id)).is_err() {
        return;
    }
    let mut state = State {
        manager,
        registered: Vec::new(),
        groups: Vec::new(),
        paused: false,
        actions,
    };
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
                Request::Apply(groups, reply) => {
                    let problems = state.apply(groups);
                    let _ = reply.send(problems);
                }
                Request::Pause(done) => {
                    state.pause();
                    let _ = done.send(());
                }
                Request::Resume(done) => {
                    state.resume();
                    let _ = done.send(());
                }
                Request::Shutdown => shutdown = true,
            }
        }
        if shutdown {
            break;
        }
    }
    state.unregister_all();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_hotkeys_are_parsed_and_grouped() {
        let (groups, problems) = parse_groups(vec![
            ("Ctrl+Shift+S".into(), vec![HotkeyAction::RegionScreenshot]),
            (
                "F9".into(),
                vec![HotkeyAction::StartVideo, HotkeyAction::StopRecording],
            ),
        ]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "Ctrl+Shift+S");
        assert_eq!(
            groups[1].2,
            vec![HotkeyAction::StartVideo, HotkeyAction::StopRecording]
        );
        assert_ne!(groups[0].1.id(), groups[1].1.id());
    }

    #[test]
    fn invalid_hotkeys_become_problems_and_keep_their_text() {
        let (groups, problems) =
            parse_groups(vec![("Ctrl+Banana".into(), vec![HotkeyAction::StartAudio])]);
        assert!(groups.is_empty());
        assert_eq!(problems.len(), 1);
        assert!(problems[0].starts_with("Ctrl+Banana:"), "{problems:?}");
    }

    #[test]
    fn whitespace_is_trimmed_before_parsing() {
        let (groups, problems) = parse_groups(vec![(
            "  PrintScreen  ".into(),
            vec![HotkeyAction::StartAudio],
        )]);
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].0, "  PrintScreen  ");
    }
}
