//! Failures produced while encoding, decoding, or checking Iris IPC messages.

/// Error returned by [`crate`] helpers.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `serde_json` rejected a payload.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// A frame header said the payload was zero bytes long.
    #[error("ipc frame is empty")]
    EmptyFrame,

    /// A frame header asked for more bytes than the daemon will buffer.
    #[error("ipc frame length {len} exceeds the {max} byte limit")]
    FrameTooLarge {
        /// Length declared by the frame header.
        len: u32,
        /// Configured ceiling, in bytes.
        max: u32,
    },

    /// The `jsonrpc` member was present and was not `"2.0"`.
    #[error("unsupported jsonrpc version `{0}`")]
    BadVersion(String),

    /// The method name is not part of this protocol version.
    #[error("unknown ipc method `{0}`")]
    UnknownMethod(String),

    /// The method is known, but `params` does not match its schema.
    #[error("invalid params for `{method}`: {detail}")]
    InvalidParams {
        /// Method name from the request.
        method: String,
        /// What failed while reading `params`.
        detail: String,
    },

    /// [`crate::WallpaperConfig`] failed a range or presence check.
    #[error("invalid wallpaper config: {0}")]
    InvalidConfig(String),

    /// A response carried both `result` and `error`, or neither.
    #[error("ipc response must contain exactly one of result or error")]
    MalformedResponse,

    /// The configured tracing filter directive did not parse.
    #[error("invalid log filter `{filter}`: {detail}")]
    LogFilter {
        /// Directive that was rejected.
        filter: String,
        /// Parser detail.
        detail: String,
    },

    /// A global tracing subscriber is already installed.
    #[error("logging subscriber is already installed: {0}")]
    LogInit(String),

    /// An Iris type could not be turned into a JSON-RPC envelope.
    #[error("protocol encoding failed: {0}")]
    Protocol(String),

    /// A transport read or write failed.
    #[error("ipc io: {0}")]
    Io(String),
}
