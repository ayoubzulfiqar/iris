//! Async length-prefixed frames over a byte stream.
//!
//! Unix sockets and Windows named pipes both carry the same `u32` little-endian
//! length plus a JSON-RPC payload. The Linux daemon keeps its own `calloop`
//! socket. This module is the transport the Windows daemon and the manager share.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::ipc::{encode_frame, FrameDecoder};
use crate::Error;

/// Write one length-prefixed payload.
pub async fn write_payload<W>(writer: &mut W, payload: &[u8]) -> Result<(), Error>
where
    W: AsyncWrite + Unpin,
{
    let frame = encode_frame(payload)?;
    write_bytes(writer, &frame).await
}

/// Write bytes that are already a complete frame, or any other buffer.
pub async fn write_bytes<W>(writer: &mut W, bytes: &[u8]) -> Result<(), Error>
where
    W: AsyncWrite + Unpin,
{
    writer
        .write_all(bytes)
        .await
        .map_err(|err| Error::Io(err.to_string()))
}

/// Read one payload, appending into `decoder` until a frame is complete.
///
/// A closed stream is [`Error::Io`]. A length above the frame cap is the same
/// error [`FrameDecoder::next_payload`] returns, and the caller should drop
/// the connection.
pub async fn read_payload<R>(reader: &mut R, decoder: &mut FrameDecoder) -> Result<Vec<u8>, Error>
where
    R: AsyncRead + Unpin,
{
    let mut buf = [0u8; 8192];
    loop {
        if let Some(payload) = decoder.next_payload()? {
            return Ok(payload);
        }
        let read = reader
            .read(&mut buf)
            .await
            .map_err(|err| Error::Io(err.to_string()))?;
        if read == 0 {
            return Err(Error::Io("connection closed".to_owned()));
        }
        decoder.push(&buf[..read]);
    }
}

/// Bind a Unix socket, creating the parent directory when it is missing.
///
/// An existing socket file at `path` is removed before the bind. The path is
/// the one from [`crate::IpcEndpoint::local_default`]: `$XDG_RUNTIME_DIR/iris/iris.sock`,
/// or `/tmp/iris-$UID.sock`.
#[cfg(unix)]
pub fn bind_unix(path: &std::path::Path) -> Result<tokio::net::UnixListener, Error> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| Error::Io(err.to_string()))?;
        }
    }
    match tokio::net::UnixListener::bind(path) {
        Ok(listener) => Ok(listener),
        Err(err)
            if err.kind() == std::io::ErrorKind::AddrInUse
                || err.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            std::fs::remove_file(path).map_err(|err| Error::Io(err.to_string()))?;
            tokio::net::UnixListener::bind(path).map_err(|err| Error::Io(err.to_string()))
        }
        Err(err) => Err(Error::Io(err.to_string())),
    }
}

/// One Windows named-pipe instance waiting for the next client.
///
/// Tokio's pipe server accepts a single connection. [`NamedPipeListener::accept`]
/// replaces it so the following client can connect.
#[cfg(windows)]
pub struct NamedPipeListener {
    name: String,
    server: Option<tokio::net::windows::named_pipe::NamedPipeServer>,
}

#[cfg(windows)]
impl NamedPipeListener {
    /// Bind `name`, which includes the `\\.\pipe\` prefix.
    ///
    /// The first attempt asks to be the first instance. If the pipe already
    /// exists, a later instance is created on the same name.
    pub fn bind(name: &str) -> Result<Self, Error> {
        let server = open_server(name, true).or_else(|_| open_server(name, false))?;
        Ok(Self {
            name: name.to_owned(),
            server: Some(server),
        })
    }

    /// Wait until a client connects and rearm the listener for the next one.
    pub async fn accept(
        &mut self,
    ) -> Result<tokio::net::windows::named_pipe::NamedPipeServer, Error> {
        let server = self
            .server
            .take()
            .ok_or_else(|| Error::Io("named pipe listener is closed".to_owned()))?;
        match server.connect().await {
            Ok(()) => {
                self.server = Some(open_server(&self.name, false)?);
                Ok(server)
            }
            Err(err) => {
                self.server = open_server(&self.name, false).ok();
                Err(Error::Io(err.to_string()))
            }
        }
    }
}

#[cfg(windows)]
fn open_server(
    name: &str,
    first: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer, Error> {
    let mut options = tokio::net::windows::named_pipe::ServerOptions::new();
    if first {
        options.first_pipe_instance(true);
    }
    options
        .create(name)
        .map_err(|err| Error::Io(err.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn length_prefix_roundtrip_on_a_duplex() {
        let (mut writer, mut reader) = tokio::io::duplex(256);
        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        write_payload(&mut writer, payload).await.expect("write");
        let mut decoder = FrameDecoder::new();
        let got = read_payload(&mut reader, &mut decoder).await.expect("read");
        assert_eq!(got, payload);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn unix_bind_accepts_one_frame() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "iris-core-transport-{}-{nanos}.sock",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let listener = bind_unix(&path).expect("bind");
        let path_for_client = path.clone();
        let client = tokio::spawn(async move {
            let mut stream = tokio::net::UnixStream::connect(&path_for_client)
                .await
                .expect("connect");
            write_payload(&mut stream, b"{\"ok\":true}")
                .await
                .expect("write");
        });
        let (mut stream, _) = listener.accept().await.expect("accept");
        let mut decoder = FrameDecoder::new();
        let payload = read_payload(&mut stream, &mut decoder).await.expect("read");
        assert_eq!(payload, b"{\"ok\":true}");
        client.await.expect("client");
        let _ = std::fs::remove_file(&path);
    }
}
