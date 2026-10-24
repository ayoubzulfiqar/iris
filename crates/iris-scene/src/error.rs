//! Failures while compiling a scene.

/// A scene-graph failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The scene document or a particle file could not be read.
    #[error("scene: {0}")]
    Message(String),
    /// `iris-pkg` rejected the document.
    #[error(transparent)]
    Pkg(#[from] iris_pkg::Error),
}
