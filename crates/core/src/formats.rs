use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    #[default]
    Png,
    Webp,
}

impl ImageFormat {
    pub const ALL: [ImageFormat; 2] = [ImageFormat::Png, ImageFormat::Webp];

    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Png => "png",
            ImageFormat::Webp => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ImageFormat::Png => "PNG",
            ImageFormat::Webp => "WebP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum VideoContainer {
    #[default]
    Mp4,
    Mkv,
}

impl VideoContainer {
    pub const ALL: [VideoContainer; 2] = [VideoContainer::Mp4, VideoContainer::Mkv];

    pub fn extension(self) -> &'static str {
        match self {
            VideoContainer::Mp4 => "mp4",
            VideoContainer::Mkv => "mkv",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            VideoContainer::Mp4 => "MP4",
            VideoContainer::Mkv => "MKV",
        }
    }

    pub fn muxer(self) -> &'static str {
        match self {
            VideoContainer::Mp4 => "mp4",
            VideoContainer::Mkv => "matroska",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AnimFormat {
    #[default]
    Gif,
    Webp,
}

impl AnimFormat {
    pub const ALL: [AnimFormat; 2] = [AnimFormat::Gif, AnimFormat::Webp];

    pub fn extension(self) -> &'static str {
        match self {
            AnimFormat::Gif => "gif",
            AnimFormat::Webp => "webp",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AnimFormat::Gif => "GIF",
            AnimFormat::Webp => "WebP",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    #[default]
    Mp3,
    M4a,
    Wav,
    Flac,
    Ogg,
}

impl AudioFormat {
    pub const ALL: [AudioFormat; 5] = [
        AudioFormat::Mp3,
        AudioFormat::M4a,
        AudioFormat::Wav,
        AudioFormat::Flac,
        AudioFormat::Ogg,
    ];

    pub fn extension(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "mp3",
            AudioFormat::M4a => "m4a",
            AudioFormat::Wav => "wav",
            AudioFormat::Flac => "flac",
            AudioFormat::Ogg => "ogg",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioFormat::Mp3 => "MP3",
            AudioFormat::M4a => "M4A",
            AudioFormat::Wav => "WAV",
            AudioFormat::Flac => "FLAC",
            AudioFormat::Ogg => "OGG",
        }
    }

    pub fn lossless(self) -> bool {
        matches!(self, AudioFormat::Wav | AudioFormat::Flac)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "format", rename_all = "lowercase")]
pub enum OutputKind {
    Video(VideoContainer),
    Anim(AnimFormat),
    Audio(AudioFormat),
}

impl Default for OutputKind {
    fn default() -> Self {
        OutputKind::Video(VideoContainer::Mp4)
    }
}

impl OutputKind {
    pub const ALL: [OutputKind; 4] = [
        OutputKind::Video(VideoContainer::Mp4),
        OutputKind::Video(VideoContainer::Mkv),
        OutputKind::Anim(AnimFormat::Gif),
        OutputKind::Anim(AnimFormat::Webp),
    ];

    pub fn extension(self) -> &'static str {
        match self {
            OutputKind::Video(c) => c.extension(),
            OutputKind::Anim(a) => a.extension(),
            OutputKind::Audio(a) => a.extension(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OutputKind::Video(c) => c.label(),
            OutputKind::Anim(a) => a.label(),
            OutputKind::Audio(a) => a.label(),
        }
    }

    pub fn supports_audio(self) -> bool {
        matches!(self, OutputKind::Video(_) | OutputKind::Audio(_))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
    Animation,
    Audio,
}

pub const IMAGE_EXTENSIONS: [&str; 7] = ["png", "jpg", "jpeg", "bmp", "webp", "tif", "tiff"];
pub const ANIMATION_EXTENSIONS: [&str; 1] = ["gif"];
pub const VIDEO_EXTENSIONS: [&str; 12] = [
    "mp4", "mkv", "mov", "webm", "avi", "m4v", "wmv", "flv", "mpg", "mpeg", "ts", "3gp",
];
pub const AUDIO_EXTENSIONS: [&str; 8] = ["mp3", "m4a", "aac", "wav", "flac", "ogg", "opus", "wma"];
pub const MAX_DECODE_PIXELS: u64 = 50_000_000;

pub fn ensure_safe_dimensions(width: u32, height: u32) -> anyhow::Result<()> {
    anyhow::ensure!(
        width > 0 && height > 0,
        "image has invalid dimensions {width}×{height}"
    );
    anyhow::ensure!(
        u64::from(width) * u64::from(height) <= MAX_DECODE_PIXELS,
        "image dimensions {width}×{height} exceed the supported limit"
    );
    Ok(())
}

pub fn all_media_extensions() -> Vec<&'static str> {
    [
        &VIDEO_EXTENSIONS[..],
        &ANIMATION_EXTENSIONS,
        &IMAGE_EXTENSIONS,
        &AUDIO_EXTENSIONS,
    ]
    .concat()
}

impl MediaKind {
    pub fn from_extension(ext: &str) -> Option<MediaKind> {
        let ext = ext.to_ascii_lowercase();
        [
            (&IMAGE_EXTENSIONS[..], MediaKind::Image),
            (&ANIMATION_EXTENSIONS[..], MediaKind::Animation),
            (&VIDEO_EXTENSIONS[..], MediaKind::Video),
            (&AUDIO_EXTENSIONS[..], MediaKind::Audio),
        ]
        .into_iter()
        .find(|(list, _)| list.contains(&ext.as_str()))
        .map(|(_, kind)| kind)
    }

    pub fn from_path(path: &Path) -> Option<MediaKind> {
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(MediaKind::from_extension)
    }

    pub fn detect(path: &Path) -> Option<MediaKind> {
        let kind = MediaKind::from_path(path)?;
        if is_webp_path(path) && webp_is_animated(path).unwrap_or(false) {
            return Some(MediaKind::Animation);
        }
        Some(kind)
    }
}

pub fn is_webp_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("webp"))
}

