use mediaa2_protocol::state::State;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;

pub struct StateStore {
    inner: Mutex<State>,
    tx: watch::Sender<Arc<State>>,
}

impl StateStore {
    pub fn new(initial: State) -> Self {
        let current = Arc::new(initial.clone());
        let (tx, _) = watch::channel(current);
        Self {
            inner: Mutex::new(initial),
            tx,
        }
    }

    pub fn snapshot(&self) -> Arc<State> {
        Arc::new(
            self.inner
                .lock()
                .expect("state store lock poisoned")
                .clone(),
        )
    }

    pub fn update(&self, change: impl FnOnce(&mut State)) -> Arc<State> {
        let next = {
            let mut state = self.inner.lock().expect("state store lock poisoned");
            change(&mut state);
            Arc::new(state.clone())
        };
        self.tx.send_replace(Arc::clone(&next));
        next
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<State>> {
        self.tx.subscribe()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::formats::{OutputKind, VideoContainer};
    use mediaa2_core::geometry::Rect;
    use mediaa2_core::recording::RecordTarget;
    use mediaa2_core::screen::{MonitorId, MonitorInfo};
    use mediaa2_core::settings::Settings;
    use mediaa2_protocol::state::{ConversionState, RecordingState, RecoveryState};

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

    fn state() -> State {
        State {
            settings: Settings::default(),
            recording: RecordingState::Idle,
            conversion: ConversionState::Idle,
            recovery: RecoveryState { running: false },
            notices: Vec::new(),
            picker: None,
            library_revision: 0,
        }
    }

    fn active() -> RecordingState {
        RecordingState::Active {
            job: 7,
            target: RecordTarget::Region {
                monitor: monitor(),
                region: Rect::new(10, 20, 300, 200),
            },
            started_ms: 1_000,
            paused: false,
            elapsed_ms: 500,
            sources: vec!["DELL U2720Q".into()],
            has_audio: true,
            output: OutputKind::Video(VideoContainer::Mkv),
        }
    }

    #[test]
    fn snapshot_returns_the_initial_state() {
        let store = StateStore::new(state());
        assert_eq!(*store.snapshot(), state());
    }

    #[test]
    fn update_changes_the_snapshot_and_returns_it() {
        let store = StateStore::new(state());
        let next = store.update(|state| {
            state.recording = active();
            state.library_revision = 1;
        });
        assert_eq!(next.library_revision, 1);
        assert_eq!(next.recording, active());
        assert_eq!(*store.snapshot(), *next);
    }

    #[test]
    fn updates_without_subscribers_keep_the_latest_state() {
        let store = StateStore::new(state());
        store.update(|state| state.library_revision = 1);
        store.update(|state| state.library_revision = 2);
        assert_eq!(store.snapshot().library_revision, 2);
        let receiver = store.subscribe();
        assert_eq!(receiver.borrow().library_revision, 2);
    }

    #[test]
    fn subscribers_see_initial_and_later_states() {
        let store = StateStore::new(state());
        let mut receiver = store.subscribe();
        assert_eq!(receiver.borrow_and_update().library_revision, 0);
        store.update(|state| state.library_revision = 1);
        assert!(receiver.has_changed().unwrap());
        assert_eq!(receiver.borrow_and_update().library_revision, 1);
        assert!(!receiver.has_changed().unwrap());
        store.update(|state| state.library_revision = 3);
        assert_eq!(receiver.borrow_and_update().library_revision, 3);
    }

    #[test]
    fn every_subscriber_sees_the_same_update() {
        let store = StateStore::new(state());
        let mut first = store.subscribe();
        let mut second = store.subscribe();
        let published = store.update(|state| {
            state.recording = active();
            state.library_revision = 4;
        });
        assert!(first.has_changed().unwrap());
        assert!(second.has_changed().unwrap());
        assert_eq!(*first.borrow_and_update(), published);
        assert_eq!(*second.borrow_and_update(), published);
    }

    #[test]
    fn snapshot_is_independent_of_later_updates() {
        let store = StateStore::new(state());
        let before = store.snapshot();
        store.update(|state| state.library_revision = 9);
        assert_eq!(before.library_revision, 0);
        assert_eq!(store.snapshot().library_revision, 9);
    }
}
