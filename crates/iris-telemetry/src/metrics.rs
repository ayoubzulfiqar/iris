//! Background sampler. The frame loop never calls into `sysinfo`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use arc_swap::ArcSwap;
use serde::{Deserialize, Serialize};
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, Networks, RefreshKind, System};

use crate::game_mode::probe_fullscreen;
use crate::power::{self, net_unit, PowerSource, PowerState};

/// Default poll period. Long enough that the sampler stays under a fraction of a core.
pub const DEFAULT_INTERVAL: Duration = Duration::from_millis(500);

/// One published sample. Rates and fractions are `0.0..=1.0`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    /// Average CPU, `0.0..=1.0`.
    pub cpu_global: f32,
    /// First 16 logical processors. Unused slots stay `0.0`.
    pub cpu_cores: [f32; 16],
    /// Resident bytes.
    pub ram_used: u64,
    /// Installed bytes. Zero when the probe could not read memory.
    pub ram_total: u64,
    /// `ram_used / ram_total`, or `0.0` when the total is unknown.
    pub ram_fraction: f32,
    /// Receive rate normalized to 100 MiB/s.
    pub net_rx_rate: f32,
    /// Transmit rate normalized to 100 MiB/s.
    pub net_tx_rate: f32,
    /// Battery fraction. `1.0` when no pack is present.
    pub battery_level: f32,
    /// The pack is charging.
    pub charging: bool,
    /// Discharging. Pacing may drop.
    pub on_battery: bool,
    /// AC or battery.
    pub power: PowerSource,
}

impl Default for TelemetrySnapshot {
    fn default() -> Self {
        Self {
            cpu_global: 0.0,
            cpu_cores: [0.0; 16],
            ram_used: 0,
            ram_total: 0,
            ram_fraction: 0.0,
            net_rx_rate: 0.0,
            net_tx_rate: 0.0,
            battery_level: 1.0,
            charging: false,
            on_battery: false,
            power: PowerSource::Ac,
        }
    }
}

struct Stamped {
    snap: TelemetrySnapshot,
    at: Instant,
}