pub fn webp_is_animated(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = [0u8; 32];
    let mut f = std::fs::File::open(path)?;
    let n = f.read(&mut head)?;
    Ok(webp_header_is_animated(&head[..n]))
}

pub fn webp_header_is_animated(head: &[u8]) -> bool {
    head.len() >= 21
        && &head[0..4] == b"RIFF"
        && &head[8..12] == b"WEBP"
        && &head[12..16] == b"VP8X"
        && head[20] & 0x02 != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extensions_are_lowercase_and_distinct() {
        let exts: Vec<_> = OutputKind::ALL.iter().map(|k| k.extension()).collect();
        assert_eq!(exts, ["mp4", "mkv", "gif", "webp"]);
        assert_eq!(ImageFormat::Webp.extension(), "webp");
        assert_eq!(VideoContainer::Mkv.muxer(), "matroska");
    }

    #[test]
    fn audio_only_for_video_containers() {
        assert!(OutputKind::Video(VideoContainer::Mkv).supports_audio());
        assert!(!OutputKind::Anim(AnimFormat::Gif).supports_audio());
    }

    #[test]
    fn media_kind_by_extension_is_case_insensitive() {
        assert_eq!(MediaKind::from_extension("MP4"), Some(MediaKind::Video));
        assert_eq!(MediaKind::from_extension("Gif"), Some(MediaKind::Animation));
        assert_eq!(MediaKind::from_extension("png"), Some(MediaKind::Image));
        assert_eq!(MediaKind::from_extension("txt"), None);
    }

    #[test]
    fn oversized_images_are_rejected_before_decoding_pixels() {
        assert!(ensure_safe_dimensions(8192, 4320).is_ok());
        assert!(ensure_safe_dimensions(50_000, 50_000).is_err());
    }

    fn vp8x_header(flags: u8) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(b"RIFF");
        head.extend_from_slice(&[0, 0, 0, 0]);
        head.extend_from_slice(b"WEBPVP8X");
        head.extend_from_slice(&[10, 0, 0, 0]);
        head.push(flags);
        head.extend_from_slice(&[0; 9]);
        head
    }

    #[test]
    fn webp_animation_flag() {
        assert!(webp_header_is_animated(&vp8x_header(0x02)));
        assert!(!webp_header_is_animated(&vp8x_header(0x10)));
        assert!(!webp_header_is_animated(b"RIFF"));
    }

    #[test]
    fn detect_sniffs_animated_webp() {
        let dir = tempfile::tempdir().unwrap();
        let anim = dir.path().join("a.webp");
        std::fs::write(&anim, vp8x_header(0x02)).unwrap();
        let still = dir.path().join("s.webp");
        std::fs::write(&still, vp8x_header(0)).unwrap();
        assert_eq!(MediaKind::detect(&anim), Some(MediaKind::Animation));
        assert_eq!(MediaKind::detect(&still), Some(MediaKind::Image));
    }

    #[test]
    fn output_kind_serde_roundtrip() {
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        struct W {
            k: OutputKind,
        }
        for k in OutputKind::ALL {
            let s = toml::to_string(&W { k }).unwrap();
            let back: W = toml::from_str(&s).unwrap();
            assert_eq!(back.k, k);
        }
    }
}
