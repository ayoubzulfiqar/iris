//! Borderless Win32 window and the thread message pump.
//!
//! The window class is registered once per process. [`pump_messages`] drains
//! the queue on the thread that created the windows. `WM_DISPLAYCHANGE` is
//! recorded for the daemon tick. The HWND values copied into [`NativeWindow`]
//! stay valid until [`DesktopWindow`] is dropped, and the caller drops any
//! wgpu surface before that.

use std::num::NonZeroIsize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, Win32WindowHandle, WindowHandle, WindowsDisplayHandle,
};

use crate::inject::parent_under_desktop;
use crate::Error;

const CLASS_NAME: &[u16] = &[
    b'I' as u16,
    b'r' as u16,
    b'i' as u16,
    b's' as u16,
    b'W' as u16,
    b'a' as u16,
    b'l' as u16,
    b'l' as u16,
    b'p' as u16,
    b'a' as u16,
    b'p' as u16,
    b'e' as u16,
    b'r' as u16,
    0,
];

static DISPLAY_CHANGED: AtomicBool = AtomicBool::new(false);
static CLASS_INSTANCE: OnceLock<isize> = OnceLock::new();

/// What the last pump observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PumpEvent {
    /// The queue had no display-change message.
    None,
    /// A top-level window received `WM_DISPLAYCHANGE`.
    DisplayChange,
}

/// Copy of an HWND the renderer can turn into a wgpu surface.
///
/// The integers do not own the window. [`DesktopWindow`] does, and it must
/// outlive every surface created from this value.
#[derive(Debug, Clone, Copy)]
pub struct NativeWindow {
    hwnd: isize,
    hinstance: isize,
}

/// A window created on the daemon thread.
pub struct DesktopWindow {
    hwnd: isize,
    hinstance: isize,
}

impl DesktopWindow {
    /// Borderless visible window covering `x,y,width,height` in virtual-screen pixels.
    pub fn open(x: i32, y: i32, width: u32, height: u32) -> Result<Self, Error> {
        create_window(x, y, width.max(1), height.max(1), true)
    }

    /// Hidden top-level window so the thread receives `WM_DISPLAYCHANGE`.
    pub fn message_window() -> Result<Self, Error> {
        create_window(0, 0, 1, 1, false)
    }

    /// HWND bits. Zero is not a window this constructor returns.
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Handle wgpu can own. The real HWND stays with this window.
    pub fn handle(&self) -> NativeWindow {
        NativeWindow {
            hwnd: self.hwnd,
            hinstance: self.hinstance,
        }
    }

    /// Reparent onto the WorkerW behind the desktop icons.
    pub fn attach_desktop(&self) -> Result<(), Error> {
        parent_under_desktop(self.hwnd)
    }

    /// Move and resize without activating the window.
    #[allow(unsafe_code)]
    pub fn move_to(&self, x: i32, y: i32, width: u32, height: u32) -> Result<(), Error> {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

        let hwnd = HWND(self.hwnd as *mut core::ffi::c_void);
        // SAFETY: `hwnd` is the window this value created and has not destroyed.
        // The call runs on the creating thread. SWP_NOZORDER keeps the shell
        // Z order after WorkerW reparenting. The size is at least one pixel.
        unsafe {
            SetWindowPos(
                hwnd,
                HWND(std::ptr::null_mut()),
                x,
                y,
                width.max(1) as i32,
                height.max(1) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .map_err(|err| Error::Win32(format!("SetWindowPos failed: {err}")))
    }
}

impl Drop for DesktopWindow {
    fn drop(&mut self) {
        if self.hwnd == 0 {
            return;
        }
        destroy_hwnd(self.hwnd);
        self.hwnd = 0;
    }
}

impl HasDisplayHandle for NativeWindow {
    #[allow(unsafe_code)]
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let raw = RawDisplayHandle::Windows(WindowsDisplayHandle::new());
        // SAFETY: a Win32 display handle has no pointer. It stays valid for
        // the process. The caller drops the wgpu surface before DesktopWindow.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

impl HasWindowHandle for NativeWindow {
    #[allow(unsafe_code)]
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let Some(hwnd) = NonZeroIsize::new(self.hwnd) else {
            return Err(HandleError::Unavailable);
        };
        let mut handle = Win32WindowHandle::new(hwnd);
        handle.hinstance = NonZeroIsize::new(self.hinstance);
        let raw = RawWindowHandle::Win32(handle);
        // SAFETY: `hwnd` is the window stored in the DesktopWindow that produced
        // this copy. That owner is dropped only after the wgpu surface. The
        // hinstance is the module handle captured at RegisterClassExW, which
        // lives for the process. borrow_raw does not take ownership.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}

/// Drain the thread queue. Call this on the thread that created the windows.
#[allow(unsafe_code)]
pub fn pump_messages() -> PumpEvent {
    use windows::Win32::UI::WindowsAndMessaging::{
        DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
    };

    // SAFETY: MSG is written by PeekMessageW before it is read. A null HWND
    // drains every window on this thread. PM_REMOVE takes each message out of
    // the queue. TranslateMessage and DispatchMessageW run the window procedure
    // on this same thread, which is the thread that created the windows.
    unsafe {
        let mut message = MSG::default();
        while PeekMessageW(
            std::ptr::addr_of_mut!(message),
            None::<&windows::Win32::Foundation::HWND>,
            0,
            0,
            PM_REMOVE,
        )
        .0 != 0
        {
            let _ = TranslateMessage(std::ptr::addr_of!(message));
            let _ = DispatchMessageW(std::ptr::addr_of!(message));
        }
    }
    if DISPLAY_CHANGED.swap(false, Ordering::Relaxed) {
        PumpEvent::DisplayChange
    } else {
        PumpEvent::None
    }
}

#[allow(unsafe_code)]
fn create_window(
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    visible: bool,
) -> Result<DesktopWindow, Error> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{HINSTANCE, HWND};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, HMENU, WINDOW_EX_STYLE, WS_POPUP, WS_VISIBLE,
    };

    let instance = register_class()?;
    let style = if visible {
        WS_POPUP | WS_VISIBLE
    } else {
        WS_POPUP
    };
    // SAFETY: CLASS_NAME is a static null-terminated UTF-16 class registered
    // above. The module handle is the one passed to RegisterClassExW. The
    // parent and menu are null, so the window is a top-level popup. Width and
    // height are at least 1. The HWND is owned by the returned DesktopWindow
    // until DestroyWindow in Drop.
    let hwnd = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            PCWSTR(CLASS_NAME.as_ptr()),
            PCWSTR(CLASS_NAME.as_ptr()),
            style,
            x,
            y,
            width as i32,
            height as i32,
            HWND(std::ptr::null_mut()),
            HMENU(std::ptr::null_mut()),
            HINSTANCE(instance as *mut core::ffi::c_void),
            None,
        )
    }
    .map_err(|err| Error::Win32(format!("CreateWindowExW failed: {err}")))?;
    if hwnd.is_invalid() {
        return Err(Error::Win32(
            "CreateWindowExW returned a null HWND".to_owned(),
        ));
    }
    Ok(DesktopWindow {
        hwnd: hwnd.0 as isize,
        hinstance: instance,
    })
}

