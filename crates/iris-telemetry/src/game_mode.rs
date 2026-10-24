//! Fullscreen and borderless-game detection.
//!
//! The decision itself is geometry: a foreground window that covers an output
//! is fullscreen. Windows supplies the foreground rectangle. On Linux, Sway's
//! tree is used when `SWAYSOCK` is set. Anything else stays inactive so a
//! missing compositor does not pause the wallpaper.

use serde_json::Value;

/// A pixel rectangle in virtual-screen coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelRect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width. Zero or negative covers nothing.
    pub width: i32,
    /// Height. Zero or negative covers nothing.
    pub height: i32,
}

/// Fraction of `output` covered by `window`, in `0.0..=1.0`.
pub fn coverage_ratio(window: PixelRect, output: PixelRect) -> f32 {
    let area = i64::from(output.width) * i64::from(output.height);
    if area <= 0 || window.width <= 0 || window.height <= 0 {
        return 0.0;
    }
    let overlap = intersection_area(window, output);
    (overlap as f32 / area as f32).clamp(0.0, 1.0)
}

/// True when `window` covers `output`, or a popup covers at least 90%.
pub fn covers_output(window: PixelRect, output: PixelRect, popup: bool) -> bool {
    let ratio = coverage_ratio(window, output);
    ratio >= 0.97 || (popup && ratio >= 0.90)
}

/// True when any output is covered.
pub fn any_output_covered(window: PixelRect, outputs: &[PixelRect], popup: bool) -> bool {
    outputs
        .iter()
        .any(|output| covers_output(window, *output, popup))
}

fn intersection_area(window: PixelRect, output: PixelRect) -> i64 {
    let left = window.x.max(output.x);
    let top = window.y.max(output.y);
    let right = (window.x.saturating_add(window.width)).min(output.x.saturating_add(output.width));
    let bottom =
        (window.y.saturating_add(window.height)).min(output.y.saturating_add(output.height));
    let width = i64::from(right.saturating_sub(left));
    let height = i64::from(bottom.saturating_sub(top));
    if width <= 0 || height <= 0 {
        0
    } else {
        width.saturating_mul(height)
    }
}

/// Sway `GET_TREE` JSON: a focused node with `fullscreen_mode` above zero.
pub fn sway_tree_is_fullscreen(value: &Value) -> bool {
    fn walk(node: &Value) -> bool {
        let focused = node
            .get("focused")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let mode = node
            .get("fullscreen_mode")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if focused && mode > 0 {
            return true;
        }
        for key in ["nodes", "floating_nodes"] {
            if let Some(children) = node.get(key).and_then(Value::as_array) {
                if children.iter().any(walk) {
                    return true;
                }
            }
        }
        false
    }
    walk(value)
}

pub(crate) fn probe_fullscreen() -> bool {
    #[cfg(windows)]
    {
        return probe_windows();
    }
    #[cfg(not(windows))]
    {
        probe_linux()
    }
}

#[cfg(not(windows))]
fn probe_linux() -> bool {
    let Some(sock) = std::env::var_os("SWAYSOCK") else {
        return false;
    };
    match sway_socket_fullscreen(std::path::Path::new(&sock)) {
        Some(active) => active,
        None => false,
    }
}

