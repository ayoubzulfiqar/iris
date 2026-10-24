//! Desktop WorkerW selection.
//!
//! `0x052C` asks Explorer to split the desktop into an icon host and a
//! wallpaper host. The wallpaper host is the next top-level `WorkerW` after
//! the window that owns `SHELLDLL_DefView`. [`target_workerw`] is that rule
//! with no Win32 calls. The FFI wrapper feeds it the `EnumWindows` list.

use crate::Error;

/// One top-level window in `EnumWindows` order, front to back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TopLevelWindow {
    /// Platform handle bits. Tests use small integers.
    pub id: u64,
    /// Window class, such as `Progman` or `WorkerW`.
    pub class_name: String,
    /// True when a child of this window is `SHELLDLL_DefView`.
    pub contains_defview: bool,
}

/// The WorkerW that sits immediately behind the desktop icons.
///
/// `windows` is front-to-back. The search starts after the window that
/// contains `SHELLDLL_DefView`, which may be `Progman` or a `WorkerW`. The
/// first later `WorkerW` is the canvas. A missing canvas is an error.
pub fn target_workerw(windows: &[TopLevelWindow]) -> Result<u64, Error> {
    let mut past_icons = false;
    for window in windows {
        if past_icons && window.class_name == "WorkerW" {
            return Ok(window.id);
        }
        if window.contains_defview {
            past_icons = true;
        }
    }
    Err(Error::Desktop(
        "WorkerW beneath the desktop icons was not found".to_owned(),
    ))
}

/// Reparent `hwnd` onto the WorkerW behind the desktop icons.
///
/// The web runtime passes the wry child window here. The injection is the
/// same [`parent_under_desktop`] path the GPU window uses.
#[cfg(windows)]
pub fn embed_window(hwnd: isize) -> Result<(), Error> {
    parent_under_desktop(hwnd)
}

/// Reparent `child` onto the WorkerW behind the desktop icons.
///
/// A custom shell that never creates that WorkerW returns [`Error::Desktop`].
#[cfg(windows)]
#[allow(unsafe_code)]
pub fn parent_under_desktop(child: isize) -> Result<(), Error> {
    let parent = find_wallpaper_workerw()?;
    // SAFETY: `child` is an HWND created by this process and still alive.
    // `parent` was returned by EnumWindows during this call. SetParent does
    // not free either window. The caller keeps the child HWND until after the
    // GPU surface that uses it has been dropped.
    let child_hwnd = windows::Win32::Foundation::HWND(child as *mut core::ffi::c_void);
    let parent_hwnd = windows::Win32::Foundation::HWND(parent as *mut core::ffi::c_void);
    unsafe { windows::Win32::UI::WindowsAndMessaging::SetParent(child_hwnd, parent_hwnd) }
        .map(|_| ())
        .map_err(|err| Error::Win32(format!("SetParent failed: {err}")))
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn find_wallpaper_workerw() -> Result<isize, Error> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, FindWindowW, SendMessageTimeoutW, SMTO_NORMAL,
    };

    const PROGMAN: &[u16] = &[
        b'P' as u16,
        b'r' as u16,
        b'o' as u16,
        b'g' as u16,
        b'm' as u16,
        b'a' as u16,
        b'n' as u16,
        0,
    ];

    // SAFETY: PROGMAN points at a null-terminated static UTF-16 class name.
    // The window-name pointer is null, which asks for any title. The returned
    // HWND is a top-level shell window, not owned by this process, and is only
    // used as a message target for the rest of this function.
    let progman = unsafe { FindWindowW(PCWSTR(PROGMAN.as_ptr()), PCWSTR::null()) }
        .map_err(|err| Error::Desktop(format!("Progman window was not found: {err}")))?;
    let mut raw_result = 0usize;
    // SAFETY: progman is the HWND just returned. 0x052C is the undocumented
    // shell message that separates the icon host from the wallpaper host.
    // The timeout is one second. A zero return means the shell ignored it;
    // enumeration still runs, and a missing WorkerW becomes Error::Desktop.
    let sent: LRESULT = unsafe {
        SendMessageTimeoutW(
            progman,
            0x052C,
            WPARAM(0),
            LPARAM(0),
            SMTO_NORMAL,
            1_000,
            Some(&mut raw_result),
        )
    };
    if sent.0 == 0 {
        tracing::debug!("shell separation message 0x052C returned 0");
    }

    let mut found = Vec::new();
    let pointer = &mut found as *mut Vec<TopLevelWindow> as isize;
    // SAFETY: `pointer` addresses `found`, which stays on this stack until
    // EnumWindows returns. The callback only appends to that vec. It does not
    // keep the pointer. EnumWindows walks top-level windows on this thread.
    let enumerated = unsafe { EnumWindows(Some(on_window), LPARAM(pointer)) };
    if let Err(err) = enumerated {
        return Err(Error::Win32(format!("EnumWindows failed: {err}")));
    }
    let id = target_workerw(&found)?;
    Ok(id as isize)
}

