//! Platform-independent core of MediaA.
//!
//! Everything here is testable without a display or FFmpeg: settings, selection geometry,
//! frame pacing, file naming, frame utilities, audio track writing, the media library index and its thumbnail cache.

pub mod agent;
pub mod annotate;
pub mod audio;
pub mod cache_db;
pub mod catalogs;
pub mod chat;
pub mod cursor;
pub mod formats;
pub mod frame;
pub mod geometry;
pub mod layout;
pub mod library;
pub mod library_index;
pub mod logging;
pub mod naming;
pub mod outline;
pub mod pacer;
pub mod queue;
pub mod retrieval;
pub mod paths;
pub mod screenshot;
pub mod secrets;
pub mod settings;
pub mod source;
pub mod thumb_cache;
pub mod transcript;

pub use formats::{AnimFormat, ImageFormat, MediaKind, OutputKind, VideoContainer};
pub use frame::Frame;
pub use geometry::{Point, Rect};
pub use settings::Settings;
