//! JSON-RPC 2.0 messages carried between the manager and `iris-daemon`.
//!
//! Frames are length-prefixed so a payload may contain any byte:
//!
//! ```text
//! u32 little-endian length | UTF-8 JSON object
//! ```
//!
//! The length counts the JSON bytes only. [`FrameDecoder`] reassembles a frame
//! that arrives in pieces. Messages themselves are JSON-RPC 2.0:
//!
//! ```json
//! {"jsonrpc":"2.0","id":1,"method":"apply_wallpaper","params":{"config":{}}}
//! ```

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;

/// JSON-RPC version written into every message.
pub const JSONRPC_VERSION: &str = "2.0";

/// Iris protocol generation. Returned by [`IpcCall::Ping`].
pub const PROTOCOL_VERSION: u32 = 1;

/// Largest JSON payload the frame codec will accept (16 MiB).
pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

/// Correlation id. JSON-RPC allows a string, a number, or null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    /// Integer id. The manager should prefer this.
    Number(i64),
    /// String id, for clients that already have a uuid.
    Text(String),
    /// Notification-style id. Iris requests still carry one so responses can be matched.
    Null,
}

/// Which output a wallpaper is bound to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum MonitorTarget {
    /// Every connected output.
    All,
    /// The output the compositor marks as primary.
    Primary,
    /// Zero-based index in the daemon's current output list.
    Index {
        /// Index into [`ResponseBody::Monitors`].
        index: u32,
    },
    /// Connector name reported by the platform (`eDP-1`, `DP-1`, `\\.\DISPLAY1`).
    Connector {
        /// Platform connector name. Compared case-sensitively.
        name: String,
    },
}

impl std::fmt::Display for MonitorTarget {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::All => formatter.write_str("all"),
            Self::Primary => formatter.write_str("primary"),
            Self::Index { index } => write!(formatter, "index:{index}"),
            Self::Connector { name } => write!(formatter, "connector:{name}"),
        }
    }
}

/// Where the wallpaper bytes come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum WallpaperSource {
    /// A Wallpaper Engine `.pkg` archive.
    Package {
        /// Filesystem path of the archive.
        path: PathBuf,
    },
    /// A video file decoded by the platform backend.
    Video {
        /// Filesystem path of the video.
        path: PathBuf,
    },
    /// A still image.
    Image {
        /// Filesystem path of the image.
        path: PathBuf,
    },
    /// An unpacked wallpaper directory that contains `project.json`.
    SceneDirectory {
        /// Directory path.
        path: PathBuf,
    },
}

impl WallpaperSource {
    fn path(&self) -> &PathBuf {
        match self {
            Self::Package { path }
            | Self::Video { path }
            | Self::Image { path }
            | Self::SceneDirectory { path } => path,
        }
    }
}

/// How a video or scene advances.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playback {
    /// Speed multiplier. `1.0` is realtime.
    #[serde(default = "default_rate")]
    pub rate: f32,
    /// Restart when the clip or scene ends.
    #[serde(default = "default_true")]
    pub looping: bool,
    /// Drop audio while the wallpaper is running.
    #[serde(default)]
    pub muted: bool,
}

impl Default for Playback {
    fn default() -> Self {
        Self {
            rate: default_rate(),
            looping: true,
            muted: false,
        }
    }
}

/// How the source is fitted to the output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScaleMode {
    /// Cover the output, cropping one axis.
    #[default]
    Fill,
    /// Fit inside the output, letterboxing the other axis.
    Fit,
    /// Stretch to the output size.
    Stretch,
    /// Draw at native size, centered.
    Center,
}

