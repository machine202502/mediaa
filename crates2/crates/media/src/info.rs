use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct MediaInfo {
    pub duration_us: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_streams: usize,
}

impl MediaInfo {
    pub fn has_video(&self) -> bool {
        self.video_codec.is_some()
    }

    pub fn has_audio(&self) -> bool {
        self.audio_streams > 0
    }

    pub fn duration_secs(&self) -> Option<f64> {
        self.duration_us.map(|d| d as f64 / 1e6)
    }
}

pub fn has_sound(path: &Path) -> anyhow::Result<bool> {
    Ok(probe(path)?.has_audio())
}

pub(crate) fn from_webp(info: crate::webp::Info) -> MediaInfo {
    let duration_us = (info.duration_ms as i64 * 1000).max(0);
    MediaInfo {
        duration_us: Some(duration_us).filter(|duration| *duration > 0),
        width: info.width,
        height: info.height,
        fps: if duration_us > 0 {
            f64::from(info.frames) * 1_000_000.0 / duration_us as f64
        } else {
            0.0
        },
        video_codec: Some("webp".into()),
        audio_codec: None,
        audio_streams: 0,
    }
}

pub fn probe(path: &Path) -> anyhow::Result<MediaInfo> {
    if mediaa2_core::formats::is_webp_path(path) {
        let reader = crate::webp::Reader::open(path)?;
        if reader.is_animated() {
            return Ok(from_webp(reader.info()));
        }
    }
    let probed = crate::ffmpeg::probe::probe(path)?;
    Ok(MediaInfo {
        duration_us: probed.duration_us,
        width: probed.width,
        height: probed.height,
        fps: probed.fps,
        video_codec: probed.video_codec,
        audio_codec: probed.audio_codec,
        audio_streams: probed.audio_streams,
    })
}
