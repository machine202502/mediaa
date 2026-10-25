use crate::cursor_overlay::CursorImage;
use anyhow::{anyhow, bail, Context as _};
use mediaa_core::source::FrameSource;
use mediaa_core::{Frame, Rect};
use xcb::{shm, x, xfixes, Connection};

#[derive(Debug, Clone, Copy)]
pub enum Target {
    Area(Rect),
    Window(u32),
}

pub struct X11Source {
    conn: Connection,
    root: x::Window,
    target: Target,
    name: String,
    size: (u32, u32),
    seg: shm::Seg,
    mem: *mut u8,
    mem_len: usize,
    with_cursor: bool,
    cursor: Option<(u32, CursorImage)>,
}

impl X11Source {
    pub fn available() -> bool {
        std::env::var_os("WAYLAND_DISPLAY").is_none()
            && std::env::var("XDG_SESSION_TYPE").map_or(true, |t| t != "wayland")
            && std::env::var_os("DISPLAY").is_some()
    }

    pub fn new(target: Target, name: String, with_cursor: bool) -> anyhow::Result<Self> {
        let (conn, screen_num) = Connection::connect_with_extensions(
            None,
            &[xcb::Extension::Shm],
            &[xcb::Extension::XFixes],
        )
        .context("X11 connection")?;
        let setup = conn.get_setup();
        if setup.image_byte_order() != x::ImageOrder::LsbFirst {
            bail!("unsupported X11 byte order");
        }
        let screen = setup
            .roots()
            .nth(screen_num as usize)
            .context("no X11 screen")?;
        let root = screen.root();
        let depth = screen.root_depth();
        let bpp = setup
            .pixmap_formats()
            .iter()
            .find(|f| f.depth() == depth)
            .map(|f| f.bits_per_pixel());
        if !matches!(depth, 24 | 32) || bpp != Some(32) {
            bail!("unsupported X11 pixel format (depth {depth}, {bpp:?} bpp)");
        }
        let size = match target {
            Target::Area(r) => (r.w as u32 & !1, r.h as u32 & !1),
            Target::Window(id) => {
                let g = conn.wait_for_reply(conn.send_request(&x::GetGeometry {
                    drawable: x::Drawable::Window(window(id)),
                }))?;
                (u32::from(g.width()) & !1, u32::from(g.height()) & !1)
            }
        };
        anyhow::ensure!(size.0 >= 16 && size.1 >= 16, "area is too small to record");
        let with_cursor = with_cursor
            && conn
                .active_extensions()
                .any(|e| e == xcb::Extension::XFixes)
            && {
                let v = conn.send_request(&xfixes::QueryVersion {
                    client_major_version: 4,
                    client_minor_version: 0,
                });
                conn.wait_for_reply(v).is_ok()
            };

        let mem_len = (size.0 * size.1 * 4) as usize;
        // SAFETY: plain System V shared memory calls; the segment is marked for removal right
        // after the X server attached it, so it disappears with the last detach.
        let (shmid, mem) = unsafe {
            let id = libc::shmget(libc::IPC_PRIVATE, mem_len, libc::IPC_CREAT | 0o600);
            if id < 0 {
                bail!("shmget failed: {}", std::io::Error::last_os_error());
            }
            let addr = libc::shmat(id, std::ptr::null(), 0);
            if addr as isize == -1 {
                libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
                bail!("shmat failed: {}", std::io::Error::last_os_error());
            }
            (id, addr.cast::<u8>())
        };
        let seg: shm::Seg = conn.generate_id();
        let attached = conn.send_and_check_request(&shm::Attach {
            shmseg: seg,
            shmid: shmid as u32,
            read_only: false,
        });
        // SAFETY: see above.
        unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };
        if let Err(e) = attached {
            // SAFETY: `mem` came from shmat above.
            unsafe { libc::shmdt(mem.cast()) };
            bail!("MIT-SHM attach failed: {e}");
        }
        let mut s = Self {
            conn,
            root,
            target,
            name,
            size,
            seg,
            mem,
            mem_len,
            with_cursor,
            cursor: None,
        };
        s.grab().context("first X11 frame")?;
        Ok(s)
    }

    fn shm_get(
        &self,
        drawable: x::Drawable,
        x: i16,
        y: i16,
        w: u16,
        h: u16,
    ) -> anyhow::Result<&[u8]> {
        let cookie = self.conn.send_request(&shm::GetImage {
            drawable,
            x,
            y,
            width: w,
            height: h,
            plane_mask: u32::MAX,
            format: x::ImageFormat::ZPixmap as u8,
            shmseg: self.seg,
            offset: 0,
        });
        self.conn
            .wait_for_reply(cookie)
            .map_err(|e| anyhow!("X11 capture: {e}"))?;
        let len = usize::from(w) * usize::from(h) * 4;
        // SAFETY: the segment holds `mem_len >= len` bytes, filled by the server for this reply.
        Ok(unsafe { std::slice::from_raw_parts(self.mem, len.min(self.mem_len)) })
    }

    fn draw_cursor(&mut self, frame: &mut Frame, origin: (i32, i32)) {
        let Ok(c) = self
            .conn
            .wait_for_reply(self.conn.send_request(&xfixes::GetCursorImage {}))
        else {
            return;
        };
        let serial = c.cursor_serial();
        if self.cursor.as_ref().is_none_or(|(s, _)| *s != serial) {
            self.cursor = Some((serial, cursor_image(&c)));
        }
        if let Some((_, img)) = &self.cursor {
            let (x, y) = (i32::from(c.x()) - origin.0, i32::from(c.y()) - origin.1);
            crate::cursor_overlay::draw(frame, img, x, y);
        }
    }
}

