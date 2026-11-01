use std::sync::{Mutex, MutexGuard};

static UI_TEST_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn ui_test_guard() -> MutexGuard<'static, ()> {
    UI_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(crate) struct LocaleGuard(String);

impl LocaleGuard {
    pub(crate) fn capture() -> Self {
        Self(crate::i18n::current())
    }
}

impl Drop for LocaleGuard {
    fn drop(&mut self) {
        rust_i18n::set_locale(&self.0);
    }
}

pub(crate) fn english_locale() -> LocaleGuard {
    let guard = LocaleGuard::capture();
    rust_i18n::set_locale("en");
    guard
}

pub(crate) fn russian_locale() -> LocaleGuard {
    let guard = LocaleGuard::capture();
    rust_i18n::set_locale("ru");
    guard
}
