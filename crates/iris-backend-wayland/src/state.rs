//! Output tracking, layer records, and Wayland protocol dispatch.
//!
//! Handlers only push [`BackendEvent`] values. The daemon drops GPU surfaces
//! before it asks the session to destroy a layer.

use std::collections::HashMap;

use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::output::{Mode, OutputData, OutputHandler, OutputInfo, OutputState};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{
    LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure,
};
use smithay_client_toolkit::shell::WaylandSurface;
use smithay_client_toolkit::{
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, registry_handlers,
};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    Event as ScaleEvent, WpFractionalScaleV1,
};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::{
    Event as ViewportEvent, WpViewporter,
};

use crate::scale::{
    connector_name, integer_scale, physical_size, scale_factor_f64, scale_from_factor,
    swapchain_extent,
};

/// One live output, in daemon order.
#[derive(Debug, Clone, PartialEq)]
pub struct OutputSnapshot {
    /// `wl_output` global name.
    pub id: u32,
    /// Stable index for this daemon process.
    pub index: u32,
    /// Connector name, or `output-{id}` when the compositor left it blank.
    pub connector: String,
    /// Human-readable name.
    pub name: String,
    /// Physical width from the current mode, or the scaled logical size.
    pub width_px: u32,
    /// Physical height from the current mode, or the scaled logical size.
    pub height_px: u32,
    /// Integer output scale. `2.0` is a scale factor of 2.
    pub scale: f64,
    /// True for the live output at logical origin, else the lowest index.
    pub primary: bool,
    /// Current mode refresh in millihertz, when the compositor reports one.
    pub refresh_millihz: Option<u32>,
}

/// Sizes the daemon uses to resize a swapchain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayerGeometry {
    /// Layer id returned by [`crate::Session::create_layer`].
    pub layer_id: u64,
    /// Output global name this layer was created on.
    pub output_id: u32,
    /// Logical width in surface coordinates.
    pub logical_w: u32,
    /// Logical height in surface coordinates.
    pub logical_h: u32,
    /// Swapchain width in physical pixels. Zero means the axis is unset.
    pub physical_w: u32,
    /// Swapchain height in physical pixels. Zero means the axis is unset.
    pub physical_h: u32,
    /// Preferred scale numerator over 120.
    pub scale_120: u32,
}

/// Events the daemon drains after each Wayland dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendEvent {
    /// An output became usable. A removed id that returns uses this variant.
    OutputAdded(OutputSnapshot),
    /// Name, mode, or position of a known output changed.
    OutputUpdated(OutputSnapshot),
    /// The compositor removed an output. The layer is still alive.
    OutputRemoved {
        /// Output global name.
        id: u32,
    },
    /// The compositor configured a layer. The serial is already acked.
    Configured(LayerGeometry),
    /// Scale or logical size changed outside a configure.
    ScaleChanged(LayerGeometry),
    /// A `wl_surface.frame` callback fired.
    Frame {
        /// Layer that requested the callback.
        layer_id: u64,
        /// Output the layer was created on.
        output_id: u32,
        /// Compositor timestamp in milliseconds.
        time_ms: u32,
    },
    /// The compositor closed a layer surface.
    Closed {
        /// Layer id.
        layer_id: u64,
    },
}

pub(crate) struct DisplayState {
    pub(crate) registry_state: RegistryState,
    pub(crate) compositor: CompositorState,
    pub(crate) output_state: OutputState,
    pub(crate) layer_shell: Option<LayerShell>,
    pub(crate) viewporter: Option<WpViewporter>,
    pub(crate) fractional: Option<WpFractionalScaleManagerV1>,
    tracked: HashMap<u32, TrackedOutput>,
    next_index: u32,
    pub(crate) layers: HashMap<u64, LayerRecord>,
    pub(crate) next_layer: u64,
    events: Vec<BackendEvent>,
}

struct TrackedOutput {
    output: WlOutput,
    index: u32,
    removed: bool,
    announced: bool,
}

pub(crate) struct LayerRecord {
    pub(crate) layer: LayerSurface,
    pub(crate) output_id: u32,
    pub(crate) viewport: Option<WpViewport>,
    pub(crate) fractional: Option<WpFractionalScaleV1>,
    pub(crate) logical_w: u32,
    pub(crate) logical_h: u32,
    pub(crate) physical_w: u32,
    pub(crate) physical_h: u32,
    pub(crate) scale_120: u32,
    pub(crate) fractional_seen: bool,
    pub(crate) configured: bool,
    pub(crate) frame_pending: bool,
}

