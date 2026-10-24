# Contributing to Iris

Iris is a live-wallpaper engine. The daemon presents frames. The Tauri app is
the manager. Changes that blur those two roles are the usual source of review
comments.

By submitting a contribution you agree that it is licensed under the
[Apache License, Version 2.0](LICENSE). See [NOTICE](NOTICE) for the copyright
line.

## Before you open a pull request

1. Read [docs/architecture.md](docs/architecture.md) and the page for the area
   you are touching.
2. Search open issues. A bug report belongs in the bug template. A
   vulnerability belongs in a
   [private advisory](https://github.com/ayoubzulfiqar/iris/security/advisories/new),
   as [SECURITY.md](SECURITY.md) describes.
3. Keep the patch limited to the behavior you are changing.

## Development setup

Follow [docs/building.md](docs/building.md). The short form on Linux is:

```bash
cargo test --offline --lib
cargo check --workspace --offline
cargo fmt --all -- --check
```

Run one Cargo command at a time. A second `cargo` against the same `target/`
directory races the build.

The workspace `rust-version` is 1.80. A full build of `iris-web` and the Tauri
shell needs Rust 1.85 or newer because `wry` 0.57 declares that floor. Use a
current stable toolchain.

`cargo test` uses the default members and does not build `src-tauri`.
`cargo test --workspace` also builds the manager and links WebKitGTK.

When you change `src/` or the Tauri command list, also run:

```bash
bun run build
```

When you change platform code, also run:

```bash
cargo check --workspace --target x86_64-pc-windows-gnu --offline
```

The first Windows check on a fresh machine may need the network once, so the
Windows crates can download. Later checks stay offline.

## Engineering rules

These rules exist because breaking them compiles on one target and fails on the
other, or it corrupts a frame.

- Runtime paths do not call `unwrap()`. Tests may use `expect`. `unwrap_or` is
  fine.
- `iris-core`, `iris-daemon`, and `iris-render` forbid `unsafe`. A new `unsafe`
  block in another crate states what pointer it uses and how long that memory
  lives.
- `src-tauri` is the manager. It does not link Wayland, wgpu, FFmpeg, or the
  wallpaper window backends. GPU and compositor code stays in the daemon and
  its crates.
- Linux presentation is paced by `wl_surface.frame`. A timer is for policy,
  IPC retries, and the webview pump. It does not replace the frame callback.
- Drop a GPU surface before the layer-shell role or the Win32 window that owns
  it.
- The packed scene uniform prefix is 344 bytes. The full frame uniform,
  including telemetry, is 552 bytes. Move the shader and the packer together,
  and keep `scene_uniform_places_the_spectra_at_the_packed_offsets` passing.
- `IpcCall` and `ResponseBody` are `#[non_exhaustive]`. Adding a method means
  a manual arm in the request decoder and a daemon match, including the
  wildcard.
- Wallpaper file access rejects `..`, absolute paths, and symlinks that leave
  the wallpaper root.
- Do not copy GPL code into the tree. `calloop-wayland-source` is MIT; do not
  paste it verbatim.

## Commit messages

Use an imperative subject that says why the change exists. One file or one
logical change per commit. Do not add `Signed-off-by`, `Co-authored-by`, or a
generated-by trailer.

## Pull requests

Use the pull request template. Fill in the checks you ran and the ones you
could not run. A UI change describes what you clicked. A protocol change names
the old and new `PROTOCOL_VERSION` behavior.

Review looks for: the daemon still survives a bad wallpaper, both target
checks, and no drive-by rewrite of an unrelated crate.

## Conduct

The [Code of Conduct](CODE_OF_CONDUCT.md) applies to issues, pull requests, and
any other project space.
