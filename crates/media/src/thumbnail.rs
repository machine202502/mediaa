//! Library thumbnails. Still images are decoded with `image`; videos and animations with FFmpeg.

use ez_ffmpeg::frame_export::{FrameExtractor, PixelLayout};
use mediaa_core::formats::MediaKind;
use mediaa_core::Frame;
use std::path::Path;

/// First frame of `path`, scaled to fit `max_size`×`max_size`.
pub fn thumbnail(path: &Path, kind: MediaKind, max_size: u32) -> anyhow::Result<Frame> {
    let max_size = max_size.max(16);
    if kind == MediaKind::Image {
        let img = image::open(path)?.thumbnail(max_size, max_size).to_rgba8();
        return Ok(Frame::from_image(img));
    }
    if mediaa_core::formats::is_webp_path(path) {
        // Animated WebP: FFmpeg decodes still WebP only.
        let mut first = None;
        let cancel = std::sync::atomic::AtomicBool::new(false);
        crate::preview::preview_frames(path, max_size, 1.0, 0.0, &cancel, &mut |f| {
            first = Some(f.frame);
            false
        })?;
        let f = first.ok_or_else(|| anyhow::anyhow!("no frames in {}", path.display()))?;
        let img = image::RgbaImage::from_raw(f.width, f.height, f.data).ok_or_else(|| anyhow::anyhow!("bad frame buffer"))?;
        return Ok(Frame::from_image(image::DynamicImage::ImageRgba8(img).thumbnail(max_size, max_size).to_rgba8()));
    }
    let frame = FrameExtractor::new(path.to_string_lossy().into_owned())
        .pixel(PixelLayout::Rgba32)
        .max_frames(1)
        .frames()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("no frames in {}", path.display()))??;
    let (w, h) = (frame.width(), frame.height());
    let img = image::RgbaImage::from_raw(w, h, frame.into_vec())
        .ok_or_else(|| anyhow::anyhow!("bad frame buffer"))?;
    let img = image::DynamicImage::ImageRgba8(img)
        .thumbnail(max_size, max_size)
        .to_rgba8();
    Ok(Frame::from_image(img))
}
