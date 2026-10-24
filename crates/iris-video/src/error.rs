//! Failures from opening a file or starting the decode thread.

use std::path::Path;

/// A video failure. The display string always includes the file path when one
/// is known, so the daemon can show it over IPC.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The file could not be opened or has no video stream.
    #[error("{path}: {message}")]
    Open { path: String, message: String },
    /// The decode thread stopped before producing a frame.
    #[error("{path}: {message}")]
    Decode { path: String, message: String },
}

impl Error {
    pub(crate) fn open(path: &Path, message: impl Into<String>) -> Self {
        Self::Open {
            path: path.display().to_string(),
            message: message.into(),
        }
    }

    pub(crate) fn decode(path: &Path, message: impl Into<String>) -> Self {
        Self::Decode {
            path: path.display().to_string(),
            message: message.into(),
        }
    }
}