fn window(id: u32) -> x::Window {
    <x::Window as xcb::XidNew>::new(id)
}

fn cursor_image(c: &xfixes::GetCursorImageReply) -> CursorImage {
    let data = c.cursor_image().iter().flat_map(|&p| {
        let a = (p >> 24) as u8;
        let un = |v: u32| {
            if a == 0 {
                0
            } else {
                ((v & 0xff) * 255 / u32::from(a)).min(255) as u8
            }
        };
        [un(p >> 16), un(p >> 8), un(p), a]
    });
    CursorImage {
        image: Frame::new(u32::from(c.width()), u32::from(c.height()), data.collect()),
        hotspot: (i32::from(c.xhot()), i32::from(c.yhot())),
    }
}

fn bgrx_to_rgba(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len());
    for p in src.as_chunks::<4>().0 {
        out.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    out
}

impl FrameSource for X11Source {
    fn name(&self) -> String {
        self.name.clone()
    }

    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn grab(&mut self) -> anyhow::Result<Frame> {
        let (w, h) = self.size;
        let (frame, origin) = match self.target {
            Target::Area(r) => {
                let data = self.shm_get(
                    x::Drawable::Window(self.root),
                    r.x as i16,
                    r.y as i16,
                    w as u16,
                    h as u16,
                )?;
                (Frame::new(w, h, bgrx_to_rgba(data)), (r.x, r.y))
            }
            Target::Window(id) => {
                let win = window(id);
                let g = self
                    .conn
                    .wait_for_reply(self.conn.send_request(&x::GetGeometry {
                        drawable: x::Drawable::Window(win),
                    }))?;
                let (cw, ch) = (u32::from(g.width()).min(w), u32::from(g.height()).min(h));
                let data = self.shm_get(x::Drawable::Window(win), 0, 0, cw as u16, ch as u16)?;
                let frame = Frame::new(cw, ch, bgrx_to_rgba(data)).fit_to(w, h, [0, 0, 0, 255]);
                let pos =
                    self.conn
                        .wait_for_reply(self.conn.send_request(&x::TranslateCoordinates {
                            src_window: win,
                            dst_window: self.root,
                            src_x: 0,
                            src_y: 0,
                        }))?;
                (frame, (i32::from(pos.dst_x()), i32::from(pos.dst_y())))
            }
        };
        let mut frame = frame;
        if self.with_cursor {
            self.draw_cursor(&mut frame, origin);
        }
        Ok(frame)
    }
}

impl Drop for X11Source {
    fn drop(&mut self) {
        let _ = self
            .conn
            .send_and_check_request(&shm::Detach { shmseg: self.seg });
        // SAFETY: `mem` came from shmat in `new` and is detached once.
        unsafe { libc::shmdt(self.mem.cast()) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bgrx_becomes_opaque_rgba() {
        assert_eq!(
            bgrx_to_rgba(&[1, 2, 3, 0, 10, 20, 30, 7]),
            [3, 2, 1, 255, 30, 20, 10, 255]
        );
    }

    #[test]
    fn records_the_root_window_when_x11_is_there() {
        if !X11Source::available() {
            return;
        }
        let mut s =
            X11Source::new(Target::Area(Rect::new(0, 0, 320, 200)), "test".into(), true).unwrap();
        let f = s.grab().unwrap();
        assert_eq!((f.width, f.height), (320, 200));
        assert!(f.data.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    }
}
