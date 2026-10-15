//! YouTube-style hover preview of a library video / animation: frames are decoded in the
//! background (downscaled, at most ~15 fps), played muted in a loop and can be scrubbed.

use crossbeam_channel::Receiver;
use eframe::egui;
use mediaa_media::preview::{preview_frames, preview_interval, PreviewFrame};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const WIDTH: u32 = 480;
const MAX_FPS: f64 = 15.0;
/// Memory bound: long clips get sparser frames instead of more.
const MAX_FRAMES: u32 = 180;

/// One decoding run: from `from` seconds until `until` (the clip end, or where the first run
/// began). `to` is the time of its newest frame so far.
#[derive(Debug, Clone, Copy)]
struct Pass {
    from: f64,
    until: f64,
    to: f64,
    done: bool,
}

/// Message from the decoder thread: a frame of pass `.0`, or `None` when that pass ended.
type Msg = (usize, Option<PreviewFrame>);

pub struct HoverPreview {
    path: PathBuf,
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    /// Decoded frames, sorted by time.
    frames: Vec<(f64, egui::TextureHandle)>,
    passes: Vec<Pass>,
    interval: f64,
    duration: Option<f64>,
    resume: f64,
    /// Playback origin: the position is `origin.elapsed()` modulo the clip length.
    origin: Instant,
}

impl HoverPreview {
    /// Starts playback at `resume` seconds (where the user left it last time): the decoder
    /// seeks there first and fills in the beginning of the clip afterwards, for the loop.
    fn start(ctx: &egui::Context, path: &Path, duration: Option<f64>, resume: f64) -> Self {
        let interval = preview_interval(duration, MAX_FPS, MAX_FRAMES);
        let resume = resume.max(0.0);
        let mut passes = vec![Pass { from: resume, until: f64::INFINITY, to: f64::NEG_INFINITY, done: false }];
        if resume > 0.0 {
            passes.push(Pass { from: 0.0, until: resume, to: f64::NEG_INFINITY, done: false });
        }
        let (tx, rx) = crossbeam_channel::unbounded::<Msg>();
        let cancel = Arc::new(AtomicBool::new(false));
        let (p, c, ctx2, runs) = (path.to_path_buf(), cancel.clone(), ctx.clone(), passes.clone());
        let decode = move || {
            for (i, pass) in runs.iter().enumerate() {
                let res = preview_frames(&p, WIDTH, interval, pass.from, &c, &mut |frame| {
                    if frame.at >= pass.until {
                        return false;
                    }
                    ctx2.request_repaint();
                    tx.send((i, Some(frame))).is_ok()
                });
                if let Err(e) = res {
                    log::debug!("preview {}: {e:#}", p.display());
                }
                let _ = tx.send((i, None));
                ctx2.request_repaint();
                if c.load(Ordering::Relaxed) {
                    break;
                }
            }
        };
        if let Err(e) = std::thread::Builder::new().name("hover-preview".into()).spawn(decode) {
            log::warn!("preview thread: {e}");
        }
        Self {
            path: path.to_path_buf(),
            rx,
            cancel,
            frames: Vec::new(),
            passes,
            interval,
            duration,
            resume,
            origin: Instant::now() - Duration::from_secs_f64(resume),
        }
    }

    /// Uploads decoded frames (a few per UI frame, so hovering never stalls the UI).
    fn poll(&mut self, ctx: &egui::Context) {
        for (pass, frame) in self.rx.try_iter().take(6) {
            let p = &mut self.passes[pass];
            match frame {
                Some(f) => {
                    p.to = p.to.max(f.at);
                    let img = egui::ColorImage::from_rgba_unmultiplied([f.frame.width as usize, f.frame.height as usize], &f.frame.data);
                    let tex = ctx.load_texture("hover-preview", img, egui::TextureOptions::LINEAR);
                    let i = self.frames.partition_point(|(t, _)| *t < f.at);
                    self.frames.insert(i, (f.at, tex));
                }
                None => p.done = true,
            }
        }
    }

    /// Clip length used for looping and the timeline.
    pub fn length(&self) -> f64 {
        let decoded = self.frames.last().map_or(0.0, |(t, _)| t + self.interval);
        self.duration.unwrap_or(decoded.max(self.resume + self.interval)).max(decoded).max(0.1)
    }

    /// Current position in seconds (loops).
    pub fn position(&self) -> f64 {
        self.origin.elapsed().as_secs_f64() % self.length()
    }

    pub fn seek(&mut self, seconds: f64) {
        let t = seconds.clamp(0.0, self.length());
        self.origin = Instant::now() - Duration::from_secs_f64(t);
    }

