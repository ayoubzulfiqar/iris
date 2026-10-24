## Summary

<!-- What changed, and why an operator or a wallpaper author needs it. -->

## Scope

- [ ] Linux daemon
- [ ] Windows daemon
- [ ] IPC (`crates/iris-core`)
- [ ] Manager (`src-tauri`, `src/`)
- [ ] Docs only

## Protocol and layout

- [ ] `PROTOCOL_VERSION` is unchanged, or this pull request bumps it and updates both sides.
- [ ] Scene uniform offsets (`SCENE_UNIFORM_SIZE`, `FRAME_UNIFORM_SIZE`) are unchanged, or the shader and the packer moved together.
- [ ] The Linux present clock is still `wl_surface.frame`.

## Verification

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo test --offline --lib`
- [ ] `cargo check --workspace --offline`
- [ ] `cargo check --workspace --target x86_64-pc-windows-gnu --offline` when platform code changed
- [ ] `bun run build` when `src/` or the Tauri command surface changed

Paste the command result for any check that failed and then passed. Say which
checks you could not run.

## Safety

- [ ] No new `unwrap()` on a runtime path.
- [ ] Any new `unsafe` block states what it points at and how long that memory lives.
- [ ] Wallpaper path handling still rejects `..` and paths that leave the wallpaper root.

## License

This contribution is made under the [Apache License 2.0](https://github.com/ayoubzulfiqar/iris/blob/main/LICENSE).
