use ez_ffmpeg::frame_export::{FrameExtractor, FrameIter, PixelLayout, Sampling as EzSampling};
use mediaa2_core::Frame;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Sampling {
    #[default]
    All,
    EverySec(f64),
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FrameRequest {
    pub width: Option<u32>,
    pub sampling: Sampling,
    pub start_us: Option<i64>,
    pub max_frames: Option<u64>,
}

#[derive(Debug)]
pub struct TimedFrame {
    pub pts_us: Option<i64>,
    pub frame: Frame,
}

pub struct FrameReader {
    inner: FrameIter,
}

pub fn frames(path: &Path, request: FrameRequest) -> anyhow::Result<FrameReader> {
    let mut extractor =
        FrameExtractor::new(path.to_string_lossy().into_owned()).pixel(PixelLayout::Rgba32);
    if let Some(width) = request.width {
        extractor = extractor.width(width);
    }
    if let Some(start_us) = request.start_us {
        extractor = extractor.start_time_us(start_us);
    }
    if let Some(max_frames) = request.max_frames {
        extractor = extractor.max_frames(max_frames);
    }
    extractor = match request.sampling {
        Sampling::All => extractor,
        Sampling::EverySec(seconds) => extractor.sampling(EzSampling::EverySec(seconds)),
    };
    Ok(FrameReader {
        inner: extractor.frames()?,
    })
}

impl Iterator for FrameReader {
    type Item = anyhow::Result<TimedFrame>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next().map(|frame| {
            frame
                .map(|frame| {
                    let pts_us = frame.pts_us();
                    let (width, height) = (frame.width(), frame.height());
                    TimedFrame {
                        pts_us,
                        frame: Frame::new(width, height, frame.into_vec()),
                    }
                })
                .map_err(Into::into)
        })
    }
}
