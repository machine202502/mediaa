use crate::cursor_overlay::CursorImage;

#[derive(Debug, Clone)]
pub struct CursorState {
    pub x: i32,
    pub y: i32,
    pub image: CursorImage,
}

pub fn current() -> Option<CursorState> {
    imp::current()
}

pub fn position() -> Option<(i32, i32)> {
    imp::position()
}

#[cfg(windows)]
use crate::windows::cursor as imp;

#[cfg(not(windows))]
use crate::unix::cursor as imp;

#[cfg(test)]
mod tests {
    #[test]
    fn current_cursor_is_sane_when_available() {
        if let Some(c) = super::current() {
            let img = &c.image.image;
            assert!(img.width > 0 && img.height > 0);
            assert!(c.image.hotspot.0 >= 0 && c.image.hotspot.0 < img.width as i32);
            assert!(
                img.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "cursor is not fully transparent"
            );
        }
    }
}
