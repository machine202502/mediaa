use mediaa2_core::paths::APP_IDENTITY;
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, WAIT_OBJECT_0,
};
use windows_sys::Win32::System::Threading::{
    CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE,
};

pub(crate) struct Guard {
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
    pub(crate) fn acquire(scope: Option<String>) -> anyhow::Result<Option<Self>> {
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

    pub(crate) fn take_activation(&self) -> bool {
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

pub(crate) fn activate_existing(scope: Option<String>) {
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
