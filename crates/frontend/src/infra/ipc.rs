use interprocess::local_socket::prelude::*;
use interprocess::local_socket::GenericFilePath;
#[cfg(test)]
use interprocess::local_socket::ListenerOptions;
use interprocess::TryClone;
use mediaa_sync::{read_frame, write_frame, Frame, JsonCodec};
use std::io::{self, BufReader};
use std::path::PathBuf;

pub(crate) const CLIENT_UI: &str = "ui";
pub(crate) const CLIENT_CAPTURE: &str = "capture";

pub(crate) fn socket_name(scope: &str) -> PathBuf {
    mediaa_platform::ipc::socket_name(scope)
}

#[cfg(test)]
pub(crate) struct Listener {
    inner: LocalSocketListener,
}

#[cfg(test)]
impl Listener {
    pub(crate) fn bind(scope: &str) -> io::Result<Self> {
        mediaa_platform::ipc::prepare(scope);
        let path = socket_name(scope);
        let name = path.as_os_str().to_fs_name::<GenericFilePath>()?;
        let inner = ListenerOptions::new().name(name).create_sync()?;
        Ok(Self { inner })
    }

    pub(crate) fn accept(&self) -> io::Result<Connection> {
        Connection::new(self.inner.accept()?)
    }
}

pub(crate) struct Connection {
    reader: BufReader<LocalSocketStream>,
    writer: LocalSocketStream,
}

impl Connection {
    fn new(stream: LocalSocketStream) -> io::Result<Self> {
        let writer = stream.try_clone()?;
        Ok(Self {
            reader: BufReader::new(stream),
            writer,
        })
    }

    pub(crate) fn connect(scope: &str) -> io::Result<Self> {
        let path = socket_name(scope);
        let name = path.as_os_str().to_fs_name::<GenericFilePath>()?;
        Self::new(LocalSocketStream::connect(name)?)
    }

    pub(crate) fn send_frame(&mut self, frame: &Frame) -> io::Result<()> {
        write_frame(&mut self.writer, &JsonCodec, frame)
    }

    pub(crate) fn recv_frame(&mut self) -> io::Result<Frame> {
        read_frame(&mut self.reader, &JsonCodec)
    }

    pub(crate) fn try_clone(&self) -> io::Result<Self> {
        let reader = BufReader::new(self.reader.get_ref().try_clone()?);
        let writer = self.writer.try_clone()?;
        Ok(Self { reader, writer })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mediaa_sync::PROTOCOL_VERSION;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_scope(test: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        format!("test-{test}-{}-{nanos}", std::process::id())
    }

    #[test]
    fn a_client_and_a_server_exchange_frames() {
        let scope = unique_scope("frame");
        let listener = Listener::bind(&scope).expect("bind");
        let server = std::thread::spawn(move || {
            let mut connection = listener.accept().expect("accept");
            let hello = connection.recv_frame().expect("hello");
            connection
                .send_frame(&Frame::Welcome {
                    version: PROTOCOL_VERSION,
                    session: 1,
                    event_seq: 0,
                    metadata: Default::default(),
                })
                .expect("welcome");
            hello
        });
        let mut client = Connection::connect(&scope).expect("connect");
        client
            .send_frame(&Frame::Hello {
                version: PROTOCOL_VERSION,
                client: CLIENT_UI.into(),
            })
            .expect("hello");
        assert!(matches!(
            client.recv_frame().expect("welcome"),
            Frame::Welcome { .. }
        ));
        assert!(matches!(
            server.join().expect("server"),
            Frame::Hello { .. }
        ));
    }

    #[test]
    fn a_cloned_connection_can_send() {
        let scope = unique_scope("clone");
        let listener = Listener::bind(&scope).expect("bind");
        let server = std::thread::spawn(move || {
            let mut connection = listener.accept().expect("accept");
            connection.recv_frame()
        });
        let client = Connection::connect(&scope).expect("connect");
        let mut clone = client.try_clone().expect("clone");
        clone.send_frame(&Frame::Ping).expect("send");
        assert!(matches!(
            server.join().expect("server").expect("frame"),
            Frame::Ping
        ));
    }

    #[test]
    fn a_closed_channel_is_detected_as_eof() {
        let scope = unique_scope("eof");
        let listener = Listener::bind(&scope).expect("bind");
        let (tx, rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let mut connection = listener.accept().expect("accept");
            tx.send(()).expect("ready");
            connection.recv_frame()
        });
        let client = Connection::connect(&scope).expect("connect");
        rx.recv().expect("ready");
        drop(client);
        let error = server.join().expect("server").expect_err("eof");
        assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof);
    }
}
