use crate::ffmpeg::{frames, FrameRequest, Sampling};
use crate::info::{probe, MediaInfo};
use crate::webp::Reader;
use mediaa2_core::formats::{is_webp_path, MediaKind};
use mediaa2_core::Frame;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub fn details(path: &Path, kind: MediaKind) -> anyhow::Result<MediaInfo> {
    details_cancellable(path, kind, &AtomicBool::new(false))
}

pub fn details_cancellable(
    path: &Path,
    kind: MediaKind,
    cancel: &AtomicBool,
) -> anyhow::Result<MediaInfo> {
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::ffmpeg::job::Cancelled.into());
    }
    if kind == MediaKind::Image {
        let (width, height) = image::image_dimensions(path)?;
        mediaa2_core::formats::ensure_safe_dimensions(width, height)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::ffmpeg::job::Cancelled.into());
        }
        return Ok(MediaInfo {
            width,
            height,
            ..MediaInfo::default()
        });
    }
    let info = probe(path)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::ffmpeg::job::Cancelled.into());
    }
    Ok(info)
}

#[derive(Debug)]
pub struct PreviewFrame {
    pub at: f64,
    pub frame: Frame,
}

pub fn preview_interval(duration: Option<f64>, max_fps: f64, max_frames: u32) -> f64 {
    let dense = 1.0 / max_fps.max(1.0);
    match duration {
        Some(d) if d > 0.0 => dense.max(d / f64::from(max_frames.max(1))),
        _ => dense,
    }
}

pub fn preview_frames(
    path: &Path,
    width: u32,
    interval: f64,
    start: f64,
    cancel: &AtomicBool,
    on_frame: &mut dyn FnMut(PreviewFrame) -> bool,
) -> anyhow::Result<()> {
    let proxy = crate::preview_cache::ready_proxy(path);
    let source = proxy.as_deref().unwrap_or(path);
    if is_webp_path(source) {
        return webp_frames(source, width, interval, start, cancel, on_frame);
    }
    let mut reader = frames(
        source,
        FrameRequest {
            width: Some(width & !1),
            sampling: Sampling::EverySec(interval),
            start_us: (start > 0.0).then_some((start * 1e6) as i64),
            max_frames: None,
        },
    )?;
    let mut first = None;
    for frame in &mut reader {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let frame = frame?;
        let pts = frame.pts_us.unwrap_or(0);
        let at = start + (pts - *first.get_or_insert(pts)) as f64 / 1e6;
        if !on_frame(PreviewFrame {
            at,
            frame: frame.frame,
        }) {
            break;
        }
    }
    Ok(())
}

fn webp_frames(
    path: &Path,
    width: u32,
    interval: f64,
    start: f64,
    cancel: &AtomicBool,
    on_frame: &mut dyn FnMut(PreviewFrame) -> bool,
) -> anyhow::Result<()> {
    let mut reader = Reader::open(path)?;
    let info = reader.info();
    let (w, h) = (info.width, info.height);
    let target_w = width.min(w).max(1);
    let target_h = ((u64::from(h) * u64::from(target_w)) / u64::from(w.max(1))).max(1) as u32;
    let (mut t, mut next) = (0.0, start);
    for _ in 0..info.frames.max(1) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let frame = reader.read_frame()?;
        let delay = if info.animated {
            f64::from(frame.delay_ms) / 1000.0
        } else {
            0.0
        };
        if t >= next {
            let frame =
                crate::webp::scale_frame(frame.data, w, h, info.has_alpha, target_w, target_h)
                    .ok_or_else(|| anyhow::anyhow!("bad webp frame"))?;
            if !on_frame(PreviewFrame { at: t, frame }) {
                break;
            }
            next = t + interval;
        }
        t += delay;
    }
    Ok(())
}

pub fn atempo_chain(speed: f64) -> String {
    let speed = speed.clamp(0.25, 4.0);
    if speed < 0.5 {
        format!("atempo=0.5,atempo={:.4}", speed / 0.5)
    } else if speed > 2.0 {
        format!("atempo=2.0,atempo={:.4}", speed / 2.0)
    } else {
        format!("atempo={speed:.4}")
    }
}

pub fn decode_audio(
    path: &Path,
    start: f64,
    speed: f64,
    rate: u32,
    cancel: Arc<AtomicBool>,
    sink: impl FnMut(&[f32]) -> bool + Send + 'static,
) -> anyhow::Result<()> {
    let graph = format!(
        "[0:a]{},aresample={rate},aformat=sample_fmts=flt:channel_layouts=stereo[a]",
        atempo_chain(speed)
    );
    crate::ffmpeg::decode::decode_pcm(path, (start * 1e6) as i64, &graph, cancel, sink)
}

pub fn decode_mono(
    path: &Path,
    start: f64,
    rate: u32,
    cancel: Arc<AtomicBool>,
    sink: impl FnMut(&[f32]) -> bool + Send + 'static,
) -> anyhow::Result<()> {
    let graph = format!("[0:a]aresample={rate},aformat=sample_fmts=flt:channel_layouts=mono[a]");
    crate::ffmpeg::decode::decode_pcm(path, (start * 1e6) as i64, &graph, cancel, sink)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atempo_covers_slow_and_fast_speeds() {
        assert_eq!(atempo_chain(1.0), "atempo=1.0000");
        assert_eq!(atempo_chain(0.25), "atempo=0.5,atempo=0.5000");
        assert_eq!(atempo_chain(3.0), "atempo=2.0,atempo=1.5000");
    }

    #[test]
    fn interval_is_dense_for_short_clips_and_sparse_for_long_ones() {
        assert!((preview_interval(Some(4.0), 15.0, 180) - 1.0 / 15.0).abs() < 1e-9);
        assert!((preview_interval(Some(600.0), 15.0, 180) - 600.0 / 180.0).abs() < 1e-9);
        assert!((preview_interval(None, 10.0, 180) - 0.1).abs() < 1e-9);
    }
}