#[cfg(not(windows))]
fn sway_socket_fullscreen(path: &std::path::Path) -> Option<bool> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let mut stream = UnixStream::connect(path).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    stream
        .set_write_timeout(Some(Duration::from_millis(200)))
        .ok()?;
    let mut header = Vec::with_capacity(14);
    header.extend_from_slice(b"i3-ipc");
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&4u32.to_le_bytes());
    stream.write_all(&header).ok()?;
    let mut magic = [0u8; 6];
    stream.read_exact(&mut magic).ok()?;
    if &magic != b"i3-ipc" {
        return None;
    }
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).ok()?;
    let len = u32::from_le_bytes(len_buf);
    if len > 8 * 1024 * 1024 {
        return None;
    }
    let mut kind = [0u8; 4];
    stream.read_exact(&mut kind).ok()?;
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload).ok()?;
    let value: Value = serde_json::from_slice(&payload).ok()?;
    Some(sway_tree_is_fullscreen(&value))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn probe_windows() -> bool {
    use windows::Win32::Graphics::Gdi::{
        GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowLongPtrW, GetWindowRect, GWL_STYLE, WS_POPUP,
    };

    // SAFETY: GetForegroundWindow returns the foreground HWND for this desktop,
    // or null when there is none. A null handle is not queried further.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.is_invalid() {
        return false;
    }
    if is_iris_window(hwnd) {
        return false;
    }
    let mut window = windows::Win32::Foundation::RECT::default();
    // SAFETY: `hwnd` is the live foreground window. `window` is a local RECT
    // written by GetWindowRect and read only after the call returns.
    if unsafe { GetWindowRect(hwnd, &mut window) }.is_err() {
        return false;
    }
    let monitor = unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) };
    if monitor.is_invalid() {
        return false;
    }
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..MONITORINFO::default()
    };
    // SAFETY: `monitor` came from MonitorFromWindow for this hwnd. `info.cbSize`
    // is the struct size GetMonitorInfoW requires. The rectangle is copied out
    // before `info` drops.
    if unsafe { GetMonitorInfoW(monitor, &mut info) }.0 == 0 {
        return false;
    }
    // SAFETY: GWL_STYLE reads the window's style bits. The hwnd is still the
    // foreground window from this call. The value is a bitfield, not a pointer.
    let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) };
    let popup = (style as u32) & WS_POPUP.0 != 0;
    let foreground = PixelRect {
        x: window.left,
        y: window.top,
        width: window.right.saturating_sub(window.left),
        height: window.bottom.saturating_sub(window.top),
    };
    let output = PixelRect {
        x: info.rcMonitor.left,
        y: info.rcMonitor.top,
        width: info.rcMonitor.right.saturating_sub(info.rcMonitor.left),
        height: info.rcMonitor.bottom.saturating_sub(info.rcMonitor.top),
    };
    covers_output(foreground, output, popup)
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn is_iris_window(hwnd: windows::Win32::Foundation::HWND) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetClassNameW;

    let mut class_buf = [0u16; 64];
    // SAFETY: `hwnd` is a window the caller just received from GetForegroundWindow.
    // `class_buf` is a writable buffer. GetClassNameW writes a null-terminated
    // class name and returns the length without the null.
    let length = unsafe { GetClassNameW(hwnd, &mut class_buf) };
    if length <= 0 {
        return false;
    }
    let end = usize::try_from(length).unwrap_or(0).min(class_buf.len());
    let name = String::from_utf16_lossy(&class_buf[..end]);
    name == "IrisWallpaper"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output() -> PixelRect {
        PixelRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        }
    }

    #[test]
    fn an_exact_cover_is_fullscreen() {
        assert!(covers_output(output(), output(), false));
    }

    #[test]
    fn a_small_window_is_not_fullscreen() {
        let window = PixelRect {
            x: 100,
            y: 100,
            width: 800,
            height: 600,
        };
        assert!(!covers_output(window, output(), true));
        assert!(!any_output_covered(window, &[output()], true));
    }

    #[test]
    fn a_borderless_popup_with_a_small_inset_counts() {
        let window = PixelRect {
            x: 0,
            y: 0,
            width: 1920,
            height: 1000,
        };
        let ratio = coverage_ratio(window, output());
        assert!(ratio < 0.97 && ratio >= 0.90, "{ratio}");
        assert!(covers_output(window, output(), true));
        assert!(!covers_output(window, output(), false));
    }

    #[test]
    fn sway_fullscreen_follows_the_focused_node() {
        let tree = serde_json::json!({
            "nodes": [{
                "focused": false,
                "fullscreen_mode": 0,
                "nodes": [{ "focused": true, "fullscreen_mode": 1, "nodes": [] }]
            }]
        });
        assert!(sway_tree_is_fullscreen(&tree));
        let idle = serde_json::json!({"focused": true, "fullscreen_mode": 0, "nodes": []});
        assert!(!sway_tree_is_fullscreen(&idle));
    }
}
