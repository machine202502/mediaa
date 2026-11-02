use std::time::{Duration, Instant};

pub const ACK_TIMEOUT: Duration = Duration::from_millis(700);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Absent,
    Starting,
    Opening,
    Open,
    Hiding,
    Hidden,
    Quitting,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    Show,
    Hide,
    Quit { immediate: bool },
    UiConnected,
    UiDisconnected,
    WindowShown,
    WindowHidden,
    Tick,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEffect {
    SpawnVisible,
    SendShow,
    SendHide,
    SendQuit,
    RestartVisible,
    FinishQuit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rejected {
    pub state: TrayState,
    pub event: TrayEvent,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transition {
    pub state: TrayState,
    pub effects: Vec<TrayEffect>,
}

#[derive(Debug)]
pub struct TrayMachine {
    state: TrayState,
    connected: bool,
    deadline: Option<Instant>,
    retried: bool,
}

impl Default for TrayMachine {
    fn default() -> Self {
        Self {
            state: TrayState::Absent,
            connected: false,
            deadline: None,
            retried: false,
        }
    }
}

impl TrayMachine {
    pub fn state(&self) -> TrayState {
        self.state
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    pub fn on(&mut self, event: TrayEvent, now: Instant) -> Result<Transition, Rejected> {
        match (self.state, event) {
            (TrayState::Quitting, TrayEvent::Show) => Err(self.rejected(event)),
            (TrayState::Quitting, TrayEvent::Hide) => Err(self.rejected(event)),
            (TrayState::Quitting, TrayEvent::UiConnected) => Ok(self.noop()),
            (TrayState::Quitting, _) => Ok(self.noop()),
            (_, TrayEvent::Quit { immediate }) => {
                self.state = TrayState::Quitting;
                self.deadline = None;
                let effect = if immediate {
                    TrayEffect::FinishQuit
                } else {
                    TrayEffect::SendQuit
                };
                Ok(self.transition(vec![effect]))
            }
            (TrayState::Absent, TrayEvent::Show) => {
                self.begin_starting(now);
                Ok(self.transition(vec![TrayEffect::SpawnVisible]))
            }
            (TrayState::Starting, TrayEvent::Show) => {
                if self.connected {
                    self.state = TrayState::Opening;
                    self.deadline = Some(now + ACK_TIMEOUT);
                    return Ok(self.transition(vec![TrayEffect::SendShow]));
                }
                self.begin_starting(now);
                Ok(self.noop())
            }
            (TrayState::Opening, TrayEvent::Show) => {
                self.deadline = Some(now + ACK_TIMEOUT);
                if self.connected {
                    return Ok(self.transition(vec![TrayEffect::SendShow]));
                }
                Ok(self.noop())
            }
            (TrayState::Open, TrayEvent::Show) => {
                self.state = TrayState::Opening;
                self.deadline = Some(now + ACK_TIMEOUT);
                Ok(self.transition(vec![TrayEffect::SendShow]))
            }
            (TrayState::Hidden, TrayEvent::Show) => {
                self.state = TrayState::Opening;
                self.deadline = Some(now + ACK_TIMEOUT);
                Ok(self.transition(vec![TrayEffect::SendShow]))
            }
            (TrayState::Hiding, TrayEvent::Show) => {
                self.state = TrayState::Opening;
                self.deadline = Some(now + ACK_TIMEOUT);
                Ok(self.transition(vec![TrayEffect::SendShow]))
            }
            (TrayState::Open, TrayEvent::Hide) => {
                self.state = TrayState::Hiding;
                Ok(self.transition(vec![TrayEffect::SendHide]))
            }
            (TrayState::Hiding, TrayEvent::Hide) | (TrayState::Hidden, TrayEvent::Hide) => {
                Ok(self.noop())
            }
            (TrayState::Absent | TrayState::Starting, TrayEvent::Hide) => Err(self.rejected(event)),
            (_, TrayEvent::UiConnected) => {
                self.connected = true;
                if self.state == TrayState::Absent {
                    self.state = TrayState::Open;
                    self.deadline = None;
                } else if self.state == TrayState::Starting {
                    self.state = TrayState::Opening;
                }
                Ok(self.noop())
            }
            (_, TrayEvent::UiDisconnected) => {
                self.connected = false;
                if self.state == TrayState::Opening && !self.retried {
                    self.retried = true;
                    self.state = TrayState::Starting;
                    self.deadline = Some(now + ACK_TIMEOUT);
                    return Ok(self.transition(vec![TrayEffect::RestartVisible]));
                }
                self.state = TrayState::Absent;
                self.deadline = None;
                Ok(self.noop())
            }
            (TrayState::Opening, TrayEvent::WindowShown) => {
                self.state = TrayState::Open;
                self.deadline = None;
                Ok(self.noop())
            }
            (TrayState::Hiding, TrayEvent::WindowHidden) => {
                self.state = TrayState::Hidden;
                Ok(self.noop())
            }
            (TrayState::Open, TrayEvent::WindowHidden) => {
                self.state = TrayState::Hidden;
                Ok(self.noop())
            }
            (TrayState::Hidden, TrayEvent::WindowShown) => {
                self.state = TrayState::Open;
                Ok(self.noop())
            }
            (TrayState::Open, TrayEvent::WindowShown) => Ok(self.noop()),
            (TrayState::Opening | TrayState::Hiding, TrayEvent::Tick) => {
                if self.deadline.is_some_and(|deadline| now >= deadline) && !self.retried {
                    self.retried = true;
                    self.connected = false;
                    self.state = TrayState::Starting;
                    self.deadline = Some(now + ACK_TIMEOUT);
                    return Ok(self.transition(vec![TrayEffect::RestartVisible]));
                }
                Ok(self.noop())
            }
            (TrayState::Starting, TrayEvent::Tick) => {
                if self.deadline.is_some_and(|deadline| now >= deadline) && !self.retried {
                    self.retried = true;
                    self.deadline = None;
                    return Ok(self.transition(vec![TrayEffect::RestartVisible]));
                }
                Ok(self.noop())
            }
            (TrayState::Absent, TrayEvent::Tick)
            | (TrayState::Open, TrayEvent::Tick)
            | (TrayState::Hidden, TrayEvent::Tick) => Ok(self.noop()),
            (TrayState::Hidden, TrayEvent::WindowHidden) => Ok(self.noop()),
            (state, event) => Err(Rejected { state, event }),
        }
    }

    fn begin_starting(&mut self, now: Instant) {
        self.state = TrayState::Starting;
        self.connected = false;
        self.retried = false;
        self.deadline = Some(now + ACK_TIMEOUT);
    }

    fn transition(&self, effects: Vec<TrayEffect>) -> Transition {
        Transition {
            state: self.state,
            effects,
        }
    }

    fn noop(&self) -> Transition {
        self.transition(Vec::new())
    }

    fn rejected(&self, event: TrayEvent) -> Rejected {
        Rejected {
            state: self.state,
            event,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> (TrayMachine, Instant) {
        (TrayMachine::default(), Instant::now())
    }

    #[test]
    fn first_show_starts_one_visible_ui() {
        let (mut machine, now) = base();
        let transition = machine.on(TrayEvent::Show, now).unwrap();
        assert_eq!(transition.state, TrayState::Starting);
        assert_eq!(transition.effects, vec![TrayEffect::SpawnVisible]);
    }

    #[test]
    fn repeated_show_does_not_spawn_again() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        let transition = machine.on(TrayEvent::Show, now).unwrap();
        assert_eq!(transition.effects, Vec::new());
        assert_eq!(transition.state, TrayState::Starting);
    }

    #[test]
    fn connection_and_ack_open_the_window() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        let transition = machine.on(TrayEvent::WindowShown, now).unwrap();
        assert_eq!(transition.state, TrayState::Open);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn hide_waits_for_the_window_ack() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        machine.on(TrayEvent::WindowShown, now).unwrap();
        let transition = machine.on(TrayEvent::Hide, now).unwrap();
        assert_eq!(transition.state, TrayState::Hiding);
        assert_eq!(transition.effects, vec![TrayEffect::SendHide]);
        assert_eq!(
            machine.on(TrayEvent::WindowHidden, now).unwrap().state,
            TrayState::Hidden
        );
    }

    #[test]
    fn show_from_hidden_sends_show() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        machine.on(TrayEvent::WindowShown, now).unwrap();
        machine.on(TrayEvent::Hide, now).unwrap();
        machine.on(TrayEvent::WindowHidden, now).unwrap();
        let transition = machine.on(TrayEvent::Show, now).unwrap();
        assert_eq!(transition.state, TrayState::Opening);
        assert_eq!(transition.effects, vec![TrayEffect::SendShow]);
    }

    #[test]
    fn ui_death_while_opening_restarts_once() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        let transition = machine.on(TrayEvent::UiDisconnected, now).unwrap();
        assert_eq!(transition.state, TrayState::Starting);
        assert_eq!(transition.effects, vec![TrayEffect::RestartVisible]);
        let again = machine.on(TrayEvent::UiDisconnected, now).unwrap();
        assert_eq!(again.state, TrayState::Absent);
        assert!(again.effects.is_empty());
    }

    #[test]
    fn lost_ack_restarts_once() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        assert!(machine
            .on(
                TrayEvent::Tick,
                now + ACK_TIMEOUT - Duration::from_millis(1)
            )
            .unwrap()
            .effects
            .is_empty());
        let transition = machine.on(TrayEvent::Tick, now + ACK_TIMEOUT * 2).unwrap();
        assert_eq!(transition.effects, vec![TrayEffect::RestartVisible]);
        assert!(machine
            .on(TrayEvent::Tick, now + ACK_TIMEOUT * 3)
            .unwrap()
            .effects
            .is_empty());
    }

    #[test]
    fn immediate_quit_finishes_and_is_terminal() {
        let (mut machine, now) = base();
        let transition = machine
            .on(TrayEvent::Quit { immediate: true }, now)
            .unwrap();
        assert_eq!(transition.state, TrayState::Quitting);
        assert_eq!(transition.effects, vec![TrayEffect::FinishQuit]);
        assert!(machine
            .on(TrayEvent::Quit { immediate: true }, now)
            .unwrap()
            .effects
            .is_empty());
        assert!(machine.on(TrayEvent::Show, now).is_err());
    }

    #[test]
    fn delayed_quit_sends_quit() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        machine.on(TrayEvent::WindowShown, now).unwrap();
        let transition = machine
            .on(TrayEvent::Quit { immediate: false }, now)
            .unwrap();
        assert_eq!(transition.effects, vec![TrayEffect::SendQuit]);
    }

    #[test]
    fn show_when_open_sends_show_again() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        machine.on(TrayEvent::WindowShown, now).unwrap();
        let transition = machine.on(TrayEvent::Show, now).unwrap();
        assert_eq!(transition.state, TrayState::Opening);
        assert_eq!(transition.effects, vec![TrayEffect::SendShow]);
    }

    #[test]
    fn a_connected_ui_disconnect_returns_to_absent() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        machine.on(TrayEvent::WindowShown, now).unwrap();
        let transition = machine.on(TrayEvent::UiDisconnected, now).unwrap();
        assert_eq!(transition.state, TrayState::Absent);
        assert!(transition.effects.is_empty());
    }

    #[test]
    fn window_hidden_moves_open_to_hidden() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        machine.on(TrayEvent::WindowShown, now).unwrap();
        assert_eq!(
            machine.on(TrayEvent::WindowHidden, now).unwrap().state,
            TrayState::Hidden
        );
        assert_eq!(
            machine.on(TrayEvent::Tick, now).unwrap().state,
            TrayState::Hidden
        );
    }

    #[test]
    fn quit_from_hidden_finishes_immediately() {
        let (mut machine, now) = base();
        machine.on(TrayEvent::Show, now).unwrap();
        machine.on(TrayEvent::UiConnected, now).unwrap();
        machine.on(TrayEvent::WindowShown, now).unwrap();
        machine.on(TrayEvent::Hide, now).unwrap();
        machine.on(TrayEvent::WindowHidden, now).unwrap();
        let transition = machine
            .on(TrayEvent::Quit { immediate: true }, now)
            .unwrap();
        assert_eq!(transition.state, TrayState::Quitting);
        assert_eq!(transition.effects, vec![TrayEffect::FinishQuit]);
    }

    #[test]
    fn hide_without_a_window_is_rejected() {
        let (mut machine, now) = base();
        assert!(machine.on(TrayEvent::Hide, now).is_err());
    }
}
