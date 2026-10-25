pub struct SingleInstance {
    _guard: imp::Guard,
}

impl SingleInstance {
    pub fn acquire() -> anyhow::Result<Option<Self>> {
        Self::acquire_scoped(mediaa_core::dev::instance_scope())
    }

    fn acquire_scoped(scope: Option<String>) -> anyhow::Result<Option<Self>> {
        Ok(imp::Guard::acquire(scope)?.map(|guard| Self { _guard: guard }))
    }

    pub fn activate_existing() {
        imp::activate_existing();
    }

    pub fn take_activation(&self) -> bool {
        self._guard.take_activation()
    }
}

#[cfg(windows)]
mod imp {
    use mediaa_core::paths::APP_IDENTITY;
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Threading::{
        CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
    };

    pub struct Guard {
        mutex: HANDLE,
        activation: HANDLE,
    }

    fn wide_name(scope: Option<&str>, suffix: &str) -> Vec<u16> {
        let mut name = format!("Local\\{}", APP_IDENTITY.instance_id);
        if let Some(scope) = scope {
            name.push('-');
            name.push_str(scope);
        }
        name.push_str(suffix);
        name.encode_utf16().chain(std::iter::once(0)).collect()
    }

    impl Guard {
        pub fn acquire(scope: Option<String>) -> anyhow::Result<Option<Self>> {
            let mutex_name = wide_name(scope.as_deref(), "");
            let activation_name = wide_name(scope.as_deref(), ".activate");
            let activation = {
                // SAFETY: the name is NUL-terminated and the returned handle is owned by Guard.
                unsafe { CreateEventW(std::ptr::null(), 0, 0, activation_name.as_ptr()) }
            };
            if activation.is_null() {
                anyhow::bail!("cannot create {} activation event", APP_IDENTITY.name);
            }
            // SAFETY: the name is NUL-terminated and the returned handle is owned by Guard.
            let mutex = unsafe { CreateMutexW(std::ptr::null(), 0, mutex_name.as_ptr()) };
            if mutex.is_null() {
                // SAFETY: this event handle is no longer needed.
                unsafe {
                    CloseHandle(activation);
                }
                anyhow::bail!("cannot create {} instance mutex", APP_IDENTITY.name);
            }
            // SAFETY: GetLastError is a thread-local status query.
            if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
                // SAFETY: the duplicate handles are no longer needed after signalling the owner.
                unsafe {
                    SetEvent(activation);
                    CloseHandle(mutex);
                    CloseHandle(activation);
                }
                return Ok(None);
            }
            Ok(Some(Self { mutex, activation }))
        }

        pub fn take_activation(&self) -> bool {
            // SAFETY: Guard owns a live auto-reset event handle.
            unsafe { WaitForSingleObject(self.activation, 0) == WAIT_OBJECT_0 }
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            // SAFETY: Guard owns exactly one mutex and one activation event.
            unsafe {
                CloseHandle(self.mutex);
                CloseHandle(self.activation);
            }
        }
    }

    pub fn activate_existing() {
        let scope = mediaa_core::dev::instance_scope();
        let name = wide_name(scope.as_deref(), ".activate");
        // SAFETY: the name is NUL-terminated and the handle is closed after signalling.
        unsafe {
            let activation = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
            if !activation.is_null() {
                SetEvent(activation);
                CloseHandle(activation);
            }
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io::{ErrorKind, Read, Write};
    use std::os::unix::io::AsRawFd;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    pub struct Guard {
        _file: File,
        activation: Arc<AtomicBool>,
        stop: Arc<AtomicBool>,
        socket: Option<PathBuf>,
        listener: Option<std::thread::JoinHandle<()>>,
    }

    impl Guard {
        pub fn take_activation(&self) -> bool {
            self.activation.swap(false, Ordering::AcqRel)
        }
    }

    impl Guard {
        pub fn acquire(scope: Option<String>) -> anyhow::Result<Option<Self>> {
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

    pub fn activate_existing() {
        let scope = super::development_scope();
        let socket = socket_path(&mediaa_core::paths::data_dir(), scope.as_deref());
        if let Ok(mut stream) = UnixStream::connect(socket) {
            let _ = stream.write_all(b"1");
        }
    }
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
mod imp {
    pub struct Guard;

    impl Guard {
        pub fn acquire(_scope: Option<String>) -> anyhow::Result<Option<Self>> {
            Ok(Some(Self))
        }

        pub fn take_activation(&self) -> bool {
            false
        }
    }

    pub fn activate_existing() {}
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_second_instance_guard_is_rejected_until_the_first_is_dropped() {
        let scope = Some("unit-test-single-instance".to_string());
        let first = super::SingleInstance::acquire_scoped(scope.clone()).unwrap();
        let Some(first) = first else {
            return;
        };
        assert!(super::SingleInstance::acquire_scoped(scope.clone())
            .unwrap()
            .is_none());
        #[cfg(windows)]
        {
            assert!(first.take_activation());
            assert!(!first.take_activation());
        }
        drop(first);
        assert!(super::SingleInstance::acquire_scoped(scope)
            .unwrap()
            .is_some());
    }
}
