use mediaa2_core::screen::MonitorInfo;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Default)]
pub enum Picker {
    #[default]
    Idle,
    Capturing {
        session: u64,
        monitor: MonitorInfo,
    },
    Selecting {
        session: u64,
        image: PathBuf,
        monitor: MonitorInfo,
        scale: f32,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PickerEvent {
    Start {
        session: u64,
        monitor: MonitorInfo,
    },
    Captured {
        session: u64,
        image: PathBuf,
        scale: f32,
    },
    CaptureFailed {
        session: u64,
        message: String,
    },
    MonitorRequested {
        session: u64,
        monitor: MonitorInfo,
    },
    Committed {
        session: u64,
    },
    Cancelled {
        session: u64,
    },
    Gone {
        session: u64,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum PickerEffect {
    CaptureMonitor { session: u64, monitor: MonitorInfo },
    SpawnPicker { session: u64 },
    OpenPicker { session: u64 },
    Commit { session: u64 },
    Cancel,
    Notice(String),
}

impl Picker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn session(&self) -> Option<u64> {
        match self {
            Picker::Idle => None,
            Picker::Capturing { session, .. } | Picker::Selecting { session, .. } => Some(*session),
        }
    }

    pub fn apply(&mut self, event: PickerEvent) -> Vec<PickerEffect> {
        match event {
            PickerEvent::Start { session, monitor } => {
                if !matches!(&self, Picker::Idle) {
                    return Vec::new();
                }
                *self = Picker::Capturing {
                    session,
                    monitor: monitor.clone(),
                };
                vec![PickerEffect::CaptureMonitor { session, monitor }]
            }
            PickerEvent::Captured {
                session,
                image,
                scale,
            } => {
                let Picker::Capturing {
                    session: active,
                    monitor,
                } = &self
                else {
                    return Vec::new();
                };
                if *active != session {
                    return Vec::new();
                }
                *self = Picker::Selecting {
                    session,
                    image,
                    monitor: monitor.clone(),
                    scale,
                };
                vec![PickerEffect::SpawnPicker { session }]
            }
            PickerEvent::CaptureFailed { session, message } => {
                if self.session() != Some(session) {
                    return Vec::new();
                }
                *self = Picker::Idle;
                vec![PickerEffect::Notice(message)]
            }
            PickerEvent::MonitorRequested { session, monitor } => {
                let Picker::Selecting {
                    session: active, ..
                } = &self
                else {
                    return Vec::new();
                };
                if *active != session {
                    return Vec::new();
                }
                *self = Picker::Capturing {
                    session,
                    monitor: monitor.clone(),
                };
                vec![PickerEffect::CaptureMonitor { session, monitor }]
            }
            PickerEvent::Committed { session } => {
                let Picker::Selecting {
                    session: active, ..
                } = &self
                else {
                    return Vec::new();
                };
                if *active != session {
                    return Vec::new();
                }
                *self = Picker::Idle;
                vec![PickerEffect::Commit { session }]
            }
            PickerEvent::Cancelled { session } | PickerEvent::Gone { session } => {
                if self.session() != Some(session) {
                    return Vec::new();
                }
                *self = Picker::Idle;
                vec![PickerEffect::Cancel]
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::screen::MonitorId;

    fn monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(1),
            device: r"\\.\DISPLAY1".into(),
            index: 0,
            name: "DELL U2720Q".into(),
            x: 0,
            y: 0,
            width: 3840,
            height: 2160,
            scale: 1.5,
            primary: true,
        }
    }

    fn second_monitor() -> MonitorInfo {
        MonitorInfo {
            id: MonitorId::new(2),
            device: r"\\.\DISPLAY2".into(),
            index: 1,
            name: "LG".into(),
            x: 3840,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
            primary: false,
        }
    }

    fn start(picker: &mut Picker, session: u64) {
        picker.apply(PickerEvent::Start {
            session,
            monitor: monitor(),
        });
    }

    fn selecting(picker: &mut Picker, session: u64) {
        start(picker, session);
        picker.apply(PickerEvent::Captured {
            session,
            image: "frame.png".into(),
            scale: 1.5,
        });
    }

    #[test]
    fn start_captures_the_monitor() {
        let mut picker = Picker::new();
        let effects = picker.apply(PickerEvent::Start {
            session: 3,
            monitor: monitor(),
        });
        assert_eq!(
            picker,
            Picker::Capturing {
                session: 3,
                monitor: monitor(),
            }
        );
        assert_eq!(
            effects,
            vec![PickerEffect::CaptureMonitor {
                session: 3,
                monitor: monitor(),
            }]
        );
    }

    #[test]
    fn start_is_rejected_while_busy() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let before = picker.clone();
        let effects = picker.apply(PickerEvent::Start {
            session: 4,
            monitor: monitor(),
        });
        assert!(effects.is_empty());
        assert_eq!(picker, before);
    }

    #[test]
    fn captured_moves_to_selecting_and_spawns_the_picker() {
        let mut picker = Picker::new();
        start(&mut picker, 3);
        let effects = picker.apply(PickerEvent::Captured {
            session: 3,
            image: "frame.png".into(),
            scale: 1.25,
        });
        assert_eq!(
            picker,
            Picker::Selecting {
                session: 3,
                image: "frame.png".into(),
                monitor: monitor(),
                scale: 1.25,
            }
        );
        assert_eq!(effects, vec![PickerEffect::SpawnPicker { session: 3 }]);
    }

    #[test]
    fn captured_with_a_stale_session_is_ignored() {
        let mut picker = Picker::new();
        start(&mut picker, 3);
        let before = picker.clone();
        assert!(picker
            .apply(PickerEvent::Captured {
                session: 4,
                image: "frame.png".into(),
                scale: 1.0,
            })
            .is_empty());
        assert_eq!(picker, before);
    }

    #[test]
    fn capture_failure_releases_the_session() {
        let mut picker = Picker::new();
        start(&mut picker, 3);
        let effects = picker.apply(PickerEvent::CaptureFailed {
            session: 3,
            message: "no access".into(),
        });
        assert_eq!(picker, Picker::Idle);
        assert_eq!(effects, vec![PickerEffect::Notice("no access".into())]);
    }

    #[test]
    fn capture_failure_while_selecting_releases_the_session() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let effects = picker.apply(PickerEvent::CaptureFailed {
            session: 3,
            message: "no access".into(),
        });
        assert_eq!(picker, Picker::Idle);
        assert_eq!(effects, vec![PickerEffect::Notice("no access".into())]);
    }

    #[test]
    fn capture_failure_with_a_stale_session_is_ignored() {
        let mut picker = Picker::new();
        start(&mut picker, 3);
        let before = picker.clone();
        assert!(picker
            .apply(PickerEvent::CaptureFailed {
                session: 4,
                message: "no access".into(),
            })
            .is_empty());
        assert_eq!(picker, before);
    }

    #[test]
    fn monitor_request_recaptures_and_keeps_the_session() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let effects = picker.apply(PickerEvent::MonitorRequested {
            session: 3,
            monitor: second_monitor(),
        });
        assert_eq!(
            picker,
            Picker::Capturing {
                session: 3,
                monitor: second_monitor(),
            }
        );
        assert_eq!(
            effects,
            vec![PickerEffect::CaptureMonitor {
                session: 3,
                monitor: second_monitor(),
            }]
        );
        let effects = picker.apply(PickerEvent::Captured {
            session: 3,
            image: "second.png".into(),
            scale: 1.0,
        });
        assert_eq!(effects, vec![PickerEffect::SpawnPicker { session: 3 }]);
        assert!(matches!(
            &picker,
            Picker::Selecting { monitor, .. } if *monitor == second_monitor()
        ));
    }

    #[test]
    fn monitor_request_with_a_stale_session_is_ignored() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let before = picker.clone();
        assert!(picker
            .apply(PickerEvent::MonitorRequested {
                session: 4,
                monitor: second_monitor(),
            })
            .is_empty());
        assert_eq!(picker, before);
    }

