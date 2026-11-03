use ez_ffmpeg::core::context::output::VSyncMethod as EzVSyncMethod;
use ez_ffmpeg::Output as EzOutput;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VSync {
    Auto,
    Cfr,
    Vfr,
    Passthrough,
    Vscfr,
}

impl VSync {
    fn ez(self) -> EzVSyncMethod {
        match self {
            VSync::Auto => EzVSyncMethod::VsyncAuto,
            VSync::Cfr => EzVSyncMethod::VsyncCfr,
            VSync::Vfr => EzVSyncMethod::VsyncVfr,
            VSync::Passthrough => EzVSyncMethod::VsyncPassthrough,
            VSync::Vscfr => EzVSyncMethod::VsyncVscfr,
        }
    }
}

pub struct Output {
    inner: EzOutput,
}

impl Output {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::from_url(path.as_ref().to_string_lossy().into_owned())
    }

    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            inner: EzOutput::from(url.into()),
        }
    }

    pub fn write_callback<F>(callback: F) -> Self
    where
        F: FnMut(&[u8]) -> i32 + Send + 'static,
    {
        Self {
            inner: EzOutput::new_by_write_callback(callback),
        }
    }

    pub fn format(mut self, format: &str) -> Self {
        self.inner = self.inner.set_format(format);
        self
    }

    pub fn format_opt(mut self, key: &str, value: impl Into<String>) -> Self {
        self.inner = self.inner.set_format_opt(key, value);
        self
    }

    pub fn stream_map(mut self, map: &str) -> Self {
        self.inner = self.inner.add_stream_map(map);
        self
    }

    pub fn copy_stream_map(mut self, map: &str) -> Self {
        self.inner = self.inner.add_stream_map_with_copy(map);
        self
    }

    pub fn video_codec(mut self, codec: &str) -> Self {
        self.inner = self.inner.set_video_codec(codec);
        self
    }

    pub fn video_opt(mut self, key: &str, value: impl Into<String>) -> Self {
        self.inner = self.inner.set_video_codec_opt(key, value);
        self
    }

    pub fn audio_codec(mut self, codec: &str) -> Self {
        self.inner = self.inner.set_audio_codec(codec);
        self
    }

    pub fn audio_opt(mut self, key: &str, value: impl Into<String>) -> Self {
        self.inner = self.inner.set_audio_codec_opt(key, value);
        self
    }

    pub fn audio_bitrate(mut self, bitrate: impl Into<String>) -> Self {
        self.inner = self.inner.set_audio_bitrate(bitrate);
        self
    }

    pub fn audio_channels(mut self, channels: i32) -> Self {
        self.inner = self.inner.set_audio_channels(channels);
        self
    }

    pub fn video_qscale(mut self, qscale: i32) -> Self {
        self.inner = self.inner.set_video_qscale(qscale);
        self
    }

    pub fn max_fps(mut self, num: i32, den: i32) -> Self {
        self.inner = self.inner.set_framerate_max(num, den);
        self
    }

    pub fn max_video_frames(mut self, frames: i64) -> Self {
        self.inner = self.inner.set_max_video_frames(Some(frames));
        self
    }

    pub fn vsync(mut self, vsync: VSync) -> Self {
        self.inner = self.inner.set_vsync_method(vsync.ez());
        self
    }

    pub(crate) fn into_inner(self) -> EzOutput {
        self.inner
    }
}
