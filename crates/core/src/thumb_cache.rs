//! Thumbnail cache of the library, kept in its cache folder (`.cache`): thumbnails are packed
//! into atlas pages (`thumbs-N.jpg`, one JPEG holds dozens of them) whose slots are rows of the
//! `thumbs` table in the cache database (see `cache_db`), so showing a screen of tiles after a
//! restart reads a couple of files instead of decoding every video again.
//!
//! Pages are filled shelf by shelf; the page being filled stays in memory and is written now
//! and then ([`ThumbCache::flush`]) together with the rows of its new thumbnails (a row never
//! points at pixels that are not on disk yet). A thumbnail is found by file name and is valid
//! while the file keeps its size and modification time. Pages whose thumbnails are all gone
//! are deleted.

use crate::Frame;
use image::{ImageFormat, RgbImage};
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Atlas page side in pixels (a 512 px thumbnail fits several times per row).
const PAGE: u32 = 2048;
/// JPEG quality of the pages.
const QUALITY: u8 = 85;
/// Decoded pages kept in memory (tiles on screen usually come from one or two pages).
const KEEP_PAGES: usize = 4;

fn page_file(dir: &Path, page: u32) -> PathBuf {
    dir.join(format!("thumbs-{page}.jpg"))
}

fn to_ms(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Where a thumbnail lives and which version of its file it shows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slot {
    size: u64,
    modified_ms: u64,
    page: u32,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// The page being filled.
struct OpenPage {
    id: u32,
    image: RgbImage,
    shelf_x: u32,
    shelf_y: u32,
    shelf_h: u32,
    used_h: u32,
}

impl OpenPage {
    fn new(id: u32) -> Self {
        Self { id, image: RgbImage::new(PAGE, PAGE), shelf_x: 0, shelf_y: 0, shelf_h: 0, used_h: 0 }
    }

    /// Top-left corner for a `w`×`h` thumbnail, `None` when the page is full.
    fn place(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        if self.shelf_x + w > PAGE {
            self.shelf_y += self.shelf_h;
            (self.shelf_x, self.shelf_h) = (0, 0);
        }
        if self.shelf_y + h > PAGE {
            return None;
        }
        let at = (self.shelf_x, self.shelf_y);
        self.shelf_x += w;
        self.shelf_h = self.shelf_h.max(h);
        self.used_h = self.used_h.max(self.shelf_y + h);
        Some(at)
    }
}

pub struct ThumbCache {
    dir: PathBuf,
    /// `None` if the database cannot be opened (thumbnails then live for this session only).
    db: Option<Connection>,
    items: HashMap<String, Slot>,
    next_page: u32,
    open: Option<OpenPage>,
    /// Thumbnails put on the open page since it was last written (their rows go with it).
    pending: Vec<String>,
    /// Recently decoded pages, most recent last.
    pages: Vec<(u32, RgbImage)>,
}

fn load_items(db: &Connection) -> rusqlite::Result<(HashMap<String, Slot>, u32)> {
    let mut q = db.prepare("SELECT name, size, modified_ms, page, x, y, w, h FROM thumbs")?;
    let items = q
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Slot {
                    size: r.get::<_, i64>(1)? as u64,
                    modified_ms: r.get::<_, i64>(2)? as u64,
                    page: r.get(3)?,
                    x: r.get(4)?,
                    y: r.get(5)?,
                    w: r.get(6)?,
                    h: r.get(7)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<HashMap<_, _>>>()?;
    let next_page: Option<u32> = db.query_row("SELECT value FROM counters WHERE key = 'next_page'", [], |r| r.get(0)).ok();
    Ok((items, next_page.unwrap_or(0)))
}

impl ThumbCache {
    /// The cache in `dir` (the library's cache folder); missing or unreadable: starts empty.
    pub fn open(dir: &Path) -> ThumbCache {
        let db = crate::cache_db::open(dir).map_err(|e| log::warn!("thumbnail cache: {e:#}")).ok();
        let (items, next_page) = db
            .as_ref()
            .and_then(|db| load_items(db).map_err(|e| log::warn!("thumbnail cache: {e}")).ok())
            .unwrap_or_default();
        ThumbCache { dir: dir.to_path_buf(), db, items, next_page, open: None, pending: Vec::new(), pages: Vec::new() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// New thumbnails wait to be written ([`ThumbCache::flush`]).
    pub fn is_dirty(&self) -> bool {
        !self.pending.is_empty()
    }

    fn sql(&self, what: impl FnOnce(&Connection) -> rusqlite::Result<()>) {
        if let Some(db) = &self.db {
            if let Err(e) = what(db) {
                log::warn!("thumbnail cache: {e}");
            }
        }
    }

    /// The cached thumbnail of file `name`, if it was made from this version of the file.
    pub fn get(&mut self, name: &str, size: u64, modified: SystemTime) -> Option<Frame> {
        let slot = *self.items.get(name)?;
        if slot.size != size || slot.modified_ms != to_ms(modified) {
            return None;
        }
        let crop = |img: &RgbImage| {
            let mut data = Vec::with_capacity((slot.w * slot.h * 4) as usize);
            for y in slot.y..slot.y + slot.h {
                for x in slot.x..slot.x + slot.w {
                    let p = img.get_pixel(x, y).0;
                    data.extend_from_slice(&[p[0], p[1], p[2], 255]);
                }
            }
            Frame::new(slot.w, slot.h, data)
        };
        if let Some(open) = self.open.as_ref().filter(|o| o.id == slot.page) {
            return Some(crop(&open.image));
        }
        let pos = match self.pages.iter().position(|(id, _)| *id == slot.page) {
            Some(pos) => pos,
            None => {
                let loaded = std::fs::read(page_file(&self.dir, slot.page))
                    .map_err(anyhow::Error::from)
                    .and_then(|b| Ok(image::load_from_memory_with_format(&b, ImageFormat::Jpeg)?.to_rgb8()));
                match loaded {
                    Ok(img) => {
                        if self.pages.len() >= KEEP_PAGES {
                            self.pages.remove(0);
                        }
                        self.pages.push((slot.page, img));
                        self.pages.len() - 1
                    }
                    Err(e) => {
                        // Page lost or damaged: forget what it held.
                        log::debug!("thumbnail page {}: {e:#}", slot.page);
                        self.items.retain(|_, s| s.page != slot.page);
                        self.sql(|db| db.execute("DELETE FROM thumbs WHERE page = ?1", [slot.page]).map(|_| ()));
                        return None;
                    }
                }
            }
        };
        let page = self.pages.remove(pos);
        let fits = page.1.width() >= slot.x + slot.w && page.1.height() >= slot.y + slot.h;
        let frame = fits.then(|| crop(&page.1));
        self.pages.push(page);
        if frame.is_none() {
            self.remove(name);
        }
        frame
    }

    /// Stores the thumbnail of file `name`. Thumbnails with transparency are not cached (pages
    /// are JPEG); returns whether it was stored.
    pub fn put(&mut self, name: &str, size: u64, modified: SystemTime, thumb: &Frame) -> bool {
        let (w, h) = (thumb.width, thumb.height);
        if w == 0 || h == 0 || w > PAGE || h > PAGE || thumb.data.as_chunks::<4>().0.iter().any(|p| p[3] != 255) {
            return false;
        }
        let at = match self.open.as_mut().and_then(|o| o.place(w, h)) {
            Some(at) => at,
            None => {
                if let Err(e) = self.flush() {
                    log::warn!("thumbnail cache: {e:#}");
                }
                let mut page = OpenPage::new(self.next_page);
                self.next_page += 1;
                let next = self.next_page;
                self.sql(|db| db.execute("INSERT OR REPLACE INTO counters VALUES ('next_page', ?1)", [next]).map(|_| ()));
                let at = page.place(w, h).expect("a thumbnail fits an empty page");
                self.open = Some(page);
                at
            }
        };
        let open = self.open.as_mut().expect("open page");
        for (i, p) in thumb.data.as_chunks::<4>().0.iter().enumerate() {
            let (x, y) = (i as u32 % w, i as u32 / w);
            open.image.put_pixel(at.0 + x, at.1 + y, image::Rgb([p[0], p[1], p[2]]));
        }
        let slot = Slot { size, modified_ms: to_ms(modified), page: open.id, x: at.0, y: at.1, w, h };
        if let Some(old) = self.items.insert(name.to_string(), slot) {
            self.release(old.page);
        }
        self.pending.push(name.to_string());
        true
    }

    /// Forgets the thumbnail of file `name` (deleted or renamed).
    pub fn remove(&mut self, name: &str) {
        if let Some(old) = self.items.remove(name) {
            self.pending.retain(|n| n != name);
            self.sql(|db| db.execute("DELETE FROM thumbs WHERE name = ?1", [name]).map(|_| ()));
            self.release(old.page);
        }
    }

    /// Keeps only the thumbnails of `names` (the files now in the library).
    pub fn retain(&mut self, names: &HashSet<String>) {
        let gone: Vec<String> = self.items.keys().filter(|n| !names.contains(*n)).cloned().collect();
        if gone.is_empty() {
            return;
        }
        for n in &gone {
            self.items.remove(n);
        }
        self.pending.retain(|n| names.contains(n));
        self.sql(|db| {
            let tx = db.unchecked_transaction()?;
            {
                let mut del = tx.prepare_cached("DELETE FROM thumbs WHERE name = ?1")?;
                for n in &gone {
                    del.execute([n])?;
                }
            }
            tx.commit()
        });
        let used: HashSet<u32> = self.items.values().map(|s| s.page).chain(self.open.as_ref().map(|o| o.id)).collect();
        for page in (0..self.next_page).filter(|p| !used.contains(p)) {
            self.pages.retain(|(id, _)| *id != page);
            let _ = std::fs::remove_file(page_file(&self.dir, page));
        }
    }

    /// Deletes page `page` once no thumbnail uses it.
    fn release(&mut self, page: u32) {
        if self.open.as_ref().is_some_and(|o| o.id == page) || self.items.values().any(|s| s.page == page) {
            return;
        }
        self.pages.retain(|(id, _)| *id != page);
        let _ = std::fs::remove_file(page_file(&self.dir, page));
    }

    /// Writes the page being filled (write-then-rename, never half-written), then the rows of
    /// its new thumbnails.
    pub fn flush(&mut self) -> anyhow::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        if let Some(open) = self.open.as_ref().filter(|o| o.used_h > 0) {
            std::fs::create_dir_all(&self.dir)?;
            let used = image::imageops::crop_imm(&open.image, 0, 0, PAGE, open.used_h).to_image();
            let mut bytes = Vec::new();
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, QUALITY).encode_image(&used)?;
            let path = page_file(&self.dir, open.id);
            let tmp = path.with_extension("jpg.tmp");
            std::fs::write(&tmp, bytes)?;
            std::fs::rename(&tmp, &path)?;
        }
        let pending = std::mem::take(&mut self.pending);
        if let Some(db) = &self.db {
            let tx = db.unchecked_transaction()?;
            {
                let mut put = tx.prepare_cached(
                    "INSERT OR REPLACE INTO thumbs (name, size, modified_ms, page, x, y, w, h) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                )?;
                for name in &pending {
                    if let Some(s) = self.items.get(name) {
                        put.execute(params![name, s.size as i64, s.modified_ms as i64, s.page, s.x, s.y, s.w, s.h])?;
                    }
                }
            }
            tx.commit()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn thumb(w: u32, h: u32, rgb: [u8; 3]) -> Frame {
        Frame::filled(w, h, [rgb[0], rgb[1], rgb[2], 255])
    }

    fn close(a: [u8; 4], b: [u8; 4]) -> bool {
        a.iter().zip(b).all(|(x, y)| x.abs_diff(y) <= 6)
    }

    fn pages(dir: &Path) -> usize {
        std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".jpg")).count()
    }

    #[test]
    fn thumbnails_survive_reopening_while_files_are_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut cache = ThumbCache::open(dir.path());
        assert!(cache.put("a.mp4", 10, t, &thumb(512, 288, [200, 30, 30])));
        assert!(cache.put("b.png", 20, t, &thumb(300, 512, [30, 200, 30])));
        assert!(!cache.put("alpha.png", 5, t, &Frame::filled(8, 8, [0, 0, 0, 0])), "transparent: not cached");
        let a = cache.get("a.mp4", 10, t).unwrap();
        assert_eq!((a.width, a.height), (512, 288));
        assert!(cache.is_dirty());
        assert!(ThumbCache::open(dir.path()).get("a.mp4", 10, t).is_none(), "rows wait for their page");
        cache.flush().unwrap();
        assert!(!cache.is_dirty());
        assert_eq!(pages(dir.path()), 1, "both in one atlas page");

        let mut cache = ThumbCache::open(dir.path());
        let b = cache.get("b.png", 20, t).unwrap();
        assert_eq!((b.width, b.height), (300, 512));
        assert!(close(b.pixel(150, 500), [30, 200, 30, 255]), "{:?}", b.pixel(150, 500));
        assert!(close(cache.get("a.mp4", 10, t).unwrap().pixel(0, 0), [200, 30, 30, 255]));
        assert!(cache.get("a.mp4", 11, t).is_none(), "file changed: stale");
        assert!(cache.get("a.mp4", 10, t + Duration::from_secs(1)).is_none());
        assert!(cache.get("missing.gif", 1, t).is_none());
        // New thumbnails go to a new page, never over the stored one.
        cache.put("c.gif", 1, t, &thumb(64, 64, [0, 0, 200]));
        cache.flush().unwrap();
        assert_eq!(pages(dir.path()), 2);
        assert!(close(ThumbCache::open(dir.path()).get("a.mp4", 10, t).unwrap().pixel(5, 5), [200, 30, 30, 255]));
    }

    #[test]
    fn pages_fill_up_and_empty_pages_are_deleted() {
        let dir = tempfile::tempdir().unwrap();
        let t = SystemTime::UNIX_EPOCH;
        let mut cache = ThumbCache::open(dir.path());
        // 4 per shelf, 4 shelves of 512 per 2048 page: the 17th starts a second page.
        for i in 0..17 {
            assert!(cache.put(&format!("{i}.png"), 1, t, &thumb(512, 512, [i as u8 * 10, 0, 0])));
        }
        cache.flush().unwrap();
        assert_eq!(pages(dir.path()), 2);
        assert!(close(cache.get("5.png", 1, t).unwrap().pixel(10, 10), [50, 0, 0, 255]));

        // Keeping only the file on the second page deletes the first page.
        cache.retain(&HashSet::from(["16.png".to_string()]));
        assert_eq!(pages(dir.path()), 1);
        drop(cache);
        let mut cache = ThumbCache::open(dir.path());
        assert!(cache.get("0.png", 1, t).is_none());
        assert!(cache.get("16.png", 1, t).is_some());
        cache.remove("16.png");
        assert_eq!(pages(dir.path()), 0, "removed at once");
        assert!(ThumbCache::open(dir.path()).get("16.png", 1, t).is_none());
    }

    #[test]
    fn lost_page_is_forgotten() {
        let dir = tempfile::tempdir().unwrap();
        let t = SystemTime::UNIX_EPOCH;
        let mut cache = ThumbCache::open(dir.path());
        cache.put("a.png", 1, t, &thumb(64, 64, [1, 2, 3]));
        cache.flush().unwrap();
        std::fs::remove_file(page_file(dir.path(), 0)).unwrap();
        let mut cache = ThumbCache::open(dir.path());
        assert!(cache.get("a.png", 1, t).is_none());
        assert!(ThumbCache::open(dir.path()).items.is_empty(), "its rows are dropped");
    }
}
