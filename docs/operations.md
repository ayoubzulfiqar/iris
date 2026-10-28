# Operations

This page is the runbook for a machine that runs Iris. Version 0.1.0 has no
installer and no daemon config file. The process uses built-in defaults.

## Processes

| Binary | How it starts | What "healthy" looks like |
| --- | --- | --- |
| `iris-daemon` | `cargo run -p iris-daemon --bin iris-daemon`, or the manager supervisor | The IPC endpoint accepts `ping` and `list_monitors` |
| `iris` | `bun run tauri dev`, or the bundled app | The tray shows a screen count. The window lists monitors |

The manager starts the daemon when the endpoint is down. It retries for about
3 seconds (`BUDGET` in `src-tauri/src/supervisor.rs`) and then surfaces the
error in the tray. The daemon is spawned in its own process group with
`kill_on_drop(false)`, so hiding or closing the manager does not signal it.

Quit Manager exits the tray process and leaves the daemon running. Stop
Wallpaper sends `clear_wallpaper` for every output. The `shutdown` IPC method
still clears wallpaper slots and, on Linux, removes the socket file. The tray
does not send it.

The `iris-daemon` binary sets `IRIS_RESTORE_WALLPAPER` and, about 200 ms after
the socket opens, reapplies `$XDG_STATE_HOME/iris/active-wallpaper.json` when
that file is present. Library tests call `run_with` and do not set the
variable, so they do not restore a wallpaper. Clear removes the file when no
assignment remains.

`packaging/iris-daemon.service` is an optional user unit for
`graphical-session.target`. The build does not install it.

## Endpoints

The daemon listens on `DaemonConfig::default()`.

| Platform | Endpoint |
| --- | --- |
| Linux, `$XDG_RUNTIME_DIR` set | `$XDG_RUNTIME_DIR/iris/iris.sock` |
| Linux, otherwise | `/tmp/iris-$UID.sock` |
| Windows | `\\.\pipe\iris-ipc-{user}` |

`{user}` keeps ASCII letters, digits, `_`, and `-`, at most 64 characters.
Anything else is stripped. An empty result becomes `user`.

Linux rejects a named-pipe config before it listens. Windows rejects a Unix
socket config. A second daemon on the same path fails at bind time. Stop the
existing process or remove a stale socket only after you have confirmed the
daemon is gone:

```bash
ss -lx | grep iris.sock
```

## Logs

The daemon writes tracing to stderr. The filter is the config default, `info`.
`init_logging` does not read `RUST_LOG`. A second subscriber in the same
process returns `Error::LogInit`.

Library tests call `run_with` and do not install the subscriber, so a test
binary can own logging itself.

Useful events:

| Event | Meaning |
| --- | --- |
| `listening on …` | Windows pipe is bound |
| `apply {output}` warning | That output failed. Other outputs may still be up |
| `webview hardware acceleration failed` | Software-raster retry is next |
| `wayland unavailable for web wallpaper` | Web placement uses a virtual 1920×1080 monitor |
| `workerw injection` warning | The shell has no wallpaper WorkerW. The window stays borderless |

## Apply a wallpaper

The manager is the supported control surface. IPC clients speak the same
calls. See [ipc.md](ipc.md).

An apply that fails validation never reaches the daemon: the client rejects
an empty id, a non-finite playback rate outside `(0, 8]`, a volume outside
`0..=1`, and a frame cap outside `1..=1000`.

Per-output results:

| Outcome | Daemon behavior |
| --- | --- |
| Image, scene, or Linux video decodes | GPU slot, state `playing` |
| Web document opens | Web slot, state `playing` |
| Webview fails after the software retry | Slot state `failed`, IPC error, daemon stays up |
| Video on Windows | IPC unsupported error, no slot |
| No decodable image in a non-web directory | IPC load error |

Pause and resume accept a target or every output. A user pause is remembered.
If fullscreen policy still matches, the next policy tick may pause the slot
again after a user resume. Failed slots are not auto-paused.

## Power and fullscreen

Both the daemon and the wallpaper default to pausing on fullscreen and on a
detected game. Pause happens only when the probe says a fullscreen app is
active and both flags allow it. The probe ignores the Win32 class
`IrisWallpaper`. On Sway, a node counts when it is focused and its fullscreen
mode is set.

On battery, the effective frame cap is the minimum of `fps_cap` and
`battery_fps` when both are set. `battery_fps` defaults to 15. A missing
battery sensor is treated as AC at charge `1.0`, so a desktop does not throttle.

The telemetry sampler runs every 500 ms on a background thread. The first CPU
reading is `0` until a second sample exists. The frame loop only loads the
published snapshot.

## Shutdown and recovery

1. Quit from the tray, or send IPC `shutdown`.
2. Confirm the socket or pipe is gone.
3. If the daemon was killed with a signal, delete a leftover
   `$XDG_RUNTIME_DIR/iris/iris.sock` or `/tmp/iris-$UID.sock` before starting
   again. A live process still holding the socket must be stopped first.
4. Start the manager or `iris-daemon` again. Wallpapers are not restored.
   Apply them again. Version 0.1.0 does not persist assignments.

## Capacity

| Limit | Value |
| --- | --- |
| IPC frame | 16 MiB, rejected from the length prefix before the body is read |
| Protocol | Version `1`, JSON-RPC `2.0` |
| Image file inside a wallpaper load | 256 MiB |
| Image edge | 16 384 px |
| Web asset served by `iris://` | 256 MiB |
| `project.json` read while classifying a web wallpaper | 8 MiB |
| Scene images compiled into one package | 64 |
| Scene particle defs | 32 |
| Directory entries walked while searching for a still | 4 096 |

A wallpaper inside these caps can still be slow to decode. That stalls the
apply call, not the process watchdog, because version 0.1.0 has no watchdog.
