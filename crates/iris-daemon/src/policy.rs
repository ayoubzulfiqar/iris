//! Fullscreen pause and battery frame cap.
//!
//! The sampler thread owns the probes. This module only reads the published
//! snapshot and decides what the frame loop should do.

use std::time::{Duration, Instant};

use iris_core::{TelemetryReport, WallpaperConfig};
use iris_render::{TelemetryFrame, TelemetryUniforms};
use iris_telemetry::{Hub, TelemetrySnapshot};

/// What one slot should do after a context sample.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextStep {
    /// Leave the slot as it is.
    Hold,
    /// Pause because a fullscreen app is in front.
    AutoPause,
    /// The fullscreen app left. Resume a slot this policy paused.
    AutoResume,
}

/// `want_pause` is the fullscreen policy for this wallpaper.
pub(crate) fn context_step(
    user_paused: bool,
    auto_paused: bool,
    playing: bool,
    want_pause: bool,
) -> ContextStep {
    if user_paused {
        return ContextStep::Hold;
    }
    if want_pause {
        if playing {
            return ContextStep::AutoPause;
        }
        return ContextStep::Hold;
    }
    if auto_paused {
        ContextStep::AutoResume
    } else {
        ContextStep::Hold
    }
}

/// Fullscreen or game focus, gated by the daemon and the wallpaper.
pub(crate) fn want_pause(
    daemon_fullscreen: bool,
    daemon_game: bool,
    config: &WallpaperConfig,
    fullscreen: bool,
) -> bool {
    if !fullscreen {
        return false;
    }
    (daemon_fullscreen && config.pause_on_fullscreen) || (daemon_game && config.pause_on_game)
}

/// Active presentation cap. On battery the wallpaper cap and `battery_fps` both apply.
pub(crate) fn effective_fps(config: &WallpaperConfig, on_battery: bool) -> Option<u32> {
    let battery = if on_battery { config.battery_fps } else { None };
    match (config.fps_cap, battery) {
        (Some(cap), Some(battery_cap)) => Some(cap.min(battery_cap)),
        (Some(cap), None) => Some(cap),
        (None, Some(battery_cap)) => Some(battery_cap),
        (None, None) => None,
    }
}

/// True when `last` is still inside the cap window.
pub(crate) fn under_cap(last: Option<Instant>, cap: Option<u32>) -> bool {
    let Some(cap) = cap else {
        return false;
    };
    let Some(last) = last else {
        return false;
    };
    let minimum = Duration::from_secs_f64(1.0 / f64::from(cap.max(1)));
    last.elapsed() < minimum
}

/// Shader inputs, linearly blended by time since the last poll.
pub(crate) fn telemetry_frame(hub: &Hub) -> TelemetryFrame {
    TelemetryFrame {
        previous: uniforms(&hub.previous()),
        current: uniforms(&hub.snapshot()),
        blend: hub.blend(Instant::now()),
    }
}

/// IPC body for `get_telemetry`.
pub(crate) fn report(hub: &Hub) -> TelemetryReport {
    let snap = hub.snapshot();
    TelemetryReport {
        cpu: snap.cpu_global,
        cores: snap.cpu_cores,
        ram_used: snap.ram_used,
        ram_total: snap.ram_total,
        ram_fraction: snap.ram_fraction,
        net_rx: snap.net_rx_rate,
        net_tx: snap.net_tx_rate,
        battery: snap.battery_level,
        on_battery: snap.on_battery,
        charging: snap.charging,
        fullscreen: hub.is_fullscreen_app_active(),
    }
}

fn uniforms(snap: &TelemetrySnapshot) -> TelemetryUniforms {
    TelemetryUniforms {
        cpu_global: snap.cpu_global,
        ram_fraction: snap.ram_fraction,
        net_rx_rate: snap.net_rx_rate,
        net_tx_rate: snap.net_tx_rate,
        battery_level: snap.battery_level,
        is_on_battery: u32::from(snap.on_battery),
        padding: [0.0, 0.0],
        cpu_cores: snap.cpu_cores,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iris_core::{MonitorTarget, ScaleMode, WallpaperSource};
    use std::path::PathBuf;

    fn config(fps: Option<u32>, battery: Option<u32>) -> WallpaperConfig {
        WallpaperConfig {
            id: "id".to_owned(),
            source: WallpaperSource::Image {
                path: PathBuf::from("still.png"),
            },
            target: MonitorTarget::All,
            playback: iris_core::Playback {
                rate: 1.0,
                looping: true,
                muted: true,
            },
            audio_reactive: false,
            volume: 1.0,
            fps_cap: fps,
            scale: ScaleMode::Fill,
            pause_on_fullscreen: true,
            pause_on_game: false,
            battery_fps: battery,
        }
    }

    #[test]
    fn battery_cap_is_the_lower_of_the_two_limits() {
        assert_eq!(effective_fps(&config(Some(60), Some(15)), true), Some(15));
        assert_eq!(effective_fps(&config(Some(10), Some(15)), true), Some(10));
        assert_eq!(effective_fps(&config(Some(60), Some(15)), false), Some(60));
        assert_eq!(effective_fps(&config(None, None), true), None);
    }

    #[test]
    fn a_user_pause_is_left_alone_when_fullscreen_ends() {
        assert_eq!(context_step(true, false, false, false), ContextStep::Hold);
        assert_eq!(
            context_step(false, true, false, false),
            ContextStep::AutoResume
        );
        assert_eq!(
            context_step(false, false, true, true),
            ContextStep::AutoPause
        );
    }

    #[test]
    fn fullscreen_requires_both_the_daemon_and_the_wallpaper() {
        let mut wallpaper = config(None, None);
        assert!(want_pause(true, true, &wallpaper, true));
        wallpaper.pause_on_fullscreen = false;
        assert!(!want_pause(true, true, &wallpaper, true));
        assert!(!want_pause(true, true, &wallpaper, false));
    }
}
