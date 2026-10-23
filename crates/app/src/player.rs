//! Media playback engine shared by the library viewer and the agent's studio: a frame decoder
//! and (if the file has sound) an audio decoder feeding the default output device, both
//! restarted on seek or speed change. Drawing is up to the caller.

use crossbeam_channel::{Receiver, TryRecvError};
use mediaa_core::Frame;
use mediaa_media::preview::{decode_audio, preview_frames, PreviewFrame};
use mediaa_platform::audio::AudioOut;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Widest decoded video frame (keeps 4K clips smooth).
const VIDEO_WIDTH: u32 = 1920;
/// Audio queued ahead of the device before the decoder waits.
const AUDIO_AHEAD: f64 = 0.5;

/// Video / animation playback: a frame decoder and (if the file has sound) an audio decoder
/// feeding the default output device, both restarted on seek or speed change. The clock
/// starts once both have data, so picture and sound begin together.
pub struct Player {
    path: PathBuf,
    duration: f64,
    looping: bool,
    picture: bool,
    speed: f64,
    volume: f32,
    paused: bool,
    /// Position at `anchor`; `None` anchor = waiting for the decoders.
    pos: f64,
    anchor: Option<Instant>,
    frames: Option<Receiver<PreviewFrame>>,
    pending: Option<PreviewFrame>,
    got_frame: bool,
    audio: Option<Arc<AudioOut>>,
    cancel: Arc<AtomicBool>,
    dragging: bool,
    /// Clicks on a reply, a word or a time. The transport bar uses [`Player::jump`] and ignores this.
    links: bool,
}

impl Player {
    /// `picture`: false for audio files (sound only, the clock follows the sound decoder).
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

    /// (Re)starts both decoders at `at` seconds; the clock waits for them.
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
                    // Back-pressure: stay a little ahead of the device.
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

    /// Whether a click in the dialog, the topics or the chat may move playback.
    pub fn set_links(&mut self, on: bool) {
        self.links = on;
    }

    /// Moves to `t` when [`Player::set_links`] is on. A click while sync is off does nothing.
    pub fn seek(&mut self, t: f64) {
        if self.links {
            self.jump(t);
        }
    }

    /// Moves to `t` whatever sync says (the transport bar, opening a recording at a time).
    pub fn jump(&mut self, t: f64) {
        self.restart(t);
    }

    pub fn toggle_pause(&mut self) {
        if self.paused && self.duration > 0.0 && self.position() >= self.duration - 0.05 {
            self.paused = false;
            self.restart(0.0); // replay from the start
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

    /// Advances playback; returns the frame to show now, if it changed.
    pub fn update(&mut self) -> Option<Frame> {
        // Start the clock once the first frame (and some sound) is there.
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
            // Sound only: stop at the end (or start over for looping media).
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
                        // All frames shown: stop at the end, or start over (animations).
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

    /// Playback speed (sound keeps its pitch); restarts the decoders at the current position.
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

    /// 0..=1.
    pub fn set_volume(&mut self, volume: f32) {
        self.volume = volume.clamp(0.0, 1.0);
        if let Some(a) = &self.audio {
            a.set_volume(self.volume);
        }
    }

    /// While a timeline is dragged: the clock follows it, the decoders restart at
    /// [`Player::end_scrub`].
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

