use crate::latest_worker::LatestWorker;
use crossbeam_channel::{Receiver, TryRecvError};
use mediaa_core::Frame;
use mediaa_media::preview::{decode_audio, preview_frames, PreviewFrame};
use mediaa_platform::audio::AudioOut;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const VIDEO_WIDTH: u32 = 1920;
const AUDIO_AHEAD: f64 = 0.5;

struct VideoRequest {
    generation: u64,
    start: f64,
}

struct AudioRequest {
    generation: u64,
    path: PathBuf,
    start: f64,
    speed: f64,
    rate: u32,
    output: Arc<AudioOut>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum AudioMessage {
    Ready(u64),
    Failed(u64),
}

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
    frames: Option<Receiver<(u64, PreviewFrame)>>,
    video_worker: Option<LatestWorker<VideoRequest>>,
    video_generation: u64,
    video_completed: Arc<AtomicU64>,
    pending: Option<PreviewFrame>,
    got_frame: bool,
    audio: Option<Arc<AudioOut>>,
    audio_pending: bool,
    audio_worker: Option<LatestWorker<AudioRequest>>,
    audio_events: Option<Receiver<AudioMessage>>,
    audio_generation: u64,
    audio_waiting: bool,
    audio_needs_alignment: bool,
    dragging: bool,
}

