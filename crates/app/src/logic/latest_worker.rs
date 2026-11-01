use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::{Condvar, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

struct State<T> {
    pending: Option<T>,
    active: Option<Arc<AtomicBool>>,
    stop: bool,
}

pub(crate) struct LatestWorker<T> {
    state: Arc<(Mutex<State<T>>, Condvar)>,
    join: Option<JoinHandle<()>>,
}

impl<T: Send + 'static> LatestWorker<T> {
    pub(crate) fn start<O: Send + 'static>(
        name: &str,
        capacity: usize,
        mut process: impl FnMut(T, Arc<AtomicBool>, Sender<O>) + Send + 'static,
    ) -> anyhow::Result<(Self, Receiver<O>)> {
        let state = Arc::new((
            Mutex::new(State {
                pending: None,
                active: None,
                stop: false,
            }),
            Condvar::new(),
        ));
        let (output, receiver) = bounded(capacity.max(1));
        let worker_state = state.clone();
        let join = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || loop {
                let (item, cancel) = {
                    let (lock, ready) = &*worker_state;
                    let mut state = lock.lock();
                    while state.pending.is_none() && !state.stop {
                        ready.wait(&mut state);
                    }
                    if state.stop {
                        break;
                    }
                    let item = state.pending.take().expect("pending request");
                    let cancel = Arc::new(AtomicBool::new(false));
                    state.active = Some(cancel.clone());
                    (item, cancel)
                };
                process(item, cancel, output.clone());
                let (lock, _) = &*worker_state;
                lock.lock().active = None;
            })?;
        Ok((
            Self {
                state,
                join: Some(join),
            },
            receiver,
        ))
    }

    pub(crate) fn request(&self, item: T) {
        let (lock, ready) = &*self.state;
        let mut state = lock.lock();
        if let Some(active) = &state.active {
            active.store(true, Ordering::Relaxed);
        }
        state.pending = Some(item);
        ready.notify_one();
    }

    pub(crate) fn cancel(&self) {
        let (lock, _) = &*self.state;
        let mut state = lock.lock();
        if let Some(active) = &state.active {
            active.store(true, Ordering::Relaxed);
        }
        state.pending = None;
    }
}

impl<T> Drop for LatestWorker<T> {
    fn drop(&mut self) {
        let (lock, ready) = &*self.state;
        let mut state = lock.lock();
        state.stop = true;
        state.pending = None;
        if let Some(active) = &state.active {
            active.store(true, Ordering::Relaxed);
        }
        ready.notify_all();
        drop(state);
        if let Some(join) = self.join.take().filter(|join| join.is_finished()) {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LatestWorker;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    #[test]
    fn newest_request_replaces_the_active_one() {
        let (worker, output) = LatestWorker::start(
            "latest-worker-test",
            1,
            |value: u32, cancel, output: crossbeam_channel::Sender<u32>| {
                std::thread::sleep(Duration::from_millis(5));
                if !cancel.load(Ordering::Relaxed) {
                    let _ = output.send(value);
                }
            },
        )
        .unwrap();
        worker.request(1);
        worker.request(2);
        assert_eq!(output.recv_timeout(Duration::from_secs(1)).unwrap(), 2);
    }
}