/// Live sampler. Dropping it stops the thread.
pub struct Hub {
    current: Arc<ArcSwap<Stamped>>,
    previous: Arc<ArcSwap<Stamped>>,
    interval: Duration,
    fullscreen: Arc<AtomicBool>,
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Hub {
    /// Start sampling. `interval` is clamped to 200 ms..=5 s.
    pub fn start(interval: Duration) -> Self {
        let interval = clamp_interval(interval);
        let now = Instant::now();
        let initial = Arc::new(Stamped {
            snap: TelemetrySnapshot::default(),
            at: now,
        });
        let current = Arc::new(ArcSwap::from(Arc::clone(&initial)));
        let previous = Arc::new(ArcSwap::from(initial));
        let fullscreen = Arc::new(AtomicBool::new(false));
        let (stop, stop_rx) = mpsc::channel();
        let current_thread = Arc::clone(&current);
        let previous_thread = Arc::clone(&previous);
        let fullscreen_thread = Arc::clone(&fullscreen);
        let thread = thread::Builder::new()
            .name("iris-telemetry".to_owned())
            .spawn(move || {
                sample_loop(
                    interval,
                    current_thread,
                    previous_thread,
                    fullscreen_thread,
                    stop_rx,
                );
            })
            .ok();
        if thread.is_none() {
            tracing::warn!("telemetry thread did not start; metrics stay at the idle snapshot");
        }
        Self {
            current,
            previous,
            interval,
            fullscreen,
            stop: Some(stop),
            thread,
        }
    }

    /// Latest sample. This is an atomic load.
    pub fn snapshot(&self) -> TelemetrySnapshot {
        self.current.load().snap.clone()
    }

    /// Previous sample, for the shader blend.
    pub fn previous(&self) -> TelemetrySnapshot {
        self.previous.load().snap.clone()
    }

    /// `0.0` at the latest poll, `1.0` when the next poll is due.
    pub fn blend(&self, now: Instant) -> f32 {
        let at = self.current.load().at;
        let elapsed = now.saturating_duration_since(at).as_secs_f32();
        let span = self.interval.as_secs_f32();
        if span <= 0.0 {
            return 1.0;
        }
        (elapsed / span).clamp(0.0, 1.0)
    }

    /// Foreground fullscreen or borderless game, from the last probe.
    pub fn is_fullscreen_app_active(&self) -> bool {
        self.fullscreen.load(Ordering::Relaxed)
    }

    /// AC, or battery when the latest sample says the pack is discharging.
    pub fn power_source(&self) -> PowerSource {
        self.snapshot().power
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn clamp_interval(interval: Duration) -> Duration {
    interval.clamp(Duration::from_millis(200), Duration::from_secs(5))
}

fn sample_loop(
    interval: Duration,
    current: Arc<ArcSwap<Stamped>>,
    previous: Arc<ArcSwap<Stamped>>,
    fullscreen: Arc<AtomicBool>,
    stop: Receiver<()>,
) {
    let mut system = System::new_with_specifics(
        RefreshKind::new()
            .with_cpu(CpuRefreshKind::new().with_cpu_usage())
            .with_memory(MemoryRefreshKind::new().with_ram()),
    );
    system.refresh_cpu_usage();
    system.refresh_memory_specifics(MemoryRefreshKind::new().with_ram());
    let mut networks = Networks::new_with_refreshed_list();
    let mut last = Instant::now();
    loop {
        match stop.recv_timeout(interval) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let seconds = now.saturating_duration_since(last).as_secs_f64().max(0.001);
        last = now;
        system.refresh_cpu_usage();
        system.refresh_memory_specifics(MemoryRefreshKind::new().with_ram());
        networks.refresh();
        let snap = collect(&system, &networks, seconds, read_power());
        fullscreen.store(probe_fullscreen(), Ordering::Relaxed);
        let stamped = Arc::new(Stamped {
            snap,
            at: Instant::now(),
        });
        let prior = current.load_full();
        previous.store(prior);
        current.store(stamped);
    }
}

fn collect(
    system: &System,
    networks: &Networks,
    seconds: f64,
    power: PowerState,
) -> TelemetrySnapshot {
    let mut cores = [0.0; 16];
    for (index, cpu) in system.cpus().iter().take(16).enumerate() {
        cores[index] = percent_unit(cpu.cpu_usage());
    }
    let total = system.total_memory();
    let used = system.used_memory();
    let ram_fraction = if total == 0 {
        0.0
    } else {
        unit_interval(used as f32 / total as f32)
    };
    let mut rx = 0u64;
    let mut tx = 0u64;
    for (_name, data) in networks {
        rx = rx.saturating_add(data.received());
        tx = tx.saturating_add(data.transmitted());
    }
    TelemetrySnapshot {
        cpu_global: percent_unit(system.global_cpu_usage()),
        cpu_cores: cores,
        ram_used: used,
        ram_total: total,
        ram_fraction,
        net_rx_rate: net_unit(rx as f64 / seconds),
        net_tx_rate: net_unit(tx as f64 / seconds),
        battery_level: unit_interval(power.level),
        charging: power.charging,
        on_battery: power.on_battery,
        power: power.source,
    }
}

/// Map a percent in `0..=100` onto `0.0..=1.0`. Non-finite values become `0.0`.
pub fn percent_unit(percent: f32) -> f32 {
    if !percent.is_finite() {
        return 0.0;
    }
    unit_interval(percent / 100.0)
}

/// Clamp a finite value into `0.0..=1.0`.
pub fn unit_interval(value: f32) -> f32 {
    if !value.is_finite() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn read_power() -> PowerState {
    #[cfg(windows)]
    {
        return read_power_windows();
    }
    #[cfg(not(windows))]
    {
        read_power_linux()
    }
}

#[cfg(not(windows))]
fn read_power_linux() -> PowerState {
    let root = std::path::Path::new("/sys/class/power_supply");
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(_) => return PowerState::plugged_in(),
    };
    let mut capacity = None;
    let mut status = None;
    let mut ac_online = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("BAT") {
            capacity = read_small(&path.join("capacity")).or(capacity);
            status = read_small(&path.join("status")).or(status);
        } else if name.starts_with("AC") || name.starts_with("ADP") {
            if let Some(text) = read_small(&path.join("online")) {
                ac_online = match text.trim() {
                    "1" => Some(true),
                    "0" => Some(false),
                    _ => ac_online,
                };
            }
        }
    }
    power::from_linux_sysfs(capacity.as_deref(), status.as_deref(), ac_online)
}

#[cfg(not(windows))]
fn read_small(path: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    if text.len() > 64 {
        return None;
    }
    Some(text)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn read_power_windows() -> PowerState {
    use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};

    let mut status = SYSTEM_POWER_STATUS::default();
    // SAFETY: `status` is a local SYSTEM_POWER_STATUS. GetSystemPowerStatus
    // writes that struct and does not retain the pointer. A failure means the
    // platform has no power reading, which is reported as AC.
    if unsafe { GetSystemPowerStatus(&mut status) }.is_err() {
        return PowerState::plugged_in();
    }
    power::from_win32(
        status.ACLineStatus,
        status.BatteryLifePercent,
        status.BatteryFlag,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percents_and_non_finite_values_stay_in_range() {
        assert_eq!(percent_unit(0.0), 0.0);
        assert_eq!(percent_unit(50.0), 0.5);
        assert_eq!(percent_unit(250.0), 1.0);
        assert_eq!(percent_unit(f32::NAN), 0.0);
        assert_eq!(unit_interval(f32::INFINITY), 0.0);
    }

    #[test]
    fn the_blend_reaches_one_when_the_interval_elapses() {
        let hub = Hub {
            current: Arc::new(ArcSwap::from_pointee(Stamped {
                snap: TelemetrySnapshot::default(),
                at: Instant::now() - Duration::from_millis(500),
            })),
            previous: Arc::new(ArcSwap::from_pointee(Stamped {
                snap: TelemetrySnapshot::default(),
                at: Instant::now(),
            })),
            interval: Duration::from_millis(500),
            fullscreen: Arc::new(AtomicBool::new(false)),
            stop: None,
            thread: None,
        };
        assert_eq!(hub.blend(Instant::now()), 1.0);
        assert!(!hub.is_fullscreen_app_active());
        assert_eq!(hub.power_source(), PowerSource::Ac);
    }
}