impl Player {
    pub fn new(path: &Path, duration: f64, looping: bool, picture: bool) -> Player {
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
            video_worker: None,
            video_generation: 0,
            video_completed: Arc::new(AtomicU64::new(0)),
            pending: None,
            got_frame: false,
            audio: None,
            audio_pending: false,
            audio_worker: None,
            audio_events: None,
            audio_generation: 0,
            audio_waiting: false,
            audio_needs_alignment: false,
            dragging: false,
        };
        if picture {
            let worker_path = path.to_path_buf();
            let video_completed = p.video_completed.clone();
            match LatestWorker::start(
                "viewer-video",
                8,
                move |request: VideoRequest, cancel, frames| {
                    let result = preview_frames(
                        &worker_path,
                        VIDEO_WIDTH,
                        1.0 / 120.0,
                        request.start,
                        &cancel,
                        &mut |frame| {
                            let _ = frames.try_send((request.generation, frame));
                            !cancel.load(Ordering::Relaxed)
                        },
                    );
                    if let Err(error) = result {
                        log::debug!("viewer video {}: {error:#}", worker_path.display());
                    }
                    video_completed.fetch_max(request.generation, Ordering::Relaxed);
                },
            ) {
                Ok((worker, frames)) => {
                    p.video_worker = Some(worker);
                    p.frames = Some(frames);
                }
                Err(error) => log::warn!("viewer video worker: {error}"),
            }
        }
        p.restart(0.0);
        p
    }

    pub fn attach_audio(&mut self, audio: AudioOut) {
        self.audio_pending = false;
        audio.set_volume(self.volume);
        let output = Arc::new(audio);
        match LatestWorker::start(
            "viewer-audio",
            8,
            |request: AudioRequest, cancel, status: crossbeam_channel::Sender<AudioMessage>| {
                let path = request.path;
                let generation = request.generation;
                let ready = Arc::new(AtomicBool::new(false));
                let ready_in_callback = ready.clone();
                let status_for_audio = status.clone();
                let result = decode_audio(
                    &path,
                    request.start,
                    request.speed,
                    request.rate,
                    cancel.clone(),
                    move |samples| {
                        if !ready_in_callback.swap(true, Ordering::Relaxed) {
                            let _ = status_for_audio.try_send(AudioMessage::Ready(generation));
                        }
                        while request.output.queued_secs() > AUDIO_AHEAD {
                            if cancel.load(Ordering::Relaxed) {
                                return false;
                            }
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        request.output.push(samples);
                        !cancel.load(Ordering::Relaxed)
                    },
                );
                if let Err(error) = result {
                    log::debug!("viewer audio: {error:#}");
                    if !ready.load(Ordering::Relaxed) {
                        let _ = status.try_send(AudioMessage::Failed(generation));
                    }
                }
            },
        ) {
            Ok((worker, events)) => {
                self.audio_worker = Some(worker);
                self.audio_events = Some(events);
                self.audio = Some(output);
            }
            Err(error) => {
                log::warn!("viewer audio worker: {error}");
                self.audio = None;
            }
        }
        self.start_audio();
    }

    pub fn expect_audio(&mut self) {
        self.audio_pending = true;
    }

    pub fn audio_unavailable(&mut self) {
        self.audio_pending = false;
        self.audio_waiting = false;
        self.audio_needs_alignment = false;
        self.audio = None;
        if let Some(worker) = &self.audio_worker {
            worker.cancel();
        }
    }

    pub fn position(&self) -> f64 {
        let t = match self.anchor {
            Some(a) if !self.paused => self.pos + a.elapsed().as_secs_f64() * self.speed,
            _ => self.pos,
        };
        if self.duration > 0.0 {
            t.min(self.duration)
        } else {
            t
        }
    }

    fn restart(&mut self, at: f64) {
        let at = if self.duration > 0.0 {
            at.clamp(0.0, self.duration)
        } else {
            at.max(0.0)
        };
        (self.pos, self.anchor, self.pending, self.got_frame) = (at, None, None, false);

        if let Some(frames) = &self.frames {
            while frames.try_recv().is_ok() {}
        }
        if let Some(worker) = &self.video_worker {
            self.video_generation += 1;
            worker.request(VideoRequest {
                generation: self.video_generation,
                start: at,
            });
        }
        self.start_audio();
    }

    fn start_audio(&mut self) {
        let Some(output) = self.audio.clone() else {
            return;
        };
        output.clear();
        output.set_paused(true);
        let Some(worker) = &self.audio_worker else {
            return;
        };
        self.audio_waiting = true;
        self.audio_needs_alignment = false;
        self.audio_generation += 1;
        let start = if self.anchor.is_some() {
            self.position()
        } else {
            self.pos
        };
        worker.request(AudioRequest {
            generation: self.audio_generation,
            path: self.path.clone(),
            start,
            speed: self.speed,
            rate: output.sample_rate(),
            output,
        });
    }

    pub fn seek(&mut self, t: f64) {
        self.jump(t);
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
        self.poll_audio();
        if self.anchor.is_none() {
            let mut video_done = false;
            if self.pending.is_none() {
                match self.frames.as_ref().map(|rx| rx.try_recv()) {
                    Some(Ok((generation, frame))) if generation == self.video_generation => {
                        self.pending = Some(frame)
                    }
                    Some(Err(TryRecvError::Disconnected)) => video_done = true,
                    _ => {}
                }
            }
            video_done |= self.video_completed.load(Ordering::Relaxed) >= self.video_generation;
            let picture_ready = self.pending.is_some() || !self.picture;
            if picture_ready || video_done {
                self.anchor = Some(Instant::now());
                if let Some(a) = &self.audio {
                    self.audio_needs_alignment = a.queued_secs() <= 0.08;
                    a.set_paused(self.paused || self.audio_needs_alignment);
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
            let mut ended = false;
            let mut empty = false;
            if self.pending.is_none() {
                match self.frames.as_ref().map(|rx| rx.try_recv()) {
                    Some(Ok((generation, frame))) if generation == self.video_generation => {
                        self.pending = Some(frame)
                    }
                    Some(Ok(_)) => continue,
                    Some(Err(TryRecvError::Disconnected)) => ended = true,
                    Some(Err(TryRecvError::Empty)) | None => empty = true,
                }
                ended |=
                    empty && self.video_completed.load(Ordering::Relaxed) >= self.video_generation;
            }
            if ended {
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
            if empty {
                break;
            }
            match &self.pending {
                Some(f) if f.at <= pos => latest = self.pending.take().map(|f| f.frame),
                _ => break,
            }
        }
        latest
    }

    fn poll_audio(&mut self) {
        let messages: Vec<_> = self
            .audio_events
            .as_ref()
            .map(|events| events.try_iter().collect())
            .unwrap_or_default();
        let mut failed = false;
        for message in messages {
            match message {
                AudioMessage::Ready(generation) if generation == self.audio_generation => {
                    if self.audio_needs_alignment && self.anchor.is_some() {
                        self.start_audio();
                    } else {
                        self.audio_waiting = false;
                        if let Some(audio) = &self.audio {
                            audio.set_paused(self.paused || self.anchor.is_none());
                        }
                    }
                }
                AudioMessage::Failed(generation) if generation == self.audio_generation => {
                    failed = true;
                }
                _ => {}
            }
        }
        if failed {
            self.audio_unavailable();
        }
    }

    pub fn duration(&self) -> f64 {
        self.duration
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn has_sound(&self) -> bool {
        self.audio.is_some() || self.audio_pending
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
        self.video_worker = None;
        self.audio_worker = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{AudioMessage, Player};
    use std::path::Path;

    #[test]
    fn pending_audio_reserves_the_controls_and_can_fall_back_to_video() {
        let mut player = Player::new(Path::new("missing.mp3"), 0.0, false, false);
        player.expect_audio();
        assert!(player.has_sound());
        player.audio_unavailable();
        assert!(!player.has_sound());
    }

    #[test]
    fn audio_decode_failure_falls_back_to_video() {
        let mut player = Player::new(Path::new("missing.mp3"), 0.0, false, false);
        let (events, receiver) = crossbeam_channel::bounded(1);
        player.audio_events = Some(receiver);
        player.audio_generation = 1;
        player.audio_pending = true;
        player.audio_waiting = true;
        events.send(AudioMessage::Failed(1)).unwrap();
        player.update();
        assert!(!player.has_sound());
        assert!(!player.audio_waiting);
    }

    #[test]
    fn missing_audio_does_not_delay_video_start() {
        let mut player = Player::new(Path::new("missing.wav"), 0.0, false, false);
        player.audio_pending = true;
        player.audio_waiting = true;
        player.update();
        assert!(player.anchor.is_some());
        assert!(player.has_sound());
        assert!(player.audio_waiting);
    }
}
