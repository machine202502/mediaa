use crossbeam_channel::{Receiver, TryRecvError};
use mediaa_core::Frame;
use mediaa_media::preview::{decode_audio, preview_frames, PreviewFrame};
use mediaa_platform::audio::AudioOut;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const VIDEO_WIDTH: u32 = 1920;
const AUDIO_AHEAD: f64 = 0.5;

pub struct Player {
    path: PathBuf,
    duration: f64,
    looping: bool,
    picture: bool,
    speed: f64,
    volume: f32,
    paused: bool,
    pos: f64,
    anchor: Option<Instant>,
    frames: Option<Receiver<PreviewFrame>>,
    pending: Option<PreviewFrame>,
    got_frame: bool,
    audio: Option<Arc<AudioOut>>,
    cancel: Arc<AtomicBool>,
    dragging: bool,
    links: bool,
}

impl Player {
    pub fn new(path: &Path, duration: f64, has_audio: bool, looping: bool, picture: bool) -> Player {
        let audio = if has_audio {
            AudioOut::open().map_err(|e| log::warn!("audio output: {e:#}")).ok().map(Arc::new)
        } else {
            None
        };
        let mut p = Player {
            path: path.to_path_buf(),
            duration,
            looping,
            picture,
            speed: 1.0,
            volume: 1.0,
            paused: false,
            pos: 0.0,
            anchor: None,
            frames: None,
            pending: None,
            got_frame: false,
            audio,
            cancel: Arc::new(AtomicBool::new(false)),
            dragging: false,
            links: true,
        };
        p.restart(0.0);
        p
    }

    pub fn position(&self) -> f64 {
        let t = match self.anchor {
            Some(a) if !self.paused => self.pos + a.elapsed().as_secs_f64() * self.speed,
            _ => self.pos,
        };
        if self.duration > 0.0 { t.min(self.duration) } else { t }
    }

    fn restart(&mut self, at: f64) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let at = if self.duration > 0.0 { at.clamp(0.0, self.duration) } else { at.max(0.0) };
        (self.pos, self.anchor, self.pending, self.got_frame) = (at, None, None, false);

        self.frames = None;
        if self.picture {
            let (tx, rx) = crossbeam_channel::bounded::<PreviewFrame>(8);
            let (path, cancel) = (self.path.clone(), self.cancel.clone());
            let _ = std::thread::Builder::new().name("viewer-video".into()).spawn(move || {
            let res = preview_frames(&path, VIDEO_WIDTH, 1.0 / 120.0, at, &cancel, &mut |f| !cancel.load(Ordering::Relaxed) && tx.send(f).is_ok());
            if let Err(e) = res {
                log::debug!("viewer video {}: {e:#}", path.display());
            }
            });
            self.frames = Some(rx);
        }

        if let Some(out) = self.audio.clone() {
            out.clear();
            out.set_paused(true);
            let (path, cancel, speed, rate) = (self.path.clone(), self.cancel.clone(), self.speed, out.sample_rate());
            let c = cancel.clone();
            let _ = std::thread::Builder::new().name("viewer-audio".into()).spawn(move || {
                let res = decode_audio(&path, at, speed, rate, cancel, move |samples| {
                    while out.queued_secs() > AUDIO_AHEAD {
                        if c.load(Ordering::Relaxed) {
                            return false;
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    out.push(samples);
                    !c.load(Ordering::Relaxed)
                });
                if let Err(e) = res {
                    log::debug!("viewer audio {}: {e:#}", path.display());
                }
            });
        }
    }

    pub fn set_links(&mut self, on: bool) {
        self.links = on;
    }

    pub fn seek(&mut self, t: f64) {
        if self.links {
            self.jump(t);
        }
    }

    pub fn jump(&mut self, t: f64) {
        self.restart(t);
    }

    pub fn toggle_pause(&mut self) {
        if self.paused && self.duration > 0.0 && self.position() >= self.duration - 0.05 {
            self.paused = false;
            self.restart(0.0);
            return;
        }
        self.pos = self.position();
        self.paused = !self.paused;
        if self.anchor.is_some() {
            self.anchor = Some(Instant::now());
        }
        if let Some(a) = &self.audio {
            a.set_paused(self.paused || self.anchor.is_none());
        }
    }

    pub fn update(&mut self) -> Option<Frame> {
        if self.anchor.is_none() {
            let mut video_done = false;
            if self.pending.is_none() {
                match self.frames.as_ref().map(|rx| rx.try_recv()) {
                    Some(Ok(f)) => self.pending = Some(f),
                    Some(Err(TryRecvError::Disconnected)) => video_done = true,
                    _ => {}
                }
            }
            let sound_ready = self.audio.as_ref().is_none_or(|a| a.queued_secs() > 0.08);
            let picture_ready = self.pending.is_some() || !self.picture;
            if (picture_ready && sound_ready) || video_done {
                self.anchor = Some(Instant::now());
                if let Some(a) = &self.audio {
                    a.set_paused(self.paused);
                }
            }
            return self.pending.as_ref().filter(|_| !self.got_frame).map(|f| {
                self.got_frame = true;
                f.frame.clone()
            });
        }

        let pos = self.position();
        let mut latest = None;
        if !self.picture {
            if self.duration > 0.0 && pos >= self.duration - 0.02 && !self.paused {
                (self.pos, self.paused) = (self.duration, true);
                if let Some(a) = &self.audio {
                    a.set_paused(true);
                }
            }
            return None;
        }
        loop {
            if self.pending.is_none() {
                match self.frames.as_ref().map(|rx| rx.try_recv()) {
                    Some(Ok(f)) => self.pending = Some(f),
                    Some(Err(TryRecvError::Disconnected)) => {
                        if self.duration <= 0.0 {
                            self.duration = pos;
                        }
                        if pos >= self.duration - 0.02 && !self.paused {
                            if self.looping {
                                self.restart(0.0);
                            } else {
                                self.pos = self.duration;
                                self.paused = true;
                            }
                        }
                        break;
                    }
                    _ => break,
                }
            }
            match &self.pending {
                Some(f) if f.at <= pos => latest = self.pending.take().map(|f| f.frame),
                _ => break,
            }
        }
        latest
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn has_sound(&self) -> bool {
        self.audio.is_some()
    }

    pub fn speed(&self) -> f64 {
        self.speed
    }

    pub fn set_speed(&mut self, speed: f64) {
        if self.speed != speed {
            let at = self.position();
            self.speed = speed;
            self.restart(at);
        }
    }

    pub fn volume(&self) -> f32 {
        self.volume
    }

    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(a) = &self.audio {
            a.set_volume(self.volume);
        }
    }

    pub fn scrub(&mut self, t: f64) {
        (self.pos, self.anchor, self.dragging) = (t, Some(Instant::now()), true);
    }

    pub fn end_scrub(&mut self) {
        if self.dragging {
            self.dragging = false;
            self.jump(self.pos);
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
