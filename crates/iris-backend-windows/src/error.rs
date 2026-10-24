//! Failures from the Win32 shell, a window, or loopback capture.

/// A Windows backend failure. Injection misses return this instead of panicking.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// A Win32 call failed.
    #[error("win32: {0}")]
    Win32(String),
    /// Progman or the WorkerW canvas was not found.
    #[error("desktop: {0}")]
    Desktop(String),
    /// WASAPI loopback could not start or read.
    #[error("audio: {0}")]
    Audio(String),
}
