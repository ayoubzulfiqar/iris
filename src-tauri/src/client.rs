//! Async length-prefixed client for `iris-daemon`.
//!
//! Linux dials the Unix socket. Windows dials `\\.\pipe\iris-ipc-{username}`.
//! One [`DaemonClient`] is shared by Tauri commands and the tray. Reads and
//! writes run on the Tokio runtime, so the window thread never blocks on a
//! frame. A broken pipe or a refused connection drops the stream and tries
//! once more before the error reaches the UI.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use iris_core::ipc::{IpcRequest, IpcResponse, Outcome};
use iris_core::{
    IpcCall, IpcEndpoint, MonitorInfo, MonitorTarget, RequestId, ResponseBody, TelemetryReport,
    WallpaperConfig, WallpaperStatus, read_payload, write_payload,
};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::Mutex;

#[cfg(unix)]
use tokio::net::UnixStream;
#[cfg(windows)]
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

const PING_TIMEOUT: Duration = Duration::from_millis(500);
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Failures talking to `iris-daemon`. The UI receives the display text.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// `connect` failed before a stream existed.
    #[error("socket connect: {0}")]
    Connect(String),
    /// A connected stream failed mid-frame.
    #[error("socket io: {0}")]
    Io(String),
    /// The bytes were not a JSON-RPC frame this client understands.
    #[error("ipc: {0}")]
    Protocol(String),
    /// The daemon answered with a JSON-RPC error object.
    #[error("daemon error {code}: {message}")]
    Daemon {
        /// JSON-RPC code.
        code: i32,
        /// Daemon message.
        message: String,
    },
    /// The response was a success body of the wrong kind.
    #[error("unexpected daemon response")]
    Unexpected,
    /// The daemon accepted the socket and then stayed silent.
    #[error("daemon call timed out")]
    Timeout,
}

impl ClientError {
    fn should_reconnect(&self) -> bool {
        matches!(
            self,
            Self::Connect(_) | Self::Io(_) | Self::Timeout | Self::Protocol(_)
        )
    }
}

struct ClientInner {
    socket: PathBuf,
    stream: Mutex<Option<DaemonStream>>,
    next_id: AtomicI64,
    supervisor: Mutex<()>,
}

/// Thread-safe client. Cloning shares the same connection.
#[derive(Clone)]
pub struct DaemonClient {
    inner: Arc<ClientInner>,
}

impl DaemonClient {
    /// Client aimed at one socket path.
    pub fn new(socket: impl Into<PathBuf>) -> Self {
        Self {
            inner: Arc::new(ClientInner {
                socket: socket.into(),
                stream: Mutex::new(None),
                next_id: AtomicI64::new(1),
                supervisor: Mutex::new(()),
            }),
        }
    }

    /// Client for [`IpcEndpoint::local_default`].
    pub fn local() -> Self {
        match IpcEndpoint::local_default() {
            IpcEndpoint::Unix { path } => Self::new(path),
            IpcEndpoint::NamedPipe { name } => Self::new(name),
        }
    }

    /// Socket this client dials.
    pub fn socket_path(&self) -> &Path {
        &self.inner.socket
    }

    /// Hold the supervisor lock so only one spawn loop runs.
    pub(crate) async fn lock_supervisor(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.inner.supervisor.lock().await
    }

    /// Liveness check. The value is the daemon protocol generation.
    pub async fn ping(&self) -> Result<u32, ClientError> {
        match self.roundtrip(&IpcCall::Ping, PING_TIMEOUT).await? {
            ResponseBody::Pong { protocol } => Ok(protocol),
            _ => Err(ClientError::Unexpected),
        }
    }

    /// Outputs the daemon currently sees.
    pub async fn list_monitors(&self) -> Result<Vec<MonitorInfo>, ClientError> {
        match self.roundtrip(&IpcCall::ListMonitors, CALL_TIMEOUT).await? {
            ResponseBody::Monitors { monitors } => Ok(monitors),
            _ => Err(ClientError::Unexpected),
        }
    }

    /// Latest CPU, memory, network, and battery sample.
    pub async fn get_telemetry(&self) -> Result<TelemetryReport, ClientError> {
        match self.roundtrip(&IpcCall::GetTelemetry, CALL_TIMEOUT).await? {
            ResponseBody::Telemetry(report) => Ok(report),
            _ => Err(ClientError::Unexpected),
        }
    }

    /// Whether the daemon is running and which wallpapers are assigned.
    pub async fn get_status(&self) -> Result<(bool, Vec<WallpaperStatus>), ClientError> {
        match self.roundtrip(&IpcCall::GetStatus, CALL_TIMEOUT).await? {
            ResponseBody::Status {
                running,
                wallpapers,
            } => Ok((running, wallpapers)),
            _ => Err(ClientError::Unexpected),
        }
    }