#[cfg(windows)]
#[allow(unsafe_code)]
unsafe extern "system" fn on_window(
    hwnd: windows::Win32::Foundation::HWND,
    data: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HWND, TRUE};
    use windows::Win32::UI::WindowsAndMessaging::{FindWindowExW, GetClassNameW};

    const DEFVIEW: &[u16] = &[
        b'S' as u16,
        b'H' as u16,
        b'E' as u16,
        b'L' as u16,
        b'L' as u16,
        b'D' as u16,
        b'L' as u16,
        b'L' as u16,
        b'_' as u16,
        b'D' as u16,
        b'e' as u16,
        b'f' as u16,
        b'V' as u16,
        b'i' as u16,
        b'e' as u16,
        b'w' as u16,
        0,
    ];

    if hwnd.is_invalid() || data.0 == 0 {
        return TRUE;
    }
    // SAFETY: `data` is the pointer passed to EnumWindows and the vec is still
    // on the caller's stack. This callback runs on that same thread before
    // EnumWindows returns, so the pointer is not used after the vec moves.
    let list = unsafe { &mut *(data.0 as *mut Vec<TopLevelWindow>) };
    let mut class_buf = [0u16; 256];
    // SAFETY: hwnd is the window EnumWindows is visiting. class_buf is a
    // writable 256-unit buffer. GetClassNameW writes a null-terminated name
    // and returns the length without the null.
    let length = unsafe { GetClassNameW(hwnd, &mut class_buf) };
    let class_name = if length > 0 {
        let end = usize::try_from(length).unwrap_or(0).min(class_buf.len());
        String::from_utf16_lossy(&class_buf[..end])
    } else {
        String::new()
    };
    // SAFETY: hwnd is still the window being enumerated. DEFVIEW is a static
    // null-terminated class name. A null child-after and a null title ask for
    // the first child of that class. An error result means the child is absent.
    let contains_defview = unsafe {
        FindWindowExW(
            hwnd,
            HWND(std::ptr::null_mut()),
            PCWSTR(DEFVIEW.as_ptr()),
            PCWSTR::null(),
        )
    }
    .is_ok();
    list.push(TopLevelWindow {
        id: hwnd.0 as usize as u64,
        class_name,
        contains_defview,
    });
    TRUE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_after_the_icon_host_is_the_canvas() {
        let windows = vec![
            TopLevelWindow {
                id: 1,
                class_name: "Progman".to_owned(),
                contains_defview: false,
            },
            TopLevelWindow {
                id: 2,
                class_name: "WorkerW".to_owned(),
                contains_defview: true,
            },
            TopLevelWindow {
                id: 3,
                class_name: "WorkerW".to_owned(),
                contains_defview: false,
            },
        ];
        assert_eq!(target_workerw(&windows).expect("canvas").to_string(), "3");
    }

    #[test]
    fn progman_icon_host_selects_the_following_worker() {
        let windows = vec![
            TopLevelWindow {
                id: 7,
                class_name: "Progman".to_owned(),
                contains_defview: true,
            },
            TopLevelWindow {
                id: 8,
                class_name: "WorkerW".to_owned(),
                contains_defview: false,
            },
        ];
        assert_eq!(target_workerw(&windows).expect("canvas"), 8);
    }

    #[test]
    fn a_missing_worker_is_an_error() {
        let windows = vec![TopLevelWindow {
            id: 2,
            class_name: "WorkerW".to_owned(),
            contains_defview: true,
        }];
        let err = target_workerw(&windows).expect_err("no canvas");
        let text = err.to_string();
        assert!(text.contains("WorkerW"), "{text}");
    }
}
