use crossbeam_channel::Receiver;
use eframe::egui;
use global_hotkey::hotkey::HotKey;
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use mediaa_core::settings::HotkeySettings;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HotkeyAction {
    RegionScreenshot,
    FullscreenScreenshot,
    StartVideo,
    StartAudio,
    StopRecording,
}

pub struct Hotkeys {
    manager: GlobalHotKeyManager,
    registered: Vec<(HotKey, HotkeyAction)>,
}

pub fn parse(text: &str) -> Result<HotKey, String> {
    HotKey::from_str(text.trim()).map_err(|e| e.to_string())
}

fn standalone(token: &str) -> bool {
    token.starts_with('F') && token[1..].parse::<u8>().is_ok()
        || matches!(token, "PRINTSCREEN" | "PAUSE" | "SCROLLLOCK" | "INSERT")
}

fn modifier_prefix(m: egui::Modifiers) -> String {
    let mut s = String::new();
    for (on, name) in [
        (m.ctrl, "Ctrl"),
        (m.alt, "Alt"),
        (m.shift, "Shift"),
        (m.mac_cmd, "Super"),
    ] {
        if on {
            s.push_str(name);
            s.push('+');
        }
    }
    s
}

pub fn combo_from_token(modifiers: egui::Modifiers, token: &str) -> Option<String> {
    let has_modifier = modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.mac_cmd;
    if !has_modifier && !standalone(token) {
        return None;
    }
    let combo = format!("{}{}", modifier_prefix(modifiers), pretty(token));
    parse(&combo).ok().map(|_| combo)
}

pub fn combo_from_key(modifiers: egui::Modifiers, key: egui::Key) -> Option<String> {
    if key == egui::Key::Escape {
        return None;
    }
    combo_from_token(modifiers, &key.name().to_uppercase().replace(' ', ""))
}

fn pretty(token: &str) -> String {
    match token {
        "PRINTSCREEN" => "PrintScreen".into(),
        "PAGEUP" => "PageUp".into(),
        "PAGEDOWN" => "PageDown".into(),
        "SCROLLLOCK" => "ScrollLock".into(),
        t if t.len() > 1 && !t.starts_with('F') => {
            let mut c = t.chars();
            c.next()
                .map(|f| f.to_string() + &c.as_str().to_lowercase())
                .unwrap_or_default()
        }
        t => t.to_string(),
    }
}

impl Hotkeys {
    pub fn new(ctx: &egui::Context) -> anyhow::Result<(Hotkeys, Receiver<u32>)> {
        let manager = GlobalHotKeyManager::new()?;
        let (tx, rx) = crossbeam_channel::bounded(32);
        let ctx = ctx.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state == HotKeyState::Pressed {
                let _ = tx.try_send(event.id);
                ctx.request_repaint();
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

    pub fn action_for(&self, id: u32) -> Option<HotkeyAction> {
        self.registered
            .iter()
            .find(|(hk, _)| hk.id() == id)
            .map(|(_, a)| *a)
    }

    pub fn apply(&mut self, settings: &HotkeySettings) -> Vec<String> {
        for (hk, _) in self.registered.drain(..) {
            let _ = self.manager.unregister(hk);
        }
        if !settings.enabled {
            return Vec::new();
        }
        let mut problems = Vec::new();
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
            if text.trim().is_empty() {
                continue;
            }
            match parse(text) {
                Err(e) => problems
                    .push(rust_i18n::t!("settings.hk_parse_error", key = text, error = e).into()),
                Ok(hk) => match self.manager.register(hk) {
                    Ok(()) => self.registered.push((hk, action)),
                    Err(e) => {
                        log::warn!("hotkey {text}: {e}");
                        problems.push(rust_i18n::t!("settings.hk_taken", key = text).into());
                    }
                },
            }
        }
        problems
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typical_hotkeys_parse() {
        for s in ["PrintScreen", "Shift+PrintScreen", "Alt+Shift+R"] {
            assert!(parse(s).is_ok(), "{s}");
        }
        assert_ne!(
            parse("PrintScreen").unwrap().id(),
            parse("Shift+PrintScreen").unwrap().id()
        );
    }

    fn mods(ctrl: bool, alt: bool, shift: bool) -> egui::Modifiers {
        egui::Modifiers {
            ctrl,
            alt,
            shift,
            ..Default::default()
        }
    }

    #[test]
    fn captured_combinations() {
        assert_eq!(
            combo_from_key(mods(true, false, true), egui::Key::S).as_deref(),
            Some("Ctrl+Shift+S")
        );
        assert_eq!(
            combo_from_key(mods(false, true, false), egui::Key::ArrowUp).as_deref(),
            Some("Alt+Up")
        );
        assert_eq!(
            combo_from_key(mods(false, false, false), egui::Key::S),
            None,
            "letters need a modifier"
        );
        assert_eq!(
            combo_from_key(mods(false, false, false), egui::Key::F9).as_deref(),
            Some("F9")
        );
        assert_eq!(
            combo_from_key(mods(true, false, false), egui::Key::Escape),
            None
        );
        assert_eq!(
            combo_from_token(mods(false, false, false), "PRINTSCREEN").as_deref(),
            Some("PrintScreen")
        );
        assert_eq!(
            combo_from_token(mods(false, false, true), "PRINTSCREEN").as_deref(),
            Some("Shift+PrintScreen")
        );
    }

    #[test]
    fn every_captured_key_is_a_valid_hotkey() {
        for &key in egui::Key::ALL {
            if let Some(combo) = combo_from_key(mods(true, false, false), key) {
                assert!(parse(&combo).is_ok(), "{key:?} -> {combo}");
            }
        }
    }

    #[test]
    fn invalid_hotkeys_are_rejected() {
        assert!(parse("Ctrl+").is_err());
        assert!(parse("Ctrl+Banana").is_err());
    }
}
