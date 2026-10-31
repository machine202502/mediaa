use ez_ffmpeg::Input as EzInput;
use std::path::Path;

pub struct Input {
    inner: EzInput,
}

impl Input {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self::from_url(path.as_ref().to_string_lossy().into_owned())
    }

    pub fn from_url(url: impl Into<String>) -> Self {
        Self {
            inner: EzInput::from(url.into()),
        }
    }

    pub fn video_codec_opt(mut self, key: &str, value: impl Into<String>) -> Self {
        self.inner = self.inner.set_video_codec_opt(key, value);
        self
    }

    pub fn start_us(mut self, start_us: i64) -> Self {
        self.inner = self.inner.set_start_time_us(start_us);
        self
    }

    pub fn duration_us(mut self, duration_us: i64) -> Self {
        self.inner = self.inner.set_recording_time_us(duration_us);
        self
    }

    pub fn format(mut self, format: &str) -> Self {
        self.inner = self.inner.set_format(format);
        self
    }

    pub fn format_opt(mut self, key: &str, value: impl Into<String>) -> Self {
        self.inner = self.inner.set_format_opt(key, value);
        self
    }

    pub(crate) fn into_inner(self) -> EzInput {
        self.inner
    }
}
