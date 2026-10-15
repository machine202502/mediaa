//! Media pipeline on top of FFmpeg (statically linked, driven through `ez-ffmpeg`, whose
//! scheduler is a port of the `ffmpeg` CLI). Everything that touches codecs lives here:
//! probing, conversions, and live recording sessions.

pub mod convert;
pub mod filters;
pub mod info;
pub mod job;
pub mod preview;
pub mod recorder;
pub mod thumbnail;

/// Quiets FFmpeg's own logging (routed through the `log` crate) down to warnings.
pub fn init() {
    ez_ffmpeg::set_ffmpeg_log_level(ez_ffmpeg::FfmpegLogLevel::Warning);
}

/// Encoders/muxers/filters this build relies on, with a flag telling whether each is present.
pub fn capabilities() -> Vec<(&'static str, bool)> {
    use ez_ffmpeg::capabilities as caps;
    let enc = |n: &str| caps::is_encoder_available(n);
    let filt = |n: &str| caps::is_filter_available(n);
    vec![
        ("libx264 encoder", enc("libx264")),
        ("libx264rgb encoder", enc("libx264rgb")),
        ("AAC encoder", enc("aac")),
        ("GIF encoder", enc("gif")),
        ("libwebp_anim encoder", enc("libwebp_anim")),
        ("palettegen filter", filt("palettegen")),
        ("paletteuse filter", filt("paletteuse")),
        ("amix filter", filt("amix")),
    ]
}
