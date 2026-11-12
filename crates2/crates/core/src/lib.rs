#![forbid(unsafe_code)]

pub mod agent;
pub mod annotate;
pub mod assistant;
pub mod atomic_file;
pub mod audio;
pub mod chat;
pub mod convert;
pub mod formats;
pub mod frame;
pub mod geometry;
pub mod hash;
pub mod latest_worker;
pub mod library;
pub mod limits;
pub mod logging;
pub mod naming;
pub mod navigation;
pub mod notices;
pub mod pacer;
pub mod paths;
pub mod pulse;
pub mod query;
pub mod recording;
pub mod screen;
pub mod settings;
pub mod source;
pub mod speakers;
pub mod transcript;

pub use formats::{AnimFormat, ImageFormat, MediaKind, OutputKind, VideoContainer};
pub use frame::Frame;
pub use geometry::{Point, Rect};
pub use settings::Settings;
