use mediaa_core::Frame;
use std::io::BufReader;
use std::path::Path;

pub struct WebpFrame<'a> {
    pub data: &'a [u8],
    pub delay_ms: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    pub animated: bool,
    pub frames: u32,
    pub duration_ms: u64,
}

pub struct Reader {
    decoder: image_webp::WebPDecoder<BufReader<std::fs::File>>,
    buffer: Vec<u8>,
    buffer_size: usize,
    info: Info,
}

impl Reader {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let decoder = image_webp::WebPDecoder::new(BufReader::new(std::fs::File::open(path)?))?;
        let (width, height) = decoder.dimensions();
        mediaa_core::formats::ensure_safe_dimensions(width, height)?;
        let buffer_size = decoder
            .output_buffer_size()
            .ok_or_else(|| anyhow::anyhow!("WebP is too large"))?;
        let buffer = if decoder.is_animated() {
            vec![0; buffer_size]
        } else {
            Vec::new()
        };
        let info = Info {
            width,
            height,
            has_alpha: decoder.has_alpha(),
            animated: decoder.is_animated(),
            frames: decoder.num_frames().max(1),
            duration_ms: decoder.loop_duration(),
        };
        Ok(Self {
            decoder,
            buffer,
            buffer_size,
            info,
        })
    }

    pub fn info(&self) -> Info {
        self.info
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.info.width, self.info.height)
    }

    pub fn has_alpha(&self) -> bool {
        self.info.has_alpha
    }

    pub fn is_animated(&self) -> bool {
        self.info.animated
    }

    pub fn frame_count(&self) -> u32 {
        self.info.frames
    }

    pub fn duration_ms(&self) -> u64 {
        self.info.duration_ms
    }

    pub fn read_frame(&mut self) -> anyhow::Result<WebpFrame<'_>> {
        if self.buffer.len() != self.buffer_size {
            self.buffer.resize(self.buffer_size, 0);
        }
        let delay_ms = if self.info.animated {
            self.decoder.read_frame(&mut self.buffer)?.max(1)
        } else {
            self.decoder.read_image(&mut self.buffer)?;
            1_000
        };
        Ok(WebpFrame {
            data: &self.buffer,
            delay_ms,
        })
    }
}

pub fn scale_frame(
    data: &[u8],
    width: u32,
    height: u32,
    alpha: bool,
    target_width: u32,
    target_height: u32,
) -> Option<Frame> {
    use image::{ImageBuffer, Rgb, Rgba};
    let rgba = if alpha {
        let source = ImageBuffer::<Rgba<u8>, &[u8]>::from_raw(width, height, data)?;
        if (target_width, target_height) == (width, height) {
            image::RgbaImage::from_raw(width, height, data.to_vec())?
        } else {
            image::imageops::thumbnail(&source, target_width, target_height)
        }
    } else {
        let source = ImageBuffer::<Rgb<u8>, &[u8]>::from_raw(width, height, data)?;
        let small = if (target_width, target_height) == (width, height) {
            image::RgbImage::from_raw(width, height, data.to_vec())?
        } else {
            image::imageops::thumbnail(&source, target_width, target_height)
        };
        image::DynamicImage::ImageRgb8(small).into_rgba8()
    };
    Some(Frame::from_image(rgba))
}

#[cfg(test)]
mod tests {
    use super::Reader;

    #[test]
    fn rejects_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.webp");
        std::fs::write(&path, b"not webp").unwrap();
        assert!(Reader::open(&path).is_err());
    }

    #[test]
    fn reads_a_static_frame_with_a_default_delay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("still.webp");
        image::RgbaImage::new(3, 2)
            .save_with_format(&path, image::ImageFormat::WebP)
            .unwrap();
        let mut reader = Reader::open(&path).unwrap();
        assert_eq!(reader.dimensions(), (3, 2));
        assert_eq!(reader.frame_count(), 1);
        assert!(reader.buffer.is_empty());
        let frame = reader.read_frame().unwrap();
        assert_eq!(frame.delay_ms, 1_000);
        assert_eq!(frame.data.len(), 3 * 2 * 4);
    }
}