#[allow(unsafe_code)]
fn register_class() -> Result<isize, Error> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::{GetLastError, ERROR_CLASS_ALREADY_EXISTS, HINSTANCE};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{RegisterClassExW, WNDCLASSEXW};

    if let Some(instance) = CLASS_INSTANCE.get() {
        return Ok(*instance);
    }
    // SAFETY: a null module name returns the HMODULE of the calling executable.
    // That handle stays valid for the process and is not freed here.
    let module = unsafe { GetModuleHandleW(PCWSTR::null()) }
        .map_err(|err| Error::Win32(format!("GetModuleHandleW failed: {err}")))?;
    let instance = HINSTANCE(module.0);
    let class = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wnd_proc),
        hInstance: instance,
        lpszClassName: PCWSTR(CLASS_NAME.as_ptr()),
        ..WNDCLASSEXW::default()
    };
    // SAFETY: `class` is fully initialized. CLASS_NAME outlives the process.
    // wnd_proc is a static function. RegisterClassExW copies the class name.
    // A zero atom is a failure; ERROR_CLASS_ALREADY_EXISTS means an earlier
    // call in this process already registered IrisWallpaper.
    let atom = unsafe { RegisterClassExW(std::ptr::addr_of!(class)) };
    if atom == 0 {
        let code = unsafe { GetLastError() };
        if code != ERROR_CLASS_ALREADY_EXISTS {
            return Err(Error::Win32(format!(
                "RegisterClassExW failed ({})",
                code.0
            )));
        }
    }
    let bits = instance.0 as isize;
    let _ = CLASS_INSTANCE.set(bits);
    Ok(bits)
}

#[allow(unsafe_code)]
fn destroy_hwnd(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::DestroyWindow;

    // SAFETY: `hwnd` was created by CreateWindowExW in this process and this
    // is the last use. The caller has already dropped the wgpu surface that
    // borrowed it. DestroyWindow fails if the window is already gone; that is
    // logged and not propagated because Drop cannot return a Result.
    // WM_DESTROY does not post a quit message.
    let result = unsafe { DestroyWindow(HWND(hwnd as *mut core::ffi::c_void)) };
    if let Err(err) = result {
        tracing::debug!("DestroyWindow: {err}");
    }
}

#[allow(unsafe_code)]
unsafe extern "system" fn wnd_proc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, WM_DISPLAYCHANGE};

    if msg == WM_DISPLAYCHANGE {
        DISPLAY_CHANGED.store(true, Ordering::Relaxed);
    }
    // SAFETY: the system is dispatching `hwnd` on the thread that created it.
    // The parameters are the message from that queue. DefWindowProcW does not
    // destroy the window. WM_DESTROY falls through and does not call
    // PostQuitMessage, so the daemon decides when to exit.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}