/// One wallpaper assignment.
///
/// The daemon treats a successful [`IpcCall::ApplyWallpaper`] as validated:
/// [`IpcRequest::decode_call`] rejects configs that fail [`WallpaperConfig::validate`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallpaperConfig {
    /// Client-chosen id. A workshop id or a local uuid both work.
    pub id: String,
    /// Asset to show.
    pub source: WallpaperSource,
    /// Output the asset is bound to.
    pub target: MonitorTarget,
    /// Playback clock.
    #[serde(default)]
    pub playback: Playback,
    /// Drive scene uniforms from the system audio meter.
    #[serde(default)]
    pub audio_reactive: bool,
    /// Linear gain in `0.0..=1.0` used when audio is not muted.
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// Optional render cap, in frames per second. `None` follows the output.
    #[serde(default)]
    pub fps_cap: Option<u32>,
    /// Fit mode.
    #[serde(default)]
    pub scale: ScaleMode,
    /// Pause this wallpaper on fullscreen, overriding the daemon default when false.
    #[serde(default = "default_true")]
    pub pause_on_fullscreen: bool,
    /// Pause this wallpaper when a game is detected.
    #[serde(default = "default_true")]
    pub pause_on_game: bool,
    /// Presentation cap while on battery. `None` keeps [`Self::fps_cap`].
    #[serde(default = "default_battery_fps")]
    pub battery_fps: Option<u32>,
}

