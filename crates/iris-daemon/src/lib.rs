//! Iris wallpaper daemon.
//!
//! Linux listens on a Unix socket and presents on Wayland layer-shell surfaces.
//! Frame pacing there comes from `wl_surface.frame`. Windows listens on
//! `\\.\pipe\iris-ipc-{username}` and presents on a borderless window reparented
//! to the desktop WorkerW. The message pump runs on that same thread.

#![forbid(unsafe_code)]

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "windows")]
mod app_windows;
mod error;
mod load;
mod loader;
mod policy;

pub use error::Error;

use iris_core::{init_logging, DaemonConfig};

/// Load the default config, start logging, and serve until shutdown.
pub fn run() -> Result<(), Error> {
    let config = DaemonConfig::default();
    init_logging(&config.log)?;
    run_with(config)
}

/// Serve with `config`.
///
/// Tests call this directly so they do not install a process-wide tracing subscriber.
pub fn run_with(config: DaemonConfig) -> Result<(), Error> {
    #[cfg(target_os = "linux")]
    {
        return app::serve(config);
    }
    #[cfg(target_os = "windows")]
    {
        return app_windows::serve(config);
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        let _ = config;
        Err(Error::Unsupported(
            "this operating system has no wallpaper backend".to_owned(),
        ))
    }
}
