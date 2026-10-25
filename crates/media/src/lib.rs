pub mod animation_backends;
pub mod convert;
pub mod error;
pub mod filters;
pub mod info;
pub mod job;
mod live_webp;
pub mod preview;
pub mod preview_cache;
pub mod recorder;
pub mod thumbnail;

pub fn init() {
    ez_ffmpeg::set_ffmpeg_log_level(ez_ffmpeg::FfmpegLogLevel::Warning);
}
