//! Failures from the Wayland connection, layer shell, and output tracking.

/// A Wayland failure the daemon can report over IPC.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A socket or file operation failed.
    #[error("wayland io: {0}")]
    Io(String),
    /// `WAYLAND_DISPLAY` is missing or the compositor refused the connection.
    #[error("wayland connect: {0}")]
    Connect(String),
    /// A protocol object did not reach the state the caller needs.
    #[error("wayland protocol: {0}")]
    Protocol(String),
    /// A required global is missing, or binding it failed.
    #[error("wayland bind: {0}")]
    Bind(String),
    /// The requested output is not in the live set.
    #[error("output {0} is not available")]
    NoOutput(u32),
    /// The layer id is not in the session.
    #[error("unknown layer {0}")]
    UnknownLayer(u64),
    /// Reading or dispatching the Wayland socket failed.
    #[error("wayland dispatch: {0}")]
    Dispatch(String),
}
