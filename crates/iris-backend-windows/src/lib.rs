//! Win32 wallpaper backend.
//!
//! WorkerW reparenting, a borderless window, monitor enumeration, and WASAPI
//! loopback live here. Rendering stays in `iris-render`: this crate does not
//! link wgpu. Win32 calls are `#[cfg(windows)]`. The WorkerW choice itself is
//! a pure function so it can be tested on Linux.

#![deny(unsafe_code)]

mod error;
mod inject;

pub use error::Error;
pub use inject::{target_workerw, TopLevelWindow};

#[cfg(windows)]
mod audio_wasapi;
#[cfg(windows)]
mod monitors;
#[cfg(windows)]
mod window;

#[cfg(windows)]
pub use audio_wasapi::Loopback;
#[cfg(windows)]
pub use inject::{embed_window, parent_under_desktop};
#[cfg(windows)]
pub use monitors::{enumerate_monitors, DisplayMonitor};
#[cfg(windows)]
pub use window::{pump_messages, DesktopWindow, NativeWindow, PumpEvent};
