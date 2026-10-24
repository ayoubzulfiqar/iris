//! Interactive web wallpapers.
//!
//! The daemon points a wry webview at `iris://wallpaper/index.html`. The custom
//! protocol reads that tree from disk. `shim.js` installs the Wallpaper Engine
//! hooks before the document loads, and the bridge pushes audio, telemetry,
//! and pointer events with `evaluate_script`.
//!
//! Unit tests never create a webview. Pass `launch: false` to [`WebWallpaperInstance::open`].

#![deny(unsafe_code)]

mod bridge;
mod detect;
mod error;
mod host;
mod protocol;

pub use detect::{detect, WebProject};
pub use error::Error;
pub use protocol::{entry_url, AssetRoot, ENTRY_URL, SCHEME};

/// Wallpaper Engine compatibility script injected before the document loads.
pub const SHIM: &str = include_str!("shim.js");

/// Monitor rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// Left edge on the virtual screen.
    pub x: i32,
    /// Top edge on the virtual screen.
    pub y: i32,
    /// Width in physical pixels.
    pub width: u32,
    /// Height in physical pixels.
    pub height: u32,
}

/// One web wallpaper. A failed webview stays here with [`Self::launched`] false.
pub struct WebWallpaperInstance {
    host: Option<host::Host>,
    properties: serde_json::Value,
    general: serde_json::Value,
    launched: bool,
    warning: Option<String>,
    paused: bool,
    pointer: Option<[f32; 2]>,
    last_audio: Option<std::time::Instant>,
    last_telemetry: Option<std::time::Instant>,
    last_pointer: Option<std::time::Instant>,
}

const AUDIO_GAP: std::time::Duration = std::time::Duration::from_millis(16);
const TELEMETRY_GAP: std::time::Duration = std::time::Duration::from_millis(200);
const POINTER_GAP: std::time::Duration = std::time::Duration::from_millis(16);

impl WebWallpaperInstance {
    /// Open `project` on `bounds`.
    ///
    /// `launch` false skips wry entirely. A webview that fails, including the
    /// software-raster retry, is reported through [`Self::warning`] and does
    /// not panic.
    pub fn open(project: WebProject, bounds: Bounds, launch: bool) -> Self {
        let properties = project.properties.clone();
        let general = project.general.clone();
        if !launch {
            return Self::inert(properties, general, None);
        }
        match host::open(&project, bounds) {
            Ok(host) => Self {
                host: Some(host),
                properties,
                general,
                launched: true,
                warning: None,
                paused: false,
                pointer: None,
                last_audio: None,
                last_telemetry: None,
                last_pointer: None,
            },
            Err(err) => {
                tracing::warn!("web wallpaper stayed unloaded: {err}");
                Self::inert(properties, general, Some(err))
            }
        }
    }

    /// The webview process is running.
    pub fn launched(&self) -> bool {
        self.launched
    }

    /// Why the webview did not start.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }

    /// Move the desktop window. A missing webview ignores the call.
    pub fn place(&mut self, bounds: Bounds) {
        if let Some(host) = &self.host {
            host.place(bounds);
        }
    }

    /// Inject the 16- and 64-band spectra. The listener receives the 64-band array.
    ///
    /// Calls closer than 16 ms are dropped so the page stays in the 30–60 fps band.
    pub fn push_audio(&mut self, bands16: &[f32; 16], bands64: &[f32; 64]) {
        if !self.live() {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(last) = self.last_audio {
            if now.saturating_duration_since(last) < AUDIO_GAP {
                self.flush_pointer();
                return;
            }
        }
        let Some(script) = bridge::audio_script(bands16, bands64) else {
            return;
        };
        self.eval(&script);
        self.last_audio = Some(now);
        self.flush_pointer();
    }

    /// Push project properties and `{ telemetry: snapshot }`.
    pub fn push_telemetry(&mut self, snapshot: &iris_telemetry::TelemetrySnapshot) {
        if !self.live() {
            return;
        }
        let now = std::time::Instant::now();
        if let Some(last) = self.last_telemetry {
            if now.saturating_duration_since(last) < TELEMETRY_GAP {
                return;
            }
        }
        let Some(script) = bridge::telemetry_script(snapshot, &self.properties, &self.general)
        else {
            return;
        };
        self.eval(&script);
        self.last_telemetry = Some(now);
        self.flush_pointer();
    }

    /// Remember the cursor and dispatch a `pointermove` when the page is playing.
    pub fn push_pointer(&mut self, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        self.pointer = Some([x, y]);
        self.flush_pointer();
    }

    /// Set `document.hidden` and stop audio, telemetry, and pointer injection.
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
        if self.host.is_none() {
            return;
        }
        self.eval(&bridge::visibility_script(paused));
    }

    fn live(&self) -> bool {
        self.launched && !self.paused && self.host.is_some()
    }

    fn flush_pointer(&mut self) {
        if !self.live() {
            return;
        }
        let Some(point) = self.pointer else {
            return;
        };
        let now = std::time::Instant::now();
        if let Some(last) = self.last_pointer {
            if now.saturating_duration_since(last) < POINTER_GAP {
                return;
            }
        }
        let Some(script) = bridge::pointer_script(point[0], point[1]) else {
            return;
        };
        self.eval(&script);
        self.last_pointer = Some(now);
    }

    fn eval(&mut self, script: &str) {
        if let Some(host) = &self.host {
            host.eval(script);
        }
    }

    fn inert(
        properties: serde_json::Value,
        general: serde_json::Value,
        warning: Option<String>,
    ) -> Self {
        Self {
            host: None,
            properties,
            general,
            launched: false,
            warning,
            paused: false,
            pointer: None,
            last_audio: None,
            last_telemetry: None,
            last_pointer: None,
        }
    }
}

/// Drain GTK events. No effect until a Linux webview has initialized GTK.
pub fn pump() {
    host::pump();
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[test]
    fn shim_installs_the_wallpaper_engine_hooks() {
        assert!(SHIM.contains("window.wallpaperRegisterAudioListener"));
        assert!(SHIM.contains("window._irisAudioListener"));
        assert!(SHIM.contains("window.wallpaperPropertyListener"));
        assert!(SHIM.contains("applyUserProperties"));
        assert!(SHIM.contains("applyGeneralProperties"));
        assert!(SHIM.contains("window.iris"));
        assert!(SHIM.contains("window.ipc.postMessage"));
        assert!(SHIM.contains("0.1.0"));
    }

    #[test]
    fn headless_open_does_not_create_a_webview() {
        let dir = std::env::temp_dir().join(format!(
            "iris-web-headless-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("temp");
        let project = WebProject {
            entry_url: ENTRY_URL.to_owned(),
            properties: serde_json::json!({}),
            general: serde_json::Value::Null,
            root: Arc::new(AssetRoot::directory(&dir).expect("root")),
        };
        let instance = WebWallpaperInstance::open(
            project,
            Bounds {
                x: 0,
                y: 0,
                width: 64,
                height: 64,
            },
            false,
        );
        assert!(!instance.launched());
        assert!(instance.warning().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
