use mediaa_core::{paths, Settings};
use parking_lot::{Condvar, Mutex};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

struct SaveRequest {
    generation: u64,
    settings: Settings,
}

#[derive(Default)]
struct WriterState {
    pending: Option<SaveRequest>,
    written: u64,
    failure: Option<(u64, String)>,
    stopping: bool,
    finished: bool,
}

pub(crate) struct SettingsWriter {
    path: PathBuf,
    state: Arc<(Mutex<WriterState>, Condvar)>,
    join: Option<JoinHandle<()>>,
    generation: u64,
}

impl SettingsWriter {
    pub(crate) fn new() -> Self {
        Self::start(paths::settings_file())
    }

    fn start(path: PathBuf) -> Self {
        Self::with_saver(path, |settings, path| settings.save_to(path))
    }

    fn with_saver(
        path: PathBuf,
        mut saver: impl FnMut(&Settings, &Path) -> anyhow::Result<()> + Send + 'static,
    ) -> Self {
        let state = Arc::new((Mutex::new(WriterState::default()), Condvar::new()));
        let worker_state = state.clone();
        let worker_path = path.clone();
        let join = std::thread::Builder::new()
            .name("settings-writer".into())
            .spawn(move || {
                loop {
                    let request = {
                        let (lock, ready) = &*worker_state;
                        let mut current = lock.lock();
                        while current.pending.is_none() && !current.stopping {
                            ready.wait(&mut current);
                        }
                        let Some(request) = current.pending.take() else {
                            break;
                        };
                        request
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        saver(&request.settings, &worker_path)
                    }))
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("settings writer panicked")));
                    let (lock, ready) = &*worker_state;
                    let mut current = lock.lock();
                    current.written = current.written.max(request.generation);
                    if let Err(error) = result {
                        current.failure = Some((request.generation, format!("{error:#}")));
                    }
                    ready.notify_all();
                }
                let (lock, ready) = &*worker_state;
                lock.lock().finished = true;
                ready.notify_all();
            })
            .map_err(|error| log::warn!("settings writer: {error}"))
            .ok();
        Self {
            path,
            state,
            join,
            generation: 0,
        }
    }

    #[cfg(test)]
    pub(crate) fn discard() -> Self {
        Self::with_saver(PathBuf::new(), |_, _| Ok(()))
    }

    pub(crate) fn request(&mut self, settings: Settings) -> anyhow::Result<u64> {
        anyhow::ensure!(self.join.is_some(), "settings writer is unavailable");
        self.generation += 1;
        let (lock, ready) = &*self.state;
        let mut current = lock.lock();
        anyhow::ensure!(!current.stopping, "settings writer is stopping");
        current.pending = Some(SaveRequest {
            generation: self.generation,
            settings,
        });
        ready.notify_one();
        Ok(self.generation)
    }

    pub(crate) fn take_error(&self) -> Option<String> {
        self.state.0.lock().failure.take().map(|(_, error)| error)
    }

    pub(crate) fn flush(&mut self, settings: Settings, timeout: Duration) -> anyhow::Result<()> {
        if self.join.is_none() {
            return settings.save_to(&self.path);
        }
        self.generation += 1;
        let generation = self.generation;
        {
            let (lock, ready) = &*self.state;
            let mut current = lock.lock();
            current.pending = Some(SaveRequest {
                generation,
                settings,
            });
            current.stopping = true;
            ready.notify_one();
        }
        let deadline = Instant::now() + timeout;
        let (lock, ready) = &*self.state;
        let mut current = lock.lock();
        while current.written < generation && !current.finished {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("settings were not saved within {} ms", timeout.as_millis());
            }
            ready.wait_for(&mut current, remaining);
        }
        if let Some((failure_generation, error)) = &current.failure {
            if *failure_generation >= generation {
                anyhow::bail!("{error}");
            }
        }
        while !current.finished {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            ready.wait_for(&mut current, remaining);
        }
        Ok(())
    }
}

impl Drop for SettingsWriter {
    fn drop(&mut self) {
        let (lock, ready) = &*self.state;
        {
            let mut current = lock.lock();
            current.stopping = true;
            current.pending = None;
            ready.notify_all();
        }
        if let Some(join) = self.join.take().filter(|join| join.is_finished()) {
            let _ = join.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsWriter;
    use mediaa_core::settings::ThemePreference;
    use mediaa_core::Settings;
    use std::time::{Duration, Instant};

    #[test]
    fn requests_do_not_wait_for_slow_disk_writes() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = SettingsWriter::with_saver(dir.path().join("settings.toml"), |_, _| {
            std::thread::sleep(Duration::from_millis(250));
            Ok(())
        });
        let started = Instant::now();
        writer.request(Settings::default()).unwrap();
        assert!(started.elapsed() < Duration::from_millis(100));
    }

    #[test]
    fn flush_persists_the_current_settings_after_earlier_requests() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let mut writer = SettingsWriter::start(path.clone());
        let older = Settings {
            theme: ThemePreference::Light,
            ..Settings::default()
        };
        writer.request(older).unwrap();
        let current = Settings {
            theme: ThemePreference::Dark,
            ..Settings::default()
        };
        writer
            .flush(current.clone(), Duration::from_secs(2))
            .unwrap();
        assert_eq!(Settings::load_from(&path), current);
    }

    #[test]
    fn flush_reports_a_terminal_write_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = SettingsWriter::with_saver(dir.path().join("settings.toml"), |_, _| {
            Err(anyhow::anyhow!("disk is read-only"))
        });
        let error = writer
            .flush(Settings::default(), Duration::from_secs(2))
            .unwrap_err();
        assert!(format!("{error:#}").contains("disk is read-only"));
    }
}
