use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Inner {
    beats: AtomicU64,
    model: Mutex<Option<String>>,
}

#[derive(Clone, Default)]
pub struct Pulse(Arc<Inner>);

thread_local! {
    static CURRENT: RefCell<Option<Pulse>> = const { RefCell::new(None) };
}

impl Pulse {
    pub fn new() -> Pulse {
        Pulse::default()
    }

    pub fn attach(&self) -> Attached {
        let previous = CURRENT.with(|current| current.borrow_mut().replace(self.clone()));
        Attached {
            previous,
            _thread: PhantomData,
        }
    }

    pub fn beats(&self) -> u64 {
        self.0.beats.load(Ordering::Relaxed)
    }

    pub fn beat(&self) {
        self.0.beats.fetch_add(1, Ordering::Relaxed);
    }

    pub fn model(&self) -> Option<String> {
        self.0.model.lock().expect("pulse model").clone()
    }

    pub fn using(&self, model: &str) {
        *self.0.model.lock().expect("pulse model") = Some(model.to_string());
    }

    pub fn release(&self) {
        self.0.model.lock().expect("pulse model").take();
    }
}

pub struct Attached {
    previous: Option<Pulse>,
    _thread: PhantomData<*const ()>,
}

impl Drop for Attached {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT.with(|current| *current.borrow_mut() = previous);
    }
}

pub fn current() -> Option<Pulse> {
    CURRENT.with(|current| current.borrow().clone())
}

pub fn beat() {
    if let Some(pulse) = current() {
        pulse.beat();
    }
}

pub fn using(model: &str) {
    if let Some(pulse) = current() {
        pulse.using(model);
    }
}

pub fn release() {
    if let Some(pulse) = current() {
        pulse.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn until(what: &str, mut ready: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready() {
            assert!(Instant::now() < deadline, "{what}");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn another_thread_sees_the_beats() {
        let pulse = Pulse::new();
        let seen = pulse.clone();
        assert_eq!(seen.beats(), 0);
        let attached = pulse.clone();
        let handle = std::thread::spawn(move || {
            let _guard = attached.attach();
            beat();
            beat();
        });
        handle.join().unwrap();
        assert_eq!(
            seen.beats(),
            2,
            "the chunks of one thread count on the shared pulse"
        );
    }

    #[test]
    fn a_thread_without_a_pulse_ignores_the_reports() {
        beat();
        using("m.gguf");
        release();
        assert!(current().is_none());
    }

    #[test]
    fn the_model_stays_after_a_call_that_did_not_return() {
        let pulse = Pulse::new();
        let watcher = pulse.clone();
        let attached = pulse.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _guard = attached.attach();
            using("C:/models/m.gguf#2048");
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(300));
            release();
        });
        rx.recv().unwrap();
        assert_eq!(watcher.model().as_deref(), Some("C:/models/m.gguf#2048"));
        until("the call released the model", || watcher.model().is_none());
    }

    #[test]
    fn a_nested_attach_restores_the_previous_pulse() {
        let outer = Pulse::new();
        let inner = Pulse::new();
        let _outer = outer.attach();
        assert_eq!(current().map(|p| p.beats()), Some(0));
        {
            let _inner = inner.attach();
            beat();
        }
        assert_eq!(outer.beats(), 0, "the beat went to the inner pulse");
        assert_eq!(inner.beats(), 1);
        beat();
        assert_eq!(outer.beats(), 1, "the outer pulse is current again");
    }
}
