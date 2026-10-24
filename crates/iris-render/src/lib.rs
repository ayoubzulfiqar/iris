//! Shared wgpu device and per-output wallpaper surfaces.
//!
//! The crate does not talk to Wayland. The caller passes a [`raw_window_handle`]
//! window and receives a [`WallpaperSurface`] that draws one textured triangle.

#![forbid(unsafe_code)]

mod context;
mod error;
mod scene_draw;
mod shader;
mod surface;
mod uniform;
mod yuv;

pub use context::RenderContext;
pub use error::Error;
pub use scene_draw::{
    pack_frame, DrawSpan, LayerBlend, SceneDraw, SceneUniforms, TelemetryFrame, TelemetryUniforms,
    FRAME_UNIFORM_SIZE, SCENE_UNIFORM_SIZE, TELEMETRY_UNIFORM_SIZE,
};
pub use surface::{DrawParams, WallpaperSurface};
pub use uniform::Fit;
pub use yuv::{limited_rgb, YuvFrame, YuvMatrix, YuvPlane};