impl LayerRecord {
    pub(crate) fn recompute_physical(&mut self) {
        self.physical_w = physical_size(self.logical_w, self.scale_120);
        self.physical_h = physical_size(self.logical_h, self.scale_120);
    }

    pub(crate) fn geometry(&self, layer_id: u64) -> LayerGeometry {
        LayerGeometry {
            layer_id,
            output_id: self.output_id,
            logical_w: self.logical_w,
            logical_h: self.logical_h,
            physical_w: self.physical_w,
            physical_h: self.physical_h,
            scale_120: self.scale_120,
        }
    }
}

impl DisplayState {
    pub(crate) fn new(
        registry_state: RegistryState,
        compositor: CompositorState,
        output_state: OutputState,
        layer_shell: Option<LayerShell>,
        viewporter: Option<WpViewporter>,
        fractional: Option<WpFractionalScaleManagerV1>,
    ) -> Self {
        Self {
            registry_state,
            compositor,
            output_state,
            layer_shell,
            viewporter,
            fractional,
            tracked: HashMap::new(),
            next_index: 0,
            layers: HashMap::new(),
            next_layer: 1,
            events: Vec::new(),
        }
    }

    pub(crate) fn take_events(&mut self) -> Vec<BackendEvent> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn snapshots(&self) -> Vec<OutputSnapshot> {
        let mut rows: Vec<OutputSnapshot> = self
            .tracked
            .keys()
            .filter_map(|id| self.snapshot_one(*id))
            .collect();
        rows.sort_by_key(|row| row.index);
        rows
    }

    pub(crate) fn output_proxy(&self, id: u32) -> Option<WlOutput> {
        let tracked = self.tracked.get(&id)?;
        if tracked.removed {
            return None;
        }
        Some(tracked.output.clone())
    }

    /// Logical size and the integer scale, in 120ths, for a live output.
    pub(crate) fn layer_extent(&self, output: &WlOutput) -> Option<(u32, u32, u32)> {
        let info = self.output_info(output)?;
        let (logical_w, logical_h) = logical_extent(&info);
        Some((logical_w, logical_h, scale_from_factor(info.scale_factor)))
    }

    fn note_output(&mut self, output: WlOutput) {
        let Some(id) = global_id(&self.output_state, &output) else {
            tracing::warn!("output has no id yet");
            return;
        };
        if let Some(tracked) = self.tracked.get_mut(&id) {
            tracked.output = output;
            if tracked.removed {
                tracked.removed = false;
                tracked.announced = false;
            }
            return;
        }
        let index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        self.tracked.insert(
            id,
            TrackedOutput {
                output,
                index,
                removed: false,
                announced: false,
            },
        );
    }

    fn publish_output(&mut self, id: u32) {
        let Some(tracked) = self.tracked.get(&id) else {
            return;
        };
        if tracked.removed {
            return;
        }
        let Some(snapshot) = self.snapshot_one(id) else {
            return;
        };
        let announced = self
            .tracked
            .get(&id)
            .is_some_and(|tracked| tracked.announced);
        if announced {
            self.events.push(BackendEvent::OutputUpdated(snapshot));
            self.refresh_layers(id);
            return;
        }
        if let Some(tracked) = self.tracked.get_mut(&id) {
            tracked.announced = true;
        }
        self.events.push(BackendEvent::OutputAdded(snapshot));
    }

    fn snapshot_one(&self, id: u32) -> Option<OutputSnapshot> {
        let tracked = self.tracked.get(&id)?;
        if tracked.removed {
            return None;
        }
        let index = tracked.index;
        let output = tracked.output.clone();
        let info = self.output_info(&output)?;
        let (logical_w, logical_h) = logical_extent(&info);
        let scale_120 = scale_from_factor(info.scale_factor);
        let (fallback_w, fallback_h) = swapchain_extent(logical_w, logical_h, scale_120);
        let (width_px, height_px) = current_pixels(&info).unwrap_or((fallback_w, fallback_h));
        let connector = connector_name(info.name.as_deref(), info.id);
        let name = human_name(&info, &connector);
        Some(OutputSnapshot {
            id: info.id,
            index,
            connector,
            name,
            width_px,
            height_px,
            scale: scale_factor_f64(scale_120),
            primary: self.primary_id() == Some(id),
            refresh_millihz: refresh_millihz(&info),
        })
    }

