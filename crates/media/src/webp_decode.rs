use crate::info::MediaInfo;
use std::io::BufReader;
use std::path::Path;

pub(crate) struct WebpFrame<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) delay_ms: u32,
}

pub(crate) struct WebpReader {
    decoder: image_webp::WebPDecoder<BufReader<std::fs::File>>,
    buffer: Vec<u8>,
    buffer_size: usize,
    width: u32,
    height: u32,
    alpha: bool,
    animated: bool,
    frames: u32,
    duration_ms: u64,
}

impl WebpReader {
    pub(crate) fn open(path: &Path) -> anyhow::Result<Self> {
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
        Ok(Self {
            width,
            height,
            alpha: decoder.has_alpha(),
            animated: decoder.is_animated(),
            frames: decoder.num_frames().max(1),
            duration_ms: decoder.loop_duration(),
            decoder,
            buffer,
            buffer_size,
        })
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn has_alpha(&self) -> bool {
        self.alpha
    }

    pub(crate) fn is_animated(&self) -> bool {
        self.animated
    }

    pub(crate) fn frame_count(&self) -> u32 {
        self.frames
    }

    pub(crate) fn duration_ms(&self) -> u64 {
        self.duration_ms
    }

    pub(crate) fn info(&self) -> MediaInfo {
        crate::info::probe_webp(&self.decoder)
    }

    pub(crate) fn read_frame(&mut self) -> anyhow::Result<WebpFrame<'_>> {
        if self.buffer.len() != self.buffer_size {
            self.buffer.resize(self.buffer_size, 0);
        }
        let delay_ms = if self.animated {
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

#[cfg(test)]
mod tests {
    use super::WebpReader;

    #[test]
    fn rejects_invalid_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.webp");
        std::fs::write(&path, b"not webp").unwrap();
        assert!(WebpReader::open(&path).is_err());
    }

    #[test]
    fn reads_a_static_frame_with_a_default_delay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("still.webp");
        image::RgbaImage::new(3, 2)
            .save_with_format(&path, image::ImageFormat::WebP)
            .unwrap();
        let mut reader = WebpReader::open(&path).unwrap();
        assert_eq!(reader.dimensions(), (3, 2));
        assert_eq!(reader.frame_count(), 1);
        assert!(reader.buffer.is_empty());
        let frame = reader.read_frame().unwrap();
        assert_eq!(frame.delay_ms, 1_000);
        assert_eq!(frame.data.len(), 3 * 2 * 4);
    }
}