impl WallpaperConfig {
    /// Check ranges the daemon relies on.
    pub fn validate(&self) -> Result<(), Error> {
        if self.id.trim().is_empty() {
            return Err(Error::InvalidConfig("id is empty".to_owned()));
        }
        if self.source.path().as_os_str().is_empty() {
            return Err(Error::InvalidConfig("source path is empty".to_owned()));
        }
        if let MonitorTarget::Connector { name } = &self.target {
            if name.trim().is_empty() {
                return Err(Error::InvalidConfig("connector name is empty".to_owned()));
            }
        }
        if !self.playback.rate.is_finite() || self.playback.rate <= 0.0 || self.playback.rate > 8.0
        {
            return Err(Error::InvalidConfig(
                "playback rate must be finite and in (0, 8]".to_owned(),
            ));
        }
        if !self.volume.is_finite() || !(0.0..=1.0).contains(&self.volume) {
            return Err(Error::InvalidConfig(
                "volume must be finite and in 0..=1".to_owned(),
            ));
        }
        if let Some(cap) = self.fps_cap {
            if !(1..=1000).contains(&cap) {
                return Err(Error::InvalidConfig(
                    "fps_cap must be between 1 and 1000".to_owned(),
                ));
            }
        }
        if let Some(cap) = self.battery_fps {
            if !(1..=1000).contains(&cap) {
                return Err(Error::InvalidConfig(
                    "battery_fps must be between 1 and 1000".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

fn default_battery_fps() -> Option<u32> {
    Some(15)
}

/// One output, as the daemon currently sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorInfo {
    /// Stable index for the lifetime of this daemon process.
    pub index: u32,
    /// Connector name.
    pub connector: String,
    /// Human-readable name. May match `connector`.
    pub name: String,
    /// Width in physical pixels.
    pub width_px: u32,
    /// Height in physical pixels.
    pub height_px: u32,
    /// Fractional scale factor. `1.0` is unscaled.
    pub scale: f64,
    /// Whether the compositor calls this the primary output.
    pub primary: bool,
    /// Refresh rate in millihertz (`60000` is 60 Hz), when the platform reports one.
    #[serde(default)]
    pub refresh_millihz: Option<u32>,
}

/// Playback state of one running wallpaper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunState {
    /// Frames are being presented.
    Playing,
    /// The render loop is paused. The last frame stays up.
    Paused,
    /// The daemon kept the slot but the asset failed to load.
    Failed,
}

/// Status row inside [`ResponseBody::Status`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WallpaperStatus {
    /// [`WallpaperConfig::id`].
    pub id: String,
    /// Output binding.
    pub target: MonitorTarget,
    /// Current state.
    pub state: RunState,
    /// Failure detail when [`RunState::Failed`].
    #[serde(default)]
    pub message: Option<String>,
}

/// A call the manager can make.
///
/// Unit variants take no parameters. New variants can be added without a parse
/// break on the wire: unknown method names become [`Error::UnknownMethod`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
#[non_exhaustive]
pub enum IpcCall {
    /// Liveness check. The result is [`ResponseBody::Pong`].
    Ping,
    /// Ask the daemon to flush state and exit.
    Shutdown,
    /// List outputs. The result is [`ResponseBody::Monitors`].
    ListMonitors,
    /// List running wallpapers. The result is [`ResponseBody::Status`].
    GetStatus,
    /// Replace the wallpaper on `config.target`.
    ApplyWallpaper {
        /// Validated assignment.
        config: WallpaperConfig,
    },
    /// Remove the wallpaper on one target. The desktop shows through again.
    ClearWallpaper {
        /// Output to clear.
        target: MonitorTarget,
    },
    /// Pause rendering. `None` pauses every output.
    Pause {
        /// Output to pause, or every output when omitted.
        target: Option<MonitorTarget>,
    },
    /// Resume rendering. `None` resumes every output.
    Resume {
        /// Output to resume, or every output when omitted.
        target: Option<MonitorTarget>,
    },
    /// Move the parallax pointer, in output pixels.
    SetPointer {
        /// Horizontal position in pixels.
        x: f32,
        /// Vertical position in pixels.
        y: f32,
    },
    /// Latest CPU, memory, network, and battery sample.
    GetTelemetry,
}

/// Successful `result` bodies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[non_exhaustive]
pub enum ResponseBody {
    /// Answer to [`IpcCall::Ping`].
    Pong {
        /// [`PROTOCOL_VERSION`].
        protocol: u32,
    },
    /// Answer to commands that have no payload.
    Ack,
    /// Answer to [`IpcCall::ListMonitors`].
    Monitors {
        /// Outputs in daemon order.
        monitors: Vec<MonitorInfo>,
    },
    /// Answer to [`IpcCall::GetStatus`].
    Status {
        /// False while the daemon is shutting down.
        running: bool,
        /// One row per assigned wallpaper.
        wallpapers: Vec<WallpaperStatus>,
    },
    /// Answer to [`IpcCall::GetTelemetry`].
    Telemetry(TelemetryReport),
}

/// One telemetry sample for the manager.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TelemetryReport {
    /// Average CPU, `0.0..=1.0`.
    pub cpu: f32,
    /// Up to 16 logical processors.
    pub cores: [f32; 16],
    /// Resident bytes.
    pub ram_used: u64,
    /// Installed bytes.
    pub ram_total: u64,
    /// Used fraction.
    pub ram_fraction: f32,
    /// Normalized receive rate.
    pub net_rx: f32,
    /// Normalized transmit rate.
    pub net_tx: f32,
    /// Battery fraction. `1.0` when no pack is present.
    pub battery: f32,
    /// The pack is discharging.
    pub on_battery: bool,
    /// The pack is charging.
    pub charging: bool,
    /// A fullscreen or borderless game is in front.
    pub fullscreen: bool,
}

/// JSON-RPC error object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IpcErrorBody {
    /// JSON-RPC code. Application codes stay in the reserved implementation range.
    pub code: i32,
    /// Single-line explanation.
    pub message: String,
    /// Optional structured detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// JSON-RPC parse error.
pub const CODE_PARSE_ERROR: i32 = -32_700;
/// JSON-RPC invalid request.
pub const CODE_INVALID_REQUEST: i32 = -32_600;
/// JSON-RPC method not found.
pub const CODE_METHOD_NOT_FOUND: i32 = -32_601;
/// JSON-RPC invalid params.
pub const CODE_INVALID_PARAMS: i32 = -32_602;
/// JSON-RPC internal error.
pub const CODE_INTERNAL: i32 = -32_603;

impl IpcErrorBody {
    /// Build an error object with no extra data.
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }

    /// Map a core error onto a JSON-RPC code.
    pub fn from_core(err: &Error) -> Self {
        match err {
            Error::Json(inner) => Self::new(CODE_PARSE_ERROR, inner.to_string()),
            Error::BadVersion(version) => Self::new(
                CODE_INVALID_REQUEST,
                format!("unsupported jsonrpc version {version}"),
            ),
            Error::UnknownMethod(method) => {
                Self::new(CODE_METHOD_NOT_FOUND, format!("method not found: {method}"))
            }
            Error::InvalidParams { detail, .. } | Error::InvalidConfig(detail) => {
                Self::new(CODE_INVALID_PARAMS, detail.clone())
            }
            Error::EmptyFrame | Error::FrameTooLarge { .. } | Error::MalformedResponse => {
                Self::new(CODE_INVALID_REQUEST, err.to_string())
            }
            Error::LogFilter { .. } | Error::LogInit(_) | Error::Protocol(_) | Error::Io(_) => {
                Self::new(CODE_INTERNAL, err.to_string())
            }
        }
    }
}

/// One request on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcRequest {
    /// Always `"2.0"` for messages this crate writes.
    #[serde(default = "default_jsonrpc")]
    pub jsonrpc: String,
    /// Correlation id copied into the response.
    pub id: RequestId,
    /// Method name, snake_case.
    pub method: String,
    /// Parameters object. Omitted params deserialize as JSON null.
    #[serde(default)]
    pub params: Value,
}

