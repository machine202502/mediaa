//! Minimal file + stderr logger (`mediaa.log` in the data directory).

use log::{Level, LevelFilter, Log, Metadata, Record};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

/// Logs bigger than this are rotated to `mediaa.log.1` at startup.
const MAX_LOG_BYTES: u64 = 2 * 1024 * 1024;

struct FileLogger {
    file: Mutex<Option<std::fs::File>>,
    level: LevelFilter,
}

impl Log for FileLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= self.level
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // FFmpeg chatter goes through `log` too; keep only warnings from it.
        if record.target().starts_with("ffmpeg") && record.level() > Level::Warn {
            return;
        }
        let line = format!(
            "{} [{}] {}: {}\n",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
            record.level(),
            record.target(),
            record.args()
        );
        if cfg!(debug_assertions) {
            eprint!("{line}");
        }
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.write_all(line.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut guard) = self.file.lock() {
            if let Some(f) = guard.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// Installs the global logger. Safe to call once; later calls are ignored.
pub fn init(dir: &Path, level: LevelFilter) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join("mediaa.log");
    if std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_LOG_BYTES) {
        let _ = std::fs::rename(&path, dir.join("mediaa.log.1"));
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    let logger = FileLogger {
        file: Mutex::new(file),
        level,
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
}
