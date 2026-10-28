# Platforms

Version 0.1.0 supports Linux and Windows. macOS has no wallpaper backend. The
daemon returns an unsupported error there.

## Support matrix

| Capability | Linux | Windows |
| --- | --- | --- |
| Still image and `.pkg` preview | Yes | Yes |
| Scene graph and particles | Yes | Yes |
| Video decode | VA-API, then NVDEC, then software FFmpeg | Unsupported |
| Audio spectrum | cpal capture | WASAPI loopback into the same engine |
| Web wallpaper | WebKitGTK via `wry` | WebView2 via `wry` |
| Desktop placement | `zwlr_layer_shell_v1` background layer | WorkerW behind the icons |
| Fullscreen probe | Sway IPC (`SWAYSOCK`) | Foreground window versus the monitor |
| Battery | sysfs `BAT*` and `AC`/`ADP` | `GetSystemPowerStatus` |
| IPC | Unix socket | Named pipe |
| Checked target | `x86_64-unknown-linux-gnu` | `x86_64-pc-windows-gnu` |

A compositor without `zwlr_layer_shell_v1` cannot host a GPU wallpaper. GNOME
is in that group. A still image is applied with `gsettings` on
`org.gnome.desktop.background`, including the dark-mode URI. Clear restores
the values saved in `$XDG_STATE_HOME/iris/desktop-background.json`.

Video and GIF are not opened as a borderless window. The daemon stages the
file and creates a GTK surface titled `IrisWallpaper:x,y,w,h` only after the
Shell extension `gnome-extension/iris-wallpaper@iris` has written
`background-group-1` to `$XDG_STATE_HOME/iris/shell-extension`. The extension
moves that surface into the Shell background group, behind other windows.
Shell imports an extension once per login, so a newly installed copy stays
idle until the next login. Until the stamp file exists, apply returns
unsupported and does not create a window. Scenes still need layer-shell.

An HTML wallpaper (`type: web`) still opens a GTK webview. On Wayland the
desktop type hint is advisory and the compositor chooses the position.

A Windows shell that never creates the wallpaper WorkerW logs an injection
warning. The borderless window remains. `embed_window` reports the same miss
for a webview child.

## Linux present loop

`iris-backend-wayland` connects once, enumerates outputs, and creates one
layer per assigned monitor. Frame callbacks carry the presentation timestamp.
The daemon:

1. Applies fullscreen and battery policy.
2. Skips the draw when the effective FPS cap has not elapsed.
3. Draws the still, the video planes, or the scene.
4. Commits and queues the next `wl_surface.frame` only while the slot is playing.

Hotplug uses the same session. An output that disappears drops its slot. A new
output receives the broadcast wallpaper when the last apply targeted every
monitor.

## Windows present loop

The daemon runs a current-thread Tokio runtime. Every tick drains the window
queue, applies policy, and presents each GPU slot. WASAPI loopback runs on its
own thread and pushes samples into the scene audio engine. `cpal` is not opened
on Windows. The capture stream is intentionally not stored in a lock: `cpal`
streams are not `Sync`.

`WM_DISPLAYCHANGE` refreshes the monitor list. Stale device names lose their
slots. A broadcast apply fills new monitors.

## Web placement

The document URL is `iris://wallpaper/index.html`, or `iris://wallpaper/{file}`
when `project.json` names a safe relative `file`. WebView2 rewrites that
scheme internally. The protocol handler still sees the `iris://` form.

Each webview gets its own `wry` data directory under the temp dir so two
instances can register the `iris` scheme. The directory is left in temp for
the life of the process.

Linux pumps GTK with `gtk::main_iteration_do(false)`, at most 32 iterations
per 16 ms tick, and only after a webview has initialized GTK. Windows relies
on the existing `PeekMessageW` pump.

## GPU backends

The wgpu instance uses DX12 or Vulkan on Windows, and Vulkan or GL elsewhere.
That selection is shared by every surface on the device.

## Logging and class names

The Win32 wallpaper class is `IrisWallpaper`. The fullscreen probe skips that
class so the wallpaper does not pause itself. Webviews use the webview's own
window class. They are created without activation and, on Linux, without
accepting focus.
