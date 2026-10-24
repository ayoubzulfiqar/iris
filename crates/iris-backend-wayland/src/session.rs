//! Wayland connection, output list, and layer-shell surfaces.
//!
//! `state` is the first field so protocol objects drop while the queue and
//! connection are still alive. [`Session::drop`] destroys layers first.

use std::os::fd::{AsFd, OwnedFd};

use smithay_client_toolkit::compositor::CompositorState;
use smithay_client_toolkit::output::OutputState;
use smithay_client_toolkit::registry::RegistryState;
use smithay_client_toolkit::shell::wlr_layer::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, SurfaceKind,
};
use smithay_client_toolkit::shell::WaylandSurface;
use wayland_client::globals::registry_queue_init;
use wayland_client::{Connection, EventQueue};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;

use crate::error::Error;
use crate::handle::WaylandWindow;
use crate::scale::{destination_edge, integer_scale, physical_size};
use crate::state::{BackendEvent, DisplayState, LayerGeometry, LayerRecord, OutputSnapshot};

/// One compositor connection and the layers created on it.
pub struct Session {
    pub(crate) state: DisplayState,
    pub(crate) queue: EventQueue<DisplayState>,
    pub(crate) connection: Connection,
}

impl Session {
    /// Connect to `WAYLAND_DISPLAY` and bind the globals a wallpaper needs.
    ///
    /// Layer shell, viewporter, and fractional scale are optional. A missing
    /// `wl_compositor` fails the connection. Two roundtrips let output names
    /// and modes arrive before the first snapshot.
    pub fn connect() -> Result<Self, Error> {
        let connection =
            Connection::connect_to_env().map_err(|err| Error::Connect(err.to_string()))?;
        let (globals, queue) = registry_queue_init::<DisplayState>(&connection)
            .map_err(|err| Error::Bind(err.to_string()))?;
        let qh = queue.handle();
        let registry_state = RegistryState::new(&globals);
        let compositor = CompositorState::bind(&globals, &qh)
            .map_err(|err| Error::Bind(format!("wl_compositor: {err}")))?;
        let output_state = OutputState::new(&globals, &qh);
        let layer_shell = match LayerShell::bind(&globals, &qh) {
            Ok(shell) => Some(shell),
            Err(err) => {
                tracing::info!("zwlr_layer_shell_v1 is not available: {err}");
                None
            }
        };
        let viewporter = globals
            .bind::<WpViewporter, DisplayState, ()>(&qh, 1..=1, ())
            .ok();
        let fractional = globals
            .bind::<WpFractionalScaleManagerV1, DisplayState, ()>(&qh, 1..=1, ())
            .ok();
        if viewporter.is_none() {
            tracing::info!("wp_viewporter is not available");
        }
        if fractional.is_none() {
            tracing::info!("wp_fractional_scale_manager_v1 is not available");
        }
        let mut session = Self {
            state: DisplayState::new(
                registry_state,
                compositor,
                output_state,
                layer_shell,
                viewporter,
                fractional,
            ),
            queue,
            connection,
        };
        session.roundtrip()?;
        session.roundtrip()?;
        Ok(session)
    }

    /// Block until the compositor has answered the requests already sent.
    pub fn roundtrip(&mut self) -> Result<usize, Error> {
        self.queue
            .roundtrip(&mut self.state)
            .map_err(|err| Error::Dispatch(err.to_string()))
    }

    /// Take the events queued by the last dispatch.
    pub fn take_events(&mut self) -> Vec<BackendEvent> {
        self.state.take_events()
    }

    /// Live outputs, sorted by stable index.
    pub fn snapshots(&self) -> Vec<OutputSnapshot> {
        self.state.snapshots()
    }

