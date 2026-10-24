# Security model

Iris runs as the desktop user. It is not a sandbox for untrusted people. It is
a boundary around wallpaper files and around the IPC socket.

Report holes through [SECURITY.md](../SECURITY.md). This page describes the
design the code implements today.

## Trust

| Actor | Trusted to |
| --- | --- |
| The desktop user | Choose wallpaper files, start the daemon, and connect to their own endpoint |
| Wallpaper bytes | Be decoded or, for web wallpapers, executed inside the webview |
| Other local users | Nothing. They must not be able to speak IPC or read another user's wallpaper root through the protocol |
| The compositor and GPU driver | Place windows and execute shaders. A hostile driver is outside this model |

A web wallpaper is allowed to run its own JavaScript. That is the feature.
It is not allowed to make the asset protocol read arbitrary files.

## Files

`iris-pkg` rejects archive paths that are absolute, contain a drive prefix, or
contain `.` or `..` segments.

`iris-web` resolves `iris://wallpaper/...` against a canonical wallpaper root:

- Percent-decoding happens before the segment check.
- `..`, NULs, and drive prefixes are rejected with HTTP 403.
- The candidate is canonicalized. A symlink that lands outside the root is 403.
- A missing file is 404.
- Responses carry `Access-Control-Allow-Origin: *` because the page origin is
  the custom scheme, not a network host you control.

Package web wallpapers are served from archive entries after the same relative
path check. They do not use `canonicalize`, because the bytes are already in
memory.

The daemon's still-image loader uses the same relative-path rule for scene
files and refuses to treat a symlink directory as a scene root
(`symlink_metadata` must be a real directory).

## IPC

The socket lives in the user runtime directory when `XDG_RUNTIME_DIR` is set,
otherwise in `/tmp/iris-$UID.sock`. The daemon creates the parent directory
with the process umask. Version 0.1.0 does not add an authentication cookie.
Anyone who can connect to the socket can pause wallpapers or shut the daemon
down. On a multi-user machine, set `XDG_RUNTIME_DIR` to a directory that is
mode `0700`, which is the usual runtime-dir setup.

The Windows pipe name cannot include separators. Characters outside
`[A-Za-z0-9_-]` are dropped before the `\\.\pipe\iris-ipc-` prefix is applied.

Frames larger than 16 MiB are rejected from the length prefix. JSON parse
errors become protocol errors. They do not panic the server.

## Process

`iris-core`, `iris-daemon`, and `iris-render` forbid unsafe code. Win32,
Wayland handles, FFmpeg, and the webview HWND live in the crates that already
contain those FFIs. Each `unsafe` block documents the pointer it uses.

The manager does not receive wallpaper pixels over IPC except a small preview
the command builder resizes for the library. Full wallpaper bytes stay in the
daemon.

Webview hardware acceleration failures are logged and retried in software.
They are not surfaced as a crash. Environment variables used for that retry
(`WEBKIT_DISABLE_COMPOSITING_MODE`, `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`)
are process-wide after the first failure.

## Data leaving the machine

The daemon does not phone home. A web wallpaper can, because the page can
fetch network URLs. That traffic is the wallpaper's, from the user's network
namespace. Telemetry is local. It is pushed into the page only as a property
bag (`cpu`, memory, battery, network rates). It is not uploaded by Iris.

## Logging

Stderr logs can contain connector names and wallpaper paths. They must not
contain secrets. Do not add token or environment dumps to tracing fields.
