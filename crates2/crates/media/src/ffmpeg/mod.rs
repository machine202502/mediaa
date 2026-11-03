pub mod caps;
pub mod context;
pub mod decode;
pub mod frames;
pub mod input;
pub mod job;
pub mod output;
pub mod probe;
pub mod push;

pub use context::{Job, JobBuilder, JobContext};
pub use frames::{frames, FrameReader, FrameRequest, Sampling, TimedFrame};
pub use input::Input;
pub use output::{Output, VSync};
pub use probe::Probe;
pub use push::{push_source, PushHandle, PushSource};

pub fn init() {
    ez_ffmpeg::set_ffmpeg_log_level(ez_ffmpeg::FfmpegLogLevel::Warning);
}
