//! Raw RGBA frames and helpers to keep frame sizes stable during a recording.

use crate::geometry::Rect;

/// Tightly packed RGBA8 frame (`width * height * 4` bytes, top row first).
#[derive(Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl std::fmt::Debug for Frame {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame({}x{})", self.width, self.height)
    }
}

impl Frame {
    pub fn new(width: u32, height: u32, data: Vec<u8>) -> Self {
        debug_assert_eq!(data.len(), width as usize * height as usize * 4);
        Self {
            width,
            height,
            data,
        }
    }

    /// Solid-colour frame.
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let data = rgba
            .iter()
            .copied()
            .cycle()
            .take(width as usize * height as usize * 4)
            .collect();
        Self::new(width, height, data)
    }

    pub fn from_image(img: image::RgbaImage) -> Self {
        let (w, h) = img.dimensions();
        Self::new(w, h, img.into_raw())
    }

    pub fn into_image(self) -> image::RgbaImage {
        image::RgbaImage::from_raw(self.width, self.height, self.data)
            .expect("frame buffer matches dimensions")
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        [self.data[i], self.data[i + 1], self.data[i + 2], self.data[i + 3]]
    }

    /// Copies `rect` out of the frame; the rect is clamped to the frame bounds.
    pub fn crop(&self, rect: Rect) -> Frame {
        let bounds = Rect::new(0, 0, self.width as i32, self.height as i32);
        let r = rect.clamp_to(&bounds);
        let (w, h) = (r.w.max(0) as usize, r.h.max(0) as usize);
        let mut data = Vec::with_capacity(w * h * 4);
        let stride = self.width as usize * 4;
        for row in 0..h {
            let start = (r.y as usize + row) * stride + r.x as usize * 4;
            data.extend_from_slice(&self.data[start..start + w * 4]);
        }
        Frame::new(w as u32, h as u32, data)
    }

    /// Centres the frame on a `width`×`height` canvas filled with `fill`, scaling it down
    /// (aspect kept) when it is larger, so nothing is cut off. Keeps recordings at a constant
    /// size when a window is resized (or a display changes resolution) mid-recording.
    pub fn fit_to(&self, width: u32, height: u32, fill: [u8; 4]) -> Frame {
        if self.width == width && self.height == height {
            return self.clone();
        }
        let scale = (f64::from(width) / f64::from(self.width.max(1))).min(f64::from(height) / f64::from(self.height.max(1)));
        // One extra pixel (sizes rounded down to even for the encoder) is cropped, not scaled.
        let slightly_larger = self.width <= width + 1 && self.height <= height + 1;
        let scaled;
        let src = if scale < 1.0 && !slightly_larger {
            let w = ((f64::from(self.width) * scale).round() as u32).clamp(1, width);
            let h = ((f64::from(self.height) * scale).round() as u32).clamp(1, height);
            scaled = self.resized(w, h);
            &scaled
        } else {
            self
        };
        let mut out = Frame::filled(width, height, fill);
        let (cw, ch) = (src.width.min(width) as usize, src.height.min(height) as usize);
        let (x0, y0) = ((width as usize - cw) / 2, (height as usize - ch) / 2);
        let (src_stride, dst_stride) = (src.width as usize * 4, width as usize * 4);
        for row in 0..ch {
            let s = row * src_stride;
            let d = (y0 + row) * dst_stride + x0 * 4;
            out.data[d..d + cw * 4].copy_from_slice(&src.data[s..s + cw * 4]);
        }
        out
    }

    /// The frame scaled to `width`×`height` (bilinear, SIMD).
    pub fn resized(&self, width: u32, height: u32) -> Frame {
        use fast_image_resize::{images::Image, images::ImageRef, FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer};
        let src = ImageRef::new(self.width, self.height, &self.data, PixelType::U8x4).expect("frame buffer matches its size");
        let mut dst = Image::new(width, height, PixelType::U8x4);
        Resizer::new()
            .resize(&src, &mut dst, &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear)))
            .expect("same pixel type");
        Frame::new(width, height, dst.into_vec())
    }

    /// Composites the frame over an opaque background colour (for formats without alpha).
    pub fn flatten(&mut self, bg: [u8; 3]) {
        for px in self.data.as_chunks_mut::<4>().0 {
            let a = u32::from(px[3]);
            if a == 255 {
                continue;
            }
            for c in 0..3 {
                px[c] = ((u32::from(px[c]) * a + u32::from(bg[c]) * (255 - a) + 127) / 255) as u8;
            }
            px[3] = 255;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Frame {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        Frame::new(w, h, data)
    }

    #[test]
    fn crop_extracts_region() {
        let f = gradient(10, 8);
        let c = f.crop(Rect::new(2, 3, 4, 2));
        assert_eq!((c.width, c.height), (4, 2));
        assert_eq!(c.pixel(0, 0), [2, 3, 0, 255]);
        assert_eq!(c.pixel(3, 1), [5, 4, 0, 255]);
    }

    #[test]
    fn crop_is_clamped() {
        let f = gradient(10, 8);
        let c = f.crop(Rect::new(8, 6, 10, 10));
        assert_eq!((c.width, c.height), (2, 2));
        assert_eq!(c.pixel(1, 1), [9, 7, 0, 255]);
    }

    #[test]
    fn fit_centres_and_scales_down_without_cutting() {
        let f = gradient(4, 4);
        // A smaller frame is centred on the canvas.
        let bigger = f.fit_to(6, 6, [9, 9, 9, 255]);
        assert_eq!(bigger.pixel(1, 1), [0, 0, 0, 255]);
        assert_eq!(bigger.pixel(4, 4), [3, 3, 0, 255]);
        assert_eq!(bigger.pixel(0, 0), [9, 9, 9, 255]);
        assert_eq!(bigger.pixel(5, 5), [9, 9, 9, 255]);
        // A larger one (a window that grew) is scaled down whole, aspect kept.
        let tall = Frame::filled(20, 40, [200, 10, 10, 255]);
        let fitted = tall.fit_to(20, 20, [0, 0, 0, 255]);
        assert_eq!((fitted.width, fitted.height), (20, 20));
        assert_eq!(fitted.pixel(10, 0), [200, 10, 10, 255], "top row kept");
        assert_eq!(fitted.pixel(10, 19), [200, 10, 10, 255], "bottom row kept, nothing cut");
        assert_eq!(fitted.pixel(0, 10), [0, 0, 0, 255], "side bars");
        assert_eq!(fitted.pixel(19, 10), [0, 0, 0, 255]);
        assert_eq!(f.resized(2, 2).width, 2);
        // An odd size rounded down to even: the extra pixel is cropped, nothing is rescaled.
        let odd = gradient(5, 5).fit_to(4, 4, [0; 4]);
        assert_eq!(odd.pixel(3, 3), [3, 3, 0, 255]);
    }

    #[test]
    fn flatten_blends_alpha() {
        let mut f = Frame::new(2, 1, vec![255, 0, 0, 0, 255, 0, 0, 128]);
        f.flatten([0, 0, 255]);
        assert_eq!(f.pixel(0, 0), [0, 0, 255, 255]);
        assert_eq!(f.pixel(1, 0), [128, 0, 127, 255]);
    }

    #[test]
    fn image_roundtrip() {
        let f = gradient(3, 2);
        let back = Frame::from_image(f.clone().into_image());
        assert_eq!(back, f);
    }
}
