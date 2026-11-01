use crate::wake::Wake;
use crossbeam_channel::Receiver;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use mediaa_core::settings::HotkeyAction;

pub type HotkeyGroup = (String, HotKey, Vec<HotkeyAction>);

pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<(HotKey, Vec<HotkeyAction>)>,
}

impl Hotkeys {
    pub fn new(wake: &Wake) -> anyhow::Result<(Hotkeys, Receiver<u32>)> {
        let manager = GlobalHotKeyManager::new()?;
        let (tx, rx) = crossbeam_channel::bounded(32);
        let wake = wake.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                let _ = tx.try_send(event.id);
                wake.now();
            }
        }));
        Ok((
            Hotkeys {
                manager,
                registered: Vec::new(),
            },
            rx,
        ))
    }

    pub fn actions_for(&self, id: u32) -> Vec<HotkeyAction> {
        self.registered
            .iter()
            .find(|(hk, _)| hk.id() == id)
            .map(|(_, actions)| actions.clone())
            .unwrap_or_default()
    }

    pub fn apply(&mut self, groups: Vec<HotkeyGroup>) -> Vec<String> {
        for (hk, _) in self.registered.drain(..) {
            let _ = self.manager.unregister(hk);
        }
        let mut problems = Vec::new();

        for (text, hk, actions) in groups {
            match self.manager.register(hk) {
                Ok(()) => self.registered.push((hk, actions)),
                Err(e) => {
                    log::warn!("hotkey {text}: {e}");
                    problems.push(rust_i18n::t!("settings.hk_taken", key = text).into());
                }
            }
        }
        problems
    }
}
