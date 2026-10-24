//! Failures from device setup, swapchain configuration, and texture upload.

/// A renderer failure the daemon can show to the client.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// `request_adapter` returned no adapter for the requested backends.
    #[error("no wgpu adapter supports this surface")]
    AdapterMissing,
    /// `request_device` failed.
    #[error("wgpu device: {0}")]
    Device(String),
    /// Surface creation, configuration, or acquire failed.
    #[error("wgpu surface: {0}")]
    Surface(String),
    /// The surface exposed neither an 8-bit RGBA nor BGRA format.
    #[error("surface has no rgba8 or bgra8 format")]
    Format,
    /// The caller asked for a target whose size cannot be represented.
    #[error("render target has a zero dimension")]
    EmptyTarget,
    /// The image is larger than the device allows, or the byte layout is short.
    #[error("texture upload: {0}")]
    Upload(String),
    /// A CPU-side GPU record could not be written.
    #[error("gpu: {0}")]
    Gpu(String),
}
