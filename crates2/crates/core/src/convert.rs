use crate::formats::{AnimFormat, AudioFormat, MediaKind, VideoContainer};
use crate::settings::{AnimSettings, GifDither};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VideoFormat {
    Mp4,
    Mov,
    Mkv,
    Webm,
    Avi,
}

impl VideoFormat {
    pub const ALL: [Self; 5] = [Self::Mp4, Self::Mov, Self::Mkv, Self::Webm, Self::Avi];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mov => "mov",
            Self::Mkv => "mkv",
            Self::Webm => "webm",
            Self::Avi => "avi",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Mp4 => "MP4",
            Self::Mov => "MOV",
            Self::Mkv => "MKV",
            Self::Webm => "WebM",
            Self::Avi => "AVI",
        }
    }

    pub fn container(self) -> Option<VideoContainer> {
        match self {
            Self::Mp4 => Some(VideoContainer::Mp4),
            Self::Mkv => Some(VideoContainer::Mkv),
            Self::Mov | Self::Webm | Self::Avi => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StillFormat {
    Jpeg,
    Png,
    Webp,
    Bmp,
    Tiff,
}

impl StillFormat {
    pub const ALL: [Self; 5] = [Self::Jpeg, Self::Png, Self::Webp, Self::Bmp, Self::Tiff];

    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "JPEG",
            Self::Png => "PNG",
            Self::Webp => "WebP",
            Self::Bmp => "BMP",
            Self::Tiff => "TIFF",
        }
    }

    pub fn has_quality(self) -> bool {
        matches!(self, Self::Jpeg | Self::Webp)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    Fastest,
    Fast,
    #[default]
    Balanced,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "format", rename_all = "snake_case")]
pub enum ConvertTarget {
    Video(VideoFormat),
    Anim(AnimFormat),
    Still(StillFormat),
    Audio(AudioFormat),
}

impl ConvertTarget {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Video(format) => format.extension(),
            Self::Anim(format) => format.extension(),
            Self::Still(format) => format.extension(),
            Self::Audio(format) => format.extension(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Video(format) => format.label(),
            Self::Anim(format) => format.label(),
            Self::Still(format) => format.label(),
            Self::Audio(format) => format.label(),
        }
    }

    pub fn for_kind(kind: MediaKind) -> Vec<Self> {
        match kind {
            MediaKind::Video => VideoFormat::ALL
                .into_iter()
                .map(Self::Video)
                .chain(
                    [AnimFormat::Gif, AnimFormat::Webp]
                        .into_iter()
                        .map(Self::Anim),
                )
                .chain(StillFormat::ALL.into_iter().map(Self::Still))
                .chain(AudioFormat::ALL.into_iter().map(Self::Audio))
                .collect(),
            MediaKind::Animation => VideoFormat::ALL
                .into_iter()
                .map(Self::Video)
                .chain(
                    [AnimFormat::Gif, AnimFormat::Webp]
                        .into_iter()
                        .map(Self::Anim),
                )
                .chain(StillFormat::ALL.into_iter().map(Self::Still))
                .collect(),
            MediaKind::Image => StillFormat::ALL.into_iter().map(Self::Still).collect(),
            MediaKind::Audio => AudioFormat::ALL.into_iter().map(Self::Audio).collect(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversionOptions {
    pub target: ConvertTarget,
    pub quality: u8,
    pub effort: Effort,
    pub max_height: Option<u32>,
    pub max_fps: Option<u32>,
    pub remove_audio: bool,
    pub animation: AnimSettings,
    pub audio_kbps: u32,
    pub mono: bool,
    pub png_colors: Option<u32>,
    pub webp_lossless: bool,
}

impl Default for ConversionOptions {
    fn default() -> Self {
        Self {
            target: ConvertTarget::Video(VideoFormat::Mp4),
            quality: 100,
            effort: Effort::Balanced,
            max_height: None,
            max_fps: None,
            remove_audio: false,
            animation: AnimSettings::default(),
            audio_kbps: 256,
            mono: false,
            png_colors: None,
            webp_lossless: false,
        }
    }
}

impl ConversionOptions {
    pub fn for_target(target: ConvertTarget) -> Self {
        Self {
            target,
            ..Self::default()
        }
    }

    pub fn animation_defaults(mut self) -> Self {
        if matches!(self.target, ConvertTarget::Anim(_)) {
            self.animation = AnimSettings {
                gif_colors: 256,
                gif_dither: GifDither::Sierra2,
                webp_quality: 80,
                ..AnimSettings::default()
            };
        }
        self
    }
}
