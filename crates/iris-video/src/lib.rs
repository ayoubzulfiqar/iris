//! Video decode for wallpaper playback.
//!
//! Hardware decode is tried in order: VA-API, then CUDA / NVDEC. If neither
//! device opens, the stream falls back to a software decoder. Frames leave
//! this crate as YUV planes. The renderer owns the color conversion.
//!
//! Unsafe FFmpeg calls stay in this crate. Each block documents what libav
//! owns and how long the pointers live. The FFmpeg binding is Linux-only so a
//! Windows check does not need a MinGW FFmpeg.

#[cfg(target_os = "linux")]
mod error;
#[cfg(target_os = "linux")]
mod frame;
#[cfg(target_os = "linux")]
mod session;

#[cfg(target_os = "linux")]
pub use error::Error;
#[cfg(target_os = "linux")]
pub use frame::{DecodedVideoFrame, Matrix, Picture, Plane};
#[cfg(target_os = "linux")]
pub use session::{HwBackend, PlaybackOptions, VideoInfo, VideoPlayer};

/// `false` on targets that do not link FFmpeg.
#[cfg(not(target_os = "linux"))]
pub fn supported() -> bool {
    false
}
