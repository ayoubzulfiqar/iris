# Iris

Iris is a live-wallpaper engine for Linux and Windows. A daemon presents still
images, Wallpaper Engine packages, scenes, Linux video, and HTML wallpapers
behind the desktop. A Tauri manager applies them, pauses them, and reads
system telemetry. The manager does not render.

Version 0.1.0. Protocol version 1. Licensed under the Apache License 2.0.
See [LICENSE](LICENSE) and [NOTICE](NOTICE).

## Status

| Platform | Presentation | Video | Web |
| --- | --- | --- | --- |
| Linux (`x86_64-unknown-linux-gnu`) | Wayland layer-shell, paced by `wl_surface.frame` | VA-API, NVDEC, then software | WebKitGTK |
| Windows (`x86_64-pc-windows-gnu`) | Borderless window on the desktop WorkerW | Not in this version | WebView2 |
| macOS | No backend | No backend | No backend |

This is a desktop product for the user who starts it. It is not a multi-tenant
service. The [security model](docs/security-model.md) states that boundary.

## Repository layout

```text
crates/iris-core              JSON-RPC, framing, logging
crates/iris-pkg               .pkg, .tex, scene.json, project.json
crates/iris-render            wgpu 23 surfaces and shaders
crates/iris-backend-wayland   layer-shell session
crates/iris-backend-windows   WorkerW, monitors, WASAPI
crates/iris-video             FFmpeg decode (Linux)
crates/iris-scene             layers, particles, FFT
crates/iris-telemetry         CPU, memory, network, battery, fullscreen
crates/iris-web               wry webview and iris:// assets
crates/iris-daemon            IPC server and frame loop
src-tauri                     tray, supervisor, IPC client
src                           React manager
docs                          operator and contributor documentation
```

## Run

Install the libraries in [docs/building.md](docs/building.md), then:

```bash
bun install
cargo run -p iris-daemon --bin iris-daemon
```

In another shell:

```bash
bun run tauri dev
```

The manager starts a daemon itself when the socket or pipe is down. Closing
the window hides it. Quit Iris shuts the daemon down.

Linux listens on `$XDG_RUNTIME_DIR/iris/iris.sock`, or `/tmp/iris-$UID.sock`
when `XDG_RUNTIME_DIR` is unset. Windows listens on `\\.\pipe\iris-ipc-{user}`.
Details, limits, and recovery are in [docs/operations.md](docs/operations.md).

## Verify

Run one Cargo command at a time:

```bash
cargo fmt --all -- --check
cargo test --offline --lib
cargo check --workspace --offline
cargo check --workspace --target x86_64-pc-windows-gnu --offline
```

`cargo test` covers the engine crates. It does not launch a webview or a
compositor. `cargo test --workspace` also builds the Tauri shell.

## Documentation

| Document | Read it when you need to |
| --- | --- |
| [Architecture](docs/architecture.md) | See which process owns the GPU, the socket, and the UI |
| [Building](docs/building.md) | Install toolchains and native libraries |
| [Operations](docs/operations.md) | Start, stop, and recover a daemon |
| [IPC](docs/ipc.md) | Call the daemon or extend the protocol |
| [Platforms](docs/platforms.md) | Compare Linux and Windows behavior |
| [Formats](docs/formats.md) | Know how a directory, package, video, or HTML wallpaper is chosen |
| [Security model](docs/security-model.md) | See what the file and IPC boundaries guarantee |
| [Testing](docs/testing.md) | Run the suite and add a test that does not open GTK |
| [Contributing](CONTRIBUTING.md) | Prepare a patch |
| [Security policy](SECURITY.md) | Report a vulnerability in private |
| [Code of conduct](CODE_OF_CONDUCT.md) | See how the project handles conduct reports |
| [llms.txt](llms.txt) | Curated map of these docs for agents and answer engines |

## Protocol at a glance

Messages are JSON-RPC 2.0 with a little-endian `u32` length prefix. Frames
larger than 16 MiB are rejected before the body is read. Methods include
`ping`, `list_monitors`, `apply_wallpaper`, `pause`, `resume`, `set_pointer`,
`get_telemetry`, and `shutdown`. The full table is [docs/ipc.md](docs/ipc.md).

## License

Iris source is under the [Apache License, Version 2.0](LICENSE). Dependencies
keep their own licenses, as recorded in `Cargo.lock` and `bun.lock`.
Contributions are accepted under the same license. See [CONTRIBUTING.md](CONTRIBUTING.md).
