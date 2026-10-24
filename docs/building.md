# Building

Iris builds as a Cargo workspace plus a Bun frontend. The engine and the
manager are separate commands.

## Toolchain

| Tool | Version used by this tree |
| --- | --- |
| Rust | Workspace `rust-version` is 1.80. `wry` 0.57 requires 1.85. Use stable 1.85 or newer. |
| Cargo | Ships with that toolchain. |
| Bun | Runs the manager frontend. `package.json` pins the script names. |
| C compiler / libclang | Required when `iris-video` rebuilds its FFmpeg bindings on Linux. |

Check the compiler:

```bash
rustc --version
bun --version
```

Add the Windows GNU target when you will cross-check:

```bash
rustup target add x86_64-pc-windows-gnu
```

## Linux packages

Package names differ by distribution. The libraries the crates look up are:

| Library | Why |
| --- | --- |
| GTK 3 and WebKit2GTK 4.1 | `wry` on Linux, and the Tauri shell |
| Wayland client and protocols | `iris-backend-wayland` |
| ALSA (`libasound`) | Scene audio capture |
| FFmpeg (`libavcodec`, `libavformat`, `libavutil`, `libswscale`) | `iris-video` |
| Vulkan loader or a GL driver | `wgpu` on Linux uses Vulkan or GL |
| `pkg-config`, a C toolchain, libclang | Native builds and bindgen |

On Fedora the usual set is `gtk3-devel`, `webkit2gtk4.1-devel`,
`wayland-devel`, `alsa-lib-devel`, `clang`, and the FFmpeg development
packages. On Debian and Ubuntu the usual set is `libgtk-3-dev`,
`libwebkit2gtk-4.1-dev`, `libwayland-dev`, `libasound2-dev`, `libclang-dev`,
and `pkg-config`.

If the distro FFmpeg package has libraries and no `.pc` files, unpack headers
into `.sysdeps/` at the repo root. `.cargo/config.toml` adds
`.sysdeps/pkgconfig` to `PKG_CONFIG_PATH`. That directory is gitignored. A
missing directory does not fail the build.

When bindgen runs for `iris-video` and cannot find libclang:

```bash
LIBCLANG_PATH=/usr/lib64 cargo check -p iris-video
```

Adjust the directory to wherever `libclang.so` lives.

## Windows

The supported check target in this tree is `x86_64-pc-windows-gnu`. A native
Windows build of the manager needs the
[WebView2](https://developer.microsoft.com/microsoft-edge/webview2/) runtime.
The daemon presents with Win32 and does not use WebView2 until a web wallpaper
is applied.

Video decode is not compiled on Windows. `iris_video::supported()` is `false`
there, and applying a video source returns an unsupported IPC error.

## Frontend

From the repository root:

```bash
bun install
bun run build
```

`bun run build` is `tsc && vite build`. The Tauri `beforeBuildCommand` runs the
same script. Dev mode is `bun run tauri dev`, which starts Vite on port 1420
and then the manager.

## Engine

Debug build and the library tests:

```bash
cargo test --offline --lib
```

The first build on a machine needs the network. Drop `--offline` once, then
use it.

Release daemon binary:

```bash
cargo build --release -p iris-daemon --bin iris-daemon
```

The binary is `target/release/iris-daemon`.

Release manager:

```bash
bun run tauri build
```

## Checks that must pass

Run them one at a time.

```bash
cargo fmt --all -- --check
cargo test --offline --lib
cargo check --workspace --offline
cargo check --workspace --target x86_64-pc-windows-gnu --offline
```

Do not pass `--edition` to `cargo fmt`. The workspace already sets it, and a
second `--edition` makes rustfmt exit with "given more than once".

`cargo check --workspace` includes `src-tauri`. On Linux that links WebKitGTK.
On the Windows GNU target it links WebView2 bindings instead.

## Cross-check notes

- `cargo check --offline` fails if a crate is not in the local registry.
  Retry that one command without `--offline`, then return to offline.
- Default features of the Linux daemon are Wayland, video, and scene audio.
  Those target dependencies are empty on the Windows check.
- `src-tauri` uses Rust edition 2024. The workspace crates use 2021.
