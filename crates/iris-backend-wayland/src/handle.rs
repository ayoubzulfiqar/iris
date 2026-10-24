//! Owned Wayland window the renderer can turn into a wgpu surface.
//!
//! The raw pointers are created inside the trait methods. This value keeps the
//! `wl_display` and `wl_surface` clones that those pointers come from.

use std::ffi::c_void;
use std::ptr::NonNull;

use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawDisplayHandle,
    RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle, WindowHandle,
};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Proxy};

/// A `'static` Wayland surface handle.
///
/// The caller drops the GPU surface before the layer that owns the protocol
/// object. The clones stored here keep the pointers valid until then.
#[derive(Debug, Clone)]
pub struct WaylandWindow {
    connection: Connection,
    surface: WlSurface,
}

impl WaylandWindow {
    pub(crate) fn new(connection: Connection, surface: WlSurface) -> Self {
        Self {
            connection,
            surface,
        }
    }
}

impl HasDisplayHandle for WaylandWindow {
    #[allow(unsafe_code)]
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        let ptr = self.connection.backend().display_ptr() as *mut c_void;
        let Some(display) = NonNull::new(ptr) else {
            return Err(HandleError::Unavailable);
        };
        let raw = RawDisplayHandle::Wayland(WaylandDisplayHandle::new(display));
        // SAFETY: `display` points at the `wl_display` owned by `connection`.
        // The pointer stays live until this `WaylandWindow` is dropped, and the
        // caller drops the wgpu surface before the layer is destroyed.
        Ok(unsafe { DisplayHandle::borrow_raw(raw) })
    }
}

impl HasWindowHandle for WaylandWindow {
    #[allow(unsafe_code)]
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let ptr = self.surface.id().as_ptr() as *mut c_void;
        let Some(surface) = NonNull::new(ptr) else {
            return Err(HandleError::Unavailable);
        };
        let raw = RawWindowHandle::Wayland(WaylandWindowHandle::new(surface));
        // SAFETY: `surface` points at the `wl_surface` kept alive by this clone
        // and by the layer that created it. The caller drops the wgpu surface
        // before `destroy_layer` destroys the protocol object.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}
