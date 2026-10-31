use ez_ffmpeg::{FramePushHandle, FramePushSource};

pub struct PushSource {
    inner: FramePushSource,
}

pub struct PushHandle {
    inner: FramePushHandle,
}

pub fn push_source(
    width: u32,
    height: u32,
    pixel_format: &str,
    time_base: (i32, i32),
) -> anyhow::Result<(PushSource, PushHandle)> {
    let (source, handle) = FramePushSource::builder(width, height)
        .pixel_format(pixel_format)
        .time_base(time_base.0, time_base.1)
        .build()?;
    Ok((PushSource { inner: source }, PushHandle { inner: handle }))
}

impl PushSource {
    pub(crate) fn into_inner(self) -> FramePushSource {
        self.inner
    }
}

impl PushHandle {
    pub fn push(&mut self, frame: &[u8], pts: i64, duration: Option<i64>) -> anyhow::Result<()> {
        self.inner
            .push(frame, pts, duration)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    pub fn push_owned(
        &mut self,
        frame: Vec<u8>,
        pts: i64,
        duration: Option<i64>,
    ) -> anyhow::Result<()> {
        self.inner
            .push_owned(frame, pts, duration)
            .map_err(|error| anyhow::anyhow!("{error}"))
    }

    pub fn finish(self) {
        self.inner.finish();
    }
}