impl IpcRequest {
    /// Encode `call` as a JSON-RPC request.
    pub fn from_call(id: RequestId, call: &IpcCall) -> Result<Self, Error> {
        let value = serde_json::to_value(call)?;
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| Error::Protocol("ipc call is missing a method name".to_owned()))?;
        let params = match value.get("params") {
            Some(params) => params.clone(),
            None => Value::Null,
        };
        Ok(Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id,
            method: method.to_owned(),
            params,
        })
    }

    /// Interpret this request as a typed call.
    ///
    /// [`IpcCall::ApplyWallpaper`] is rejected unless [`WallpaperConfig::validate`] succeeds.
    pub fn decode_call(&self) -> Result<IpcCall, Error> {
        if self.jsonrpc != JSONRPC_VERSION {
            return Err(Error::BadVersion(self.jsonrpc.clone()));
        }
        let params = params_object(&self.method, &self.params)?;
        match self.method.as_str() {
            "ping" => Ok(IpcCall::Ping),
            "shutdown" => Ok(IpcCall::Shutdown),
            "list_monitors" => Ok(IpcCall::ListMonitors),
            "get_status" => Ok(IpcCall::GetStatus),
            "apply_wallpaper" => {
                let parsed: ApplyParams = parse_params(&self.method, params)?;
                parsed.config.validate()?;
                Ok(IpcCall::ApplyWallpaper {
                    config: parsed.config,
                })
            }
            "clear_wallpaper" => {
                let parsed: ClearParams = parse_params(&self.method, params)?;
                validate_target(&parsed.target)?;
                Ok(IpcCall::ClearWallpaper {
                    target: parsed.target,
                })
            }
            "pause" => {
                let parsed: TargetParams = parse_params(&self.method, params)?;
                if let Some(target) = &parsed.target {
                    validate_target(target)?;
                }
                Ok(IpcCall::Pause {
                    target: parsed.target,
                })
            }
            "resume" => {
                let parsed: TargetParams = parse_params(&self.method, params)?;
                if let Some(target) = &parsed.target {
                    validate_target(target)?;
                }
                Ok(IpcCall::Resume {
                    target: parsed.target,
                })
            }
            "set_pointer" => {
                let parsed: PointerParams = parse_params(&self.method, params)?;
                if !parsed.x.is_finite() || !parsed.y.is_finite() {
                    return Err(Error::InvalidParams {
                        method: "set_pointer".to_owned(),
                        detail: "pointer components must be finite".to_owned(),
                    });
                }
                Ok(IpcCall::SetPointer {
                    x: parsed.x,
                    y: parsed.y,
                })
            }
            "get_telemetry" => Ok(IpcCall::GetTelemetry),
            other => Err(Error::UnknownMethod(other.to_owned())),
        }
    }

    /// JSON bytes of this request, without the length prefix.
    pub fn to_payload(&self) -> Result<Vec<u8>, Error> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Length-prefixed frame.
    pub fn to_frame(&self) -> Result<Vec<u8>, Error> {
        encode_frame(&self.to_payload()?)
    }

    /// Decode a JSON payload that has already been split out of its frame.
    pub fn from_payload(payload: &[u8]) -> Result<Self, Error> {
        Ok(serde_json::from_slice(payload)?)
    }
}

/// One response on the wire.
///
/// Exactly one of `result` and `error` must be set. [`IpcResponse::outcome`] checks that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IpcResponse {
    /// Always `"2.0"` for messages this crate writes.
    pub jsonrpc: String,
    /// Id copied from the request.
    pub id: RequestId,
    /// Success body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<ResponseBody>,
    /// Failure body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<IpcErrorBody>,
}

