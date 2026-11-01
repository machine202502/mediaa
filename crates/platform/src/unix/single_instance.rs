use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub(crate) struct Guard {
    _file: File,
    activation: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
    socket: Option<PathBuf>,
    listener: Option<std::thread::JoinHandle<()>>,
}

impl Guard {
    pub(crate) fn take_activation(&self) -> bool {
        self.activation.swap(false, Ordering::AcqRel)
    }
}

impl Guard {
    pub(crate) fn acquire(scope: Option<String>) -> anyhow::Result<Option<Self>> {
        let dir = mediaa_core::paths::data_dir();
        std::fs::create_dir_all(&dir)?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(dir.join(match &scope {
                Some(scope) => format!("instance-{scope}.lock"),
                None => "instance.lock".into(),
            }))?;
        // SAFETY: flock operates on the live file descriptor owned by `file`.
        let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if result == 0 {
            let activation = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let (socket, listener) =
                start_activation(&dir, scope, activation.clone(), stop.clone()).unwrap_or_else(
                    |error| {
                        log::warn!("single-instance activation unavailable: {error}");
                        (None, None)
                    },
                );
            Ok(Some(Self {
                _file: file,
                activation,
                stop,
                socket,
                listener,
            }))
        } else {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::WouldBlock {
                Ok(None)
            } else {
                Err(error.into())
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(socket) = &self.socket {
            let _ = UnixStream::connect(socket);
        }
        if let Some(listener) = self.listener.take() {
            let _ = listener.join();
        }
        if let Some(socket) = self.socket.take() {
            let _ = std::fs::remove_file(socket);
        }
    }
}

fn start_activation(
    dir: &std::path::Path,
    scope: Option<String>,
    activation: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
) -> anyhow::Result<(Option<PathBuf>, Option<std::thread::JoinHandle<()>>)> {
    let socket = socket_path(dir, scope.as_deref());
    match std::fs::remove_file(&socket) {
        Ok(()) => {}
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    let listener = UnixListener::bind(&socket)?;
    listener.set_nonblocking(true)?;
    let worker_socket = socket.clone();
    let thread = std::thread::Builder::new()
        .name("single-instance-activation".into())
        .spawn(move || {
            while !stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        let _ = stream.set_read_timeout(Some(Duration::from_millis(250)));
                        let mut request = [0u8; 1];
                        if stream.read_exact(&mut request).is_ok() {
                            activation.store(true, Ordering::Release);
                        }
                    }
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(50));
                    }
                    Err(_) => break,
                }
            }
            let _ = std::fs::remove_file(worker_socket);
        })?;
    Ok((Some(socket), Some(thread)))
}

fn socket_path(dir: &std::path::Path, scope: Option<&str>) -> PathBuf {
    dir.join(match scope {
        Some(scope) => format!("instance-{scope}.sock"),
        None => "instance.sock".into(),
    })
}

pub(crate) fn activate_existing() {
    let scope = mediaa_core::dev::instance_scope();
    let socket = socket_path(&mediaa_core::paths::data_dir(), scope.as_deref());
    if let Ok(mut stream) = UnixStream::connect(socket) {
        let _ = stream.write_all(b"1");
    }
}
