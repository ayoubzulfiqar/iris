//! AC versus battery, with a missing sensor reported as plugged in.

use serde::{Deserialize, Serialize};

/// Where the machine is drawing power.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PowerSource {
    /// Mains power, or no battery sensor.
    Ac,
    /// The battery is discharging.
    Battery,
}

/// Battery reading after a probe. Unknown sensors use AC and a full level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PowerState {
    /// AC or battery.
    pub source: PowerSource,
    /// `0.0..=1.0`. Unknown batteries stay at `1.0`.
    pub level: f32,
    /// The pack is charging.
    pub charging: bool,
    /// Wallpaper pacing should throttle.
    pub on_battery: bool,
}

impl PowerState {
    /// Desktop or VM with no battery.
    pub fn plugged_in() -> Self {
        Self {
            source: PowerSource::Ac,
            level: 1.0,
            charging: false,
            on_battery: false,
        }
    }
}

/// Bytes per second that maps to a normalized rate of `1.0` (100 MiB/s).
pub const NET_FULL_SCALE_BPS: f64 = 100.0 * 1024.0 * 1024.0;

/// Map a throughput onto `0.0..=1.0`.
pub fn net_unit(bytes_per_sec: f64) -> f32 {
    if !bytes_per_sec.is_finite() || bytes_per_sec <= 0.0 {
        return 0.0;
    }
    (bytes_per_sec / NET_FULL_SCALE_BPS).clamp(0.0, 1.0) as f32
}

/// Build a reading from the fields a platform probe managed to collect.
///
/// `has_battery` false, an unknown percent (`None` or above 100), and an
/// unknown AC line all become [`PowerState::plugged_in`].
pub fn from_probe(
    has_battery: bool,
    percent: Option<u8>,
    discharging: bool,
    ac_online: Option<bool>,
    charging: bool,
) -> PowerState {
    if !has_battery {
        return PowerState::plugged_in();
    }
    let level = match percent {
        Some(value) if value <= 100 => f32::from(value) / 100.0,
        _ => 1.0,
    };
    let on_battery = match ac_online {
        Some(true) => false,
        Some(false) => true,
        None => discharging,
    };
    let source = if on_battery {
        PowerSource::Battery
    } else {
        PowerSource::Ac
    };
    PowerState {
        source,
        level,
        charging: charging && !on_battery,
        on_battery,
    }
}

/// Linux `capacity` and `status` files, plus an optional AC `online` value.
#[cfg(not(windows))]
pub fn from_linux_sysfs(
    capacity: Option<&str>,
    status: Option<&str>,
    ac_online: Option<bool>,
) -> PowerState {
    let percent = capacity.and_then(|text| text.trim().parse::<u8>().ok());
    let status = status.unwrap_or("").trim();
    let discharging = status.eq_ignore_ascii_case("Discharging");
    let charging = status.eq_ignore_ascii_case("Charging");
    from_probe(
        percent.is_some() || status_is_known(status),
        percent,
        discharging,
        ac_online,
        charging,
    )
}

#[cfg(not(windows))]
fn status_is_known(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "charging" | "discharging" | "full" | "not charging"
    )
}

/// Windows `SYSTEM_POWER_STATUS` fields.
///
/// `ac_line` is 0 offline, 1 online, 255 unknown. `percent` 255 means no
/// reading. `flags` bit 7 (`128`) means no battery. Bit 3 (`8`) means charging.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn from_win32(ac_line: u8, percent: u8, flags: u8) -> PowerState {
    if flags & 128 != 0 {
        return PowerState::plugged_in();
    }
    let ac_online = match ac_line {
        0 => Some(false),
        1 => Some(true),
        _ => None,
    };
    let percent = if percent <= 100 { Some(percent) } else { None };
    let has_battery =
        percent.is_some() || ac_online == Some(false) || (flags & 128 == 0 && ac_line != 255);
    from_probe(
        has_battery,
        percent,
        ac_online == Some(false),
        ac_online,
        flags & 8 != 0,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn a_missing_battery_stays_on_ac() {
        let state = from_linux_sysfs(None, None, None);
        assert_eq!(state.source, PowerSource::Ac);
        assert_eq!(state.level, 1.0);
        assert!(!state.on_battery);
    }

    #[cfg(not(windows))]
    #[test]
    fn discharging_is_battery_power() {
        let state = from_linux_sysfs(Some("40\n"), Some("Discharging\n"), Some(false));
        assert_eq!(state.source, PowerSource::Battery);
        assert!((state.level - 0.4).abs() < 1.0e-6, "{}", state.level);
        assert!(state.on_battery);
        assert!(!state.charging);
    }

    #[test]
    fn ac_online_overrides_a_stale_discharge_flag() {
        let state = from_win32(1, 80, 8);
        assert_eq!(state.source, PowerSource::Ac);
        assert!((state.level - 0.8).abs() < 1.0e-6);
        assert!(state.charging);
        assert!(!state.on_battery);
    }

    #[test]
    fn an_unknown_windows_percent_without_a_battery_is_ac() {
        let state = from_win32(255, 255, 128);
        assert_eq!(state, PowerState::plugged_in());
    }

    #[test]
    fn network_rate_saturates_at_the_reference() {
        assert_eq!(net_unit(0.0), 0.0);
        assert_eq!(net_unit(-5.0), 0.0);
        assert!((net_unit(NET_FULL_SCALE_BPS / 2.0) - 0.5).abs() < 1.0e-6);
        assert_eq!(net_unit(NET_FULL_SCALE_BPS * 4.0), 1.0);
    }
}
