use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;

pub async fn write_frame<W, T>(writer: &mut W, message: &T) -> io::Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(message)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if bytes.is_empty() || bytes.len() > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "message size is out of range: {} bytes (limit {MAX_FRAME_BYTES})",
                bytes.len()
            ),
        ));
    }
    writer
        .write_all(&(bytes.len() as u32).to_be_bytes())
        .await?;
    writer.write_all(&bytes).await?;
    writer.flush().await
}

pub async fn read_frame<R, T>(reader: &mut R) -> io::Result<T>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut header = [0_u8; 4];
    reader.read_exact(&mut header).await?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "message size is out of range: {length} bytes (limit {MAX_FRAME_BYTES}, header {:02x?})",
                header
            ),
        ));
    }
    let mut bytes = vec![0_u8; length];
    reader.read_exact(&mut bytes).await?;
    serde_json::from_slice(&bytes)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum Message {
        Hello { protocol: u32 },
        State { revision: u64, title: String },
    }

    #[tokio::test]
    async fn messages_survive_the_wire() {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let hello = Message::Hello { protocol: 1 };
        write_frame(&mut client, &hello).await.expect("write");
        assert_eq!(
            read_frame::<_, Message>(&mut server).await.expect("read"),
            hello
        );

        let state = Message::State {
            revision: 7,
            title: "Recording".into(),
        };
        write_frame(&mut client, &state).await.expect("write");
        assert_eq!(
            read_frame::<_, Message>(&mut server).await.expect("read"),
            state
        );
    }

    #[tokio::test]
    async fn zero_length_is_rejected() {
        let (mut client, mut server) = tokio::io::duplex(16);
        client.write_all(&[0, 0, 0, 0]).await.expect("write");
        let error = read_frame::<_, Message>(&mut server)
            .await
            .expect_err("zero length");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn oversize_length_is_rejected() {
        let (mut client, mut server) = tokio::io::duplex(16);
        client
            .write_all(&((MAX_FRAME_BYTES + 1) as u32).to_be_bytes())
            .await
            .expect("write");
        let error = read_frame::<_, Message>(&mut server)
            .await
            .expect_err("oversize");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }

    #[tokio::test]
    async fn truncated_message_is_rejected() {
        let (mut client, mut server) = tokio::io::duplex(64);
        let bytes = serde_json::to_vec(&Message::Hello { protocol: 1 }).expect("json");
        client
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .await
            .expect("header");
        client
            .write_all(&bytes[..bytes.len() - 2])
            .await
            .expect("partial");
        drop(client);
        let error = read_frame::<_, Message>(&mut server)
            .await
            .expect_err("truncated");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