    fn primary_id(&self) -> Option<u32> {
        let mut best: Option<(u32, bool, u32)> = None;
        for (id, tracked) in &self.tracked {
            if tracked.removed {
                continue;
            }
            let Some(info) = self.output_info(&tracked.output) else {
                continue;
            };
            let at_origin = info.logical_position == Some((0, 0));
            let replace = match best {
                None => true,
                Some((_, best_origin, best_index)) => {
                    if at_origin != best_origin {
                        at_origin
                    } else {
                        tracked.index < best_index
                    }
                }
            };
            if replace {
                best = Some((*id, at_origin, tracked.index));
            }
        }
        best.map(|(id, _, _)| id)
    }

    fn output_info(&self, output: &WlOutput) -> Option<OutputInfo> {
        if let Some(info) = self.output_state.info(output) {
            return Some(info);
        }
        output
            .data::<OutputData>()
            .map(|data| data.with_output_info(Clone::clone))
    }

    fn refresh_layers(&mut self, output_id: u32) {
        let Some(tracked) = self.tracked.get(&output_id) else {
            return;
        };
        if tracked.removed {
            return;
        }
        let output = tracked.output.clone();
        let Some(info) = self.output_info(&output) else {
            return;
        };
        let (logical_w, logical_h) = logical_extent(&info);
        let scale_120 = scale_from_factor(info.scale_factor);
        let use_buffer_scale = self.viewporter.is_none() && self.fractional.is_none();
        let ids: Vec<u64> = self
            .layers
            .iter()
            .filter(|(_, record)| record.output_id == output_id)
            .map(|(id, _)| *id)
            .collect();
        for layer_id in ids {
            let geometry = {
                let Some(record) = self.layers.get_mut(&layer_id) else {
                    continue;
                };
                record.logical_w = logical_w;
                record.logical_h = logical_h;
                if !record.fractional_seen {
                    record.scale_120 = scale_120;
                    if use_buffer_scale {
                        let _ = record
                            .layer
                            .set_buffer_scale(integer_scale(record.scale_120));
                    }
                }
                record.recompute_physical();
                record.geometry(layer_id)
            };
            self.events.push(BackendEvent::ScaleChanged(geometry));
        }
    }

    fn on_integer_scale(&mut self, surface: &WlSurface, factor: i32) {
        let Some(layer_id) = self.layer_id_for_surface(surface) else {
            return;
        };
        let use_buffer_scale = self.viewporter.is_none() && self.fractional.is_none();
        let geometry = {
            let Some(record) = self.layers.get_mut(&layer_id) else {
                return;
            };
            if record.fractional_seen {
                return;
            }
            record.scale_120 = scale_from_factor(factor);
            if use_buffer_scale {
                let _ = record
                    .layer
                    .set_buffer_scale(integer_scale(record.scale_120));
            }
            record.recompute_physical();
            record.geometry(layer_id)
        };
        self.events.push(BackendEvent::ScaleChanged(geometry));
    }

    fn on_preferred_scale(&mut self, layer_id: u64, scale: u32) {
        if scale == 0 {
            return;
        }
        let geometry = {
            let Some(record) = self.layers.get_mut(&layer_id) else {
                return;
            };
            record.fractional_seen = true;
            record.scale_120 = scale;
            record.recompute_physical();
            record.geometry(layer_id)
        };
        self.events.push(BackendEvent::ScaleChanged(geometry));
    }

    fn on_configure(&mut self, layer: &LayerSurface, configure: LayerSurfaceConfigure) {
        let Some(layer_id) = self.layer_id_of(layer) else {
            return;
        };
        let (new_w, new_h) = configure.new_size;
        let geometry = {
            let Some(record) = self.layers.get_mut(&layer_id) else {
                return;
            };
            if new_w > 0 {
                record.logical_w = new_w;
            }
            if new_h > 0 {
                record.logical_h = new_h;
            }
            record.recompute_physical();
            record.configured = true;
            record.geometry(layer_id)
        };
        self.events.push(BackendEvent::Configured(geometry));
    }

    fn on_frame(&mut self, surface: &WlSurface, time_ms: u32) {
        let Some(layer_id) = self.layer_id_for_surface(surface) else {
            return;
        };
        let output_id = {
            let Some(record) = self.layers.get_mut(&layer_id) else {
                return;
            };
            record.frame_pending = false;
            record.output_id
        };
        self.events.push(BackendEvent::Frame {
            layer_id,
            output_id,
            time_ms,
        });
    }

    fn on_closed(&mut self, layer: &LayerSurface) {
        if let Some(layer_id) = self.layer_id_of(layer) {
            self.events.push(BackendEvent::Closed { layer_id });
        }
    }