    #[test]
    fn monitor_request_before_selecting_is_ignored() {
        let mut picker = Picker::new();
        start(&mut picker, 3);
        let before = picker.clone();
        assert!(picker
            .apply(PickerEvent::MonitorRequested {
                session: 3,
                monitor: second_monitor(),
            })
            .is_empty());
        assert_eq!(picker, before);
    }

    #[test]
    fn commit_releases_the_session() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let effects = picker.apply(PickerEvent::Committed { session: 3 });
        assert_eq!(picker, Picker::Idle);
        assert_eq!(effects, vec![PickerEffect::Commit { session: 3 }]);
    }

    #[test]
    fn commit_with_a_stale_session_is_ignored() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let before = picker.clone();
        assert!(picker
            .apply(PickerEvent::Committed { session: 4 })
            .is_empty());
        assert_eq!(picker, before);
    }

    #[test]
    fn cancelling_releases_the_session() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let effects = picker.apply(PickerEvent::Cancelled { session: 3 });
        assert_eq!(picker, Picker::Idle);
        assert_eq!(effects, vec![PickerEffect::Cancel]);
    }

    #[test]
    fn a_gone_ui_releases_the_session() {
        let mut picker = Picker::new();
        start(&mut picker, 3);
        let effects = picker.apply(PickerEvent::Gone { session: 3 });
        assert_eq!(picker, Picker::Idle);
        assert_eq!(effects, vec![PickerEffect::Cancel]);
    }

    #[test]
    fn cancel_from_a_stale_session_is_ignored() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        let before = picker.clone();
        assert!(picker
            .apply(PickerEvent::Cancelled { session: 4 })
            .is_empty());
        assert_eq!(picker, before);
    }

    #[test]
    fn a_new_session_can_start_after_the_previous_one_ends() {
        let mut picker = Picker::new();
        selecting(&mut picker, 3);
        picker.apply(PickerEvent::Committed { session: 3 });
        let effects = picker.apply(PickerEvent::Start {
            session: 4,
            monitor: second_monitor(),
        });
        assert_eq!(
            effects,
            vec![PickerEffect::CaptureMonitor {
                session: 4,
                monitor: second_monitor(),
            }]
        );
        assert_eq!(picker.session(), Some(4));
    }

    #[test]
    fn events_in_idle_are_noops() {
        let mut picker = Picker::new();
        for event in [
            PickerEvent::Captured {
                session: 1,
                image: "x".into(),
                scale: 1.0,
            },
            PickerEvent::CaptureFailed {
                session: 1,
                message: "x".into(),
            },
            PickerEvent::MonitorRequested {
                session: 1,
                monitor: second_monitor(),
            },
            PickerEvent::Committed { session: 1 },
            PickerEvent::Cancelled { session: 1 },
            PickerEvent::Gone { session: 1 },
        ] {
            let before = picker.clone();
            assert!(picker.apply(event).is_empty());
            assert_eq!(picker, before);
        }
    }
}
