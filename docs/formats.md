# Wallpaper formats

The daemon picks a loader from `WallpaperSource` and, for a directory or a
package, from `project.json`.

## Still image

`source.kind` is `image`. The file must be a regular file of at most 256 MiB.
Decodable types are PNG, JPEG, GIF, and BMP. The image is uploaded as RGBA.

`scale` controls fit: `fill` covers the output, `fit` letterboxes, `stretch`
ignores aspect, `center` draws at native size.

## Package

`source.kind` is `package`. The file is a Wallpaper Engine `PKGV` archive.
Paths inside the archive use `/`. Absolute paths and `..` are rejected.

If `project.json` says `"type": "web"`, the package is a web wallpaper and the
protocol serves archive entries. Any other type is a still or a scene.

A still package uses `preview` when that entry decodes, then the first scene
image, then the first decodable entry. A scene package with objects is
compiled when `scene.json` is present and non-empty.

## Scene directory

`source.kind` is `scene_directory`. The path must be a real directory, not a
symlink.

Classification order:

1. `project.json` with `"type": "web"` opens the web runtime.
2. Any other non-empty `type` stays on the GPU path.
3. A `scene.json` stays on the GPU path.
4. An `index.html` with no scene and no explicit type opens the web runtime.
5. Otherwise the loader looks for a preview image, then scene images, then a
   walk of up to 4 096 files.

`scene.json` needs a `camera` and a `general` object. Missing vectors become
zero. Unknown keys are preserved. Objects are drawn in archive order: images,
models (as references), particles, sounds, text, lights, and cameras. Text is
a tinted quad, not a glyph raster. Particles step on a fixed timestep and stop
at the pool cap.

User properties for a web project come from `general.properties` when that
object exists, otherwise from a top-level `properties` object.

## Video

`source.kind` is `video`. Linux opens FFmpeg, tries VA-API, then CUDA/NVDEC,
then a software decoder. Frames are YUV or NV12 planes. The renderer converts
them. Playback rate, looping, and mute come from `playback`. Pause freezes the
media clock and the decoder.

Windows rejects video sources. The error names the path.

## Web

The webview navigates to `iris://wallpaper/index.html`, or to the relative
`file` from `project.json` when that path is safe. `../` in `file` is ignored
and the entry stays `index.html`.

Before the document loads, the view injects:

- `window.wallpaperRegisterAudioListener`
- `window.wallpaperPropertyListener.applyUserProperties`
- `window.wallpaperPropertyListener.applyGeneralProperties`
- `window.iris.version` and `window.iris.postMessage`, which forwards to
  `window.ipc.postMessage`

While the slot is playing, the daemon evaluates, at 30–60 FPS for audio:

- `_irisAudioListener` with the 64-band spectrum, and stores both 16- and
  64-band arrays on `window.iris.audio`
- `applyUserProperties` with the project properties, then with
  `{ telemetry: snapshot }`
- a `pointermove` using the latest IPC pointer as client coordinates

Pause sets `document.hidden` and `visibilityState` and stops those injections.
A page that replaces `wallpaperPropertyListener` after load still receives
later evaluations.

Hardware acceleration is the first attempt. Failure logs a warning and retries
with software rasterization. A second failure leaves the slot in `failed` and
keeps the daemon running.

## Colors and audio ranges

Shader and telemetry fractions are `0.0..=1.0`. CPU percentages from the
sampler are divided by 100 before they are published. Audio bands are
normalized. Non-finite bands are not injected into the page.