impl IpcResponse {
    /// Success response.
    pub fn success(id: RequestId, result: ResponseBody) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id,
            result: Some(result),
            error: None,
        }
    }

    /// Failure response.
    pub fn failure(id: RequestId, error: IpcErrorBody) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id,
            result: None,
            error: Some(error),
        }
    }

    /// Borrow whichever body is present.
    pub fn outcome(&self) -> Result<Outcome<'_>, Error> {
        match (&self.result, &self.error) {
            (Some(result), None) => Ok(Outcome::Success(result)),
            (None, Some(error)) => Ok(Outcome::Failure(error)),
            _ => Err(Error::MalformedResponse),
        }
    }

    /// JSON bytes of this response, without the length prefix.
    pub fn to_payload(&self) -> Result<Vec<u8>, Error> {
        Ok(serde_json::to_vec(self)?)
    }

    /// Length-prefixed frame.
    pub fn to_frame(&self) -> Result<Vec<u8>, Error> {
        encode_frame(&self.to_payload()?)
    }

    /// Decode a JSON payload that has already been split out of its frame.
    pub fn from_payload(payload: &[u8]) -> Result<Self, Error> {
        Ok(serde_json::from_slice(payload)?)
    }
}

/// The one body inside an [`IpcResponse`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome<'a> {
    /// `result` was set.
    Success(&'a ResponseBody),
    /// `error` was set.
    Failure(&'a IpcErrorBody),
}

/// Reassembles length-prefixed frames from a byte stream.
#[derive(Debug, Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    /// Empty decoder.
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// Append bytes read from the socket.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Take the next complete payload.
    ///
    /// `Ok(None)` means the header, or the declared body, has not arrived yet.
    /// A declared length above [`MAX_FRAME_BYTES`] is an error immediately, before
    /// the body is buffered. The caller should drop the connection after that.
    pub fn next_payload(&mut self) -> Result<Option<Vec<u8>>, Error> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let header = self
            .buf
            .get(..4)
            .ok_or_else(|| Error::Protocol("frame header vanished".to_owned()))?;
        let header: [u8; 4] = header
            .try_into()
            .map_err(|_| Error::Protocol("frame header is not 4 bytes".to_owned()))?;
        let len = u32::from_le_bytes(header);
        if len == 0 {
            return Err(Error::EmptyFrame);
        }
        if len > MAX_FRAME_BYTES {
            return Err(Error::FrameTooLarge {
                len,
                max: MAX_FRAME_BYTES,
            });
        }
        let total = 4usize.saturating_add(usize::try_from(len).unwrap_or(usize::MAX));
        if self.buf.len() < total {
            return Ok(None);
        }
        let rest = self.buf.split_off(total);
        let frame = std::mem::replace(&mut self.buf, rest);
        let payload = frame.get(4..).unwrap_or_default().to_vec();
        if payload.len() != usize::try_from(len).unwrap_or(usize::MAX) {
            return Err(Error::Protocol(
                "frame length changed while splitting".to_owned(),
            ));
        }
        Ok(Some(payload))
    }
}

/// Prefix `payload` with its little-endian length.
pub fn encode_frame(payload: &[u8]) -> Result<Vec<u8>, Error> {
    let len = u32::try_from(payload.len()).map_err(|_| Error::FrameTooLarge {
        len: u32::MAX,
        max: MAX_FRAME_BYTES,
    })?;
    if len == 0 {
        return Err(Error::EmptyFrame);
    }
    if len > MAX_FRAME_BYTES {
        return Err(Error::FrameTooLarge {
            len,
            max: MAX_FRAME_BYTES,
        });
    }
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(payload);
    Ok(out)
}

pub(crate) fn default_true() -> bool {
    true
}

fn default_rate() -> f32 {
    1.0
}

fn default_volume() -> f32 {
    1.0
}

fn default_jsonrpc() -> String {
    JSONRPC_VERSION.to_owned()
}

fn params_object(method: &str, params: &Value) -> Result<Value, Error> {
    match params {
        Value::Null => Ok(Value::Object(serde_json::Map::new())),
        Value::Object(_) => Ok(params.clone()),
        _ => Err(Error::InvalidParams {
            method: method.to_owned(),
            detail: "params must be a JSON object".to_owned(),
        }),
    }
}

