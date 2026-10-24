//! Shared contract between the Iris manager and `iris-daemon`.
//!
//! This crate owns the JSON-RPC messages, the daemon configuration schema,
//! process logging, and the async length-prefixed frame helpers. The Linux
//! daemon still drives its socket with `calloop`. The Windows daemon and the
//! manager use the listeners and streams in [`transport`]. This crate does not
//! talk to Wayland or link a GPU backend.

#![forbid(unsafe_code)]

pub mod config;
pub mod error;
pub mod ipc;
pub mod logging;
pub mod transport;

pub use config::{named_pipe_path, DaemonConfig, IpcEndpoint, LogConfig};
pub use error::Error;
pub use ipc::{
    FrameDecoder, IpcCall, IpcErrorBody, IpcRequest, IpcResponse, MonitorInfo, MonitorTarget,
    Playback, RequestId, ResponseBody, RunState, ScaleMode, TelemetryReport, WallpaperConfig,
    WallpaperSource, WallpaperStatus, JSONRPC_VERSION, MAX_FRAME_BYTES, PROTOCOL_VERSION,
};
pub use logging::init as init_logging;
#[cfg(unix)]
pub use transport::bind_unix;
#[cfg(windows)]
pub use transport::NamedPipeListener;
pub use transport::{read_payload, write_bytes, write_payload};
