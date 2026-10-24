//! Failures the daemon reports over IPC or prints on exit.

use iris_core::ipc::{IpcErrorBody, CODE_INTERNAL, CODE_INVALID_PARAMS};

/// A daemon failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A socket or filesystem call failed.
    #[error("io: {0}")]
    Io(String),
    /// An IPC frame or config check failed.
    #[error(transparent)]
    Core(#[from] iris_core::Error),
    /// A package or texture could not be decoded.
    #[error(transparent)]
    Pkg(#[from] iris_pkg::Error),
    /// The GPU rejected a surface, upload, or draw.
    #[error(transparent)]
    Render(#[from] iris_render::Error),
    /// A video file could not be opened or decoded.
    #[cfg(target_os = "linux")]
    #[error(transparent)]
    Video(#[from] iris_video::Error),
    /// A scene graph could not be compiled.
    #[error(transparent)]
    Scene(#[from] iris_scene::Error),
    /// The Wayland connection or a layer failed.
    #[cfg(target_os = "linux")]
    #[error(transparent)]
    Wayland(#[from] iris_backend_wayland::Error),
    /// A Win32 window, monitor query, or desktop injection failed.
    #[cfg(target_os = "windows")]
    #[error(transparent)]
    Windows(#[from] iris_backend_windows::Error),
    /// The image crate rejected a file.
    #[error("image: {0}")]
    Image(String),
    /// A wallpaper source had no usable pixels.
    #[error("load: {0}")]
    Load(String),
    /// A web wallpaper could not be classified.
    #[error(transparent)]
    Web(#[from] iris_web::Error),
    /// The source or target is outside this phase.
    #[error("unsupported: {0}")]
    Unsupported(String),
    /// The requested output is not in the live set.
    #[error("invalid target: {0}")]
    InvalidTarget(String),
    /// The calloop loop stopped with an error.
    #[error("event loop: {0}")]
    Loop(String),
}

impl Error {
    /// JSON-RPC error object for this failure.
    pub(crate) fn to_ipc(&self) -> IpcErrorBody {
        match self {
            Self::InvalidTarget(message) => IpcErrorBody::new(CODE_INVALID_PARAMS, message.clone()),
            Self::Core(err) => IpcErrorBody::from_core(err),
            other => IpcErrorBody::new(CODE_INTERNAL, other.to_string()),
        }
    }
}
