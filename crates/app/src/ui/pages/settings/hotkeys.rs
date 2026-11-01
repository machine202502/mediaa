use crate::logic::hotkeys::{combo_token_is_standalone, parse};
use eframe::egui;

fn modifier_prefix(modifiers: egui::Modifiers) -> String {
    let mut prefix = String::new();
    for (on, name) in [
        (modifiers.ctrl, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
        (modifiers.mac_cmd, "Super"),
    ] {
        if on {
            prefix.push_str(name);
            prefix.push('+');
        }
    }
    prefix
}

pub(crate) fn combo_from_token(modifiers: egui::Modifiers, token: &str) -> Option<String> {
    let has_modifier = modifiers.ctrl || modifiers.alt || modifiers.shift || modifiers.mac_cmd;
    if !has_modifier && !combo_token_is_standalone(token) {
        return None;
    }
    let combo = format!("{}{}", modifier_prefix(modifiers), pretty(token));
    parse(&combo).ok().map(|_| combo)
}

pub(crate) fn combo_from_key(modifiers: egui::Modifiers, key: egui::Key) -> Option<String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn modifiers(ctrl: bool, alt: bool, shift: bool) -> egui::Modifiers {
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
            combo_from_key(modifiers(true, false, true), egui::Key::S).as_deref(),
            Some("Ctrl+Shift+S")
        );
        assert_eq!(
            combo_from_key(modifiers(false, true, false), egui::Key::ArrowUp).as_deref(),
            Some("Alt+Up")
        );
        assert_eq!(
            combo_from_key(modifiers(false, false, false), egui::Key::S),
            None,
            "letters need a modifier"
        );
        assert_eq!(
            combo_from_key(modifiers(false, false, false), egui::Key::F9).as_deref(),
            Some("F9")
        );
        assert_eq!(
            combo_from_key(modifiers(true, false, false), egui::Key::Escape),
            None
        );
        assert_eq!(
            combo_from_token(modifiers(false, false, false), "PRINTSCREEN").as_deref(),
            Some("PrintScreen")
        );
        assert_eq!(
            combo_from_token(modifiers(false, false, true), "PRINTSCREEN").as_deref(),
            Some("Shift+PrintScreen")
        );
    }

    #[test]
    fn every_captured_key_is_a_valid_hotkey() {
        for &key in egui::Key::ALL {
            if let Some(combo) = combo_from_key(modifiers(true, false, false), key) {
                assert!(parse(&combo).is_ok(), "{key:?} -> {combo}");
            }
        }
    }
}
