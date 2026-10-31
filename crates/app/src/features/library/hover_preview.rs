use crate::latest_worker::LatestWorker;
use crossbeam_channel::Receiver;
use eframe::egui;
use mediaa_media::preview::{preview_frames, preview_interval, PreviewFrame};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const WIDTH: u32 = 320;
const MAX_FPS: f64 = 15.0;
const MAX_FRAMES: u32 = 240;

#[derive(Debug, Clone, Copy)]
struct Pass {
    from: f64,
    until: f64,
    to: f64,
    done: bool,
}

struct Request {
    generation: u64,
    path: PathBuf,
    interval: f64,
    passes: Vec<Pass>,
}

type Msg = (u64, usize, Option<PreviewFrame>);

fn passes(resume: f64) -> Vec<Pass> {
    let mut passes = vec![Pass {
        from: resume,
        until: f64::INFINITY,
        to: f64::NEG_INFINITY,
        done: false,
    }];
    if resume > 0.0 {
        passes.push(Pass {
            from: 0.0,
            until: resume,
            to: f64::NEG_INFINITY,
            done: false,
        });
    }
    passes
}

pub struct HoverPreview {
    path: PathBuf,
    ready: bool,
    frames: Vec<(f64, egui::TextureHandle)>,
    passes: Vec<Pass>,
    interval: f64,
    duration: Option<f64>,
    resume: f64,
    origin: Instant,
}

impl HoverPreview {
    fn start(path: &Path, duration: Option<f64>, resume: f64, ready: bool) -> Self {
        let resume = resume.clamp(0.0, duration.unwrap_or(f64::INFINITY));
        let interval = preview_interval(duration, MAX_FPS, MAX_FRAMES);
        Self {
            path: path.to_path_buf(),
            ready,
            frames: Vec::new(),
            passes: passes(resume),
            interval,
            duration,
            resume,
            origin: Instant::now() - Duration::from_secs_f64(resume),
        }
    }

    fn poll(&mut self, ctx: &egui::Context, rx: &Receiver<Msg>, generation: u64) {
        for (_, pass, frame) in rx
            .try_iter()
            .filter(|(message_generation, _, _)| *message_generation == generation)
            .take(6)
        {
            let p = &mut self.passes[pass];
            match frame {
                Some(f) => {
                    p.to = p.to.max(f.at);
                    let img = egui::ColorImage::from_rgba_unmultiplied(
                        [f.frame.width as usize, f.frame.height as usize],
                        &f.frame.data,
                    );
                    let tex = ctx.load_texture("hover-preview", img, egui::TextureOptions::LINEAR);
                    let i = self.frames.partition_point(|(t, _)| *t < f.at);
                    self.frames.insert(i, (f.at, tex));
                }
                None => p.done = true,
            }
        }
    }

    pub fn length(&self) -> f64 {
        let decoded = self.frames.last().map_or(0.0, |(t, _)| t + self.interval);
        self.duration
            .unwrap_or(decoded.max(self.resume + self.interval))
            .max(decoded)
            .max(0.1)
    }

    pub fn position(&self) -> f64 {
        self.origin.elapsed().as_secs_f64() % self.length()
    }

    pub fn seek(&mut self, seconds: f64) {
        let t = seconds.clamp(0.0, self.length());
        self.origin = Instant::now() - Duration::from_secs_f64(t);
    }

    fn covered(&self, pos: f64) -> bool {
        let slack = self.interval * 1.5;
        self.passes
            .iter()
            .any(|p| pos >= p.from - 1e-6 && (pos <= p.to + slack || (p.done && pos <= p.until)))
    }

    pub fn frame(&self) -> Option<&egui::TextureHandle> {
        let pos = self.position();
        if !self.covered(pos) || self.frames.is_empty() {
            return None;
        }
        let index = self
            .frames
            .partition_point(|(time, _)| *time <= pos)
            .saturating_sub(1);
        Some(&self.frames[index].1)
    }

    pub fn is_loading(&self) -> bool {
        self.ready && !self.covered(self.position()) && !self.passes.iter().all(|p| p.done)
    }

    pub fn is_generating(&self) -> bool {
        !self.ready && self.origin.elapsed() < Duration::from_secs(300)
    }
}

#[cfg(test)]
fn frame_at(times: &[f64], pos: f64) -> Option<usize> {
    if times.is_empty() {
        return None;
    }
    Some(times.partition_point(|t| *t <= pos).saturating_sub(1))
}

#[derive(Default)]
pub struct Previews {
    current: Option<HoverPreview>,
    resume: HashMap<PathBuf, f64>,
    requested: HashSet<PathBuf>,
    worker: Option<LatestWorker<Request>>,
    rx: Option<Receiver<Msg>>,
    generation: u64,
}

