use ez_ffmpeg::container_info;
use ez_ffmpeg::stream_info::{find_all_stream_infos, StreamInfo};
use std::path::Path;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Probe {
    pub duration_us: Option<i64>,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub audio_streams: usize,
}

pub fn duration_us(path: &Path) -> Option<i64> {
    container_info::get_duration_us(path.to_string_lossy().into_owned())
        .ok()
        .filter(|duration| *duration > 0)
}

pub fn probe(path: &Path) -> anyhow::Result<Probe> {
    let url = path.to_string_lossy().into_owned();
    let mut probe = Probe {
        duration_us: duration_us(path),
        ..Probe::default()
    };
    for stream in find_all_stream_infos(url)? {
        match stream {
            StreamInfo::Video {
                width,
                height,
                fps,
                codec_name,
                ..
            } if probe.video_codec.is_none() => {
                probe.width = width.max(0) as u32;
                probe.height = height.max(0) as u32;
                probe.fps = fps;
                probe.video_codec = Some(codec_name);
            }
            StreamInfo::Audio { codec_name, .. } => {
                probe.audio_streams += 1;
                probe.audio_codec.get_or_insert(codec_name);
            }
            _ => {}
        }
    }
    Ok(probe)
}
