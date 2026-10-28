//! Platform window around one wry webview.
//!
//! Linux opens a normal GTK window and pumps it with `gtk::main_iteration_do`.
//! That pump is not the Wayland frame clock. The window has a close button.
//! Closing it hides the window and tells the daemon to drop the wallpaper.
//! The title starts with `IrisWallpaper:` so a GNOME Shell extension can lower
//! it behind every other window after the next login.
//! Windows builds the webview as a child of a borderless window and reparents
//! the child HWND with [`iris_backend_windows::embed_window`].
//!
//! A failed accelerated webview is logged and opened again with software
//! rasterization. The second failure is returned to the caller. Nothing here
//! panics the daemon on purpose: wry panics are caught and turned into errors.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{AssetRoot, SCHEME};
use crate::Bounds;

static SOFTWARE: AtomicBool = AtomicBool::new(false);
static PROFILE: AtomicU64 = AtomicU64::new(1);

#[cfg(target_os = "linux")]
mod platform {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    use gtk::prelude::*;

    use super::{profile_dir, webview_builder};
    use crate::detect::WebProject;
    use crate::Bounds;

    static GTK_READY: AtomicBool = AtomicBool::new(false);

    pub struct Host {
        view: wry::WebView,
        window: gtk::Window,
        /// Dropped after the view so the custom protocol stays registered.
        #[allow(dead_code)]
        context: wry::WebContext,
        closed: Arc<AtomicBool>,
    }

    pub fn open(project: &WebProject, bounds: Bounds) -> Result<Host, String> {
        if super::SOFTWARE.load(Ordering::Relaxed) {
            return try_open(project, bounds, true);
        }
        match try_open(project, bounds, false) {
            Ok(host) => Ok(host),
            Err(err) => {
                tracing::warn!(
                    "webview hardware acceleration failed ({err}); falling back to software rasterization"
                );
                super::SOFTWARE.store(true, Ordering::Relaxed);
                try_open(project, bounds, true)
                    .map_err(|second| format!("{err}; software rasterization failed: {second}"))
            }
        }
    }

    pub fn pump() {
        if !GTK_READY.load(Ordering::SeqCst) {
            return;
        }
        for _ in 0..32 {
            if !gtk::events_pending() {
                break;
            }
            let _ = gtk::main_iteration_do(false);
        }
    }

    impl Host {
        pub fn eval(&self, script: &str) {
            if let Err(err) = self.view.evaluate_script(script) {
                tracing::debug!("evaluate_script: {err}");
            }
        }

        pub fn place(&self, bounds: Bounds) {
            if self.closed() {
                return;
            }
            self.window
                .resize(bounds.width.max(1) as i32, bounds.height.max(1) as i32);
            self.window.move_(bounds.x, bounds.y);
        }

        pub fn closed(&self) -> bool {
            self.closed.load(Ordering::SeqCst)
        }
    }

