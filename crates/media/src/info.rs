use ez_ffmpeg::stream_info::{find_all_stream_infos, StreamInfo};
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

pub fn probe_webp(
    decoder: &image_webp::WebPDecoder<impl std::io::BufRead + std::io::Seek>,
) -> MediaInfo {
    let (width, height) = decoder.dimensions();
    let duration_ms = decoder.loop_duration();
    MediaInfo {
        duration_us: Some(duration_ms as i64 * 1000).filter(|d| *d > 0),
        width,
        height,
        fps: if duration_ms > 0 {
            f64::from(decoder.num_frames()) * 1000.0 / duration_ms as f64
        } else {
            0.0
        },
        video_codec: Some("webp".into()),
        audio_codec: None,
        audio_streams: 0,
    }
}

pub fn has_sound(path: &Path) -> anyhow::Result<bool> {
    Ok(probe(path)?.has_audio())
}

pub fn probe(path: &Path) -> anyhow::Result<MediaInfo> {
    if mediaa_core::formats::is_webp_path(path) {
        let reader = crate::webp_decode::WebpReader::open(path)?;
        if reader.is_animated() {
            return Ok(reader.info());
        }
    }
    let url = path.to_string_lossy().into_owned();
    let mut info = MediaInfo {
        duration_us: ez_ffmpeg::container_info::get_duration_us(url.clone())
            .ok()
            .filter(|d| *d > 0),
        ..MediaInfo::default()
    };
    for stream in find_all_stream_infos(url)? {
        match stream {
            StreamInfo::Video {
                width,
                height,
                fps,
                codec_name,
                ..
            } if info.video_codec.is_none() => {
                info.width = width.max(0) as u32;
                info.height = height.max(0) as u32;
                info.fps = fps;
                info.video_codec = Some(codec_name);
            }
            StreamInfo::Audio { codec_name, .. } => {
                info.audio_streams += 1;
                info.audio_codec.get_or_insert(codec_name);
            }
            _ => {}
        }
    }
    Ok(info)
}
