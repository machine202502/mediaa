use super::player::Player;
use crate::logic::jobs::Job;
#[derive(Clone, Copy, Default)]
pub(crate) struct Offset {
    pub x: f32,
    pub y: f32,
}
use mediaa_core::{Frame, MediaKind};
use mediaa_platform::audio::AudioOut;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const MAX_TEXTURE: u32 = 8192;
pub(crate) const SPEEDS: [f64; 6] = [0.5, 0.75, 1.0, 1.25, 1.5, 2.0];

pub enum ViewerAction {
    Close,
    OpenFile(PathBuf),
    Volume(f32),
}

pub struct Viewer {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) kind: MediaKind,
    pub(crate) image_job: Option<Job<Option<Frame>>>,
    pub(crate) audio_job: Option<Job<Result<AudioOut, String>>>,
    pub(crate) zoom: f32,
    pub(crate) pan: Offset,
    pub(crate) speed_open: bool,
    pub(crate) player: Option<Player>,
}

impl Viewer {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn cancel_pending(&mut self) {
        if let Some(job) = self.image_job.take() {
            job.cancel();
        }
        if let Some(job) = self.audio_job.take() {
            job.cancel();
        }
    }

    pub fn open(
        path: &Path,
        kind: MediaKind,
        duration: Option<f64>,
        audio: bool,
        volume: f32,
    ) -> Viewer {
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut v = Viewer {
            path: path.to_path_buf(),
            name,
            kind,
            image_job: None,
            audio_job: None,
            zoom: 1.0,
            pan: Offset::default(),
            speed_open: false,
            player: None,
        };
        if kind == MediaKind::Image {
            let p = path.to_path_buf();
            let cancel = Arc::new(AtomicBool::new(false));
            let worker_cancel = cancel.clone();
            match Job::spawn(
                "viewer-image",
                cancel,
                move || match image::image_dimensions(&p)
                    .map_err(anyhow::Error::from)
                    .and_then(|(width, height)| {
                        mediaa_core::formats::ensure_safe_dimensions(width, height)?;
                        Ok(())
                    })
                    .and_then(|()| image::open(&p).map_err(anyhow::Error::from))
                {
                    Ok(img) => {
                        let img = if img.width().max(img.height()) > MAX_TEXTURE {
                            img.thumbnail(MAX_TEXTURE, MAX_TEXTURE)
                        } else {
                            img
                        };
                        (!worker_cancel.load(Ordering::Relaxed))
                            .then(|| Frame::from_image(img.to_rgba8()))
                    }
                    Err(e) => {
                        log::warn!("viewer {}: {e}", p.display());
                        None
                    }
                },
            ) {
                Ok(job) => v.image_job = Some(job),
                Err(error) => log::warn!("viewer image worker: {error}"),
            }
        } else {
            let mut player = Player::new(
                path,
                duration.unwrap_or(0.0),
                kind == MediaKind::Animation,
                kind != MediaKind::Audio,
            );
            player.set_volume(volume);
            if audio {
                player.expect_audio();
            }
            v.player = Some(player);
            if audio {
                let cancel = Arc::new(AtomicBool::new(false));
                let worker_cancel = cancel.clone();
                match Job::spawn("viewer-audio-open", cancel, move || {
                    if worker_cancel.load(Ordering::Relaxed) {
                        return Err("cancelled".into());
                    }
                    AudioOut::open().map_err(|error| format!("{error:#}"))
                }) {
                    Ok(job) => v.audio_job = Some(job),
                    Err(error) => log::warn!("viewer audio worker: {error}"),
                }
            }
        }
        v
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.cancel_pending();
    }
}

pub(crate) const MIN_ZOOM: f32 = 0.1;
pub(crate) const MAX_ZOOM: f32 = 10.0;

pub(crate) fn zoom_to_frac(zoom: f32) -> f32 {
    (0.5 + zoom.clamp(MIN_ZOOM, MAX_ZOOM).ln() / (2.0 * MAX_ZOOM.ln())).clamp(0.0, 1.0)
}

pub(crate) fn frac_to_zoom(frac: f32) -> f32 {
    ((frac - 0.5) * 2.0 * MAX_ZOOM.ln())
        .exp()
        .clamp(MIN_ZOOM, MAX_ZOOM)
}

#[cfg(test)]
mod tests {
    use super::{frac_to_zoom, zoom_to_frac, Viewer, MAX_ZOOM, MIN_ZOOM};
    use mediaa_core::MediaKind;
    use std::path::Path;

    #[test]
    fn a_viewer_starts_with_the_saved_volume() {
        let viewer = Viewer::open(
            Path::new("missing.mp4"),
            MediaKind::Video,
            Some(4.0),
            false,
            0.25,
        );
        assert_eq!(viewer.player.as_ref().unwrap().volume(), 0.25);
    }
    #[test]
    fn closing_a_viewer_cancels_its_owned_lookup() {
        let mut viewer = Viewer::open(Path::new("missing.png"), MediaKind::Image, None, false, 1.0);
        assert!(viewer.image_job.is_some());
        viewer.cancel_pending();
        assert!(viewer.image_job.is_none());
    }

    #[test]
    fn zoom_slider_is_centred_on_100_percent() {
        assert!((zoom_to_frac(1.0) - 0.5).abs() < 1e-6);
        assert_eq!(zoom_to_frac(MIN_ZOOM), 0.0);
        assert_eq!(zoom_to_frac(MAX_ZOOM), 1.0);
        for z in [0.25, 0.5, 2.0, 4.0] {
            assert!((frac_to_zoom(zoom_to_frac(z)) - z).abs() < 1e-4, "{z}");
        }
        assert!(
            zoom_to_frac(2.0) - 0.5 > 0.0
                && (zoom_to_frac(2.0) - 0.5 - (0.5 - zoom_to_frac(0.5))).abs() < 1e-6,
            "symmetric"
        );
    }
}