    /// Whether the frame for `pos` has been decoded already.
    fn covered(&self, pos: f64) -> bool {
        let slack = self.interval * 1.5;
        self.passes
            .iter()
            .any(|p| pos >= p.from - 1e-6 && (pos <= p.to + slack || (p.done && pos <= p.until)))
    }

    /// Frame to show now, `None` until the decoder has reached the position (the tile then
    /// keeps its still thumbnail instead of racing through the clip).
    pub fn frame(&self) -> Option<&egui::TextureHandle> {
        let pos = self.position();
        if !self.covered(pos) {
            return None;
        }
        let times: Vec<f64> = self.frames.iter().map(|(t, _)| *t).collect();
        frame_at(&times, pos).map(|i| &self.frames[i].1)
    }

    pub fn is_loading(&self) -> bool {
        !self.covered(self.position()) && !self.passes.iter().all(|p| p.done)
    }
}

impl Drop for HoverPreview {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Index of the frame to show at `pos`: the last one starting at or before it (the first one
/// before any has started; `None` without frames). `times` is sorted.
fn frame_at(times: &[f64], pos: f64) -> Option<usize> {
    if times.is_empty() {
        return None;
    }
    Some(times.partition_point(|t| *t <= pos).saturating_sub(1))
}

/// The one preview playing at a time (the hovered tile), and where each clip was left so
/// hovering it again continues from there.
#[derive(Default)]
pub struct Previews {
    current: Option<HoverPreview>,
    resume: HashMap<PathBuf, f64>,
}

impl Previews {
    /// Preview of `path`, started if another (or none) is playing.
    pub fn play(&mut self, ctx: &egui::Context, path: &Path, duration: Option<f64>) -> &mut HoverPreview {
        if self.current.as_ref().is_none_or(|p| p.path != path) {
            self.stop();
            let resume = self.resume.get(path).copied().unwrap_or(0.0);
            self.current = Some(HoverPreview::start(ctx, path, duration, resume));
        }
        let p = self.current.as_mut().expect("just set");
        p.poll(ctx);
        ctx.request_repaint_after(Duration::from_millis(16));
        p
    }

    /// Stops the preview, remembering its position.
    pub fn stop(&mut self) {
        if let Some(p) = self.current.take() {
            self.resume.insert(p.path.clone(), p.position());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{frame_at, Previews};
    use std::path::Path;

    #[test]
    fn hovering_again_resumes_where_it_was_left() {
        let ctx = eframe::egui::Context::default();
        let mut previews = Previews::default();
        let (a, b) = (Path::new("missing-a.mp4"), Path::new("missing-b.mp4"));
        previews.play(&ctx, a, Some(10.0)).seek(4.0);
        previews.stop();
        previews.play(&ctx, b, Some(10.0)).seek(7.0);
        // Switching tiles also remembers the previous one.
        let pos = previews.play(&ctx, a, Some(10.0)).position();
        assert!((4.0..4.5).contains(&pos), "{pos}");
        let pos = previews.play(&ctx, b, Some(10.0)).position();
        assert!((7.0..7.5).contains(&pos), "{pos}");
        assert!(previews.play(&ctx, Path::new("new.mp4"), Some(10.0)).position() < 0.5, "new clips start at 0");
    }

    #[test]
    fn only_decoded_stretches_count_as_ready() {
        use super::{HoverPreview, Pass};
        let ctx = eframe::egui::Context::default();
        // Resumed at 4 s: the first pass decoded 4.0..=5.0, the second (0..4) nothing yet.
        let mut pv = HoverPreview::start(&ctx, Path::new("missing.mp4"), Some(10.0), 4.0);
        pv.interval = 0.1;
        pv.passes = vec![
            Pass { from: 4.0, until: f64::INFINITY, to: 5.0, done: false },
            Pass { from: 0.0, until: 4.0, to: f64::NEG_INFINITY, done: false },
        ];
        assert!(pv.covered(4.0) && pv.covered(5.1));
        assert!(!pv.covered(6.0), "ahead of the decoder");
        assert!(!pv.covered(1.0), "beginning not decoded yet");
        pv.passes[1].done = true;
        assert!(pv.covered(1.0) && pv.covered(3.99));
        assert!(!pv.covered(7.0), "a finished second pass does not cover the end");
        pv.passes[0].done = true;
        assert!(pv.covered(9.9));
    }

    #[test]
    fn frame_follows_the_position() {
        let t = [0.0, 0.1, 0.2, 0.3];
        assert_eq!(frame_at(&t, 0.0), Some(0));
        assert_eq!(frame_at(&t, 0.15), Some(1));
        assert_eq!(frame_at(&t, 0.3), Some(3));
        assert_eq!(frame_at(&t, 5.0), Some(3), "decoder behind: newest frame");
        assert_eq!(frame_at(&[], 1.0), None);
    }
}
