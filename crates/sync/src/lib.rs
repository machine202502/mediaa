pub mod client;
pub mod events;
pub mod frame;
pub mod server;
pub mod state;

pub use client::{Client, ClientUpdate};
pub use events::{EventEntry, EventLog, EventsSince};
pub use frame::{Frame, PROTOCOL_VERSION};
pub use server::{CommandRequest, Server, ServerAction};
pub use state::{StateHub, StateView};

use std::fmt;
use std::io::{self, Read, Write};

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodecError(pub String);

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for CodecError {}

pub trait Codec {
    fn encode(&self, frame: &Frame) -> Result<Vec<u8>, CodecError>;
    fn decode(&self, bytes: &[u8]) -> Result<Frame, CodecError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct JsonCodec;

impl Codec for JsonCodec {
    fn encode(&self, frame: &Frame) -> Result<Vec<u8>, CodecError> {
        serde_json::to_vec(frame).map_err(|error| CodecError(error.to_string()))
    }

    fn decode(&self, bytes: &[u8]) -> Result<Frame, CodecError> {
        serde_json::from_slice(bytes).map_err(|error| CodecError(error.to_string()))
    }
}

pub fn write_frame<W: Write>(writer: &mut W, codec: &impl Codec, frame: &Frame) -> io::Result<()> {
    let bytes = codec
        .encode(frame)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame is too large",
        ));
    }
    writer.write_all(&(bytes.len() as u32).to_be_bytes())?;
    writer.write_all(&bytes)?;
    writer.flush()
}

pub fn read_frame<R: Read>(reader: &mut R, codec: &impl Codec) -> io::Result<Frame> {
    let mut length = [0_u8; 4];
    reader.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes)?;
    codec
        .decode(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_survive_the_codec() {
        let frames = vec![
            Frame::Hello {
                version: PROTOCOL_VERSION,
                client: "ui".into(),
            },
            Frame::Subscribe {
                stores: vec!["settings".into(), "recording".into()],
                versions: [("recording".into(), 4)].into_iter().collect(),
                event_seq: 7,
            },
            Frame::Changed {
                store: "recording".into(),
                version: 5,
                payload: vec![1, 2, 3],
            },
            Frame::Command {
                id: 9,
                name: "start_recording".into(),
                payload: b"{}".to_vec(),
            },
            Frame::Result {
                id: 9,
                ok: false,
                payload: Vec::new(),
                error: Some("busy".into()),
            },
            Frame::Event {
                seq: 12,
                kind: "notice".into(),
                payload: vec![9],
            },
            Frame::Ping,
        ];
        let codec = JsonCodec;
        for frame in frames {
            let bytes = codec.encode(&frame).expect("encode");
            assert_eq!(codec.decode(&bytes).expect("decode"), frame);
        }
    }

    #[test]
    fn framed_roundtrip_preserves_the_frame() {
        let frame = Frame::Snapshot {
            store: "library".into(),
            version: 42,
            payload: vec![7; 128],
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &JsonCodec, &frame).expect("write");
        let mut cursor = std::io::Cursor::new(bytes);
        assert_eq!(read_frame(&mut cursor, &JsonCodec).expect("read"), frame);
    }

    #[test]
    fn an_invalid_length_is_rejected() {
        let mut cursor = std::io::Cursor::new(vec![0, 0, 0, 0]);
        let error = read_frame(&mut cursor, &JsonCodec).expect_err("zero length");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);

        let mut cursor = std::io::Cursor::new(vec![0xFF, 0xFF, 0xFF, 0xFF]);
        let error = read_frame(&mut cursor, &JsonCodec).expect_err("oversize");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[test]
    fn a_truncated_frame_is_an_error() {
        let mut bytes = Vec::new();
        write_frame(
            &mut bytes,
            &JsonCodec,
            &Frame::Error {
                code: "x".into(),
                message: "y".into(),
            },
        )
        .expect("write");
        bytes.truncate(bytes.len() - 2);
        let error =
            read_frame(&mut std::io::Cursor::new(bytes), &JsonCodec).expect_err("truncated");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
