//! Wayland layer-shell surfaces for Iris wallpapers.
//!
//! The daemon multiplexes this connection with its IPC socket. Rendering stays
//! in `iris-render`: this crate does not link wgpu. On targets other than
//! Linux the crate is a stub so the workspace can be checked without libwayland.

#![cfg_attr(target_os = "linux", deny(unsafe_code))]

#[cfg(target_os = "linux")]
mod error;
#[cfg(target_os = "linux")]
mod handle;
#[cfg(target_os = "linux")]
mod pump;
#[cfg(target_os = "linux")]
mod scale;
#[cfg(target_os = "linux")]
mod session;
#[cfg(target_os = "linux")]
mod state;

#[cfg(target_os = "linux")]
pub use error::Error;
#[cfg(target_os = "linux")]
pub use handle::WaylandWindow;
#[cfg(target_os = "linux")]
pub use pump::WaylandPump;
#[cfg(target_os = "linux")]
pub use session::Session;
#[cfg(target_os = "linux")]
pub use state::{BackendEvent, LayerGeometry, OutputSnapshot};

/// Present so non-Linux builds of this crate have a callable symbol.
#[cfg(not(target_os = "linux"))]
pub fn unavailable() {}
