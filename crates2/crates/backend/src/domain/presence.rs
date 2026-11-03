use mediaa2_protocol::message::{UiRequest, UiStartup};

const START_TIMEOUT_MS: u64 = 5_000;
const MAX_START_ATTEMPTS: u8 = 2;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Presence {
    #[default]
    NoUi,
    Starting {
        mode: UiStartup,
        attempts: u8,
        since_ms: u64,
    },
    Open {
        visible: bool,
        show_on_ready: bool,
    },
    Picker {
        session: u64,
        previous_visible: bool,
        ready: bool,
    },
    Quitting,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceEvent {
    OpenRequested,
    UiConnected { startup: UiStartup },
    UiReady,
    WindowShown,
    WindowHidden,
    HideRequested,
    ShowRequested,
    PickerRequested { session: u64 },
    PickerDone,
    PickerFailed,
    UiGone,
    Quit { immediate: bool },
    Tick { now_ms: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceEffect {
    Spawn(UiStartup),
    Send(UiRequest),
    CancelPicker,
    FinishQuit,
    Notice(String),
}

impl Presence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn apply(&mut self, event: PresenceEvent, now_ms: u64) -> Vec<PresenceEffect> {
        match event {
            PresenceEvent::OpenRequested => self.open_requested(now_ms),
            PresenceEvent::UiConnected { startup } => self.connected(startup),
            PresenceEvent::UiReady => self.ready(),
            PresenceEvent::WindowShown => self.window_shown(),
            PresenceEvent::WindowHidden => self.window_hidden(),
            PresenceEvent::HideRequested => self.hide_requested(),
            PresenceEvent::ShowRequested => self.show_requested(),
            PresenceEvent::PickerRequested { session } => self.picker_requested(session),
            PresenceEvent::PickerDone => self.picker_done(),
            PresenceEvent::PickerFailed => self.picker_failed(),
            PresenceEvent::UiGone => self.ui_gone(now_ms),
            PresenceEvent::Quit { immediate } => self.quit(immediate),
            PresenceEvent::Tick { now_ms } => self.tick(now_ms),
        }
    }

    fn open_requested(&mut self, now_ms: u64) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::NoUi => {
                *self = Presence::Starting {
                    mode: UiStartup::Show,
                    attempts: 0,
                    since_ms: now_ms,
                };
                vec![PresenceEffect::Spawn(UiStartup::Show)]
            }
            Presence::Open { .. } => {
                *self = Presence::Open {
                    visible: true,
                    show_on_ready: false,
                };
                vec![PresenceEffect::Send(UiRequest::ShowWindow)]
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn connected(&mut self, startup: UiStartup) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Starting { .. } => {
                *self = match startup {
                    UiStartup::Show => Presence::Open {
                        visible: false,
                        show_on_ready: true,
                    },
                    UiStartup::Tray => Presence::Open {
                        visible: false,
                        show_on_ready: false,
                    },
                    UiStartup::Picker { session } => Presence::Picker {
                        session,
                        previous_visible: false,
                        ready: false,
                    },
                };
            }
            other => *self = other,
        }
        Vec::new()
    }

    fn ready(&mut self) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Open {
                visible,
                show_on_ready,
            } => {
                if show_on_ready {
                    *self = Presence::Open {
                        visible: true,
                        show_on_ready: false,
                    };
                    vec![PresenceEffect::Send(UiRequest::ShowWindow)]
                } else {
                    *self = Presence::Open {
                        visible,
                        show_on_ready: false,
                    };
                    Vec::new()
                }
            }
            Presence::Picker {
                session,
                previous_visible,
                ready,
            } => {
                *self = Presence::Picker {
                    session,
                    previous_visible,
                    ready: true,
                };
                if ready {
                    Vec::new()
                } else {
                    vec![PresenceEffect::Send(UiRequest::OpenPicker { session })]
                }
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn window_shown(&mut self) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Open { show_on_ready, .. } => {
                *self = Presence::Open {
                    visible: true,
                    show_on_ready,
                };
            }
            other => *self = other,
        }
        Vec::new()
    }

    fn window_hidden(&mut self) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Open { show_on_ready, .. } => {
                *self = Presence::Open {
                    visible: false,
                    show_on_ready,
                };
            }
            other => *self = other,
        }
        Vec::new()
    }

    fn hide_requested(&mut self) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Open { show_on_ready, .. } => {
                *self = Presence::Open {
                    visible: false,
                    show_on_ready,
                };
                vec![PresenceEffect::Send(UiRequest::HideWindow)]
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn show_requested(&mut self) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Open { .. } => {
                *self = Presence::Open {
                    visible: true,
                    show_on_ready: false,
                };
                vec![PresenceEffect::Send(UiRequest::ShowWindow)]
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn picker_requested(&mut self, session: u64) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::NoUi => {
                *self = Presence::Picker {
                    session,
                    previous_visible: false,
                    ready: false,
                };
                vec![PresenceEffect::Spawn(UiStartup::Picker { session })]
            }
            Presence::Open { visible, .. } => {
                *self = Presence::Picker {
                    session,
                    previous_visible: visible,
                    ready: true,
                };
                let mut effects = Vec::new();
                if visible {
                    effects.push(PresenceEffect::Send(UiRequest::HideWindow));
                }
                effects.push(PresenceEffect::Send(UiRequest::OpenPicker { session }));
                effects
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn picker_done(&mut self) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Picker {
                previous_visible, ..
            } => {
                *self = Presence::Open {
                    visible: previous_visible,
                    show_on_ready: false,
                };
                if previous_visible {
                    vec![PresenceEffect::Send(UiRequest::ShowWindow)]
                } else {
                    Vec::new()
                }
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn picker_failed(&mut self) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Picker {
                previous_visible, ..
            } => {
                *self = Presence::Open {
                    visible: previous_visible,
                    show_on_ready: false,
                };
                let mut effects = vec![PresenceEffect::CancelPicker];
                if previous_visible {
                    effects.push(PresenceEffect::Send(UiRequest::ShowWindow));
                }
                effects.push(PresenceEffect::Notice("Picker failed".into()));
                effects
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn ui_gone(&mut self, now_ms: u64) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Starting { mode, attempts, .. } => self.retry_start(mode, attempts, now_ms),
            Presence::Picker { .. } => {
                *self = Presence::NoUi;
                vec![PresenceEffect::CancelPicker]
            }
            Presence::Open { .. } => {
                *self = Presence::NoUi;
                Vec::new()
            }
            Presence::Quitting => {
                *self = Presence::Quitting;
                vec![PresenceEffect::FinishQuit]
            }
            Presence::NoUi => {
                *self = Presence::NoUi;
                Vec::new()
            }
        }
    }

    fn quit(&mut self, immediate: bool) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Quitting => {
                *self = Presence::Quitting;
                Vec::new()
            }
            other => {
                let picker = matches!(&other, Presence::Picker { .. });
                let has_ui = matches!(&other, Presence::Open { .. } | Presence::Picker { .. });
                *self = Presence::Quitting;
                let mut effects = Vec::new();
                if picker {
                    effects.push(PresenceEffect::CancelPicker);
                }
                if immediate || !has_ui {
                    effects.push(PresenceEffect::FinishQuit);
                } else {
                    effects.push(PresenceEffect::Send(UiRequest::Quit));
                }
                effects
            }
        }
    }

    fn tick(&mut self, now_ms: u64) -> Vec<PresenceEffect> {
        match std::mem::take(self) {
            Presence::Starting {
                mode,
                attempts,
                since_ms,
            } if now_ms.saturating_sub(since_ms) >= START_TIMEOUT_MS => {
                self.retry_start(mode, attempts, now_ms)
            }
            other => {
                *self = other;
                Vec::new()
            }
        }
    }

    fn retry_start(&mut self, mode: UiStartup, attempts: u8, now_ms: u64) -> Vec<PresenceEffect> {
        if attempts < MAX_START_ATTEMPTS {
            let next = mode;
            *self = Presence::Starting {
                mode,
                attempts: attempts + 1,
                since_ms: now_ms,
            };
            vec![PresenceEffect::Spawn(next)]
        } else {
            *self = Presence::NoUi;
            vec![PresenceEffect::Notice("UI did not start".into())]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(presence: &mut Presence, now_ms: u64) -> Vec<PresenceEffect> {
        presence.apply(PresenceEvent::OpenRequested, now_ms)
    }

    fn connected(presence: &mut Presence, startup: UiStartup) -> Vec<PresenceEffect> {
        presence.apply(PresenceEvent::UiConnected { startup }, 0)
    }

    fn open_ui(presence: &mut Presence) {
        opened(presence, 100);
        connected(presence, UiStartup::Show);
    }

    #[test]
    fn open_from_no_ui_spawns_exactly_once() {
        let mut presence = Presence::new();
        let effects = opened(&mut presence, 100);
        assert_eq!(
            presence,
            Presence::Starting {
                mode: UiStartup::Show,
                attempts: 0,
                since_ms: 100,
            }
        );
        assert_eq!(effects, vec![PresenceEffect::Spawn(UiStartup::Show)]);
        assert!(opened(&mut presence, 150).is_empty());
        assert_eq!(
            presence,
            Presence::Starting {
                mode: UiStartup::Show,
                attempts: 0,
                since_ms: 100,
            }
        );
    }

    #[test]
    fn open_again_when_open_sends_show() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        let effects = opened(&mut presence, 200);
        assert_eq!(effects, vec![PresenceEffect::Send(UiRequest::ShowWindow)]);
        assert_eq!(
            presence,
            Presence::Open {
                visible: true,
                show_on_ready: false,
            }
        );
    }

    #[test]
    fn show_startup_waits_for_ready() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        assert!(connected(&mut presence, UiStartup::Show).is_empty());
        assert_eq!(
            presence,
            Presence::Open {
                visible: false,
                show_on_ready: true,
            }
        );
    }

    #[test]
    fn tray_startup_stays_hidden() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        connected(&mut presence, UiStartup::Tray);
        assert_eq!(
            presence,
            Presence::Open {
                visible: false,
                show_on_ready: false,
            }
        );
        assert!(presence.apply(PresenceEvent::UiReady, 0).is_empty());
        assert_eq!(
            presence,
            Presence::Open {
                visible: false,
                show_on_ready: false,
            }
        );
    }

    #[test]
    fn picker_startup_enters_picker_without_a_window() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        connected(&mut presence, UiStartup::Picker { session: 9 });
        assert_eq!(
            presence,
            Presence::Picker {
                session: 9,
                previous_visible: false,
                ready: false,
            }
        );
    }

    #[test]
    fn ready_shows_when_requested_and_is_idempotent() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        connected(&mut presence, UiStartup::Show);
        assert_eq!(
            presence.apply(PresenceEvent::UiReady, 0),
            vec![PresenceEffect::Send(UiRequest::ShowWindow)]
        );
        assert_eq!(
            presence,
            Presence::Open {
                visible: true,
                show_on_ready: false,
            }
        );
        assert!(presence.apply(PresenceEvent::UiReady, 0).is_empty());
        assert_eq!(
            presence,
            Presence::Open {
                visible: true,
                show_on_ready: false,
            }
        );
    }

    #[test]
    fn ui_gone_retries_a_starting_ui_twice_then_gives_up() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        assert_eq!(
            presence.apply(PresenceEvent::UiGone, 200),
            vec![PresenceEffect::Spawn(UiStartup::Show)]
        );
        assert_eq!(
            presence,
            Presence::Starting {
                mode: UiStartup::Show,
                attempts: 1,
                since_ms: 200,
            }
        );
        assert_eq!(
            presence.apply(PresenceEvent::UiGone, 300),
            vec![PresenceEffect::Spawn(UiStartup::Show)]
        );
        assert_eq!(
            presence,
            Presence::Starting {
                mode: UiStartup::Show,
                attempts: 2,
                since_ms: 300,
            }
        );
        let effects = presence.apply(PresenceEvent::UiGone, 400);
        assert_eq!(presence, Presence::NoUi);
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], PresenceEffect::Notice(_)));
    }

    #[test]
    fn stalled_start_is_retried_by_tick() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        assert!(presence
            .apply(
                PresenceEvent::Tick {
                    now_ms: 100 + START_TIMEOUT_MS - 1,
                },
                0,
            )
            .is_empty());
        assert_eq!(
            presence.apply(
                PresenceEvent::Tick {
                    now_ms: 100 + START_TIMEOUT_MS,
                },
                0,
            ),
            vec![PresenceEffect::Spawn(UiStartup::Show)]
        );
        assert_eq!(
            presence,
            Presence::Starting {
                mode: UiStartup::Show,
                attempts: 1,
                since_ms: 100 + START_TIMEOUT_MS,
            }
        );
    }

    #[test]
    fn open_window_dying_resets_presence() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        assert!(presence.apply(PresenceEvent::UiGone, 200).is_empty());
        assert_eq!(presence, Presence::NoUi);
        assert_eq!(
            opened(&mut presence, 300),
            vec![PresenceEffect::Spawn(UiStartup::Show)]
        );
    }

    #[test]
    fn window_events_track_visibility() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        presence.apply(PresenceEvent::UiReady, 0);
        assert!(presence.apply(PresenceEvent::WindowHidden, 0).is_empty());
        assert_eq!(
            presence,
            Presence::Open {
                visible: false,
                show_on_ready: false,
            }
        );
        assert!(presence.apply(PresenceEvent::WindowShown, 0).is_empty());
        assert_eq!(
            presence,
            Presence::Open {
                visible: true,
                show_on_ready: false,
            }
        );
    }

    #[test]
    fn hide_and_show_requests_are_forwarded() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        presence.apply(PresenceEvent::UiReady, 0);
        assert_eq!(
            presence.apply(PresenceEvent::HideRequested, 0),
            vec![PresenceEffect::Send(UiRequest::HideWindow)]
        );
        assert_eq!(
            presence,
            Presence::Open {
                visible: false,
                show_on_ready: false,
            }
        );
        assert_eq!(
            presence.apply(PresenceEvent::ShowRequested, 0),
            vec![PresenceEffect::Send(UiRequest::ShowWindow)]
        );
        assert_eq!(
            presence,
            Presence::Open {
                visible: true,
                show_on_ready: false,
            }
        );
        let mut idle = Presence::new();
        assert!(idle.apply(PresenceEvent::HideRequested, 0).is_empty());
        assert!(idle.apply(PresenceEvent::ShowRequested, 0).is_empty());
        assert_eq!(idle, Presence::NoUi);
    }

    #[test]
    fn picker_from_a_visible_window_hides_and_opens_the_picker() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        presence.apply(PresenceEvent::UiReady, 0);
        let effects = presence.apply(PresenceEvent::PickerRequested { session: 4 }, 200);
        assert_eq!(
            presence,
            Presence::Picker {
                session: 4,
                previous_visible: true,
                ready: true,
            }
        );
        assert_eq!(
            effects,
            vec![
                PresenceEffect::Send(UiRequest::HideWindow),
                PresenceEffect::Send(UiRequest::OpenPicker { session: 4 }),
            ]
        );
    }

    #[test]
    fn picker_from_a_hidden_window_only_opens_the_picker() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        connected(&mut presence, UiStartup::Tray);
        let effects = presence.apply(PresenceEvent::PickerRequested { session: 5 }, 200);
        assert_eq!(
            presence,
            Presence::Picker {
                session: 5,
                previous_visible: false,
                ready: true,
            }
        );
        assert_eq!(
            effects,
            vec![PresenceEffect::Send(UiRequest::OpenPicker { session: 5 })]
        );
    }

    #[test]
    fn picker_from_no_ui_spawns_a_picker_process() {
        let mut presence = Presence::new();
        let effects = presence.apply(PresenceEvent::PickerRequested { session: 6 }, 200);
        assert_eq!(
            presence,
            Presence::Picker {
                session: 6,
                previous_visible: false,
                ready: false,
            }
        );
        assert_eq!(
            effects,
            vec![PresenceEffect::Spawn(UiStartup::Picker { session: 6 })]
        );
    }

    #[test]
    fn picker_request_is_ignored_while_starting() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        let before = presence.clone();
        assert!(presence
            .apply(PresenceEvent::PickerRequested { session: 6 }, 200)
            .is_empty());
        assert_eq!(presence, before);
    }

    #[test]
    fn picker_ready_sends_open_picker_once() {
        let mut presence = Presence::new();
        presence.apply(PresenceEvent::PickerRequested { session: 6 }, 200);
        assert_eq!(
            presence.apply(PresenceEvent::UiReady, 0),
            vec![PresenceEffect::Send(UiRequest::OpenPicker { session: 6 })]
        );
        assert!(presence.apply(PresenceEvent::UiReady, 0).is_empty());
        assert_eq!(
            presence,
            Presence::Picker {
                session: 6,
                previous_visible: false,
                ready: true,
            }
        );
    }

    #[test]
    fn picker_done_restores_a_visible_window() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        presence.apply(PresenceEvent::UiReady, 0);
        presence.apply(PresenceEvent::PickerRequested { session: 4 }, 200);
        let effects = presence.apply(PresenceEvent::PickerDone, 300);
        assert_eq!(
            presence,
            Presence::Open {
                visible: true,
                show_on_ready: false,
            }
        );
        assert_eq!(effects, vec![PresenceEffect::Send(UiRequest::ShowWindow)]);
    }

    #[test]
    fn picker_done_keeps_a_hidden_window_hidden() {
        let mut presence = Presence::new();
        opened(&mut presence, 100);
        connected(&mut presence, UiStartup::Tray);
        presence.apply(PresenceEvent::PickerRequested { session: 5 }, 200);
        assert!(presence.apply(PresenceEvent::PickerDone, 300).is_empty());
        assert_eq!(
            presence,
            Presence::Open {
                visible: false,
                show_on_ready: false,
            }
        );
    }

    #[test]
    fn picker_failure_cancels_and_restores_the_window() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        presence.apply(PresenceEvent::UiReady, 0);
        presence.apply(PresenceEvent::PickerRequested { session: 4 }, 200);
        let effects = presence.apply(PresenceEvent::PickerFailed, 300);
        assert_eq!(
            presence,
            Presence::Open {
                visible: true,
                show_on_ready: false,
            }
        );
        assert_eq!(effects[0], PresenceEffect::CancelPicker);
        assert_eq!(effects[1], PresenceEffect::Send(UiRequest::ShowWindow));
        assert!(matches!(&effects[2], PresenceEffect::Notice(_)));
    }

    #[test]
    fn picker_ui_disconnect_cancels_the_session() {
        let mut presence = Presence::new();
        presence.apply(PresenceEvent::PickerRequested { session: 6 }, 200);
        let effects = presence.apply(PresenceEvent::UiGone, 300);
        assert_eq!(presence, Presence::NoUi);
        assert_eq!(effects, vec![PresenceEffect::CancelPicker]);
    }

    #[test]
    fn quit_with_a_live_ui_waits_for_it() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        let effects = presence.apply(PresenceEvent::Quit { immediate: false }, 200);
        assert_eq!(presence, Presence::Quitting);
        assert_eq!(effects, vec![PresenceEffect::Send(UiRequest::Quit)]);
    }

    #[test]
    fn immediate_quit_finishes_without_waiting() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        let effects = presence.apply(PresenceEvent::Quit { immediate: true }, 200);
        assert_eq!(presence, Presence::Quitting);
        assert_eq!(effects, vec![PresenceEffect::FinishQuit]);
    }

    #[test]
    fn quit_without_a_ui_finishes_immediately() {
        let mut presence = Presence::new();
        let effects = presence.apply(PresenceEvent::Quit { immediate: false }, 200);
        assert_eq!(presence, Presence::Quitting);
        assert_eq!(effects, vec![PresenceEffect::FinishQuit]);
        assert!(presence
            .apply(PresenceEvent::Quit { immediate: false }, 300)
            .is_empty());
    }

    #[test]
    fn quit_from_picker_cancels_the_session() {
        let mut presence = Presence::new();
        presence.apply(PresenceEvent::PickerRequested { session: 6 }, 200);
        let effects = presence.apply(PresenceEvent::Quit { immediate: false }, 300);
        assert_eq!(presence, Presence::Quitting);
        assert_eq!(
            effects,
            vec![
                PresenceEffect::CancelPicker,
                PresenceEffect::Send(UiRequest::Quit),
            ]
        );
    }

    #[test]
    fn quit_finishes_when_the_ui_disconnects() {
        let mut presence = Presence::new();
        open_ui(&mut presence);
        presence.apply(PresenceEvent::Quit { immediate: false }, 200);
        let effects = presence.apply(PresenceEvent::UiGone, 300);
        assert_eq!(presence, Presence::Quitting);
        assert_eq!(effects, vec![PresenceEffect::FinishQuit]);
    }

    #[test]
    fn stray_events_in_no_ui_are_ignored() {
        let mut presence = Presence::new();
        for event in [
            PresenceEvent::UiConnected {
                startup: UiStartup::Show,
            },
            PresenceEvent::UiReady,
            PresenceEvent::WindowShown,
            PresenceEvent::WindowHidden,
            PresenceEvent::PickerDone,
            PresenceEvent::PickerFailed,
            PresenceEvent::UiGone,
        ] {
            let before = presence.clone();
            assert!(presence.apply(event, 100).is_empty());
            assert_eq!(presence, before);
        }
    }
}
