use eframe::egui;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::Instant;

struct HoverPlayer {
    file: PathBuf,
    frames: Vec<(f64, egui::TextureHandle)>,
    interval: f64,
    duration: Option<f64>,
    origin: Instant,
    rx: Receiver<(f64, mediaa2_core::Frame)>,
}

impl HoverPlayer {
    fn start(ctx: &egui::Context, file: &Path, duration: Option<f64>) -> Self {
        let interval = mediaa2_media::preview::preview_interval(duration, 15.0, 240);
        let (tx, rx) = std::sync::mpsc::channel();
        let path = file.to_path_buf();
        let cancel = Arc::new(AtomicBool::new(false));
        std::thread::spawn(move || {
            let mut count = 0;
            let _ = mediaa2_media::preview::preview_frames(
                &path,
                320,
                interval,
                0.0,
                &cancel,
                &mut |frame| {
                    count += 1;
                    tx.send((frame.at, frame.frame)).is_ok() && count < 240
                },
            );
        });
        let _ = ctx;
        Self {
            file: file.to_path_buf(),
            frames: Vec::new(),
            interval,
            duration,
            origin: Instant::now(),
            rx,
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        for (at, frame) in self.rx.try_iter().take(6) {
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [frame.width as usize, frame.height as usize],
                &frame.data,
            );
            let texture = ctx.load_texture("library-hover", image, egui::TextureOptions::LINEAR);
            let index = self.frames.partition_point(|(time, _)| *time < at);
            self.frames.insert(index, (at, texture));
        }
    }

    fn length(&self) -> f64 {
        let decoded = self.frames.last().map_or(0.0, |(at, _)| at + self.interval);
        self.duration.unwrap_or(decoded).max(decoded).max(0.1)
    }

    fn frame(&self) -> Option<egui::TextureHandle> {
        if self.frames.is_empty() {
            return None;
        }
        let position = self.origin.elapsed().as_secs_f64() % self.length();
        let index = self
            .frames
            .partition_point(|(at, _)| *at <= position)
            .saturating_sub(1);
        Some(self.frames[index].1.clone())
    }
}

#[derive(Default)]
pub struct Textures {
    previews: HashMap<PathBuf, egui::TextureHandle>,
    icons: HashMap<&'static str, egui::TextureHandle>,
    hover: HashMap<PathBuf, HoverPlayer>,
    #[cfg(test)]
    static_hover: HashMap<PathBuf, PathBuf>,
}

impl Textures {
    #[cfg(test)]
    pub(crate) fn seed_hover_static(&mut self, key: &Path, file: &Path) {
        self.static_hover
            .insert(key.to_path_buf(), file.to_path_buf());
    }
    pub fn get_or_load(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
    ) -> Option<&egui::TextureHandle> {
        if !self.previews.contains_key(path) {
            let color = if is_raw_frame(path) {
                read_raw_frame(path)?
            } else {
                let image = image::open(path).ok()?;
                let size = [image.width() as usize, image.height() as usize];
                let rgba = image.to_rgba8();
                let pixels = rgba.as_flat_samples();
                egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_slice())
            };
            let texture =
                ctx.load_texture(path.to_string_lossy(), color, egui::TextureOptions::LINEAR);
            self.previews.insert(path.to_path_buf(), texture);
        }
        self.previews.get(path)
    }

    pub fn clear(&mut self, path: &Path) {
        self.previews.remove(path);
        self.hover.remove(path);
    }

    pub fn hover_frame(
        &mut self,
        ctx: &egui::Context,
        key: &Path,
        file: &Path,
        duration: Option<f64>,
    ) -> Option<egui::TextureHandle> {
        #[cfg(test)]
        if let Some(file) = self.static_hover.get(key).cloned() {
            return self.get_or_load(ctx, &file).cloned();
        }
        let restart = self.hover.get(key).is_none_or(|player| player.file != file);
        if restart {
            self.hover
                .insert(key.to_path_buf(), HoverPlayer::start(ctx, file, duration));
        }
        let player = self.hover.get_mut(key)?;
        player.poll(ctx);
        player.frame()
    }

    pub fn app_icon(&mut self, ctx: &egui::Context, agent: bool) -> egui::TextureHandle {
        let key = if agent { "mediaa-agent" } else { "mediaa" };
        if let Some(texture) = self.icons.get(key) {
            return texture.clone();
        }
        let icon = mediaa2_platform::tray_icon::icon(agent, 2);
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [icon.width as usize, icon.height as usize],
            &icon.rgba,
        );
        let texture = ctx.load_texture(key, image, egui::TextureOptions::LINEAR);
        self.icons.insert(key, texture.clone());
        texture
    }
}

fn is_raw_frame(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("rgba")
}

fn read_raw_frame(path: &Path) -> Option<egui::ColorImage> {
    const MAGIC: &[u8; 4] = b"MPK1";
    let bytes = std::fs::read(path).ok()?;
    if bytes.len() < 16 || &bytes[..4] != MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
    let height = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let stride = u32::from_le_bytes(bytes[12..16].try_into().ok()?) as usize;
    if stride != width * 4 || bytes.len() != 16 + width * height * 4 {
        return None;
    }
    Some(egui::ColorImage::from_rgba_unmultiplied(
        [width, height],
        &bytes[16..],
    ))
}