    fn on_output_gone(&mut self, output: WlOutput) {
        let id = match global_id(&self.output_state, &output) {
            Some(id) => id,
            None => {
                let wanted = output.id();
                match self
                    .tracked
                    .iter()
                    .find_map(|(id, tracked)| (tracked.output.id() == wanted).then_some(*id))
                {
                    Some(id) => id,
                    None => {
                        tracing::warn!("destroyed output has no id");
                        return;
                    }
                }
            }
        };
        if let Some(tracked) = self.tracked.get_mut(&id) {
            tracked.removed = true;
        }
        self.events.push(BackendEvent::OutputRemoved { id });
    }

    fn layer_id_of(&self, layer: &LayerSurface) -> Option<u64> {
        self.layers
            .iter()
            .find_map(|(id, record)| (record.layer == *layer).then_some(*id))
    }

    fn layer_id_for_surface(&self, surface: &WlSurface) -> Option<u64> {
        let wanted = surface.id();
        self.layers
            .iter()
            .find_map(|(id, record)| (record.layer.wl_surface().id() == wanted).then_some(*id))
    }
}

fn global_id(outputs: &OutputState, output: &WlOutput) -> Option<u32> {
    if let Some(info) = outputs.info(output) {
        return Some(info.id);
    }
    let data = output.data::<OutputData>()?;
    Some(data.with_output_info(|info| info.id))
}

fn logical_extent(info: &OutputInfo) -> (u32, u32) {
    if let Some((width, height)) = info.logical_size {
        if let (Ok(width), Ok(height)) = (u32::try_from(width), u32::try_from(height)) {
            if width > 0 && height > 0 {
                return (width, height);
            }
        }
    }
    if let Some((width, height)) = current_pixels(info) {
        let scale = u32::try_from(info.scale_factor.max(1)).unwrap_or(1).max(1);
        return ((width / scale).max(1), (height / scale).max(1));
    }
    (1, 1)
}

fn current_pixels(info: &OutputInfo) -> Option<(u32, u32)> {
    let mode = current_mode(info)?;
    let width = u32::try_from(mode.dimensions.0).ok()?;
    let height = u32::try_from(mode.dimensions.1).ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    Some((width, height))
}

fn current_mode(info: &OutputInfo) -> Option<&Mode> {
    info.modes.iter().find(|mode| mode.current)
}

fn refresh_millihz(info: &OutputInfo) -> Option<u32> {
    let mode = current_mode(info)?;
    u32::try_from(mode.refresh_rate)
        .ok()
        .filter(|rate| *rate > 0)
}

fn human_name(info: &OutputInfo, connector: &str) -> String {
    info.description
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .unwrap_or(connector)
        .to_owned()
}

impl CompositorHandler for DisplayState {
    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        new_factor: i32,
    ) {
        self.on_integer_scale(surface, new_factor);
    }

    fn transform_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &WlSurface,
        time: u32,
    ) {
        self.on_frame(surface, time);
    }

    fn surface_enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &WlSurface,
        _output: &WlOutput,
    ) {
    }
}

impl OutputHandler for DisplayState {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        self.note_output(output.clone());
        if let Some(id) = global_id(&self.output_state, &output) {
            self.publish_output(id);
        } else {
            tracing::warn!("new output has no id");
        }
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        self.note_output(output.clone());
        if let Some(id) = global_id(&self.output_state, &output) {
            self.publish_output(id);
        }
    }

    fn output_destroyed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, output: WlOutput) {
        self.on_output_gone(output);
    }
}

impl LayerShellHandler for DisplayState {
    fn closed(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, layer: &LayerSurface) {
        self.on_closed(layer);
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.on_configure(layer, configure);
    }
}

impl ProvidesRegistryState for DisplayState {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers!(OutputState);
}

delegate_compositor!(DisplayState);
delegate_output!(DisplayState);
delegate_layer!(DisplayState);
delegate_registry!(DisplayState);

impl Dispatch<WpViewporter, ()> for DisplayState {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewporter,
        event: ViewportEvent,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            _ => {}
        }
    }
}

impl Dispatch<WpViewport, ()> for DisplayState {
    fn event(
        _state: &mut Self,
        _proxy: &WpViewport,
        event: <WpViewport as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleManagerV1, ()> for DisplayState {
    fn event(
        _state: &mut Self,
        _proxy: &WpFractionalScaleManagerV1,
        event: <WpFractionalScaleManagerV1 as Proxy>::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, u64> for DisplayState {
    fn event(
        state: &mut Self,
        _proxy: &WpFractionalScaleV1,
        event: ScaleEvent,
        data: &u64,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            ScaleEvent::PreferredScale { scale } => state.on_preferred_scale(*data, scale),
            _ => {}
        }
    }
}
