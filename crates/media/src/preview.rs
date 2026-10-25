use crate::info::{probe, MediaInfo};
use crate::webp_decode::WebpReader;
use ez_ffmpeg::frame_export::{FrameExtractor, PixelLayout, Sampling};
use ez_ffmpeg::{FfmpegContext, Input, Output};
use mediaa_core::formats::{is_webp_path, MediaKind};
use mediaa_core::Frame;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub fn details(path: &Path, kind: MediaKind) -> anyhow::Result<MediaInfo> {
    details_cancellable(path, kind, &AtomicBool::new(false))
}

pub fn details_cancellable(
    path: &Path,
    kind: MediaKind,
    cancel: &AtomicBool,
) -> anyhow::Result<MediaInfo> {
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::job::Cancelled.into());
    }
    if kind == MediaKind::Image {
        let (width, height) = image::image_dimensions(path)?;
        mediaa_core::formats::ensure_safe_dimensions(width, height)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(crate::job::Cancelled.into());
        }
        return Ok(MediaInfo {
            width,
            height,
            ..MediaInfo::default()
        });
    }
    let info = probe(path)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(crate::job::Cancelled.into());
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
    let mut extractor = FrameExtractor::new(source.to_string_lossy().into_owned())
        .pixel(PixelLayout::Rgba32)
        .width(width & !1)
        .sampling(Sampling::EverySec(interval));
    if start > 0.0 {
        extractor = extractor.start_time_us((start * 1e6) as i64);
    }
    let frames = extractor.frames()?;
    let mut first = None;
    for f in frames {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let f = f?;
        let pts = f.pts_us().unwrap_or(0);
        let at = start + (pts - *first.get_or_insert(pts)) as f64 / 1e6;
        let (w, h) = (f.width(), f.height());
        if !on_frame(PreviewFrame {
            at,
            frame: Frame::new(w, h, f.into_vec()),
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
    let mut reader = WebpReader::open(path)?;
    let (w, h) = reader.dimensions();
    let alpha = reader.has_alpha();
    let animated = reader.is_animated();
    let frames = reader.frame_count();
    let target_w = width.min(w).max(1);
    let target_h = ((u64::from(h) * u64::from(target_w)) / u64::from(w.max(1))).max(1) as u32;
    let (mut t, mut next) = (0.0, start);
    for _ in 0..frames.max(1) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let frame = reader.read_frame()?;
        let delay = if animated {
            f64::from(frame.delay_ms) / 1000.0
        } else {
            0.0
        };
        if t >= next {
            let frame = scale_webp_frame(frame.data, w, h, alpha, target_w, target_h)
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

fn scale_webp_frame(buf: &[u8], w: u32, h: u32, alpha: bool, tw: u32, th: u32) -> Option<Frame> {
    use image::{ImageBuffer, Rgb, Rgba};
    let rgba = if alpha {
        let src = ImageBuffer::<Rgba<u8>, &[u8]>::from_raw(w, h, buf)?;
        if (tw, th) == (w, h) {
            image::RgbaImage::from_raw(w, h, buf.to_vec())?
        } else {
            image::imageops::thumbnail(&src, tw, th)
        }
    } else {
        let src = ImageBuffer::<Rgb<u8>, &[u8]>::from_raw(w, h, buf)?;
        let small = if (tw, th) == (w, h) {
            image::RgbImage::from_raw(w, h, buf.to_vec())?
        } else {
            image::imageops::thumbnail(&src, tw, th)
        };
        image::DynamicImage::ImageRgb8(small).into_rgba8()
    };
    Some(Frame::from_image(rgba))
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
    decode_pcm(path, start, &graph, cancel, sink)
}

pub fn decode_mono(
    path: &Path,
    start: f64,
    rate: u32,
    cancel: Arc<AtomicBool>,
    sink: impl FnMut(&[f32]) -> bool + Send + 'static,
) -> anyhow::Result<()> {
    decode_pcm(
        path,
        start,
        &format!("[0:a]aresample={rate},aformat=sample_fmts=flt:channel_layouts=mono[a]"),
        cancel,
        sink,
    )
}

fn decode_pcm(
    path: &Path,
    start: f64,
    graph: &str,
    cancel: Arc<AtomicBool>,
    mut sink: impl FnMut(&[f32]) -> bool + Send + 'static,
) -> anyhow::Result<()> {
    let mut input = Input::from(path.to_string_lossy().into_owned());
    if start > 0.0 {
        input = input.set_start_time_us((start * 1e6) as i64);
    }
    let mut rest: Vec<u8> = Vec::new();
    let c = cancel.clone();
    let output = Output::new_by_write_callback(move |bytes: &[u8]| {
        if c.load(Ordering::Relaxed) {
            return bytes.len() as i32;
        }
        rest.extend_from_slice(bytes);
        let whole = rest.len() / 4 * 4;
        let samples: Vec<f32> = rest[..whole]
            .as_chunks::<4>()
            .0
            .iter()
            .map(|b| f32::from_le_bytes(*b))
            .collect();
        rest.drain(..whole);
        if !sink(&samples) {
            c.store(true, Ordering::Relaxed);
        }
        bytes.len() as i32
    })
    .set_format("f32le")
    .set_audio_codec("pcm_f32le")
    .add_stream_map("[a]");
    let mut job = Some(
        FfmpegContext::builder()
            .input(input)
            .filter_desc(graph)
            .output(output)
            .build()?
            .start()?,
    );
    loop {
        if cancel.load(Ordering::Relaxed) {
            if let Some(job) = job.take() {
                job.abort();
            }
            return Ok(());
        }
        if job.as_ref().is_some_and(|job| job.is_ended()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let job = job.take().expect("the job ends only after `job` is taken");
    job.wait().map_err(Into::into)
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
