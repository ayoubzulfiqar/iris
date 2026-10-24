//! Start `iris-daemon` when the manager cannot reach its socket.
//!
//! The daemon is spawned with `kill_on_drop(false)` and its own process group,
//! so closing the manager does not signal it. A later Quit command asks the
//! daemon to shut down over IPC.

use std::path::PathBuf;
use std::time::Duration;

use tokio::process::Command;

use crate::client::DaemonClient;

const BUDGET: Duration = Duration::from_secs(3);

/// The socket stayed dark for the whole backoff window.
#[derive(Debug, thiserror::Error)]
pub enum SupervisorError {
    /// Ping never succeeded.
    #[error("{message}")]
    Unavailable {
        /// Text the UI can show.
        message: String,
    },
}

/// Ping the default socket. Spawn `iris-daemon` when the ping fails, then
/// retry with exponential backoff for up to three seconds.
pub async fn ensure_running(client: &DaemonClient) -> Result<(), SupervisorError> {
    ensure_with(client, spawn_iris_daemon).await
}

async fn ensure_with<F, Fut>(client: &DaemonClient, spawn: F) -> Result<(), SupervisorError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    let _guard = client.lock_supervisor().await;
    if client.ping().await.is_ok() {
        return Ok(());
    }
    let spawned = spawn().await;
    let started = std::time::Instant::now();
    let mut delay = Duration::from_millis(100);
    let mut last_error = String::from("socket is not active");
    while started.elapsed() < BUDGET {
        let remaining = BUDGET.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        tokio::time::sleep(delay.min(remaining)).await;
        match client.ping().await {
            Ok(_) => return Ok(()),
            Err(err) => last_error = err.to_string(),
        }
        delay = delay.saturating_mul(2).min(Duration::from_millis(800));
    }
    let socket = client.socket_path().display();
    let message = match spawned {
        Ok(()) => {
            format!("iris-daemon did not open its socket {socket} within 3 seconds ({last_error})")
        }
        Err(err) => format!("iris-daemon is not available at {socket} ({last_error}). {err}"),
    };
    Err(SupervisorError::Unavailable { message })
}

async fn spawn_iris_daemon() -> Result<(), String> {
    match try_spawn(Command::new("iris-daemon")) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => match sibling_daemon() {
            Some(path) => try_spawn(Command::new(path))
                .map_err(|err| format!("failed to spawn the iris-daemon next to this app: {err}")),
            None => Err(format!(
                "iris-daemon was not found on PATH ({err}). Build it with `cargo build -p iris-daemon` and place it on PATH, or next to the Iris executable."
            )),
        },
        Err(err) => Err(format!("failed to spawn iris-daemon: {err}")),
    }
}

fn try_spawn(mut command: Command) -> std::io::Result<()> {
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(false);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = command.spawn()?;
    tokio::spawn(async move {
        match child.wait().await {
            Ok(status) => tracing::info!("iris-daemon exited: {status}"),
            Err(err) => tracing::warn!("iris-daemon wait failed: {err}"),
        }
    });
    Ok(())
}

fn sibling_daemon() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let path = exe.parent()?.join(daemon_file_name());
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if meta.file_type().is_file() {
        Some(path)
    } else {
        None
    }
}

fn daemon_file_name() -> &'static str {
    if cfg!(windows) {
        "iris-daemon.exe"
    } else {
        "iris-daemon"
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use iris_core::{IpcResponse, PROTOCOL_VERSION, ResponseBody};
    #[cfg(unix)]
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    #[cfg(unix)]
    use tokio::net::UnixListener;

    use super::*;

    fn temp_socket(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        PathBuf::from(format!(
            "/tmp/iris-sup-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    #[tokio::test]
    async fn gives_up_when_the_socket_stays_down() {
        let client = DaemonClient::new(temp_socket("down"));
        let spawned = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&spawned);
        let started = std::time::Instant::now();
        let result = ensure_with(&client, || {
            flag.store(true, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await;
        assert!(spawned.load(Ordering::SeqCst));
        let message = result.expect_err("socket stays down").to_string();
        assert!(
            message.contains("did not open its socket"),
            "message should report the timeout, got {message}"
        );
        let elapsed = started.elapsed();
        assert!(
            elapsed >= Duration::from_millis(2800),
            "backoff ended early: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(6),
            "backoff ran long: {elapsed:?}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn skips_spawn_when_ping_succeeds() {
        let path = temp_socket("up");
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.expect("accept");
            let mut decoder = iris_core::FrameDecoder::new();
            let mut buf = vec![0u8; 1024];
            let payload = loop {
                let read = stream.read(&mut buf).await.expect("read");
                decoder.push(&buf[..read]);
                if let Some(payload) = decoder.next_payload().expect("frame") {
                    break payload;
                }
            };
            let request = iris_core::IpcRequest::from_payload(&payload).expect("request");
            let response = IpcResponse::success(
                request.id,
                ResponseBody::Pong {
                    protocol: PROTOCOL_VERSION,
                },
            );
            stream
                .write_all(&response.to_frame().expect("frame"))
                .await
                .expect("write");
        });

        let client = DaemonClient::new(&path);
        let spawned = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&spawned);
        ensure_with(&client, || {
            flag.store(true, Ordering::SeqCst);
            async { Ok(()) }
        })
        .await
        .expect("daemon already answers");
        assert!(!spawned.load(Ordering::SeqCst));
        server.await.expect("server");
        let _ = std::fs::remove_file(&path);
    }
}