    fn try_open(project: &WebProject, bounds: Bounds, software: bool) -> Result<Host, String> {
        if software {
            std::env::set_var("WEBKIT_DISABLE_COMPOSITING_MODE", "1");
            std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        }
        // The dmabuf renderer paints a blank white view on GNOME's compositor.
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
        let window = desktop_window(bounds)?;
        let closed = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&closed);
        window.connect_delete_event(move |window, _| {
            flag.store(true, Ordering::SeqCst);
            window.hide();
            gtk::glib::Propagation::Stop
        });
        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&content);
        window.show_all();
        pass_clicks_through(&window);
        let mut context = wry::WebContext::new(Some(profile_dir()?));
        let root = Arc::clone(&project.root);
        let entry = project.entry_url.clone();
        let init = crate::bridge::properties_script(&project.properties, &project.general);
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let builder = webview_builder(&mut context, root, &entry, init.clone(), bounds)?;
            use wry::WebViewBuilderExtUnix;
            builder.build_gtk(&content).map_err(|err| err.to_string())
        }));
        match built {
            Ok(Ok(view)) => {
                window.show_all();
                Ok(Host {
                    view,
                    window,
                    context,
                    closed,
                })
            }
            Ok(Err(err)) => Err(err),
            Err(_) => Err("webview creation panicked".to_owned()),
        }
    }

    fn desktop_window(bounds: Bounds) -> Result<gtk::Window, String> {
        if !gtk::is_initialized() {
            gtk::init().map_err(|err| format!("gtk init: {err}"))?;
        }
        GTK_READY.store(true, Ordering::SeqCst);
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        let title = format!(
            "IrisWallpaper:{},{},{},{}",
            bounds.x,
            bounds.y,
            bounds.width.max(1),
            bounds.height.max(1)
        );
        window.set_title(&title);
        // WebKit replaces the title with the page URL. The Shell extension
        // only recognises the IrisWallpaper prefix, so put the title back.
        let locked = title.clone();
        window.connect_notify(Some("title"), move |window, _| {
            let current = window.title().map(|text| text.to_string()).unwrap_or_default();
            if current != locked {
                window.set_title(&locked);
            }
        });
        window.set_decorated(false);
        window.set_resizable(false);
        window.set_deletable(true);
        window.set_keep_below(true);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_accept_focus(false);
        window.set_can_focus(false);
        window.set_focus_on_map(false);
        window.set_type_hint(gtk::gdk::WindowTypeHint::Desktop);
        window.set_role("iris-wallpaper");
        window.set_default_size(bounds.width.max(1) as i32, bounds.height.max(1) as i32);
        window.move_(bounds.x, bounds.y);
        window.connect_realize(|window| pass_clicks_through(window));
        Ok(window)
    }

    /// Clicks fall through to windows and desktop icons above this surface.
    fn pass_clicks_through(window: &gtk::Window) {
        let empty = cairo::Region::create();
        window.input_shape_combine_region(Some(&empty));
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    use super::{profile_dir, webview_builder};
    use crate::detect::WebProject;
    use crate::Bounds;

    pub struct Host {
        view: wry::WebView,
        window: iris_backend_windows::DesktopWindow,
        /// Dropped after the view so the custom protocol stays registered.
        #[allow(dead_code)]
        context: wry::WebContext,
        hwnd: isize,
    }

    impl Host {
        pub fn closed(&self) -> bool {
            false
        }
    }

    pub fn open(project: &WebProject, bounds: Bounds) -> Result<Host, String> {
        if super::SOFTWARE.load(Ordering::Relaxed) {
            return try_open(project, bounds, true);
        }
        match try_open(project, bounds, false) {
            Ok(host) => Ok(host),
            Err(err) => {
                tracing::warn!(
                    "webview hardware acceleration failed ({err}); falling back to software rasterization"
                );
                super::SOFTWARE.store(true, Ordering::Relaxed);
                try_open(project, bounds, true)
                    .map_err(|second| format!("{err}; software rasterization failed: {second}"))
            }
        }
    }

    pub fn pump() {}

    impl Host {
        pub fn eval(&self, script: &str) {
            if let Err(err) = self.view.evaluate_script(script) {
                tracing::debug!("evaluate_script: {err}");
            }
        }

        pub fn place(&self, bounds: Bounds) {
            if let Err(err) = self.window.move_to(
                bounds.x,
                bounds.y,
                bounds.width.max(1),
                bounds.height.max(1),
            ) {
                tracing::warn!("move webview parent: {err}");
            }
            place_hwnd(self.hwnd, bounds);
        }
    }

    fn try_open(project: &WebProject, bounds: Bounds, software: bool) -> Result<Host, String> {
        if software {
            std::env::set_var(
                "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS",
                "--disable-gpu --disable-gpu-compositing",
            );
        }
        let window = iris_backend_windows::DesktopWindow::open(
            bounds.x,
            bounds.y,
            bounds.width.max(1),
            bounds.height.max(1),
        )
        .map_err(|err| err.to_string())?;
        if let Err(err) = window.attach_desktop() {
            tracing::warn!("workerw injection: {err}");
        }
        let native = window.handle();
        let mut context = wry::WebContext::new(Some(profile_dir()?));
        let root = Arc::clone(&project.root);
        let entry = project.entry_url.clone();
        let init = crate::bridge::properties_script(&project.properties, &project.general);
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut builder = webview_builder(&mut context, root, &entry, init.clone(), bounds)?;
            if software {
                use wry::WebViewBuilderExtWindows;
                builder = builder.with_additional_browser_args(
                    "--disable-gpu --disable-gpu-compositing --disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection --autoplay-policy=no-user-gesture-required",
                );
            }
            let view = builder
                .build_as_child(&native)
                .map_err(|err| err.to_string())?;
            use wry::WebViewExtWindows;
            let hwnd = view.hwnd().0 as isize;
            Ok((view, hwnd))
        }));
        match built {
            Ok(Ok((view, hwnd))) => {
                if hwnd != 0 {
                    if let Err(err) = iris_backend_windows::embed_window(hwnd) {
                        tracing::warn!("embed webview: {err}");
                    }
                    place_hwnd(hwnd, bounds);
                }
                Ok(Host {
                    view,
                    window,
                    context,
                    hwnd,
                })
            }
            Ok(Err(err)) => Err(err),
            Err(_) => Err("webview creation panicked".to_owned()),
        }
    }

    #[allow(unsafe_code)]
    fn place_hwnd(hwnd: isize, bounds: Bounds) {
        use windows::Win32::Foundation::HWND;
        use windows::Win32::UI::WindowsAndMessaging::{SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER};

        if hwnd == 0 {
            return;
        }
        let handle = HWND(hwnd as *mut core::ffi::c_void);
        // SAFETY: `hwnd` is the wry child created above and still owned by the
        // WebView. SetWindowPos does not free it. The call runs on the thread
        // that created the window. SWP_NOZORDER keeps the WorkerW Z order.
        let placed = unsafe {
            SetWindowPos(
                handle,
                HWND(std::ptr::null_mut()),
                bounds.x,
                bounds.y,
                bounds.width.max(1) as i32,
                bounds.height.max(1) as i32,
                SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
        if let Err(err) = placed {
            tracing::warn!("place webview: {err}");
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
mod platform {
    use crate::detect::WebProject;
    use crate::Bounds;

    pub struct Host;

    pub fn open(_project: &WebProject, _bounds: Bounds) -> Result<Host, String> {
        Err("this operating system has no webview".to_owned())
    }

    pub fn pump() {}

    impl Host {
        pub fn eval(&self, _script: &str) {}

        pub fn place(&self, _bounds: Bounds) {}

        pub fn closed(&self) -> bool {
            false
        }
    }
}

pub(crate) use platform::{open, pump, Host};

fn profile_dir() -> Result<PathBuf, String> {
    let id = PROFILE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("iris-webview-{id}-{nanos}"));
    std::fs::create_dir_all(&dir).map_err(|err| format!("webview profile: {err}"))?;
    Ok(dir)
}

fn webview_builder<'a>(
    context: &'a mut wry::WebContext,
    root: Arc<AssetRoot>,
    entry: &str,
    init: Option<String>,
    bounds: Bounds,
) -> Result<wry::WebViewBuilder<'a>, String> {
    let entry = entry.to_owned();
    let mut builder = wry::WebViewBuilder::new_with_web_context(context)
        .with_initialization_script(crate::SHIM)
        .with_custom_protocol(SCHEME.to_owned(), move |_id, request| {
            root.respond(&request.uri().to_string())
        })
        .with_url(entry)
        .with_ipc_handler(|request| {
            tracing::debug!("web ipc: {}", request.body());
        })
        .with_autoplay(true)
        .with_bounds(wry::Rect {
            position: wry::dpi::LogicalPosition::new(0.0, 0.0).into(),
            size: wry::dpi::PhysicalSize::new(bounds.width.max(1), bounds.height.max(1)).into(),
        });
    if let Some(init) = init {
        builder = builder.with_initialization_script(init);
    }
    Ok(builder)
}
