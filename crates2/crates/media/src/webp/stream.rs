use crate::ffmpeg::{push_source, PushHandle, PushSource};
use crate::webp::reader::{Info, Reader};
use std::path::Path;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::Duration;

const DECODE_STOP_TIMEOUT: Duration = Duration::from_secs(20);

pub struct FrameStream {
    pub source: PushSource,
    pub info: Info,
    pub producer: Producer,
}

pub struct Producer {
    result: mpsc::Receiver<anyhow::Result<()>>,
}

pub fn frame_stream(path: &Path) -> anyhow::Result<FrameStream> {
    let mut reader = Reader::open(path)?;
    let info = reader.info();
    let (width, height) = (info.width, info.height);
    let (source, handle) = push_source(
        width,
        height,
        if info.has_alpha { "rgba" } else { "rgb24" },
        (1, 1000),
    )?;
    let (done_tx, result) = mpsc::channel();
    std::thread::Builder::new()
        .name("webp-decode".into())
        .spawn(move || {
            let result = decode(&mut reader, handle);
            let _ = done_tx.send(result);
        })?;
    Ok(FrameStream {
        source,
        info,
        producer: Producer { result },
    })
}

fn decode(reader: &mut Reader, mut handle: PushHandle) -> anyhow::Result<()> {
    let mut pts = 0i64;
    for _ in 0..reader.frame_count() {
        let frame = reader.read_frame()?;
        let delay = i64::from(frame.delay_ms);
        if handle.push(frame.data, pts, Some(delay)).is_err() {
            break;
        }
        pts += delay;
    }
    handle.finish();
    Ok(())
}

impl Producer {
    pub fn wait(self) -> anyhow::Result<()> {
        match self.result.recv_timeout(DECODE_STOP_TIMEOUT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                format!(
                    "WebP decoder did not stop within {} s",
                    DECODE_STOP_TIMEOUT.as_secs()
                ),
            )
            .into()),
            Err(RecvTimeoutError::Disconnected) => {
                Err(anyhow::anyhow!("WebP decoder stopped without a result"))
            }
        }
    }
}