    /// Assign `config` to `target`. `target` replaces `config.target`.
    pub async fn apply_wallpaper(
        &self,
        target: MonitorTarget,
        mut config: WallpaperConfig,
    ) -> Result<(), ClientError> {
        config.target = target;
        config
            .validate()
            .map_err(|err| ClientError::Protocol(err.to_string()))?;
        let body = self
            .roundtrip(&IpcCall::ApplyWallpaper { config }, CALL_TIMEOUT)
            .await?;
        expect_ack(body)
    }

    /// Remove the wallpaper on `target`.
    pub async fn clear_wallpaper(&self, target: MonitorTarget) -> Result<(), ClientError> {
        let body = self
            .roundtrip(&IpcCall::ClearWallpaper { target }, CALL_TIMEOUT)
            .await?;
        expect_ack(body)
    }

    /// Pause rendering on `target`.
    pub async fn pause(&self, target: MonitorTarget) -> Result<(), ClientError> {
        let body = self
            .roundtrip(
                &IpcCall::Pause {
                    target: Some(target),
                },
                CALL_TIMEOUT,
            )
            .await?;
        expect_ack(body)
    }

    /// Move the parallax pointer, in output pixels.
    pub async fn set_pointer(&self, x: f32, y: f32) -> Result<(), ClientError> {
        let body = self
            .roundtrip(&IpcCall::SetPointer { x, y }, CALL_TIMEOUT)
            .await?;
        expect_ack(body)
    }

    /// Resume rendering on `target`.
    pub async fn resume(&self, target: MonitorTarget) -> Result<(), ClientError> {
        let body = self
            .roundtrip(
                &IpcCall::Resume {
                    target: Some(target),
                },
                CALL_TIMEOUT,
            )
            .await?;
        expect_ack(body)
    }

    /// Ask the daemon to exit, then drop the local stream.
    pub async fn shutdown_daemon(&self) -> Result<(), ClientError> {
        let body = self.roundtrip(&IpcCall::Shutdown, SHUTDOWN_TIMEOUT).await?;
        self.disconnect().await;
        expect_ack(body)
    }

    async fn disconnect(&self) {
        let mut guard = self.inner.stream.lock().await;
        *guard = None;
    }

    async fn roundtrip(
        &self,
        call: &IpcCall,
        timeout: Duration,
    ) -> Result<ResponseBody, ClientError> {
        match self.roundtrip_once(call, timeout).await {
            Err(err) if err.should_reconnect() => {
                tracing::debug!("reconnecting to {}", self.inner.socket.display());
                self.disconnect().await;
                self.roundtrip_once(call, timeout).await
            }
            other => other,
        }
    }

    async fn roundtrip_once(
        &self,
        call: &IpcCall,
        timeout: Duration,
    ) -> Result<ResponseBody, ClientError> {
        let id = RequestId::Number(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let request = IpcRequest::from_call(id.clone(), call)
            .map_err(|err| ClientError::Protocol(err.to_string()))?;
        let payload = request
            .to_payload()
            .map_err(|err| ClientError::Protocol(err.to_string()))?;

        let mut guard = self.inner.stream.lock().await;
        if guard.is_none() {
            match connect(&self.inner.socket).await {
                Ok(stream) => *guard = Some(stream),
                Err(err) => return Err(err),
            }
        }
        let stream = match guard.as_mut() {
            Some(stream) => stream,
            None => return Err(ClientError::Connect("socket is not connected".to_owned())),
        };

        let result = match tokio::time::timeout(timeout, transact(stream, &payload, &id)).await {
            Ok(inner) => inner,
            Err(_) => Err(ClientError::Timeout),
        };
        if result.as_ref().is_err_and(ClientError::should_reconnect) {
            *guard = None;
        }
        result
    }
}

fn expect_ack(body: ResponseBody) -> Result<(), ClientError> {
    match body {
        ResponseBody::Ack => Ok(()),
        _ => Err(ClientError::Unexpected),
    }
}

enum DaemonStream {
    #[cfg(unix)]
    Unix(UnixStream),
    #[cfg(windows)]
    Pipe(NamedPipeClient),
}

#[cfg(unix)]
async fn connect(socket: &Path) -> Result<DaemonStream, ClientError> {
    UnixStream::connect(socket)
        .await
        .map(|stream| DaemonStream::Unix(stream))
        .map_err(|err| ClientError::Connect(format!("{}: {err}", socket.display())))
}

#[cfg(windows)]
async fn connect(socket: &Path) -> Result<DaemonStream, ClientError> {
    const PIPE_BUSY: i32 = 231;
    let name = socket.to_string_lossy();
    let mut delay = Duration::from_millis(50);
    for _ in 0..8 {
        match ClientOptions::new().open(name.as_ref()) {
            Ok(client) => return Ok(DaemonStream::Pipe(client)),
            Err(err) if err.raw_os_error() == Some(PIPE_BUSY) => {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_millis(200));
            }
            Err(err) => {
                return Err(ClientError::Connect(format!("{name}: {err}")));
            }
        }
    }
    Err(ClientError::Connect(format!("{name}: pipe is busy")))
}

