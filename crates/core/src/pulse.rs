//! The pulse of a chunk of work: the evidence the queue's watchdog uses to tell a slow model
//! call from a stuck one.
//!
//! Long work that cannot be interrupted (a model call, a download, a decode) reports that it is
//! moving with [`beat`]. The queue's runner watches the pulse of each chunk from its own thread:
//! a chunk whose pulse has not moved for longer than its stale limit is stuck, and is reaped.
//!
//! The pulse belongs to the thread the work runs on. [`Pulse::attach`] installs it for the
//! current thread (the runner does this for the chunk thread), and the code below reports with
//! the free [`beat`] without passing the pulse through every call. Work that hands a long job to
//! another thread takes a [`Pulse`] with it ([`Pulse::clone`]) and beats on that copy.

use std::cell::RefCell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct Inner {
    beats: AtomicU64,
    /// The model the thread's call is using. It stays after a call that never returns, so the
    /// watchdog knows which model to unload.
    model: Mutex<Option<String>>,
}

/// The pulse of one chunk of work. Cheap to clone; clones share the count.
#[derive(Clone, Default)]
pub struct Pulse(Arc<Inner>);

thread_local! {
    static CURRENT: RefCell<Option<Pulse>> = const { RefCell::new(None) };
}

impl Pulse {
    pub fn new() -> Pulse {
        Pulse::default()
    }

    /// Installs this pulse on the calling thread until the returned guard is dropped. Work below
    /// this point reports with [`beat`].
    pub fn attach(&self) -> Attached {
        let previous = CURRENT.with(|current| current.borrow_mut().replace(self.clone()));
        Attached { previous, _thread: PhantomData }
    }

    /// How many times the work moved.
    pub fn beats(&self) -> u64 {
        self.0.beats.load(Ordering::Relaxed)
    }

    /// Reports that the work moved.
    pub fn beat(&self) {
        self.0.beats.fetch_add(1, Ordering::Relaxed);
    }

    /// The model file the call is using, if a call set it and has not returned yet.
    pub fn model(&self) -> Option<String> {
        self.0.model.lock().expect("pulse model").clone()
    }

    /// Names the model the call is about to use.
    pub fn using(&self, model: &str) {
        *self.0.model.lock().expect("pulse model") = Some(model.to_string());
    }

    /// The call returned: there is no model to unload any more.
    pub fn release(&self) {
        self.0.model.lock().expect("pulse model").take();
    }
}

/// Restores the pulse that was installed before [`Pulse::attach`]. Must be dropped on the thread
/// that made it.
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

/// The pulse of the calling thread, `None` outside a chunk.
pub fn current() -> Option<Pulse> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Reports that the calling thread's work moved. A no-op outside a chunk.
pub fn beat() {
    if let Some(pulse) = current() {
        pulse.beat();
    }
}

/// Names the model the calling thread's call is using (see [`Pulse::using`]).
pub fn using(model: &str) {
    if let Some(pulse) = current() {
        pulse.using(model);
    }
}

/// The call returned (see [`Pulse::release`]).
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
        assert_eq!(seen.beats(), 2, "the chunks of one thread count on the shared pulse");
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
            // The call never returns: the thread stays here with the model set.
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
