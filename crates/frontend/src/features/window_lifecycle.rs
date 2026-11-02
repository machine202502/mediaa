use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowState {
    Visible,
    Hidden {
        since: Instant,
    },
    Quitting {
        quit_daemon: bool,
        wait_for_idle: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEvent {
    Show,
    Hide,
    CloseRequested { busy: bool, close_to_tray: bool },
    Quit { busy: bool },
    Tick { busy: bool, idle_after: Duration },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowEffect {
    ShowWindow,
    HideWindow,
    SendShowAck,
    SendHideAck,
    CancelClose,
    CloseWindow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowTransition {
    pub state: WindowState,
    pub effects: Vec<WindowEffect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowRejected {
    pub state: WindowState,
    pub event: WindowEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowLifecycle {
    state: WindowState,
}

impl WindowLifecycle {
    pub fn new(hidden: bool, now: Instant) -> Self {
        Self {
            state: if hidden {
                WindowState::Hidden { since: now }
            } else {
                WindowState::Visible
            },
        }
    }

    pub fn is_hidden(&self) -> bool {
        matches!(self.state, WindowState::Hidden { .. })
    }

    pub fn is_quitting(&self) -> bool {
        matches!(self.state, WindowState::Quitting { .. })
    }

    pub fn quit_daemon(&self) -> bool {
        matches!(
            self.state,
            WindowState::Quitting {
                quit_daemon: true,
                ..
            }
        )
    }

    pub fn on(
        &mut self,
        event: WindowEvent,
        now: Instant,
    ) -> Result<WindowTransition, WindowRejected> {
        match event {
            WindowEvent::Show => {
                if self.is_quitting() {
                    return Err(WindowRejected {
                        state: self.state,
                        event,
                    });
                }
                self.state = WindowState::Visible;
                Ok(self.transition(vec![WindowEffect::ShowWindow, WindowEffect::SendShowAck]))
            }
            WindowEvent::Hide => {
                if self.is_quitting() {
                    return Err(WindowRejected {
                        state: self.state,
                        event,
                    });
                }
                if let WindowState::Hidden { .. } = self.state {
                    return Ok(self.noop());
                }
                self.state = WindowState::Hidden { since: now };
                Ok(self.transition(vec![WindowEffect::HideWindow, WindowEffect::SendHideAck]))
            }
            WindowEvent::CloseRequested {
                busy,
                close_to_tray,
            } => {
                if self.is_quitting() {
                    return Ok(self.noop());
                }
                if close_to_tray && busy {
                    self.state = WindowState::Hidden { since: now };
                    return Ok(self.transition(vec![
                        WindowEffect::CancelClose,
                        WindowEffect::HideWindow,
                        WindowEffect::SendHideAck,
                    ]));
                }
                self.state = WindowState::Quitting {
                    quit_daemon: !close_to_tray,
                    wait_for_idle: false,
                };
                Ok(self.transition(vec![WindowEffect::CloseWindow]))
            }
            WindowEvent::Quit { busy } => {
                if self.is_quitting() {
                    return Ok(self.noop());
                }
                self.state = WindowState::Quitting {
                    quit_daemon: true,
                    wait_for_idle: busy,
                };
                let effects = if busy {
                    Vec::new()
                } else {
                    vec![WindowEffect::CloseWindow]
                };
                Ok(self.transition(effects))
            }
            WindowEvent::Tick { busy, idle_after } => {
                if let WindowState::Quitting { wait_for_idle, .. } = self.state {
                    if wait_for_idle && !busy {
                        self.state = WindowState::Quitting {
                            quit_daemon: self.quit_daemon(),
                            wait_for_idle: false,
                        };
                        return Ok(self.transition(vec![WindowEffect::CloseWindow]));
                    }
                    return Ok(self.noop());
                }
                if let WindowState::Hidden { since } = self.state {
                    if !busy && now.duration_since(since) >= idle_after {
                        self.state = WindowState::Quitting {
                            quit_daemon: false,
                            wait_for_idle: false,
                        };
                        return Ok(self.transition(vec![WindowEffect::CloseWindow]));
                    }
                }
                Ok(self.noop())
            }
        }
    }

    fn transition(&self, effects: Vec<WindowEffect>) -> WindowTransition {
        WindowTransition {
            state: self.state,
            effects,
        }
    }

    fn noop(&self) -> WindowTransition {
        self.transition(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn show_moves_hidden_to_visible_and_emits_ack() {
        let now = now();
        let mut window = WindowLifecycle::new(true, now);
        let transition = window.on(WindowEvent::Show, now).unwrap();
        assert_eq!(transition.state, WindowState::Visible);
        assert_eq!(
            transition.effects,
            vec![WindowEffect::ShowWindow, WindowEffect::SendShowAck]
        );
    }

    #[test]
    fn hide_moves_visible_to_hidden_and_emits_ack() {
        let now = now();
        let mut window = WindowLifecycle::new(false, now);
        let transition = window.on(WindowEvent::Hide, now).unwrap();
        assert_eq!(transition.state, WindowState::Hidden { since: now });
        assert_eq!(
            transition.effects,
            vec![WindowEffect::HideWindow, WindowEffect::SendHideAck]
        );
    }

    #[test]
    fn close_to_tray_with_work_hides_and_cancels_close() {
        let now = now();
        let mut window = WindowLifecycle::new(false, now);
        let transition = window
            .on(
                WindowEvent::CloseRequested {
                    busy: true,
                    close_to_tray: true,
                },
                now,
            )
            .unwrap();
        assert!(matches!(transition.state, WindowState::Hidden { .. }));
        assert_eq!(
            transition.effects,
            vec![
                WindowEffect::CancelClose,
                WindowEffect::HideWindow,
                WindowEffect::SendHideAck
            ]
        );
    }

    #[test]
    fn close_without_tray_quits_the_daemon() {
        let now = now();
        let mut window = WindowLifecycle::new(false, now);
        let transition = window
            .on(
                WindowEvent::CloseRequested {
                    busy: false,
                    close_to_tray: false,
                },
                now,
            )
            .unwrap();
        assert!(window.is_quitting());
        assert!(window.quit_daemon());
        assert_eq!(transition.effects, vec![WindowEffect::CloseWindow]);
    }

    #[test]
    fn close_to_tray_without_work_leaves_daemon_running() {
        let now = now();
        let mut window = WindowLifecycle::new(false, now);
        let transition = window
            .on(
                WindowEvent::CloseRequested {
                    busy: false,
                    close_to_tray: true,
                },
                now,
            )
            .unwrap();
        assert!(window.is_quitting());
        assert!(!window.quit_daemon());
        assert_eq!(transition.effects, vec![WindowEffect::CloseWindow]);
    }

    #[test]
    fn hidden_window_closes_after_the_idle_timeout() {
        let now = now();
        let mut window = WindowLifecycle::new(true, now);
        assert!(window
            .on(
                WindowEvent::Tick {
                    busy: false,
                    idle_after: Duration::from_secs(3),
                },
                now + Duration::from_secs(2)
            )
            .unwrap()
            .effects
            .is_empty());
        let transition = window
            .on(
                WindowEvent::Tick {
                    busy: false,
                    idle_after: Duration::from_secs(3),
                },
                now + Duration::from_secs(3),
            )
            .unwrap();
        assert_eq!(transition.effects, vec![WindowEffect::CloseWindow]);
        assert!(!window.quit_daemon());
    }

    #[test]
    fn show_cancels_the_hidden_timeout() {
        let now = now();
        let mut window = WindowLifecycle::new(true, now);
        window.on(WindowEvent::Show, now).unwrap();
        let transition = window
            .on(
                WindowEvent::Tick {
                    busy: false,
                    idle_after: Duration::from_secs(0),
                },
                now,
            )
            .unwrap();
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn quit_waits_for_work_and_closes_once() {
        let now = now();
        let mut window = WindowLifecycle::new(false, now);
        assert!(window
            .on(WindowEvent::Quit { busy: true }, now)
            .unwrap()
            .effects
            .is_empty());
        let transition = window
            .on(
                WindowEvent::Tick {
                    busy: false,
                    idle_after: Duration::from_secs(0),
                },
                now,
            )
            .unwrap();
        assert_eq!(transition.effects, vec![WindowEffect::CloseWindow]);
        assert!(window.is_quitting());
        assert!(window.quit_daemon());
    }

    #[test]
    fn duplicate_quit_does_not_close_twice() {
        let now = now();
        let mut window = WindowLifecycle::new(false, now);
        window.on(WindowEvent::Quit { busy: false }, now).unwrap();
        let transition = window.on(WindowEvent::Quit { busy: false }, now).unwrap();
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn show_after_quit_is_rejected() {
        let now = now();
        let mut window = WindowLifecycle::new(false, now);
        window.on(WindowEvent::Quit { busy: false }, now).unwrap();
        assert!(window.on(WindowEvent::Show, now).is_err());
    }
}