async fn transact(
    stream: &mut DaemonStream,
    payload: &[u8],
    expected_id: &RequestId,
) -> Result<ResponseBody, ClientError> {
    let bytes = match stream {
        #[cfg(unix)]
        DaemonStream::Unix(stream) => exchange(stream, payload).await,
        #[cfg(windows)]
        DaemonStream::Pipe(stream) => exchange(stream, payload).await,
    }?;
    let response =
        IpcResponse::from_payload(&bytes).map_err(|err| ClientError::Protocol(err.to_string()))?;
    if &response.id != expected_id {
        return Err(ClientError::Protocol(
            "response id did not match the request".to_owned(),
        ));
    }
    match response
        .outcome()
        .map_err(|err| ClientError::Protocol(err.to_string()))?
    {
        Outcome::Success(body) => Ok(body.clone()),
        Outcome::Failure(error) => Err(ClientError::Daemon {
            code: error.code,
            message: error.message.clone(),
        }),
    }
}

async fn exchange<S>(stream: &mut S, payload: &[u8]) -> Result<Vec<u8>, ClientError>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    write_payload(stream, payload).await.map_err(map_core)?;
    let mut decoder = iris_core::FrameDecoder::new();
    read_payload(stream, &mut decoder).await.map_err(map_core)
}

fn map_core(err: iris_core::Error) -> ClientError {
    match err {
        iris_core::Error::Io(message) => ClientError::Io(message),
        other => ClientError::Protocol(other.to_string()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use iris_core::{IpcResponse, PROTOCOL_VERSION, ResponseBody};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::UnixListener;

    use super::*;

    fn temp_socket(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        PathBuf::from(format!(
            "/tmp/iris-mgr-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    async fn read_payload(stream: &mut UnixStream) -> Vec<u8> {
        let mut decoder = iris_core::FrameDecoder::new();
        let mut buf = vec![0u8; 8192];
        loop {
            let read = stream.read(&mut buf).await.expect("server read");
            assert!(read > 0, "client closed before a frame");
            decoder.push(&buf[..read]);
            if let Some(payload) = decoder.next_payload().expect("frame") {
                return payload;
            }
        }
    }

    async fn write_pong(stream: &mut UnixStream) {
        let payload = read_payload(stream).await;
        let request = IpcRequest::from_payload(&payload).expect("request");
        assert_eq!(request.method, "ping");
        let response = IpcResponse::success(
            request.id,
            ResponseBody::Pong {
                protocol: PROTOCOL_VERSION,
            },
        );
        let frame = response.to_frame().expect("frame");
        stream.write_all(&frame).await.expect("server write");
    }

    #[tokio::test]
    async fn ping_roundtrip_on_a_temp_socket() {
        let path = temp_socket("ping");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            write_pong(&mut stream).await;
        });

        let client = DaemonClient::new(&path);
        let protocol = client.ping().await.expect("ping");
        assert_eq!(protocol, PROTOCOL_VERSION);
        server.await.expect("server");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn broken_connection_reconnects_once() {
        let path = temp_socket("retry");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("first accept");
            drop(stream);
            let (mut stream, _) = listener.accept().await.expect("second accept");
            write_pong(&mut stream).await;
        });

        let client = DaemonClient::new(&path);
        let protocol = client.ping().await.expect("ping after reconnect");
        assert_eq!(protocol, PROTOCOL_VERSION);
        server.await.expect("server");
        let _ = std::fs::remove_file(&path);
    }

    #[tokio::test]
    async fn missing_socket_returns_an_error() {
        let path = temp_socket("missing");
        let _ = std::fs::remove_file(&path);
        let client = DaemonClient::new(&path);
        let err = client.ping().await.expect_err("socket is absent");
        let text = err.to_string();
        assert!(
            text.contains("socket connect"),
            "error should name the connect failure, got {text}"
        );
    }
}
