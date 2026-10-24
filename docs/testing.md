# Testing

The engine suite is `cargo test --offline --lib`. It runs the default workspace
members and skips `src-tauri`, so it does not open WebKitGTK. Web protocol,
path traversal, and wallpaper detection tests build temp directories and do
not create a webview.

Run Cargo once at a time.

## Required commands

```bash
cargo fmt --all -- --check
cargo test --offline --lib
cargo check --workspace --offline
cargo check --workspace --target x86_64-pc-windows-gnu --offline
```

`cargo fmt` must not be given `--edition`. The workspace edition is already
set, and a second copy makes rustfmt fail.

A Windows check that exits because a crate is not cached should be repeated
once without `--offline`.

When `src/` or the Tauri commands change:

```bash
bun run build
```

## What the suite covers

| Area | What a failure means |
| --- | --- |
| `iris-core` | A frame, a method, or a config default drifted |
| `iris-pkg` | An archive, DXT block, or scene document no longer round-trips |
| `iris-render` | Uniform offsets or the scene shader parse moved |
| `iris-daemon` | Load, policy, or the Linux socket ping/shutdown path broke |
| `iris-web` | Traversal, MIME, the shim text, or web-versus-scene detection broke |
| `iris-telemetry` | A power or fullscreen pure function changed |
| `iris-scene` | Particle caps or the FFT band mapping changed |

Scene audio tests may print ALSA `pcm_route.c` lines when the machine has no
matching channel map. The tests still pass when the assertions do. The
manager's "socket stays down" test, when you run the Tauri crate tests, waits
out the 3 second supervisor budget.

## What the suite does not cover

- A real compositor, a real WorkerW, or a visible webview.
- VA-API or NVDEC on hardware. Decode tests use fixtures that do not need a GPU.
- The React UI in a browser. `bun run build` typechecks and bundles. Click
  through the manager when you change layout or shared client state.
- macOS.

Say so in the pull request when your change can only be proven on a desktop
session you do not have.

## Adding a test

Prefer a pure function and a temp directory over a live daemon. Daemon tests
that bind a socket must use a private path and shut the server down. Do not
call `gtk::init` from a unit test. `WebWallpaperInstance::open` with
`launch: false` is the headless constructor.

If you add a field to `WallpaperConfig`, update the struct literals in the IPC
tests and the policy tests. Serde defaults cover omitted JSON. They do not
cover a Rust literal that forgot the field.

If you add a field to `SceneDraw`, update every construction site in
`app.rs` and `app_windows.rs`.
