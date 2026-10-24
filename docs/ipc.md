# IPC

The manager and the daemon share `iris-core`. The wire format is JSON-RPC 2.0
inside a frame. `PROTOCOL_VERSION` is `1`.

## Frame

Each message is a little-endian `u32` length, then that many JSON bytes. A
declared length above `MAX_FRAME_BYTES` (16 MiB) is an error before the body
is read. The daemon accepts one request, writes one response, and keeps the
connection open.

Linux transport is a Unix socket. Windows transport is a named pipe. Paths are
in [operations](operations.md).

## Request

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "ping",
  "params": null
}
```

`id` is a number or `null`. Method names are `snake_case`. Parameters are an
object for calls that have fields. A positional parameter array is rejected.
Unknown methods become `Error::UnknownMethod` and do not crash the daemon.
`IpcCall` is `#[non_exhaustive]`, and the daemon match ends in a wildcard.

The client decodes by method name. It does not rely on serde of the whole enum
alone.

## Methods

| Method | Params | Success body |
| --- | --- | --- |
| `ping` | none | `pong` with `protocol` |
| `shutdown` | none | `ack`, then the daemon exits |
| `list_monitors` | none | `monitors` |
| `get_status` | none | `status` |
| `apply_wallpaper` | `config` | `ack` |
| `clear_wallpaper` | `target` | `ack` |
| `pause` | `target` optional | `ack` |
| `resume` | `target` optional | `ack` |
| `set_pointer` | `x`, `y` | `ack` |
| `get_telemetry` | none | `telemetry` |

`pause` and `resume` with no target affect every slot, including web slots.

### Monitor target

```json
{ "kind": "all" }
{ "kind": "primary" }
{ "kind": "index", "index": 0 }
{ "kind": "connector", "name": "HDMI-A-1" }
```

On Windows the connector string is the device name, such as `\\.\DISPLAY1`.

### Wallpaper config

```json
{
  "id": "local-1",
  "source": { "kind": "image", "path": "/path/to/wallpaper.png" },
  "target": { "kind": "primary" },
  "playback": { "rate": 1.0, "looping": true, "muted": false },
  "audio_reactive": false,
  "volume": 1.0,
  "fps_cap": null,
  "scale": "fill",
  "pause_on_fullscreen": true,
  "pause_on_game": true,
  "battery_fps": 15
}
```

Omitted fields use serde defaults: playback rate `1.0`, looping on, muted off,
volume `1.0`, scale `fill`, both pause flags on, `battery_fps` 15. The manager
TypeScript types omit `battery_fps` on purpose so the daemon default applies.

`source.kind` is `package`, `video`, `image`, or `scene_directory`.

`scale` is `fill`, `fit`, `stretch`, or `center`.

Validation rejects an empty id, an empty path, an empty connector name, a
playback rate that is non-finite or outside `(0, 8]`, a volume outside
`0..=1`, and a frame cap outside `1..=1000`. `set_pointer` rejects non-finite
coordinates.

### Status and telemetry

`get_status` returns `running` and one row per GPU or web slot: id, target,
`playing` | `paused` | `failed`, and an optional message.

`get_telemetry` returns fractions in `0.0..=1.0` for CPU, memory, network, and
battery, plus `on_battery`, `charging`, and `fullscreen`. Network rates are
normalized to 100 MiB/s. Core array length is 16. Unused cores stay `0`.

## Errors

Failures are JSON-RPC error objects.

| Condition | Code class |
| --- | --- |
| Unknown method, bad params, failed wallpaper validation | Invalid params |
| I/O, decode, GPU, Wayland, Win32, web classify | Internal |
| Unknown monitor | Invalid params (`InvalidTarget`) |

`Error::Io` is an internal error with a protocol message. The client maps an
unexpected success body to `ClientError::Unexpected`. Tauri commands return
`Result<_, String>` and the UI shows that string. The connect prefix is
`socket connect: {0}`.

## Compatibility

Add a method by extending `IpcCall`, the manual decoder, and the daemon
wildcard match. Old peers keep working because unknown methods are errors, not
disconnects. A breaking change to an existing params object or to
`PROTOCOL_VERSION` needs a version bump and a manager change in the same
release.

Pointer events from the manager are throttled to 50 ms in the UI. The daemon
stores the latest point and, for web slots, injects a `pointermove`.