fn parse_params<T: for<'de> Deserialize<'de>>(method: &str, params: Value) -> Result<T, Error> {
    serde_json::from_value(params).map_err(|err| Error::InvalidParams {
        method: method.to_owned(),
        detail: err.to_string(),
    })
}

fn validate_target(target: &MonitorTarget) -> Result<(), Error> {
    if let MonitorTarget::Connector { name } = target {
        if name.trim().is_empty() {
            return Err(Error::InvalidConfig("connector name is empty".to_owned()));
        }
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct ApplyParams {
    config: WallpaperConfig,
}

#[derive(Debug, Deserialize)]
struct ClearParams {
    target: MonitorTarget,
}

#[derive(Debug, Deserialize)]
struct TargetParams {
    #[serde(default)]
    target: Option<MonitorTarget>,
}

#[derive(Debug, Deserialize)]
struct PointerParams {
    x: f32,
    y: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must<T, E: std::fmt::Debug>(result: Result<T, E>) -> T {
        match result {
            Ok(value) => value,
            Err(err) => panic!("expected Ok, got {err:?}"),
        }
    }

    fn sample_config() -> WallpaperConfig {
        WallpaperConfig {
            id: "workshop-1".to_owned(),
            source: WallpaperSource::Package {
                path: PathBuf::from("/wallpapers/scene.pkg"),
            },
            target: MonitorTarget::Connector {
                name: "DP-1".to_owned(),
            },
            playback: Playback {
                rate: 1.0,
                looping: true,
                muted: false,
            },
            audio_reactive: true,
            volume: 0.4,
            fps_cap: Some(60),
            scale: ScaleMode::Fit,
            pause_on_fullscreen: true,
            pause_on_game: false,
            battery_fps: Some(15),
        }
    }

    #[test]
    fn wallpaper_config_defaults_fill_omitted_fields() {
        let raw = r#"{"id":"a","source":{"kind":"image","path":"still.png"},"target":{"kind":"primary"}}"#;
        let config: WallpaperConfig = must(serde_json::from_str(raw));
        assert_eq!(config.scale, ScaleMode::Fill);
        assert!(config.playback.looping);
        assert!(!config.playback.muted);
        assert_eq!(config.volume, 1.0);
        assert!(config.pause_on_fullscreen);
        assert_eq!(config.battery_fps, Some(15));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn invalid_volume_fails_validation() {
        let mut config = sample_config();
        config.volume = 1.5;
        assert!(matches!(config.validate(), Err(Error::InvalidConfig(_))));
    }

    #[test]
    fn apply_wallpaper_roundtrips_through_a_split_frame() {
        let request = must(IpcRequest::from_call(
            RequestId::Number(7),
            &IpcCall::ApplyWallpaper {
                config: sample_config(),
            },
        ));
        assert_eq!(request.method, "apply_wallpaper");
        let frame = must(request.to_frame());
        let mut decoder = FrameDecoder::new();
        let split = frame.len() / 2;
        decoder.push(&frame[..split]);
        assert!(must(decoder.next_payload()).is_none());
        decoder.push(&frame[split..]);
        let payload = must(decoder.next_payload()).expect("complete frame");
        assert!(must(decoder.next_payload()).is_none());
        let parsed = must(IpcRequest::from_payload(&payload));
        let call = must(parsed.decode_call());
        match call {
            IpcCall::ApplyWallpaper { config } => {
                assert_eq!(config.id, "workshop-1");
                assert!(config.audio_reactive);
                assert_eq!(config.fps_cap, Some(60));
            }
            other => panic!("unexpected call {other:?}"),
        }
    }

    #[test]
    fn get_telemetry_roundtrips() {
        let request = must(IpcRequest::from_call(
            RequestId::Number(4),
            &IpcCall::GetTelemetry,
        ));
        assert_eq!(request.method, "get_telemetry");
        let call = must(request.decode_call());
        assert!(matches!(call, IpcCall::GetTelemetry));
        let report = TelemetryReport {
            cpu: 0.25,
            cores: [0.0; 16],
            ram_used: 10,
            ram_total: 40,
            ram_fraction: 0.25,
            net_rx: 0.1,
            net_tx: 0.0,
            battery: 1.0,
            on_battery: false,
            charging: false,
            fullscreen: false,
        };
        let response = IpcResponse::success(
            RequestId::Number(4),
            ResponseBody::Telemetry(report.clone()),
        );
        let parsed = must(IpcResponse::from_payload(&must(response.to_payload())));
        match parsed.outcome() {
            Ok(Outcome::Success(ResponseBody::Telemetry(body))) => assert_eq!(body, &report),
            other => panic!("unexpected outcome {other:?}"),
        }
    }

    #[test]
    fn pause_without_a_target_means_every_output() {
        let request = must(IpcRequest::from_call(
            RequestId::Text("pause".to_owned()),
            &IpcCall::Pause { target: None },
        ));
        let call = must(request.decode_call());
        assert!(matches!(call, IpcCall::Pause { target: None }));
    }

    #[test]
    fn set_pointer_roundtrips_and_rejects_nan() {
        let request = must(IpcRequest::from_call(
            RequestId::Number(4),
            &IpcCall::SetPointer { x: 12.0, y: -3.5 },
        ));
        let call = must(request.decode_call());
        assert!(matches!(call, IpcCall::SetPointer { x, y } if x == 12.0 && y == -3.5));
        let bad = IpcRequest {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id: RequestId::Null,
            method: "set_pointer".to_owned(),
            params: serde_json::json!({ "x": f32::NAN, "y": 0.0 }),
        };
        assert!(bad.decode_call().is_err());
    }

    #[test]
    fn unknown_method_and_bad_version_are_distinct() {
        let unknown = IpcRequest {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id: RequestId::Null,
            method: "explode".to_owned(),
            params: Value::Null,
        };
        let err = unknown.decode_call().expect_err("unknown method");
        assert!(matches!(err, Error::UnknownMethod(_)));
        assert_eq!(IpcErrorBody::from_core(&err).code, CODE_METHOD_NOT_FOUND);

        let old = IpcRequest {
            jsonrpc: "1.0".to_owned(),
            id: RequestId::Number(1),
            method: "ping".to_owned(),
            params: Value::Null,
        };
        assert!(matches!(old.decode_call(), Err(Error::BadVersion(_))));
    }

    #[test]
    fn null_request_id_roundtrips() {
        let raw = r#"{"jsonrpc":"2.0","id":null,"method":"ping"}"#;
        let request: IpcRequest = must(serde_json::from_str(raw));
        assert_eq!(request.id, RequestId::Null);
        assert!(matches!(must(request.decode_call()), IpcCall::Ping));
    }

    #[test]
    fn oversized_frame_header_fails_before_the_body_arrives() {
        let mut decoder = FrameDecoder::new();
        decoder.push(&u32::MAX.to_le_bytes());
        assert!(matches!(
            decoder.next_payload(),
            Err(Error::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn response_outcome_rejects_both_bodies() {
        let response = IpcResponse {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id: RequestId::Number(1),
            result: Some(ResponseBody::Ack),
            error: Some(IpcErrorBody::new(CODE_INTERNAL, "nope")),
        };
        assert!(matches!(response.outcome(), Err(Error::MalformedResponse)));

        let ok = IpcResponse::success(
            RequestId::Number(1),
            ResponseBody::Pong {
                protocol: PROTOCOL_VERSION,
            },
        );
        let payload = must(ok.to_payload());
        let parsed = must(IpcResponse::from_payload(&payload));
        match parsed.outcome() {
            Ok(Outcome::Success(ResponseBody::Pong { protocol })) => {
                assert_eq!(*protocol, PROTOCOL_VERSION);
            }
            other => panic!("unexpected outcome {other:?}"),
        }
    }

    #[test]
    fn positional_params_are_rejected() {
        let request = IpcRequest {
            jsonrpc: JSONRPC_VERSION.to_owned(),
            id: RequestId::Number(1),
            method: "ping".to_owned(),
            params: Value::Array(vec![Value::from(1)]),
        };
        assert!(matches!(
            request.decode_call(),
            Err(Error::InvalidParams { .. })
        ));
    }
}
