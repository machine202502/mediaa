use crate::annotate;
use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{ExtendedColorType, ImageEncoder};
use mediaa2_core::annotate::Shape;
use mediaa2_core::atomic_file::AtomicFile;
use mediaa2_core::formats::ImageFormat;
use mediaa2_core::frame::Frame;
use mediaa2_core::geometry::Rect;
use std::path::Path;

pub fn save(frame: &Frame, format: ImageFormat, path: &Path) -> anyhow::Result<()> {
    let img = image::RgbaImage::from_raw(frame.width, frame.height, frame.data.clone())
        .ok_or_else(|| anyhow::anyhow!("frame buffer does not match its dimensions"))?;
    let output = AtomicFile::new(path)?;
    match format {
        ImageFormat::Png => {
            let file = std::fs::File::create(output.path())?;
            let writer = std::io::BufWriter::new(file);
            let encoder =
                PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::NoFilter);
            encoder.write_image(
                img.as_raw(),
                img.width(),
                img.height(),
                ExtendedColorType::Rgba8,
            )?;
        }
        ImageFormat::Webp => {
            img.save_with_format(output.path(), image::ImageFormat::WebP)?;
        }
    }
    output.commit()?;
    Ok(())
}

pub fn compose(frame: &Frame, selection: Rect, shapes: &[Shape]) -> Frame {
    annotate::render_crop(frame, selection, shapes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa2_core::annotate::{ShapeKind, Style};

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

    #[test]
    fn compose_crops_the_selection_and_rasterizes_shapes() {
        let frame = Frame::filled(120, 90, [255, 255, 255, 255]);
        let shape = Shape {
            kind: ShapeKind::Line {
                from: (60.0, 50.0),
                to: (110.0, 50.0),
            },
            style: Style::new([255, 0, 0, 255], 4.0),
        };
        let out = compose(&frame, Rect::new(50, 40, 40, 30), &[shape]);
        assert_eq!((out.width, out.height), (40, 30));
        let painted = out.pixel(20, 10);
        assert!(
            painted[0] > 200 && painted[1] < 60 && painted[2] < 60,
            "annotation is shifted into the crop: {painted:?}"
        );
        assert_eq!(out.pixel(35, 25), [255, 255, 255, 255]);
    }

    #[test]
    fn compose_without_shapes_is_a_plain_crop() {
        let frame = Frame::filled(40, 30, [7, 8, 9, 255]);
        let out = compose(&frame, Rect::new(10, 5, 20, 10), &[]);
        assert_eq!((out.width, out.height), (20, 10));
        assert_eq!(out.pixel(0, 0), [7, 8, 9, 255]);
    }

    #[test]
    fn save_refuses_a_buffer_of_the_wrong_size() {
        let dir = tempfile::tempdir().unwrap();
        let frame = Frame {
            width: 4,
            height: 4,
            data: vec![0; 8],
        };
        let path = dir.path().join("broken.png");
        assert!(save(&frame, ImageFormat::Png, &path).is_err());
        assert!(!path.exists());
    }
}
