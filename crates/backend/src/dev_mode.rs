use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

pub struct AppMode {
    pub background: bool,
    pub agent: bool,
    pub picker: bool,
    pub input: Option<PathBuf>,
    pub library: Option<PathBuf>,
    pub convert: Option<PathBuf>,
    pub view: Option<String>,
    pub preview: Option<String>,
    pub scope: Option<String>,
}

impl AppMode {
    fn from_env() -> Self {
        Self {
            background: mediaa_core::dev::background(),
            agent: mediaa_core::dev::agent_copy(),
            picker: mediaa_core::dev::picker_enabled(),
            input: mediaa_core::dev::input_file(),
            library: mediaa_core::dev::library_dir(),
            convert: mediaa_core::dev::convert_input(),
            view: mediaa_core::dev::view_filter(),
            preview: mediaa_core::dev::preview_filter(),
            scope: mediaa_core::dev::instance_scope(),
        }
    }
}

static MODE: OnceLock<AppMode> = OnceLock::new();
static FOREGROUND: AtomicIsize = AtomicIsize::new(0);

pub fn mode() -> &'static AppMode {
    MODE.get_or_init(AppMode::from_env)
}
pub fn remember_foreground(window: isize) {
    FOREGROUND.store(window, Ordering::Relaxed);
}
pub fn remembered_foreground() -> isize {
    FOREGROUND.load(Ordering::Relaxed)
}
