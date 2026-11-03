use crate::ffmpeg::context::JobBuilder;
use crate::ffmpeg::input::Input;
use crate::ffmpeg::output::Output;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

pub fn decode_pcm(
    path: &Path,
    start_us: i64,
    graph: &str,
    cancel: Arc<AtomicBool>,
    mut sink: impl FnMut(&[f32]) -> bool + Send + 'static,
) -> anyhow::Result<()> {
    let mut input = Input::new(path);
    if start_us > 0 {
        input = input.start_us(start_us);
    }
    let mut rest: Vec<u8> = Vec::new();
    let stop = cancel.clone();
    let output = Output::write_callback(move |bytes: &[u8]| {
        if stop.load(Ordering::Relaxed) {
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
            stop.store(true, Ordering::Relaxed);
        }
        bytes.len() as i32
    })
    .format("f32le")
    .audio_codec("pcm_f32le")
    .stream_map("[a]");
    let mut job = Some(
        JobBuilder::new()
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
    job.wait()
}
