use eframe::egui;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::OnceLock;

const OFFSCREEN: [f32; 2] = [-32000.0, -32000.0];

pub(crate) struct AppMode {
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

pub(crate) fn mode() -> &'static AppMode {
    MODE.get_or_init(AppMode::from_env)
}

pub(crate) fn remember_foreground(window: isize) {
    FOREGROUND.store(window, Ordering::Relaxed);
}

pub(crate) fn remembered_foreground() -> isize {
    FOREGROUND.load(Ordering::Relaxed)
}

pub(crate) struct OverlayChrome {
    pub position: [f32; 2],
    pub always_on_top: bool,
}

pub(crate) fn overlay_chrome(monitor_x: i32, monitor_y: i32, scale: f32) -> OverlayChrome {
    if mode().background {
        OverlayChrome {
            position: OFFSCREEN,
            always_on_top: false,
        }
    } else {
        OverlayChrome {
            position: [monitor_x as f32 / scale, monitor_y as f32 / scale],
            always_on_top: true,
        }
    }
}

pub(crate) fn prepare_overlay_window(title: &str) {
    if mode().background {
        let _ = crate::infra::native_window::prepare_background(title);
    }
}

pub(crate) fn show_overlay_window(title: &str) {
    if mode().background {
        let _ = crate::infra::native_window::background_show(title);
    }
}

pub(crate) fn capture_foreground() -> Option<isize> {
    if !mode().background {
        return None;
    }
    let remembered = remembered_foreground();
    if remembered != 0 {
        Some(remembered)
    } else {
        Some(crate::infra::native_window::foreground_window())
    }
}

pub(crate) fn keep_awake(ctx: &egui::Context) {
    if mode().input.is_some() {
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
    }
}