    /// Create a background layer on `output_id` and wait until it configures.
    ///
    /// The initial commits carry no buffer. The caller uploads and presents
    /// after this returns.
    pub fn create_layer(&mut self, output_id: u32) -> Result<u64, Error> {
        if self.state.layer_shell.is_none() {
            return Err(Error::Bind(
                "zwlr_layer_shell_v1 is not available".to_owned(),
            ));
        }
        let output = self
            .state
            .output_proxy(output_id)
            .ok_or(Error::NoOutput(output_id))?;
        let (logical_w, logical_h, scale_120) = self
            .state
            .layer_extent(&output)
            .ok_or(Error::NoOutput(output_id))?;
        let id = self.state.next_layer;
        self.state.next_layer = self.state.next_layer.saturating_add(1);
        let qh = self.queue.handle();
        let surface = self.state.compositor.create_surface(&qh);
        let layer = {
            let shell =
                self.state.layer_shell.as_ref().ok_or_else(|| {
                    Error::Bind("zwlr_layer_shell_v1 is not available".to_owned())
                })?;
            shell.create_layer_surface(&qh, surface, Layer::Background, Some("iris"), Some(&output))
        };
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_size(0, 0);
        let has_fractional = self.state.fractional.is_some();
        let viewport = match self.state.viewporter.as_ref() {
            Some(viewporter) => Some(viewporter.get_viewport(layer.wl_surface(), &qh, ())),
            None => None,
        };
        let fractional = match self.state.fractional.as_ref() {
            Some(manager) => Some(manager.get_fractional_scale(layer.wl_surface(), &qh, id)),
            None => None,
        };
        if let Some(viewport) = &viewport {
            viewport.set_destination(destination_edge(logical_w), destination_edge(logical_h));
        } else if !has_fractional {
            let _ = layer.set_buffer_scale(integer_scale(scale_120));
        }
        layer.commit();
        self.state.layers.insert(
            id,
            LayerRecord {
                layer,
                output_id,
                viewport,
                fractional,
                logical_w,
                logical_h,
                physical_w: physical_size(logical_w, scale_120),
                physical_h: physical_size(logical_h, scale_120),
                scale_120,
                fractional_seen: false,
                configured: false,
                frame_pending: false,
            },
        );
        for _ in 0..8 {
            if let Err(err) = self.roundtrip() {
                let _ = self.destroy_layer(id);
                return Err(err);
            }
            if self
                .state
                .layers
                .get(&id)
                .is_some_and(|record| record.configured)
            {
                self.prepare_viewport(id)?;
                self.commit(id)?;
                return Ok(id);
            }
        }
        let _ = self.destroy_layer(id);
        Err(Error::Protocol("output did not configure".to_owned()))
    }

    /// Request the next frame callback. A request that is already pending is left alone.
    pub fn queue_frame(&mut self, layer_id: u64) -> Result<(), Error> {
        let qh = self.queue.handle();
        let record = self
            .state
            .layers
            .get_mut(&layer_id)
            .ok_or(Error::UnknownLayer(layer_id))?;
        if record.frame_pending {
            return Ok(());
        }
        let surface = record.layer.wl_surface().clone();
        surface.frame(&qh, surface.clone());
        record.frame_pending = true;
        Ok(())
    }

    /// Commit the surface. Does not attach a buffer.
    pub fn commit(&mut self, layer_id: u64) -> Result<(), Error> {
        let record = self
            .state
            .layers
            .get(&layer_id)
            .ok_or(Error::UnknownLayer(layer_id))?;
        record.layer.commit();
        Ok(())
    }

    /// Queue `wp_viewport.set_destination` for the logical size.
    pub fn prepare_viewport(&mut self, layer_id: u64) -> Result<(), Error> {
        let record = self
            .state
            .layers
            .get(&layer_id)
            .ok_or(Error::UnknownLayer(layer_id))?;
        if let Some(viewport) = &record.viewport {
            viewport.set_destination(
                destination_edge(record.logical_w),
                destination_edge(record.logical_h),
            );
        }
        Ok(())
    }

    /// Destroy one layer. Missing ids succeed.
    pub fn destroy_layer(&mut self, layer_id: u64) -> Result<(), Error> {
        let Some(mut record) = self.state.layers.remove(&layer_id) else {
            return Ok(());
        };
        if let Some(viewport) = record.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional) = record.fractional.take() {
            fractional.destroy();
        }
        match record.layer.kind() {
            SurfaceKind::Wlr(surface) => surface.destroy(),
            _ => {}
        }
        drop(record);
        Ok(())
    }

    /// Destroy every layer. Safe to call more than once.
    pub fn destroy_all(&mut self) {
        let ids: Vec<u64> = self.state.layers.keys().copied().collect();
        for id in ids {
            let _ = self.destroy_layer(id);
        }
    }

    /// Window handle for the renderer. The caller drops it before [`Self::destroy_layer`].
    pub fn window(&self, layer_id: u64) -> Result<WaylandWindow, Error> {
        let record = self
            .state
            .layers
            .get(&layer_id)
            .ok_or(Error::UnknownLayer(layer_id))?;
        Ok(WaylandWindow::new(
            self.connection.clone(),
            record.layer.wl_surface().clone(),
        ))
    }

    /// Latest logical and physical size for a layer.
    pub fn geometry(&self, layer_id: u64) -> Result<LayerGeometry, Error> {
        let record = self
            .state
            .layers
            .get(&layer_id)
            .ok_or(Error::UnknownLayer(layer_id))?;
        Ok(record.geometry(layer_id))
    }

    /// Whether a frame callback is waiting on this layer.
    pub fn frame_pending(&self, layer_id: u64) -> Result<bool, Error> {
        let record = self
            .state
            .layers
            .get(&layer_id)
            .ok_or(Error::UnknownLayer(layer_id))?;
        Ok(record.frame_pending)
    }

    pub(crate) fn connection_fd(&self) -> Result<OwnedFd, Error> {
        self.queue
            .as_fd()
            .try_clone_to_owned()
            .map_err(|err| Error::Io(err.to_string()))
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.destroy_all();
    }
}
