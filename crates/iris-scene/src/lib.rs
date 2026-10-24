//! Scene composition for Iris.
//!
//! The audio thread publishes a spectrum through an atomic snapshot. Layer
//! matrices and particle positions are evaluated on the caller, which is the
//! daemon frame loop. This crate does not talk to Wayland.

mod audio;
mod error;
mod instance;
mod particles;
mod scene;

pub use audio::{bands_from_samples, engine_protocol, smooth_sample, AudioEngine, AudioFftData};
pub use error::Error;
pub use instance::ParticleInstance;
pub use particles::{Emitter, ParticleDef, FIXED_DT, MAX_PARTICLES};
pub use scene::{Layer, LayerClass, PlacedLayer, ScenePackage, SceneRuntime, Tick};
