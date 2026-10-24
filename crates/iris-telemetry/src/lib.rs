//! System telemetry for wallpaper uniforms and power policy.
//!
//! A background thread samples CPU, memory, network, and battery on a 500 ms
//! cadence and publishes the result through [`arc_swap::ArcSwap`]. The frame
//! loop only loads that snapshot. Fullscreen detection uses the same cadence.

#![deny(unsafe_code)]

mod game_mode;
mod metrics;
mod power;

pub use game_mode::{any_output_covered, coverage_ratio, sway_tree_is_fullscreen, PixelRect};
pub use metrics::{percent_unit, unit_interval, Hub, TelemetrySnapshot, DEFAULT_INTERVAL};
pub use power::{net_unit, PowerSource, PowerState, NET_FULL_SCALE_BPS};

/// Fullscreen or borderless-game focus, as of the latest sample.
pub fn is_fullscreen_app_active(hub: &Hub) -> bool {
    hub.is_fullscreen_app_active()
}
