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
mod imp {
    use super::CursorState;
    use crate::cursor_overlay::CursorImage;
    use mediaa_core::Frame;
    use std::cell::RefCell;
    use windows_sys::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GetObjectW, SelectObject,
        BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, DI_NORMAL, HCURSOR,
        ICONINFO,
    };

    thread_local! {
        static CACHE: RefCell<Option<(isize, CursorImage)>> = const { RefCell::new(None) };
    }

    fn info() -> Option<CURSORINFO> {
        let mut ci = CURSORINFO {
            cbSize: std::mem::size_of::<CURSORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: `ci` is a properly sized CURSORINFO.
        (unsafe { GetCursorInfo(&mut ci) } != 0).then_some(ci)
    }

    pub fn position() -> Option<(i32, i32)> {
        info().map(|ci| (ci.ptScreenPos.x, ci.ptScreenPos.y))
    }

    pub fn current() -> Option<CursorState> {
        let ci = info()?;
        if ci.flags & CURSOR_SHOWING == 0 || ci.hCursor.is_null() {
            return None;
        }
        let key = ci.hCursor as isize;
        let image = CACHE.with(|c| {
            let mut c = c.borrow_mut();
            match c.as_ref() {
                Some((k, img)) if *k == key => Some(img.clone()),
                _ => {
                    let img = picture(ci.hCursor)?;
                    *c = Some((key, img.clone()));
                    Some(img)
                }
            }
        })?;
        Some(CursorState {
            x: ci.ptScreenPos.x,
            y: ci.ptScreenPos.y,
            image,
        })
    }

    fn picture(cursor: HCURSOR) -> Option<CursorImage> {
        // SAFETY: plain GDI calls on objects created and released here; buffers are sized
        // from the bitmap dimensions GDI reports.
        unsafe {
            let mut ii = ICONINFO::default();
            if GetIconInfo(cursor, &mut ii) == 0 {
                return None;
            }
            let mut bm = BITMAP::default();
            GetObjectW(
                ii.hbmMask,
                std::mem::size_of::<BITMAP>() as i32,
                (&mut bm as *mut BITMAP).cast(),
            );
            let (w, mut h) = (bm.bmWidth, bm.bmHeight);
            if ii.hbmColor.is_null() {
                h /= 2;
            }
            DeleteObject(ii.hbmMask);
            if !ii.hbmColor.is_null() {
                DeleteObject(ii.hbmColor);
            }
            if !(1..=256).contains(&w) || !(1..=256).contains(&h) {
                return None;
            }
            let on_black = render(cursor, w, h, 0x00)?;
            let on_white = render(cursor, w, h, 0xFF)?;
            let mut data = Vec::with_capacity((w * h * 4) as usize);
            for (b, wh) in on_black
                .as_chunks::<4>()
                .0
                .iter()
                .zip(on_white.as_chunks::<4>().0)
            {
                let diff = i32::from(wh[1]) - i32::from(b[1]);
                if diff < 0 {
                    data.extend_from_slice(&[0, 0, 0, 255]);
                    continue;
                }
                let a = (255 - diff).clamp(0, 255);
                let un = |c: u8| {
                    if a == 0 {
                        0
                    } else {
                        (i32::from(c) * 255 / a).min(255) as u8
                    }
                };
                data.extend_from_slice(&[un(b[2]), un(b[1]), un(b[0]), a as u8]);
            }
            Some(CursorImage {
                image: Frame::new(w as u32, h as u32, data),
                hotspot: (ii.xHotspot as i32, ii.yHotspot as i32),
            })
        }
    }

    unsafe fn render(cursor: HCURSOR, w: i32, h: i32, bg: u8) -> Option<Vec<u8>> {
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                ..Default::default()
            },
            ..Default::default()
        };
        // SAFETY: see `picture`; `bits` points to `w * h * 4` bytes owned by the DIB.
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            if dc.is_null() {
                return None;
            }
            let mut bits = std::ptr::null_mut();
            let dib =
                CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0);
            if dib.is_null() || bits.is_null() {
                DeleteDC(dc);
                return None;
            }
            let len = (w * h * 4) as usize;
            let buf = std::slice::from_raw_parts_mut(bits.cast::<u8>(), len);
            buf.fill(bg);
            let old = SelectObject(dc, dib);
            DrawIconEx(dc, 0, 0, cursor, w, h, 0, std::ptr::null_mut(), DI_NORMAL);
            let out = buf.to_vec();
            SelectObject(dc, old);
            DeleteObject(dib);
            DeleteDC(dc);
            Some(out)
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use super::CursorState;
    use crate::cursor_overlay::{arrow, CursorImage};
    use device_query::{DeviceQuery, DeviceState};
    use std::sync::OnceLock;

    pub fn position() -> Option<(i32, i32)> {
        Some(DeviceState::new().get_mouse().coords)
    }

    pub fn current() -> Option<CursorState> {
        static ARROW: OnceLock<CursorImage> = OnceLock::new();
        let (x, y) = position()?;
        Some(CursorState {
            x,
            y,
            image: ARROW.get_or_init(|| arrow(1.0)).clone(),
        })
    }
}

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
