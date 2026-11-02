#![allow(
    dead_code,
    reason = "hotkey grouping moved to backend; UI keeps actions"
)]
use global_hotkey::hotkey::HotKey;
use mediaa_core::settings::HotkeySettings;
use std::str::FromStr;

pub use mediaa_core::settings::HotkeyAction;

pub fn parse(text: &str) -> Result<HotKey, String> {
    HotKey::from_str(text.trim()).map_err(|e| e.to_string())
}

fn standalone(token: &str) -> bool {
    token.starts_with('F') && token[1..].parse::<u8>().is_ok()
        || matches!(token, "PRINTSCREEN" | "PAUSE" | "SCROLLLOCK" | "INSERT")
}

pub(crate) fn combo_token_is_standalone(token: &str) -> bool {
    standalone(token)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HotkeySlot {
    RegionScreenshot,
    FullscreenScreenshot,
    StartVideo,
    StartAudio,
    StopRecording,
}

impl HotkeySlot {
    pub(crate) const ALL: [HotkeySlot; 5] = [
        HotkeySlot::RegionScreenshot,
        HotkeySlot::FullscreenScreenshot,
        HotkeySlot::StartVideo,
        HotkeySlot::StartAudio,
        HotkeySlot::StopRecording,
    ];

    fn value(self, settings: &HotkeySettings) -> &str {
        match self {
            HotkeySlot::RegionScreenshot => &settings.region_screenshot,
            HotkeySlot::FullscreenScreenshot => &settings.fullscreen_screenshot,
            HotkeySlot::StartVideo => &settings.start_video,
            HotkeySlot::StartAudio => &settings.start_audio,
            HotkeySlot::StopRecording => &settings.stop_recording,
        }
    }
}

pub(crate) fn slot_for_index(index: usize) -> Option<HotkeySlot> {
    HotkeySlot::ALL.get(index).copied()
}

fn actions_may_share(left: HotkeyAction, right: HotkeyAction) -> bool {
    matches!(
        (left, right),
        (HotkeyAction::StopRecording, HotkeyAction::StartVideo)
            | (HotkeyAction::StartVideo, HotkeyAction::StopRecording)
            | (HotkeyAction::StopRecording, HotkeyAction::StartAudio)
            | (HotkeyAction::StartAudio, HotkeyAction::StopRecording)
    )
}

fn same_combo(left: &str, right: &str) -> bool {
    if left.trim().is_empty() || right.trim().is_empty() {
        return false;
    }
    match (parse(left), parse(right)) {
        (Ok(left), Ok(right)) => left.id() == right.id(),
        _ => left.trim().eq_ignore_ascii_case(right.trim()),
    }
}

pub(crate) fn conflicting_slot(
    settings: &HotkeySettings,
    slot: HotkeySlot,
    candidate: &str,
) -> Option<HotkeySlot> {
    for (i, left) in HotkeySlot::ALL.into_iter().enumerate() {
        for right in HotkeySlot::ALL.into_iter().skip(i + 1) {
            let left_value = if left == slot {
                candidate
            } else {
                left.value(settings)
            };
            let right_value = if right == slot {
                candidate
            } else {
                right.value(settings)
            };
            if same_combo(left_value, right_value)
                && !actions_may_share(slot_action(left), slot_action(right))
            {
                return Some(if right == slot { left } else { right });
            }
        }
    }
    None
}

fn slot_action(slot: HotkeySlot) -> HotkeyAction {
    match slot {
        HotkeySlot::RegionScreenshot => HotkeyAction::RegionScreenshot,
        HotkeySlot::FullscreenScreenshot => HotkeyAction::FullscreenScreenshot,
        HotkeySlot::StartVideo => HotkeyAction::StartVideo,
        HotkeySlot::StartAudio => HotkeyAction::StartAudio,
        HotkeySlot::StopRecording => HotkeyAction::StopRecording,
    }
}

pub(crate) fn ordered_actions(mut actions: Vec<HotkeyAction>) -> Vec<HotkeyAction> {
    actions.sort_by_key(|action| u8::from(*action != HotkeyAction::StopRecording));
    actions
}

pub(crate) type HotkeyGroup = (String, HotKey, Vec<HotkeyAction>);

pub(crate) fn grouped_actions(settings: &HotkeySettings) -> (Vec<HotkeyGroup>, Vec<String>) {
    let mut groups: Vec<HotkeyGroup> = Vec::new();
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
            Ok(hk) => {
                if let Some(group) = groups
                    .iter_mut()
                    .find(|(_, existing, _)| existing.id() == hk.id())
                {
                    group.2.push(action);
                } else {
                    groups.push((text.clone(), hk, vec![action]));
                }
            }
            Err(e) => problems
                .push(rust_i18n::t!("settings.hk_parse_error", key = text, error = e).into()),
        }
    }
    (groups, problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_runs_before_start_when_the_key_is_shared() {
        let actions = ordered_actions(vec![
            HotkeyAction::StartVideo,
            HotkeyAction::StopRecording,
            HotkeyAction::StartAudio,
        ]);
        assert_eq!(actions[0], HotkeyAction::StopRecording);
    }

    #[test]
    fn only_stop_may_share_a_key_with_video_or_audio() {
        let mut settings = HotkeySettings {
            start_video: "F9".into(),
            stop_recording: "F9".into(),
            ..HotkeySettings::default()
        };
        assert_eq!(
            conflicting_slot(&settings, HotkeySlot::StartVideo, "F9"),
            None
        );
        assert_eq!(
            conflicting_slot(&settings, HotkeySlot::StopRecording, "F9"),
            None
        );
        settings.start_audio = "F9".into();
        assert!(conflicting_slot(&settings, HotkeySlot::StartVideo, "F9").is_some());
        assert!(conflicting_slot(&settings, HotkeySlot::StartAudio, "F9").is_some());
        assert!(conflicting_slot(&settings, HotkeySlot::StopRecording, "F9").is_some());

        let mut settings = HotkeySettings {
            region_screenshot: "Ctrl+Shift+S".into(),
            start_video: "Ctrl+Shift+S".into(),
            ..HotkeySettings::default()
        };
        assert!(
            conflicting_slot(&settings, HotkeySlot::RegionScreenshot, "Ctrl+Shift+S").is_some()
        );
        settings.stop_recording = "Ctrl+Shift+S".into();
        assert!(conflicting_slot(&settings, HotkeySlot::StopRecording, "Ctrl+Shift+S").is_some());
    }

    #[test]
    fn one_key_can_start_and_stop_the_same_recording() {
        for start in ["F9", "Ctrl+Alt+R"] {
            for action in [HotkeyAction::StartVideo, HotkeyAction::StartAudio] {
                let mut settings = HotkeySettings {
                    enabled: true,
                    stop_recording: start.to_string(),
                    ..HotkeySettings::default()
                };
                match action {
                    HotkeyAction::StartVideo => settings.start_video = start.to_string(),
                    HotkeyAction::StartAudio => settings.start_audio = start.to_string(),
                    _ => unreachable!(),
                }
                let (groups, problems) = grouped_actions(&settings);
                assert!(problems.is_empty(), "{problems:?}");
                assert_eq!(groups.len(), 1, "one registered key, not two");
                assert!(groups[0].2.contains(&action));
                assert!(groups[0].2.contains(&HotkeyAction::StopRecording));
            }
        }
    }

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

    #[test]
    fn invalid_hotkeys_are_rejected() {
        assert!(parse("Ctrl+").is_err());
        assert!(parse("Ctrl+Banana").is_err());
    }
}
