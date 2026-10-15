//! The mouse cursor drawn into recordings (screen capture APIs grab the screen without it).

use crate::frame::Frame;
use tiny_skia::{FillRule, Paint, PathBuilder, Pixmap, Stroke, Transform};

/// A cursor picture and its hotspot (the pixel that points), in image pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorImage {
    pub image: Frame,
    pub hotspot: (i32, i32),
}

/// The classic arrow (white, black outline) at `scale` × its 1x size of 12×19 px; used where
/// the OS cursor picture is not available.
pub fn arrow(scale: f32) -> CursorImage {
    let scale = scale.clamp(0.5, 8.0);
    let (w, h) = ((13.0 * scale).ceil() as u32, (20.0 * scale).ceil() as u32);
    let mut pixmap = Pixmap::new(w, h).expect("non-zero size");
    let mut pb = PathBuilder::new();
    let pts = [(0.0, 0.0), (0.0, 16.0), (4.0, 12.5), (7.0, 18.5), (9.5, 17.5), (6.5, 11.5), (11.5, 11.5)];
    let off = 0.75; // keep the outline inside the image
    pb.move_to(off + pts[0].0 * scale, off + pts[0].1 * scale);
    for (x, y) in &pts[1..] {
        pb.line_to(off + x * scale, off + y * scale);
    }
    pb.close();
    let path = pb.finish().expect("valid polygon");
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    paint.set_color_rgba8(255, 255, 255, 255);
    pixmap.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    paint.set_color_rgba8(0, 0, 0, 255);
    let stroke = Stroke { width: scale.max(1.0), ..Stroke::default() };
    pixmap.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    let data = pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    });
    CursorImage { image: Frame::new(w, h, data.collect()), hotspot: (1, 1) }
}

/// Alpha-blends `cursor` into `frame` with its hotspot at `(x, y)` (frame pixels); parts
/// outside the frame are clipped.
pub fn draw(frame: &mut Frame, cursor: &CursorImage, x: i32, y: i32) {
    let (w, h) = (frame.width, frame.height);
    let Some(mut canvas) = image::ImageBuffer::<image::Rgba<u8>, &mut [u8]>::from_raw(w, h, &mut frame.data[..]) else {
        return;
    };
    let top = image::ImageBuffer::<image::Rgba<u8>, &[u8]>::from_raw(cursor.image.width, cursor.image.height, &cursor.image.data[..])
        .expect("cursor buffer matches its size");
    image::imageops::overlay(&mut canvas, &top, i64::from(x - cursor.hotspot.0), i64::from(y - cursor.hotspot.1));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arrow_is_white_with_black_outline() {
        let a = arrow(2.0);
        assert_eq!((a.image.width, a.image.height), (26, 40));
        let inner = a.image.pixel(6, 14);
        assert_eq!(inner, [255, 255, 255, 255], "inside is white");
        let edge = a.image.pixel(1, 10);
        assert!(edge[3] > 200 && edge[0] < 80, "left edge is the black outline: {edge:?}");
        assert_eq!(a.image.pixel(25, 0)[3], 0, "outside is transparent");
    }

    #[test]
    fn draw_places_hotspot_and_clips() {
        let mut f = Frame::filled(40, 40, [0, 0, 255, 255]);
        let c = CursorImage { image: Frame::filled(4, 4, [255, 0, 0, 255]), hotspot: (1, 1) };
        draw(&mut f, &c, 10, 10);
        assert_eq!(f.pixel(9, 9), [255, 0, 0, 255], "hotspot offset applied");
        assert_eq!(f.pixel(12, 12), [255, 0, 0, 255]);
        assert_eq!(f.pixel(13, 13), [0, 0, 255, 255]);
        draw(&mut f, &c, 39, -1); // partly outside: no panic, visible part drawn
        assert_eq!(f.pixel(38, 0), [255, 0, 0, 255]);
        draw(&mut f, &c, 500, 500); // fully outside
    }

    #[test]
    fn translucent_pixels_blend() {
        let mut f = Frame::filled(4, 4, [0, 0, 0, 255]);
        let c = CursorImage { image: Frame::filled(1, 1, [255, 255, 255, 128]), hotspot: (0, 0) };
        draw(&mut f, &c, 1, 1);
        let p = f.pixel(1, 1);
        assert!(p[0] > 100 && p[0] < 160 && p[3] >= 254, "{p:?}");
    }
}
