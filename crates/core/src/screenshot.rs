//! Saving still images.

use crate::formats::ImageFormat;
use crate::frame::Frame;
use std::path::Path;

/// Encodes `frame` as PNG or (lossless) WebP into `path`.
pub fn save(frame: &Frame, format: ImageFormat, path: &Path) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.data.clone())
        .ok_or_else(|| anyhow::anyhow!("frame buffer does not match its dimensions"))?;
    let fmt = match format {
        ImageFormat::Png => image::ImageFormat::Png,
        ImageFormat::Webp => image::ImageFormat::WebP,
    };
    img.save_with_format(path, fmt)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_png_and_webp_losslessly() {
        let dir = tempfile::tempdir().unwrap();
        let mut f = Frame::filled(8, 4, [10, 20, 30, 255]);
        f.data[0..4].copy_from_slice(&[255, 0, 0, 255]);
        for fmt in ImageFormat::ALL {
            let p = dir.path().join(format!("shot.{}", fmt.extension()));
            save(&f, fmt, &p).unwrap();
            let back = image::open(&p).unwrap().to_rgba8();
            assert_eq!(back.dimensions(), (8, 4));
            assert_eq!(back.get_pixel(0, 0).0, [255, 0, 0, 255]);
            assert_eq!(back.get_pixel(7, 3).0, [10, 20, 30, 255]);
        }
    }
}
