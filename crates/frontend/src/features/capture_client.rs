use crate::infra::ipc::{Connection, CLIENT_CAPTURE};
use mediaa_core::Frame;
use mediaa_protocol::backend::{
    decode_shot_result, encode_capture_request, encode_screenshot_commit, CaptureRequestWire,
    CaptureSourceWire, CaptureTargetWire, ScreenshotCommitWire, ShotResultWire,
};
use mediaa_sync::Frame as SyncFrame;
use parking_lot::Mutex;
use std::path::Path;
use std::sync::Arc;

const MAGIC: &[u8; 4] = b"MCA1";

#[derive(Clone, Default)]
pub(crate) struct CaptureClient {
    connection: Arc<Mutex<Option<Connection>>>,
    next_id: Arc<Mutex<u64>>,
}

impl CaptureClient {
    pub(crate) fn capture(&self, target: CaptureTargetWire) -> Result<CaptureSourceWire, String> {
        let payload = encode_capture_request(&CaptureRequestWire { target });
        let reply = self.request(mediaa_protocol::backend::CAPTURE_REQUEST, payload)?;
        serde_json::from_slice(&reply).map_err(|error| error.to_string())
    }

    pub(crate) fn commit(&self, commit: &ScreenshotCommitWire) -> Result<ShotResultWire, String> {
        let payload = encode_screenshot_commit(commit);
        let reply = self.request(mediaa_protocol::backend::SCREENSHOT_COMMIT, payload)?;
        decode_shot_result(&reply)
    }

    fn request(&self, name: &str, payload: Vec<u8>) -> Result<Vec<u8>, String> {
        let mut guard = self.connection.lock();
        if guard.is_none() {
            *guard = Some(connect()?);
        }
        let connection = guard.as_mut().expect("capture connection");
        let id = {
            let mut next = self.next_id.lock();
            *next += 1;
            *next
        };
        connection
            .send_frame(&SyncFrame::Command {
                id,
                name: name.to_string(),
                payload,
            })
            .map_err(|error| error.to_string())?;
        loop {
            let frame = connection.recv_frame().map_err(|error| error.to_string())?;
            let SyncFrame::Result {
                id: answer,
                ok,
                payload,
                error,
            } = frame
            else {
                continue;
            };
            if answer != id {
                continue;
            }
            return if ok {
                Ok(payload)
            } else {
                Err(error.unwrap_or_else(|| "capture failed".into()))
            };
        }
    }
}

fn connect() -> Result<Connection, String> {
    let scope = crate::dev_mode::mode()
        .scope
        .clone()
        .unwrap_or_else(|| "default".to_string());
    let mut connection = Connection::connect(&scope).map_err(|error| error.to_string())?;
    connection
        .send_frame(&SyncFrame::Hello {
            version: mediaa_sync::PROTOCOL_VERSION,
            client: CLIENT_CAPTURE.into(),
        })
        .map_err(|error| error.to_string())?;
    match connection.recv_frame().map_err(|error| error.to_string())? {
        SyncFrame::Welcome { .. } => Ok(connection),
        SyncFrame::Reject { reason } => Err(reason),
        _ => Err("unexpected capture handshake reply".into()),
    }
}

pub(crate) fn read_source(path: &Path) -> Result<Frame, String> {
    let data = std::fs::read(path).map_err(|error| error.to_string())?;
    if data.len() < 16 || &data[..4] != MAGIC {
        return Err("bad capture file".into());
    }
    let width = u32::from_le_bytes(data[4..8].try_into().map_err(|_| "width")?);
    let height = u32::from_le_bytes(data[8..12].try_into().map_err(|_| "height")?);
    let stride = u32::from_le_bytes(data[12..16].try_into().map_err(|_| "stride")?);
    if stride != width * 4 {
        return Err("bad capture stride".into());
    }
    let pixels = data[16..].to_vec();
    if pixels.len() != (width as usize) * (height as usize) * 4 {
        return Err("bad capture size".into());
    }
    Ok(Frame {
        width,
        height,
        data: pixels,
    })
}

pub(crate) fn write_source(path: &Path, frame: &Frame) -> Result<(), String> {
    let mut out = Vec::with_capacity(16 + frame.data.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&frame.width.to_le_bytes());
    out.extend_from_slice(&frame.height.to_le_bytes());
    out.extend_from_slice(&(frame.width * 4).to_le_bytes());
    out.extend_from_slice(&frame.data);
    std::fs::write(path, out).map_err(|error| error.to_string())
}
