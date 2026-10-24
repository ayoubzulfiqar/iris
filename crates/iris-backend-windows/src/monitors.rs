//! Physical displays from `EnumDisplayMonitors`.
//!
//! Each record carries the `\\.\DISPLAY1` device name, the virtual-screen
//! rectangle, DPI divided by 96, and the current mode's refresh in millihertz.

use crate::Error;

/// One monitor the daemon can bind a wallpaper to.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayMonitor {
    /// Zero-based index in enumeration order for this call.
    pub index: u32,
    /// Device name, for example `\\.\DISPLAY1`.
    pub device_name: String,
    /// Left edge on the virtual screen.
    pub x: i32,
    /// Top edge on the virtual screen.
    pub y: i32,
    /// Width in physical pixels.
    pub width_px: u32,
    /// Height in physical pixels.
    pub height_px: u32,
    /// `dpi / 96`. `1.0` when DPI is unavailable.
    pub scale: f64,
    /// `MONITORINFOF_PRIMARY`.
    pub primary: bool,
    /// `dmDisplayFrequency * 1000` when the mode reports a rate.
    pub refresh_millihz: Option<u32>,
}

/// Enumerate attached displays. An empty successful list means the call ran
/// and the system reported no monitors.
#[allow(unsafe_code)]
pub fn enumerate_monitors() -> Result<Vec<DisplayMonitor>, Error> {
    use windows::Win32::Foundation::{BOOL, LPARAM};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, HDC, HMONITOR};

    let mut found = Vec::new();
    let pointer = &mut found as *mut Vec<HMONITOR> as isize;
    // SAFETY: `pointer` addresses `found` on this stack. The callback only
    // pushes HMONITOR values and returns before this function reads them.
    // A null DC and a null clip enumerate the virtual screen.
    let ok: BOOL = unsafe {
        EnumDisplayMonitors(
            HDC(std::ptr::null_mut()),
            None,
            Some(on_monitor),
            LPARAM(pointer),
        )
    };
    if ok.0 == 0 {
        return Err(Error::Win32("EnumDisplayMonitors failed".to_owned()));
    }
    let mut monitors = Vec::with_capacity(found.len());
    for (index, monitor) in found.into_iter().enumerate() {
        if let Some(info) = read_monitor(monitor, index as u32) {
            monitors.push(info);
        }
    }
    Ok(monitors)
}

#[allow(unsafe_code)]
unsafe extern "system" fn on_monitor(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    _dc: windows::Win32::Graphics::Gdi::HDC,
    _rect: *mut windows::Win32::Foundation::RECT,
    data: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    use windows::Win32::Foundation::TRUE;

    if data.0 == 0 || monitor.is_invalid() {
        return TRUE;
    }
    // SAFETY: `data` is the vec pointer passed to EnumDisplayMonitors. The vec
    // lives until that call returns, and this callback runs on the same thread.
    let list = unsafe { &mut *(data.0 as *mut Vec<windows::Win32::Graphics::Gdi::HMONITOR>) };
    list.push(monitor);
    TRUE
}

#[allow(unsafe_code)]
fn read_monitor(
    monitor: windows::Win32::Graphics::Gdi::HMONITOR,
    index: u32,
) -> Option<DisplayMonitor> {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITORINFO, MONITORINFOEXW};
    use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
    use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    // SAFETY: `monitor` came from EnumDisplayMonitors in this call. `info` is
    // a MONITORINFOEXW whose cbSize includes the device-name tail. GetMonitorInfoW
    // is declared to take MONITORINFO; the larger struct is the documented
    // input when cbSize says so. The pointer stays valid for the call.
    let ok = unsafe { GetMonitorInfoW(monitor, std::ptr::addr_of_mut!(info) as *mut MONITORINFO) };
    if ok.0 == 0 {
        return None;
    }
    let rect: RECT = info.monitorInfo.rcMonitor;
    let width = rect.right.saturating_sub(rect.left).max(0) as u32;
    let height = rect.bottom.saturating_sub(rect.top).max(0) as u32;
    let mut end = 0usize;
    while end < info.szDevice.len() && info.szDevice[end] != 0 {
        end += 1;
    }
    let device_name = String::from_utf16_lossy(&info.szDevice[..end]);
    let mut dpi_x = 96u32;
    let mut dpi_y = 96u32;
    // SAFETY: `monitor` is still the handle from this enumeration. The DPI
    // outputs are local u32s. A failure leaves the scale at 1.
    let _ = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    let scale = if dpi_x == 0 {
        1.0
    } else {
        f64::from(dpi_x) / 96.0
    };
    let refresh_millihz = refresh_of(&device_name);
    Some(DisplayMonitor {
        index,
        device_name,
        x: rect.left,
        y: rect.top,
        width_px: width.max(1),
        height_px: height.max(1),
        scale,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        refresh_millihz,
    })
}

#[allow(unsafe_code)]
fn refresh_of(device_name: &str) -> Option<u32> {
    use windows::core::PCWSTR;
    use windows::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};

    if device_name.is_empty() {
        return None;
    }
    let mut wide: Vec<u16> = device_name.encode_utf16().collect();
    wide.push(0);
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..DEVMODEW::default()
    };
    // SAFETY: `wide` is null-terminated and lives for this call. `mode.dmSize`
    // is the full struct. ENUM_CURRENT_SETTINGS asks for the active mode.
    // dmDisplayFrequency is copied out before `mode` is dropped.
    let ok =
        unsafe { EnumDisplaySettingsW(PCWSTR(wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut mode) };
    if ok.0 == 0 {
        return None;
    }
    let hz = mode.dmDisplayFrequency;
    if hz == 0 {
        None
    } else {
        Some(hz.saturating_mul(1000))
    }
}
