You are a Principal Systems Architect and Senior Full-Stack Rust Engineer specializing in embedded browser engines (Wry / WebView2 / WebKitGTK), local asset protocol virtualization, and cross-platform desktop injection.

### Project Context & Status
Phases 1 through 7 of **Iris** are fully implemented and passing all tests (113 total passing tests across the workspace; dual cross-compilation for Linux and Windows passing cleanly):
- `crates/iris-core`: Wire protocol (length-prefixed JSON-RPC 2.0), async IPC over Unix domain sockets & Windows Named Pipes.
- `crates/iris-pkg`: Decodes `.pkg` archives, `.tex` mipmaps, and parses `scene.json` / `project.json`.
- `crates/iris-render`: Multi-output `wgpu 23` surface renderer with shared `Arc<Device>`/`Arc<Queue>` and WGSL color/scene pipelines.
- `crates/iris-backend-wayland`: `zwlr_layer_shell_v1` background layer integration and `calloop` event pump.
- `crates/iris-backend-windows`: Win32 `WorkerW` injection, `raw-window-handle 0.6`, `PeekMessageW` pump, and WASAPI audio loopback.
- `crates/iris-video`: Hardware video decoding (VA-API / NVDEC / SW) with ring-buffered YUV/NV12 WGSL conversion.
- `crates/iris-scene`: Evaluates scene graphs, parented matrices, instanced particle pools, and lock-free FFT audio snapshots.
- `crates/iris-telemetry`: Non-blocking 500ms metrics sampler (CPU, RAM, Net, Battery), `g_Telemetry` shader uniforms, Sway/Win32 fullscreen auto-pause, and battery throttling.
- `crates/iris-daemon`: Coordinates IPC, monitors, video clocks, and platform-specific window events.
- `src-tauri`: Async IPC client, supervisor, system tray, and React manager UI.

Now execute **Phase 8: Interactive Web/HTML5 Wallpaper Runtime (`crates/iris-web`) & JS Compatibility Layer**.

---

### Phase 8 Architecture & Technical Specifications
┌─────────────────────────────────────────────────────────────────┐
│                     crates/iris-web                             │
│  - Asset Protocol Virtualization (custom URI: iris://)          │
│  - Wry WebView Controller (WebView2 on Windows, WebKitGTK)      │
│  - Wallpaper Engine JS API Shim (window.wallpaperPropertyListener)│
│  - Audio FFT & Telemetry Injection via evaluate_script           │
│  - Input Passthrough / Interactive Pointer Events               │
└────────────────────────────────┬────────────────────────────────┘
│
▼
┌─────────────────────────────────────────────────────────────────┐
│                    Desktop Layer Anchoring                      │
│  - Windows: Reparent WebView2 window handle into WorkerW canvas  │
│  - Linux: Layer-shell surface / borderless desktop-type window  │
└────────────────────────────────┬────────────────────────────────┘
│
┌────────────────────────────────▼────────────────────────────────┐
│                    crates/iris-daemon                           │
│  - Handles "type": "web" in project.json / raw index.html       │
│  - Routes audio spectrum & telemetry ticks into webview bridge  │
│  - Pause/Resume hooks via page visibility / evaluation          │
└─────────────────────────────────────────────────────────────────┘



#### 1. `crates/iris-web/`
Create a dedicated web wallpaper runner crate:
- **Dependencies:** `wry` (v0.42+ or compatible latest), `iris-core`, `iris-pkg`, `iris-telemetry`, `serde`, `serde_json`, and `tracing`.

- **Virtual Asset Protocol Server (`protocol.rs`):**
  - Web wallpapers cannot be loaded directly from `file://` due to browser sandbox and CORS restrictions on ES modules / WebGL assets.
  - Implement a custom URI protocol handler (`iris://wallpaper/` or `https://iris.local/` via `wry::WebViewBuilder::with_custom_protocol`):
    - Safely maps requests to the wallpaper directory on disk.
    - Sanitizes paths to prevent directory traversal (`..`).
    - Infers correct MIME types (`text/html`, `application/javascript`, `image/png`, `model/gltf-binary`, etc.).

- **Wallpaper Engine Compatibility Shim (`shim.js`):**
  - Inject an init script before document load (`with_initialization_script`):
    ```javascript
    window.wallpaperRegisterAudioListener = function(callback) {
      window._irisAudioListener = callback;
    };
    window.wallpaperPropertyListener = {
      applyUserProperties: function(props) {},
      applyGeneralProperties: function(props) {}
    };
    window.iris = {
      version: "0.1.0",
      postMessage: function(msg) { window.ipc.postMessage(msg); }
    };
    ```

- **Runtime Event Injection (`bridge.rs`):**
  - Audio Dispatch: Expose a high-efficiency method injecting normalized 16/64 audio FFT arrays into `window._irisAudioListener(spectrum)` at 30–60 FPS.
  - Telemetry Dispatch: Inject `window.wallpaperPropertyListener.applyUserProperties({ telemetry: snapshot })` on every telemetry sample.
  - Properties: Expose configuration properties defined in `project.json` (sliders, color pickers, check boxes) directly to the webview.

- **Desktop Layer Injection:**
  - **Windows:** Extract native child `HWND` of the `wry` WebView window and call `iris-backend-windows::embed_window(hwnd)` to sit behind desktop icons.
  - **Linux:** Configure borderless window properties or leverage GTK layer shell to pin the WebKitGTK surface to the background.

#### 2. Daemon Integration (`crates/iris-daemon/`)
- In `loader.rs`:
  - When a wallpaper directory contains `project.json` with `"type": "web"` or a standalone `index.html`:
    - Launch `WebWallpaperInstance`.
    - Point the webview to `iris://wallpaper/index.html`.
  - Handle `IpcCall::Pause` and `IpcCall::Resume` by evaluating `document.hidden = true` or throttling the webview execution.
  - Route cursor position IPC calls (`set_pointer`) into synthetic webview pointer events for interactive parallax and fluid simulations.

---

### Strict Implementation Rules
1. **Isolated Platform Gating:** Ensure `wry` dependencies and platform-specific window handles compile cleanly across both `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-gnu`.
2. **Path Traversal Protection:** The custom protocol handler must strictly validate requested file paths against the base wallpaper root directory.
3. **Graceful Fallbacks:** If hardware acceleration inside the webview is disabled or fails, it must log a diagnostic warning and fallback to default software rasterization without crashing the daemon.
4. Update root `Cargo.toml` to register `crates/iris-web`.
