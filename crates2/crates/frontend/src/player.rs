use crossbeam_channel::{Receiver, Sender, TryRecvError};
use mediaa2_core::latest_worker::LatestWorker;
use mediaa2_core::Frame;
use mediaa2_media::preview::{decode_audio, preview_frames, PreviewFrame};
use mediaa2_platform::audio::AudioOut;
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

/// Plays a recording in the viewer: decodes video frames in the background,
/// keeps the audio output fed ahead of the playback position, and pauses
/// while the user is scrubbing.
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
    audio_opening: Option<std::sync::mpsc::Receiver<Result<AudioOut, String>>>,
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
            audio_opening: None,
            audio_pending: false,
            audio_worker: None,
            audio_events: None,
            audio_generation: 0,
            audio_waiting: false,
            audio_needs_alignment: false,
            dragging: false,
        };
        if picture {
            let path = path.to_path_buf();
            let video_completed = p.video_completed.clone();
            match LatestWorker::start(
                "viewer-video",
                8,
                move |request: VideoRequest, cancel, frames| {
                    let result = preview_frames(
                        &path,
                        VIDEO_WIDTH,
                        1.0 / 120.0,
                        request.start,
                        &cancel,
                        &mut |frame| send_preview(&frames, &cancel, (request.generation, frame)),
                    );
                    if let Err(error) = result {
                        log::debug!("viewer video {}: {error:#}", path.display());
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

    /// Starts opening the audio device in the background; call
    /// [`Player::poll_audio_open`] every frame until it lands.
    pub fn open_audio(&mut self) {
        if self.audio.is_some() || self.audio_pending || self.audio_opening.is_some() {
            return;
        }
        self.expect_audio();
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("viewer-audio-open".into())
            .spawn(move || {
                let result =
                    mediaa2_platform::audio::AudioOut::open().map_err(|error| format!("{error:#}"));
                let _ = tx.send(result);
            }) {
            Ok(_) => self.audio_opening = Some(rx),
            Err(error) => log::warn!("viewer audio worker: {error}"),
        }
    }

    /// Adopts the audio device when its opening thread has finished.
    pub fn poll_audio_open(&mut self) {
        let Some(rx) = &self.audio_opening else {
            return;
        };
        if let Ok(result) = rx.try_recv() {
            self.audio_opening = None;
            match result {
                Ok(audio) => self.attach_audio(audio),
                Err(_) => self.audio_unavailable(),
            }
        }
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

    pub fn path(&self) -> &Path {
        &self.path
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

    /// The frame to show now, if a new one has been decoded since the last call.
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

    /// The library probe may finish after the viewer opened: adopt the
    /// longer known duration so the timeline becomes usable.
    pub fn set_duration(&mut self, duration: f64) {
        if duration > self.duration {
            self.duration = duration;
        }
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

fn send_preview(
    frames: &Sender<(u64, PreviewFrame)>,
    cancel: &AtomicBool,
    item: (u64, PreviewFrame),
) -> bool {
    let mut item = item;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        match frames.send_timeout(item, Duration::from_millis(20)) {
            Ok(()) => return !cancel.load(Ordering::Relaxed),
            Err(crossbeam_channel::SendTimeoutError::Timeout(returned)) => item = returned,
            Err(crossbeam_channel::SendTimeoutError::Disconnected(_)) => return false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{send_preview, AudioMessage, Player};
    use mediaa2_core::Frame;
    use mediaa2_media::preview::PreviewFrame;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    fn preview(at: f64) -> PreviewFrame {
        PreviewFrame {
            at,
            frame: Frame::filled(1, 1, [0, 0, 0, 255]),
        }
    }

    #[test]
    fn preview_sender_waits_for_room_instead_of_dropping_frames() {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let cancel = Arc::new(AtomicBool::new(false));
        assert!(send_preview(&tx, &cancel, (1, preview(0.0))));

        let (done_tx, done_rx) = crossbeam_channel::bounded(1);
        let worker_tx = tx.clone();
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let sent = send_preview(&worker_tx, &worker_cancel, (1, preview(1.0)));
            let _ = done_tx.send(sent);
        });
        assert!(
            done_rx.recv_timeout(Duration::from_millis(80)).is_err(),
            "the second frame must wait for the UI"
        );
        assert_eq!(rx.recv().unwrap().1.at, 0.0);
        assert!(done_rx.recv_timeout(Duration::from_secs(1)).unwrap());
        assert_eq!(rx.recv().unwrap().1.at, 1.0);
        worker.join().unwrap();
    }

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

    #[test]
    fn a_video_without_a_decodable_track_still_opens_and_reaches_its_end() {
        // no worker: the "video completed" flag imitates a finished decode
        let mut player = Player::new(Path::new("missing.mp4"), 0.0, false, false);
        player
            .video_completed
            .store(player.video_generation, Ordering::Relaxed);
        player.update();
        assert!(player.anchor.is_some(), "the playback starts anyway");
        assert!(!player.is_paused());
    }

    #[test]
    fn the_playback_stops_at_the_end_without_looping() {
        let mut player = Player::new(Path::new("missing.mp4"), 2.0, false, false);
        player
            .video_completed
            .store(player.video_generation, Ordering::Relaxed);
        player.update();
        player.seek(1.99);
        player.update();
        std::thread::sleep(Duration::from_millis(40));
        player.update();
        assert!(player.position() >= 2.0 - 0.02, "reached the end");
        assert!(player.is_paused(), "stopped at the end");
    }
}