impl Previews {
    pub fn play(
        &mut self,
        ctx: &egui::Context,
        path: &Path,
        duration: Option<f64>,
    ) -> &mut HoverPreview {
        let hover = mediaa_media::preview_cache::ready_hover_proxy(path);
        let replace = self.current.as_ref().is_none_or(|p| {
            p.path != path
                || (p.duration.is_none() && duration.is_some())
                || (!p.ready && hover.is_some())
        });
        if replace {
            self.stop();
            let resume = self.resume.get(path).copied().unwrap_or(0.0);
            let interval = preview_interval(duration, MAX_FPS, MAX_FRAMES);
            let source = hover
                .clone()
                .map(|proxy| proxy.into_path())
                .unwrap_or_else(|| path.to_path_buf());
            let ready = hover.is_some();
            let preview = HoverPreview::start(path, duration, resume, ready);
            self.generation += 1;
            let generation = self.generation;
            if let Some(rx) = &self.rx {
                while rx.try_recv().is_ok() {}
            }
            if ready {
                if let Some(worker) = self.ensure_worker() {
                    worker.request(Request {
                        generation,
                        path: source,
                        interval,
                        passes: passes(resume),
                    });
                }
            } else if self.requested.insert(path.to_path_buf()) {
                let proxy_path = path.to_path_buf();
                let _ = std::thread::Builder::new()
                    .name("hover-proxy".into())
                    .spawn(move || {
                        let cancel = std::sync::atomic::AtomicBool::new(false);
                        if let Err(error) =
                            mediaa_media::preview_cache::ensure_hover_proxy(&proxy_path, &cancel)
                        {
                            log::debug!("hover proxy {}: {error:#}", proxy_path.display());
                        }
                    });
            }
            self.current = Some(preview);
        }
        let generation = self.generation;
        let p = self.current.as_mut().expect("just set");
        if let Some(rx) = &self.rx {
            p.poll(ctx, rx, generation);
        }
        ctx.request_repaint_after(Duration::from_millis(16));
        p
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.current.take() {
            self.resume.insert(p.path.clone(), p.position());
            if let Some(worker) = &self.worker {
                worker.cancel();
            }
        }
    }

    fn ensure_worker(&mut self) -> Option<&LatestWorker<Request>> {
        if self.worker.is_none() {
            match LatestWorker::start(
                "hover-preview",
                MAX_FRAMES as usize + 4,
                |request: Request, cancel, frames| {
                    let mut emitted = 0u32;
                    for (index, pass) in request.passes.iter().enumerate() {
                        let result = preview_frames(
                            &request.path,
                            WIDTH,
                            request.interval,
                            pass.from,
                            &cancel,
                            &mut |frame| {
                                if emitted >= MAX_FRAMES || frame.at >= pass.until {
                                    return false;
                                }
                                emitted += 1;
                                let _ = frames.try_send((request.generation, index, Some(frame)));
                                !cancel.load(std::sync::atomic::Ordering::Relaxed)
                            },
                        );
                        if let Err(error) = result {
                            log::debug!("preview {}: {error:#}", request.path.display());
                        }
                        let _ = frames.try_send((request.generation, index, None));
                        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                    }
                },
            ) {
                Ok((worker, rx)) => {
                    self.worker = Some(worker);
                    self.rx = Some(rx);
                }
                Err(error) => log::warn!("preview worker: {error}"),
            }
        }
        self.worker.as_ref()
    }

    pub fn retain(&mut self, dir: &Path, entries: &HashMap<String, std::time::SystemTime>) {
        self.resume.retain(|path, _| {
            path.parent() == Some(dir)
                && path
                    .file_name()
                    .is_some_and(|name| entries.contains_key(name.to_string_lossy().as_ref()))
        });
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
        previews.play(&ctx, a, Some(2.5)).seek(1.0);
        previews.stop();
        previews.play(&ctx, b, Some(2.5)).seek(2.0);
        let pos = previews.play(&ctx, a, Some(2.5)).position();
        assert!((1.0..1.5).contains(&pos), "{pos}");
        let pos = previews.play(&ctx, b, Some(2.5)).position();
        assert!((2.0..2.5).contains(&pos), "{pos}");
        assert!(
            previews
                .play(&ctx, Path::new("new.mp4"), Some(2.5))
                .position()
                < 0.5,
            "new clips start at 0"
        );
    }

    #[test]
    fn metadata_arrival_restarts_with_a_bounded_interval_and_keeps_position() {
        let ctx = eframe::egui::Context::default();
        let mut previews = Previews::default();
        let path = Path::new("missing-long.mp4");
        previews.play(&ctx, path, None).seek(4.0);
        let pos = previews.play(&ctx, path, Some(600.0)).position();
        assert!(pos < 0.5, "{pos}");
        assert!(previews.current.as_ref().is_some_and(|preview| {
            preview.duration == Some(600.0) && preview.interval >= 600.0 / 240.0
        }));
    }

    #[test]
    fn missing_proxy_keeps_full_duration_without_a_spinner() {
        use super::HoverPreview;
        let pv = HoverPreview::start(Path::new("missing.webp"), Some(300.0), 0.0, false);
        assert_eq!(pv.length(), 300.0);
        assert!(!pv.is_loading(), "a still thumbnail is not a spinner");
        assert!(pv.is_generating(), "the proxy is still being generated");
        assert!(pv.frame().is_none());
    }

    #[test]
    fn only_decoded_stretches_count_as_ready() {
        use super::{HoverPreview, Pass};
        let mut pv = HoverPreview::start(Path::new("missing.mp4"), Some(10.0), 4.0, true);
        pv.interval = 0.1;
        pv.passes = vec![
            Pass {
                from: 4.0,
                until: f64::INFINITY,
                to: 5.0,
                done: false,
            },
            Pass {
                from: 0.0,
                until: 4.0,
                to: f64::NEG_INFINITY,
                done: false,
            },
        ];
        assert!(pv.covered(4.0) && pv.covered(5.1));
        assert!(!pv.covered(6.0), "ahead of the decoder");
        assert!(!pv.covered(1.0), "beginning not decoded yet");
        pv.passes[1].done = true;
        assert!(pv.covered(1.0) && pv.covered(3.99));
        assert!(
            !pv.covered(7.0),
            "a finished second pass does not cover the end"
        );
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
